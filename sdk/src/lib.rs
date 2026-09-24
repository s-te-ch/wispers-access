//! The guest side of Wispers Access, shared by every client app: joining and
//! restoring shares over any transport, the local share store, the guest API
//! calls, and the loopback HTTP proxy.
//!
//! The entry point is [`Client`], one per data directory. The wire types an
//! integrator meets ([`Invite`], [`App`], …) are re-exported from the wire
//! crate, so the SDK is the only dependency an app needs.

mod guest_node;
mod http;
mod iroh_transport;
mod storage;
mod transports;
mod wispers_connect_transport;

pub use storage::ShareId;
pub use wispers_access_wire::{App, AppKind, Invite, InviteError, Transport};

use anyhow::{Context, Result};
use guest_node::GuestNode;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;
use transports::TerminalState;
use wispers_access_wire as wire;

pub struct ClientConfig {
    /// Where the client keeps its state. Created if missing.
    pub data_dir: PathBuf,
    /// The tokio runtime to run on. Without one, the client starts its own.
    pub runtime: Option<tokio::runtime::Handle>,
}

/// This device's view of its shares: the store, the runtime everything runs
/// on, and one guest node per share once it has been dialled. Cheap to clone;
/// all clones are the same client.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    db: Arc<storage::DB>,
    runtime: tokio::runtime::Handle,
    /// Set when the client started the runtime itself, so it can stop it.
    owned_runtime: Mutex<Option<tokio::runtime::Runtime>>,
    /// Guest nodes by share id, each restored on first use.
    guest_nodes: Mutex<HashMap<ShareId, Arc<OnceCell<Arc<GuestNode>>>>>,
}

/// A share as this device knows it: what the host node last said it is, and
/// whether it still lets this device in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    pub id: ShareId,
    /// The share's name, as the host node reports it.
    pub name: String,
    /// The `<share>` in `http://<app>.<share>.localhost`. Unique per device,
    /// derived from the name at join.
    pub label: String,
    pub transport: Transport,
    /// The apps as last fetched from the host node, in its order.
    pub apps: Vec<App>,
    pub state: ShareState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareState {
    Live,
    /// The host node no longer knows this device's key: the share was
    /// removed, or this device was forgotten. Only `leave` makes sense.
    Removed,
    /// The host node revoked this device. Only `leave` makes sense.
    Revoked,
}

/// How the proxy maps a request to an app. Chosen once per proxy.
pub enum ProxyMode {
    /// One port; the app and share are in the `Host` header, as
    /// `<app>.<share>.localhost`. For desktop and Android.
    HostRouted { port: u16 },
}

