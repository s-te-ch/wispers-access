//! The guest side of Wispers Access, shared by every client app: joining and
//! restoring shares over any transport, the local share store, the guest API
//! calls, and the loopback HTTP proxy.
//!
//! The entry point is [`Client`], one per data directory. The wire types an
//! integrator meets ([`SharedApp`], [`Transport`]) are re-exported from the wire
//! crate, so the SDK is the only dependency an app needs. The same surface
//! is what UniFFI turns into the Swift and Kotlin bindings. The SDK logs
//! through `tracing`; a Rust app installs its own subscriber, the bindings
//! get [`install_log_sink`].

uniffi::setup_scaffolding!();

mod guest_node;
mod http;
mod iroh_transport;
mod logging;
mod secrets;
mod storage;
mod transports;
mod wispers_connect_transport;

pub use http::RequiredCookie;
pub use logging::{LogLevel, LogSink, install_log_sink};
pub use secrets::{FileSecretStore, SecretStore, SecretStoreError};
pub use storage::ShareId;
pub use wispers_access_wire::{AppKind, SharedApp, Transport};

use guest_node::GuestNode;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;
use transports::TerminalState;
use wispers_access_wire as wire;

pub type Result<T> = std::result::Result<T, SdkError>;

/// What can go wrong at the SDK's surface. Flat on purpose: the bindings
/// carry the variant and the message, and an app decides by the variant.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum SdkError {
    #[error("invalid invite code: {0}")]
    InvalidInvite(String),
    #[error("no share {0}")]
    NoSuchShare(String),
    /// The store or the secret store failed.
    #[error("{0}")]
    Storage(String),
    /// The host node, or the hub in front of it, could not be reached or
    /// refused. Includes the transport's own bookkeeping on the way.
    #[error("{0}")]
    HostNode(String),
    #[error("{0}")]
    Internal(String),
}

impl SdkError {
    fn storage(e: impl Into<anyhow::Error>) -> Self {
        SdkError::Storage(format!("{:#}", e.into()))
    }

    fn host_node(e: impl Into<anyhow::Error>) -> Self {
        SdkError::HostNode(format!("{:#}", e.into()))
    }
}

impl From<SecretStoreError> for SdkError {
    fn from(e: SecretStoreError) -> Self {
        SdkError::Storage(e.to_string())
    }
}

#[derive(uniffi::Record)]
pub struct ClientConfig {
    /// Where the client keeps its state. Created if missing.
    pub data_dir: String,
    /// Where key material goes. Without one, a [`FileSecretStore`] under
    /// `data_dir`.
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// Told whenever a stored share changes. Without one, nobody is.
    pub observer: Option<Arc<dyn Observer>>,
}

/// What an app implements to follow its shares without polling.
#[uniffi::export(with_foreign)]
pub trait Observer: Send + Sync {
    /// The stored share changed: it was joined, its name or apps changed,
    /// or its host node turned this device away for good. Called from the
    /// client's runtime, for `leave` never (the caller knows).
    fn on_share_changed(&self, share: Share);
}

/// This device's view of its shares: the store, the runtime everything runs
/// on, and one guest node per share once it has been dialled. Cheap to clone;
/// all clones are the same client.
#[derive(Clone, uniffi::Object)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    db: Arc<storage::DB>,
    secrets: Arc<dyn SecretStore>,
    observer: Arc<dyn Observer>,
    runtime: tokio::runtime::Handle,
    /// Set when the client started the runtime itself, so it can stop it.
    owned_runtime: Mutex<Option<tokio::runtime::Runtime>>,
    /// Guest nodes by share id, each restored on first use.
    guest_nodes: Mutex<HashMap<ShareId, Arc<OnceCell<Arc<GuestNode>>>>>,
}

/// A share as this device knows it: what the host node last said it is, and
/// whether it still lets this device in.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Share {
    pub id: ShareId,
    /// The share's name, as the host node reports it.
    pub name: String,
    /// The `<share>` in `http://<app>.<share>.localhost`. Unique per device,
    /// derived from the name at join.
    pub label: String,
    pub transport: Transport,
    /// The apps as last fetched from the host node, in its order.
    pub apps: Vec<SharedApp>,
    pub state: ShareState,
    /// When this device joined.
    pub joined_at: std::time::SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ShareState {
    Live,
    /// The host node no longer knows this device's key: the share was
    /// removed, or this device was forgotten. Only `leave` makes sense.
    Removed,
    /// The host node revoked this device. Only `leave` makes sense.
    Revoked,
}

// The wire types the surface carries, mirrored for the bindings.

#[uniffi::remote(Record)]
pub struct SharedApp {
    pub id: String,
    pub name: String,
    pub kind: AppKind,
}

#[uniffi::remote(Enum)]
pub enum AppKind {
    Web,
    Jellyfin,
    Immich,
}

#[uniffi::remote(Enum)]
pub enum Transport {
    WispersConnect,
    Iroh,
    Tailscale,
}

/// Which transport an invite code names, or why it is no invite. Offline;
/// for checking a scanned code before `join`.
#[uniffi::export]
pub fn validate_invite(invite_code: String) -> Result<Transport> {
    Ok(wire::Invite::parse(&invite_code)
        .map_err(|e| SdkError::InvalidInvite(e.to_string()))?
        .transport())
}

#[uniffi::export]
impl Client {
    /// Opens the store under `data_dir` on a runtime of the client's own,
    /// and installs the process-wide crypto provider the transports need,
    /// if nobody has yet.
    #[uniffi::constructor]
    pub fn new(config: ClientConfig) -> Result<Arc<Client>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| SdkError::Internal(format!("starting the client's tokio runtime: {e}")))?;
        let handle = runtime.handle().clone();
        Self::open(config, handle, Some(runtime))
    }

    /// Joins the share the invite is for: makes this device a guest node of
    /// it and stores what the host node says the share is. A failed join
    /// leaves nothing behind.
    pub async fn join(&self, invite_code: String) -> Result<Share> {
        let invite = wire::Invite::parse(&invite_code)
            .map_err(|e| SdkError::InvalidInvite(e.to_string()))?;
        let client = self.clone();
        self.on_runtime(async move {
            let row = client.inner.db.new_row().map_err(SdkError::storage)?;
            let result = async {
                let info = transports::join(invite, &row, &client.inner.secrets)
                    .await
                    .map_err(SdkError::host_node)?;
                client.record_join(&row, &info).map_err(SdkError::storage)
            }
            .await;
            match result {
                Ok(share) => {
                    client.inner.observer.on_share_changed(share.clone());
                    Ok(share)
                }
                Err(e) => {
                    let _ = row.delete_row();
                    Err(e)
                }
            }
        })
        .await
    }

    /// Every joined share, from the store. Never waits for the network.
    pub fn shares(&self) -> Result<Vec<Share>> {
        self.inner
            .db
            .get_all_rows()
            .map_err(SdkError::storage)?
            .iter()
            .map(|row| row.read_share().map_err(SdkError::storage))
            .collect()
    }

    /// One share by id or label, from the store.
    pub fn share(&self, key: String) -> Result<Option<Share>> {
        self.inner
            .db
            .find_row(&key)
            .map_err(SdkError::storage)?
            .as_ref()
            .map(|row| row.read_share().map_err(SdkError::storage))
            .transpose()
    }

    /// Asks the host node whether the share changed since the stored copy.
    /// `Some` carries the share as it is now, which may also mean that the
    /// host node has turned this device away for good; `None` means nothing
    /// changed. The observer hears about a change too.
    pub async fn refresh(&self, share: ShareId) -> Result<Option<Share>> {
        let client = self.clone();
        self.on_runtime(async move {
            let row = client.row(&share)?;
            let before = row.read_share().map_err(SdkError::storage)?;
            if before.state != ShareState::Live {
                return Ok(None);
            }
            let node = match client.guest_node(share.as_str()).await? {
                Lookup::Live(node) => node,
                // The store learned this during restore, above.
                Lookup::Dead(_) => return Ok(Some(row.read_share().map_err(SdkError::storage)?)),
                Lookup::Unknown => return Err(SdkError::NoSuchShare(share.to_string())),
            };
            let after = match node.refresh().await {
                Ok(Some(_)) => row.read_share().map_err(SdkError::storage)?,
                Ok(None) => return Ok(None),
                Err(guest_node::RefreshError::Terminal) => {
                    row.read_share().map_err(SdkError::storage)?
                }
                Err(guest_node::RefreshError::Transient(e)) => return Err(SdkError::host_node(e)),
            };
            Ok((after != before).then_some(after))
        })
        .await
    }

    /// Leaves the share: tells the host node, or the hub, where possible, and
    /// forgets the share and its secrets on this device either way.
    pub async fn leave(&self, share: ShareId) -> Result<()> {
        let client = self.clone();
        self.on_runtime(async move {
            let row = client.row(&share)?;
            // Let the live guest node go first: the transport that says
            // goodbye must not compete with it for the same key.
            client
                .inner
                .guest_nodes
                .lock()
                .expect("unpoisoned")
                .remove(&share);
            transports::leave(&row, &client.inner.secrets)
                .await
                .map_err(SdkError::host_node)?;
            row.delete_row().map_err(SdkError::storage)
        })
        .await
    }

    /// Starts a loopback proxy on one port, routing by the `Host` header:
    /// `http://<app>.<share>.localhost:<port>`. For desktop and Android. It
    /// serves every share the client knows, including ones joined later,
    /// until the proxy is dropped. With a `required_cookie`, requests
    /// without it get a 403.
    pub async fn start_host_routed_proxy(
        &self,
        port: u16,
        required_cookie: Option<RequiredCookie>,
    ) -> Result<Arc<HostRoutedProxy>> {
        let client = self.clone();
        self.on_runtime(async move {
            let (port, listeners) = http::bind_loopback_port(port)
                .await
                .map_err(SdkError::host_node)?;
            let accept_loops = listeners
                .into_iter()
                .map(|listener| {
                    client.inner.runtime.spawn(http::accept_loop(
                        listener,
                        client.clone(),
                        http::Route::FromHost,
                        required_cookie.clone(),
                    ))
                })
                .collect();
            Ok(Arc::new(HostRoutedProxy {
                client,
                bound_port: BoundPort { port, accept_loops },
            }))
        })
        .await
    }

    /// A loopback proxy with one `127.0.0.1` port per app, each bound when
    /// the app's URL is first asked for. For iOS, where `*.localhost` does
    /// not resolve. Serves until dropped. With a `required_cookie`, requests
    /// without it get a 403.
    pub fn start_per_app_proxy(&self, required_cookie: Option<RequiredCookie>) -> Arc<PerAppProxy> {
        Arc::new(PerAppProxy {
            client: self.clone(),
            required_cookie,
            bound_ports: Mutex::new(HashMap::new()),
        })
    }
}