impl Client {
    /// Opens the store under `data_dir` and installs the process-wide
    /// crypto provider the transports need, if nobody has yet.
    pub fn new(config: ClientConfig) -> Result<Client> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // A second installer racing us loses harmlessly: either provider
            // will do.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let db = storage::DB::open(&config.data_dir)?;
        let (runtime, owned_runtime) = match config.runtime {
            Some(handle) => (handle, None),
            None => {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("starting the client's tokio runtime")?;
                (runtime.handle().clone(), Some(runtime))
            }
        };
        Ok(Client {
            inner: Arc::new(ClientInner {
                db,
                runtime,
                owned_runtime: Mutex::new(owned_runtime),
                guest_nodes: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Joins the share the invite is for: makes this device a guest node of
    /// it and stores what the host node says the share is. A failed join
    /// leaves nothing behind.
    pub async fn join(&self, invite: Invite) -> Result<Share> {
        let client = self.clone();
        self.on_runtime(async move {
            let row = client.inner.db.new_row()?;
            let result = async {
                let info = transports::join(invite, &row).await?;
                client.record_join(&row, &info)
            }
            .await;
            if result.is_err() {
                let _ = row.delete_row();
            }
            result
        })
        .await
    }

    /// The local bookkeeping of any join, once the host node has answered:
    /// the names, the app list, and marking the row complete so it survives
    /// the next start.
    fn record_join(&self, row: &storage::Row, info: &wire::ShareInfo) -> Result<Share> {
        let share_id = row.share_id()?;
        row.write_share_info(info)?;
        let name = if info.name.is_empty() {
            share_id.to_string()
        } else {
            info.name.clone()
        };
        row.write_display_name(&name)?;
        let label = host_slug(&name).unwrap_or_else(|| share_id.to_string());
        row.write_deduped_hostname(&label)?;
        row.mark_complete()?;
        share_from_row(row)
    }

    /// Every joined share, from the store. Never waits for the network.
    pub fn shares(&self) -> Result<Vec<Share>> {
        self.inner
            .db
            .get_all_rows()?
            .iter()
            .map(share_from_row)
            .collect()
    }

    /// One share by id or label, from the store.
    pub fn share(&self, key: &str) -> Result<Option<Share>> {
        self.inner
            .db
            .find_row(key)?
            .as_ref()
            .map(share_from_row)
            .transpose()
    }

    /// Asks the host node whether the share changed since the stored copy.
    /// `Some` carries the share as it is now, which may also mean that the
    /// host node has turned this device away for good; `None` means nothing
    /// changed.
    pub async fn refresh(&self, share: &ShareId) -> Result<Option<Share>> {
        let client = self.clone();
        let share = share.clone();
        self.on_runtime(async move {
            let row = client
                .inner
                .db
                .find_row(share.as_str())?
                .with_context(|| format!("no share {}", share))?;
            let before = share_from_row(&row)?;
            if before.state != ShareState::Live {
                return Ok(None);
            }
            let node = match client.guest_node(share.as_str()).await? {
                Lookup::Live(node) => node,
                // The store learned this during restore, above.
                Lookup::Dead(_) => return Ok(Some(share_from_row(&row)?)),
                Lookup::Unknown => anyhow::bail!("no share {}", share),
            };
            let after = match node.refresh().await {
                Ok(Some(_)) => share_from_row(&row)?,
                Ok(None) => return Ok(None),
                Err(guest_node::RefreshError::Terminal) => share_from_row(&row)?,
                Err(guest_node::RefreshError::Transient(e)) => return Err(e),
            };
            Ok((after != before).then_some(after))
        })
        .await
    }

    /// Leaves the share: tells the host node, or the hub, where possible, and
    /// forgets the share on this device either way.
    pub async fn leave(&self, share: &ShareId) -> Result<()> {
        let client = self.clone();
        let share = share.clone();
        self.on_runtime(async move {
            let row = client
                .inner
                .db
                .find_row(share.as_str())?
                .with_context(|| format!("no share {}", share))?;
            // Let the live guest node go first: the transport that says
            // goodbye must not compete with it for the same key.
            client
                .inner
                .guest_nodes
                .lock()
                .expect("unpoisoned")
                .remove(&share);
            transports::leave(&row).await?;
            row.delete_row()
        })
        .await
    }

    /// Starts the loopback proxy. It serves every share the client knows,
    /// including ones joined later, until the `Proxy` is dropped.
    pub async fn proxy(&self, mode: ProxyMode) -> Result<Proxy> {
        let client = self.clone();
        self.on_runtime(async move {
            let ProxyMode::HostRouted { port } = mode;
            let listener = http::bind_loopback_port(port).await?;
            let port = listener.local_addr()?.port();
            let runtime = client.inner.runtime.clone();
            let accept_loop = runtime.spawn(http::accept_loop(listener, client));
            Ok(Proxy { port, accept_loop })
        })
        .await
    }

    /// The guest node of a share, by id or label, restoring its transport on
    /// first use. A terminal answer from the transport is recorded in the
    /// store, so the share is dead from then on without dialling again.
    pub(crate) async fn guest_node(&self, key: &str) -> Result<Lookup> {
        let Some(row) = self.inner.db.find_row(key)? else {
            return Ok(Lookup::Unknown);
        };
        if let Some(state) = row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            return Ok(Lookup::Dead(state));
        }
        let share_id = row.share_id()?;
        let cell = self
            .inner
            .guest_nodes
            .lock()
            .expect("unpoisoned")
            .entry(share_id.clone())
            .or_default()
            .clone();
        let restored = cell
            .get_or_try_init(|| async {
                let label = row
                    .read_names()
                    .map_err(transports::TransportError::Transient)?
                    .2;
                let transport = transports::restore(row.clone()).await?;
                Ok::<_, transports::TransportError>(Arc::new(GuestNode::new(
                    label,
                    row.clone(),
                    transport,
                )))
            })
            .await;
        match restored {
            Ok(node) => Ok(Lookup::Live(node.clone())),
            Err(transports::TransportError::Terminal(state)) => {
                row.write_terminal_state(state.as_str())?;
                Ok(Lookup::Dead(state))
            }
            Err(transports::TransportError::Transient(e)) => Err(e),
        }
    }

    /// Runs `work` on the client's runtime, whatever context the caller
    /// awaits from.
    async fn on_runtime<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        self.inner
            .runtime
            .spawn(work)
            .await
            .context("the client's task was cancelled")?
    }
}

/// What the proxy finds for a share label.
pub(crate) enum Lookup {
    Live(Arc<GuestNode>),
    Dead(TerminalState),
    Unknown,
}

fn share_from_row(row: &storage::Row) -> Result<Share> {
    let (id, name, label) = row.read_names()?;
    let state = match row
        .read_terminal_state()?
        .as_deref()
        .and_then(TerminalState::parse)
    {
        None => ShareState::Live,
        Some(TerminalState::Removed) => ShareState::Removed,
        Some(TerminalState::Revoked) => ShareState::Revoked,
    };
    Ok(Share {
        id,
        name,
        label,
        transport: row.read_transport_kind()?,
        apps: row.read_apps()?,
        state,
    })
}

/// Free-form name -> DNS-label-safe slug, or None if nothing usable remains.
fn host_slug(name: &str) -> Option<String> {
    // Remove apostrophes, so "Bob's app" becomes "bobs-app", not "bob-s-app".
    let cleaned = name.replace(['\'', '’'], "");
    // Slugify, but keep it to 63 chars, to produce a legal DNS label.
    let mut s = slug::slugify(cleaned);
    s.truncate(63);
    // Truncation can leave a trailing '-'.
    let s = s.trim_end_matches('-');
    (!s.is_empty()).then(|| s.to_string())
}

/// A running loopback proxy. Dropping it stops accepting connections.
pub struct Proxy {
    port: u16,
    accept_loop: tokio::task::JoinHandle<()>,
}

impl Proxy {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// What to open for an app of a share: `http://<app>.<share>.localhost:<port>`.
    pub fn base_url(&self, share: &Share, app_id: &str) -> String {
        format!("http://{}.{}.localhost:{}", app_id, share.label, self.port)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        // A runtime must not be dropped from inside another runtime's task,
        // which is where a client may well go out of scope.
        if let Some(runtime) = self.owned_runtime.lock().expect("unpoisoned").take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("wispers-access-sdk-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn a_client_starts_offline_with_its_own_runtime() {
        let data_dir = scratch_dir();
        let client = Client::new(ClientConfig {
            data_dir: data_dir.clone(),
            runtime: None,
        })
        .unwrap();
        assert!(client.shares().unwrap().is_empty());
        assert!(client.share("nope").unwrap().is_none());
        drop(client);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn a_client_can_be_dropped_inside_another_runtime() {
        let data_dir = scratch_dir();
        let client = Client::new(ClientConfig {
            data_dir: data_dir.clone(),
            runtime: None,
        })
        .unwrap();
        let proxy = client
            .proxy(ProxyMode::HostRouted { port: 0 })
            .await
            .unwrap();
        assert_ne!(proxy.port(), 0);
        drop(proxy);
        drop(client);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn labels_are_dns_safe() {
        assert_eq!(host_slug("Bob's Apps").as_deref(), Some("bobs-apps"));
        assert_eq!(host_slug("!!!"), None);
        assert_eq!(host_slug(&"a".repeat(70)).unwrap().len(), 63);
    }
}