impl Client {
    /// Opens the store under `data_dir` on the caller's runtime. For Rust
    /// apps that run tokio already.
    pub fn new_with_runtime(
        config: ClientConfig,
        runtime: tokio::runtime::Handle,
    ) -> Result<Client> {
        Ok(Arc::try_unwrap(Self::open(config, runtime, None)?)
            .unwrap_or_else(|client| (*client).clone()))
    }

    fn open(
        config: ClientConfig,
        runtime: tokio::runtime::Handle,
        owned_runtime: Option<tokio::runtime::Runtime>,
    ) -> Result<Arc<Client>> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // A second installer racing us loses harmlessly: either provider
            // will do.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let data_dir = PathBuf::from(&config.data_dir);
        let db = storage::DB::open(&data_dir).map_err(SdkError::storage)?;
        let secrets = config
            .secrets
            .unwrap_or_else(|| Arc::new(FileSecretStore::new(data_dir.join("secrets"))));
        let observer = config.observer.unwrap_or_else(|| Arc::new(NoObserver));
        Ok(Arc::new(Client {
            inner: Arc::new(ClientInner {
                db,
                secrets,
                observer,
                runtime,
                owned_runtime: Mutex::new(owned_runtime),
                guest_nodes: Mutex::new(HashMap::new()),
            }),
        }))
    }

    /// The local bookkeeping of any join, once the host node has answered:
    /// the names, the app list, and marking the row complete so it survives
    /// the next start.
    fn record_join(&self, row: &storage::Row, info: &wire::ShareInfo) -> anyhow::Result<Share> {
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
        row.read_share()
    }

    fn row(&self, share: &ShareId) -> Result<storage::Row> {
        self.inner
            .db
            .find_row(share.as_str())
            .map_err(SdkError::storage)?
            .ok_or_else(|| SdkError::NoSuchShare(share.to_string()))
    }

    /// The guest node of a share, by id or label, restoring its transport on
    /// first use. A terminal answer from the transport is recorded in the
    /// store, so the share is dead from then on without dialling again.
    pub(crate) async fn guest_node(&self, key: &str) -> Result<Lookup> {
        let Some(row) = self.inner.db.find_row(key).map_err(SdkError::storage)? else {
            return Ok(Lookup::Unknown);
        };
        if let Some(state) = row
            .read_terminal_state()
            .map_err(SdkError::storage)?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            return Ok(Lookup::Dead(state));
        }
        let share_id = row.share_id().map_err(SdkError::storage)?;
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
                let transport =
                    transports::restore(row.clone(), self.inner.secrets.clone()).await?;
                Ok::<_, transports::TransportError>(Arc::new(GuestNode::new(
                    label,
                    row.clone(),
                    transport,
                    self.inner.observer.clone(),
                )))
            })
            .await;
        match restored {
            Ok(node) => Ok(Lookup::Live(node.clone())),
            Err(transports::TransportError::Terminal(state)) => {
                row.write_terminal_state(state.as_str())
                    .map_err(SdkError::storage)?;
                self.inner
                    .observer
                    .on_share_changed(row.read_share().map_err(SdkError::storage)?);
                Ok(Lookup::Dead(state))
            }
            Err(transports::TransportError::Transient(e)) => Err(SdkError::host_node(e)),
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
            .map_err(|e| SdkError::Internal(format!("the client's task was cancelled: {e}")))?
    }
}

/// What the proxy finds for a share label.
pub(crate) enum Lookup {
    Live(Arc<GuestNode>),
    Dead(TerminalState),
    Unknown,
}

struct NoObserver;

impl Observer for NoObserver {
    fn on_share_changed(&self, _: Share) {}
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

/// A running loopback proxy on one port; the app and share are in the
/// `Host` header. Dropping it stops accepting connections.
#[derive(uniffi::Object)]
pub struct HostRoutedProxy {
    client: Client,
    bound_port: BoundPort,
}

#[uniffi::export]
impl HostRoutedProxy {
    pub fn port(&self) -> u16 {
        self.bound_port.port
    }

    /// What to open for an app of a share:
    /// `http://<app>.<share>.localhost:<port>`.
    pub fn base_url(&self, share: ShareId, app_id: String) -> Result<String> {
        let label = self
            .client
            .row(&share)?
            .read_names()
            .map_err(SdkError::storage)?
            .2;
        Ok(format!("http://{app_id}.{label}.localhost:{}", self.port()))
    }
}

impl Drop for HostRoutedProxy {
    fn drop(&mut self) {
        self.bound_port.release();
    }
}

/// A running loopback proxy with a port per app, bound on first use and
/// remembered across restarts so a webview's origin stays put. Dropping it
/// stops accepting connections.
#[derive(uniffi::Object)]
pub struct PerAppProxy {
    client: Client,
    required_cookie: Option<RequiredCookie>,
    /// Each app's port, bound by whoever asks for it first.
    bound_ports: Mutex<HashMap<AppKey, Arc<OnceCell<BoundPort>>>>,
}

/// An app, as the proxy tells them apart: its share's id and its own.
type AppKey = (ShareId, String);

#[uniffi::export]
impl PerAppProxy {
    /// What to open for an app of a share: `http://127.0.0.1:<port>`, a
    /// literal address so no name resolution is involved. Binds the port
    /// first if this is the app's first use.
    pub async fn base_url(&self, share: ShareId, app_id: String) -> Result<String> {
        let port = self.app_port(share, app_id).await?;
        Ok(format!("http://127.0.0.1:{port}"))
    }
}

impl PerAppProxy {
    /// The app's port, binding it on the app's first use: the remembered
    /// port if it is still free, else a new one, which is then remembered.
    async fn app_port(&self, share: ShareId, app_id: String) -> Result<u16> {
        let cell = self
            .bound_ports
            .lock()
            .expect("unpoisoned")
            .entry((share.clone(), app_id.clone()))
            .or_default()
            .clone();
        let bound = cell
            .get_or_try_init(|| async {
                let row = self.client.row(&share)?;
                let app = app_id.clone();
                let (port, listeners) = self
                    .client
                    .on_runtime(async move {
                        if let Some(port) = row.read_app_port(&app).map_err(SdkError::storage)?
                            && let Ok(bound) = http::bind_loopback_port(port).await
                        {
                            return Ok(bound);
                        }
                        let (port, listeners) = http::bind_loopback_port(0)
                            .await
                            .map_err(SdkError::host_node)?;
                        row.write_app_port(&app, port).map_err(SdkError::storage)?;
                        Ok((port, listeners))
                    })
                    .await?;
                let route = http::Route::Fixed { share, app: app_id };
                let accept_loops = listeners
                    .into_iter()
                    .map(|listener| {
                        self.client.inner.runtime.spawn(http::accept_loop(
                            listener,
                            self.client.clone(),
                            route.clone(),
                            self.required_cookie.clone(),
                        ))
                    })
                    .collect();
                Ok::<_, SdkError>(BoundPort { port, accept_loops })
            })
            .await?;
        Ok(bound.port)
    }
}

impl Drop for PerAppProxy {
    fn drop(&mut self) {
        for cell in self.bound_ports.lock().expect("unpoisoned").values() {
            if let Some(bound) = cell.get() {
                bound.release();
            }
        }
    }
}

/// A port bound on each loopback address, with an accept loop per address.
struct BoundPort {
    port: u16,
    accept_loops: Vec<tokio::task::JoinHandle<()>>,
}

impl BoundPort {
    /// Stops the accept loops, which closes the port.
    fn release(&self) {
        for accept_loop in &self.accept_loops {
            accept_loop.abort();
        }
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
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("wispers-access-sdk-{}", uuid::Uuid::new_v4()))
    }

    fn client_in(data_dir: &std::path::Path) -> Arc<Client> {
        Client::new(ClientConfig {
            data_dir: data_dir.to_string_lossy().into_owned(),
            secrets: None,
            observer: None,
        })
        .unwrap()
    }

    #[test]
    fn a_client_starts_offline_with_its_own_runtime() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        assert!(client.shares().unwrap().is_empty());
        assert!(client.share("nope".into()).unwrap().is_none());
        assert!(matches!(
            validate_invite("nope".into()),
            Err(SdkError::InvalidInvite(_))
        ));
        drop(client);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn a_client_can_be_dropped_inside_another_runtime() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let proxy = client.start_host_routed_proxy(0, None).await.unwrap();
        let port = proxy.port();
        assert_ne!(port, 0);
        // Both loopback families answer; IPv6 only where the machine has it.
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        if tokio::net::TcpStream::connect((Ipv6Addr::LOCALHOST, port))
            .await
            .is_err()
        {
            eprintln!("no IPv6 loopback on this machine");
        }
        drop(proxy);
        drop(client);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn per_app_ports_survive_a_new_proxy() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        // A share the store knows, without a host node behind it.
        let row = client.inner.db.new_row().unwrap();
        row.write_deduped_hostname("rt").unwrap();
        row.mark_complete().unwrap();
        let share = row.share_id().unwrap();

        let proxy = client.start_per_app_proxy(None);
        let first = proxy.base_url(share.clone(), "echo".into()).await.unwrap();
        assert!(first.starts_with("http://127.0.0.1:"));
        assert_eq!(
            proxy.base_url(share.clone(), "echo".into()).await.unwrap(),
            first
        );
        assert_ne!(
            proxy.base_url(share.clone(), "other".into()).await.unwrap(),
            first
        );
        drop(proxy);

        let proxy = client.start_per_app_proxy(None);
        assert_eq!(proxy.base_url(share, "echo".into()).await.unwrap(), first);
        drop(proxy);
        drop(client);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[tokio::test]
    async fn a_required_cookie_gates_every_request() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let proxy = client
            .start_host_routed_proxy(0, Some(cookie.clone()))
            .await
            .unwrap();
        let url = format!("http://127.0.0.1:{}/", proxy.port());
        let status = |req: hyper::Request<String>| async {
            let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port()))
                .await
                .unwrap();
            let (mut sender, conn) =
                hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                    .await
                    .unwrap();
            tokio::spawn(conn);
            sender.send_request(req).await.unwrap().status()
        };
        let without = hyper::Request::get(&url)
            .header("host", "echo.nope.localhost")
            .body(String::new())
            .unwrap();
        assert_eq!(status(without).await, hyper::StatusCode::FORBIDDEN);
        let with = hyper::Request::get(&url)
            .header("host", "echo.nope.localhost")
            .header("cookie", format!("{}={}", cookie.name, cookie.value))
            .body(String::new())
            .unwrap();
        // Past the gate: the share is what's unknown now.
        assert_eq!(status(with).await, hyper::StatusCode::NOT_FOUND);
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
