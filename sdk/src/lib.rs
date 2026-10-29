//! The guest side of Wispers Access, shared by every client app: joining and
//! restoring shares over any transport, the local share store, the guest API
//! calls, and the loopback HTTP proxies.
//!
//! The entry point is [`Client`], one per data directory. The wire types an
//! integrator meets ([`SharedApp`], [`Transport`]) are re-exported from the wire
//! crate, so the SDK is the only dependency an app needs. The same surface
//! is what UniFFI turns into the Swift and Kotlin bindings. The SDK logs
//! through `tracing`; a Rust app installs its own subscriber, the bindings
//! get [`install_log_sink`].

uniffi::setup_scaffolding!();

mod certificates;
mod forward_proxy;
mod guest_node;
mod iroh_transport;
mod logging;
mod loopback;
mod loopback_forwarding;
mod pairing;
mod reverse_proxy;
mod secrets;
mod socket_owner;
mod storage;
#[cfg(test)]
mod test_support;
mod transports;
mod wispers_connect_transport;

pub use certificates::{CertificateDescription, CertificateKind};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use certificates::{OriginTrust, TrustVerdict};
pub use logging::{LogLevel, LogSink, install_log_sink};
pub use loopback::RequiredCookie;
pub use secrets::{FileSecretStore, SecretScope, SecretStore, SecretStoreError};
pub use storage::ShareId;
pub use wispers_access_wire::{AppKind, AppProxyMode, SharedApp, Transport};

use guest_node::GuestNode;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
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
    /// The share turned a well-formed invite down, because it's used, expired
    /// or unknown. The message is the description to show to the user.
    #[error("{0}")]
    InviteRefused(String),
    #[error("no share {0}")]
    NoSuchShare(String),
    /// The store or the secret store failed.
    #[error("{0}")]
    Storage(String),
    /// The host node, or the hub in front of it, could not be reached or
    /// refused. Includes the transport's own bookkeeping on the way.
    #[error("{0}")]
    HostNode(String),
    /// The host node runs a newer Wispers Access than this SDK understands
    /// and turned the request away. The user should update the app.
    #[error("{0}")]
    ClientTooOld(String),
    /// A port that one of the share's loopback origins needs is taken, by
    /// another share's open app or by another process. The message, a
    /// sentence to show the user, says which.
    #[error("{0}")]
    PortInUse(String),
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
    /// its host node turned this device away for good, or it needs a newer
    /// app (or stopped needing one). Called from the client's runtime, for
    /// `leave` never (the caller knows).
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
    /// The `<share>` in `http://<app>.<share>.wa.localhost`. Unique per device,
    /// derived from the name at join.
    pub label: String,
    pub transport: Transport,
    /// The apps as last fetched from the host node, in its order.
    pub apps: Vec<SharedApp>,
    /// Certificates for the share's `https` origins, one PEM certificate each,
    /// as last fetched from the host node, for the guest's user to install on
    /// the device.
    pub ca_certificates: Vec<String>,
    pub state: ShareState,
    /// When this device joined.
    pub joined_at: std::time::SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ShareState {
    Live,
    /// The host node runs a newer Wispers Access than this SDK understands
    /// and no longer serves it. The apps are not usable until the user
    /// updates the app.
    Outdated,
    /// The host node no longer knows this device's key: the share was
    /// removed, or this device was forgotten. Only `leave` makes sense.
    Removed,
    /// The host node revoked this device. Only `leave` makes sense.
    Revoked,
}

impl ShareState {
    /// True if the state means we can't use this share anymore. I.e. the host
    /// node has forgotten about our guest node or revoked its access.
    pub fn is_terminal(self) -> bool {
        matches!(self, ShareState::Removed | ShareState::Revoked)
    }
}

// The wire types the surface carries, mirrored for the bindings.

#[uniffi::remote(Record)]
pub struct SharedApp {
    pub id: String,
    pub name: String,
    pub kind: AppKind,
    pub mode: AppProxyMode,
    pub start_url: Option<String>,
    pub proxied_origins: Vec<String>,
}

#[uniffi::remote(Enum)]
pub enum AppKind {
    Web,
    Jellyfin,
    Immich,
}

#[uniffi::remote(Enum)]
pub enum AppProxyMode {
    ReverseProxy,
    ForwardProxy,
    Unknown,
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

/// Where a forward-proxy app's proxied origins point, for the client app to
/// set up its WebView's proxy with. `None` if one of them doesn't parse: the
/// host node is newer than this SDK, and the app can't be opened.
#[uniffi::export]
pub fn proxied_targets(app: SharedApp) -> Option<Vec<ProxiedTarget>> {
    app.proxied_origins
        .iter()
        .map(|origin| {
            let origin = origin.parse::<wire::Origin>().ok()?;
            Some(ProxiedTarget {
                host: origin.host().to_owned(),
                port: origin.port(),
                https: origin.is_https(),
                loopback: origin.is_loopback(),
            })
        })
        .collect()
}

/// One of a forward-proxy app's proxied origins, taken apart.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProxiedTarget {
    /// A lowercase DNS name, an IPv4 literal, or an IPv6 literal in brackets.
    pub host: String,
    /// The port, the scheme's default if the origin names none.
    pub port: u16,
    pub https: bool,
    /// Whether the host names the loopback (`localhost`, `127.0.0.1`,
    /// `[::1]`), which some WebViews never send to a proxy.
    pub loopback: bool,
}

/// What one of a share's certificates ([`Share::ca_certificates`]) can vouch
/// for, for the text that offers to install it. `None` if it isn't an X.509
/// certificate in PEM.
#[uniffi::export]
pub fn describe_certificate(pem: String) -> Option<CertificateDescription> {
    certificates::describe(&pem)
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
                    .map_err(|e| {
                        if e.is::<guest_node::ClientTooOldError>() {
                            SdkError::ClientTooOld(e.to_string())
                        } else if let Some(refused) =
                            e.downcast_ref::<transports::InviteRefusedError>()
                        {
                            SdkError::InviteRefused(refused.to_string())
                        } else {
                            SdkError::host_node(e)
                        }
                    })?;
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

    /// Asks the host node whether the share has changed since we stored a copy
    /// of its config. `None` means nothing changed. `Some` can mean several
    /// things: the share config has changed, or the host node has changed its
    /// mind about serving our guest node (it got removed, revoked, or is simply
    /// too old).
    pub async fn refresh(&self, share: ShareId) -> Result<Option<Share>> {
        let client = self.clone();
        self.on_runtime(async move {
            let row = client.row(&share)?;
            let before = row.read_share().map_err(SdkError::storage)?;
            if before.state.is_terminal() {
                return Ok(None);
            }
            let node = match client.restore_guest_node(share.as_str()).await? {
                Lookup::Live(node) => node,
                // The store learned this during restore, above.
                Lookup::Dead(_) => return Ok(Some(row.read_share().map_err(SdkError::storage)?)),
                Lookup::Unknown => return Err(SdkError::NoSuchShare(share.to_string())),
            };
            // Refresh the guest node's metadata. Fail on transient errors
            // (unreachable etc). Other errors get written to the DB.
            if let Err(guest_node::RefreshError::Transient(e)) = node.refresh().await {
                return Err(SdkError::host_node(e));
            }
            // Re-read the row, which was updated by refresh().
            let after = row.read_share().map_err(SdkError::storage)?;
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
            let live = client
                .inner
                .guest_nodes
                .lock()
                .expect("unpoisoned")
                .remove(&share);
            if let Some(node) = live.as_ref().and_then(|cell| cell.get()) {
                node.close().await;
            }
            transports::leave(&row, &client.inner.secrets)
                .await
                .map_err(SdkError::host_node)?;
            row.delete_row().map_err(SdkError::storage)
        })
        .await
    }

    /// Check connection health and drop/redial the connection if necessary.
    /// This is primarily useful on mobile, when the app was in the background
    /// and just returned to the foreground. Dead connections get redialed if
    /// they had been used recently. Otherwise, we just drop them.
    ///
    /// The check costs one round trip on a live connection and up to 2 seconds
    /// on a dead one.
    pub async fn check_connections(&self) {
        // Each check runs on the client's runtime; an outcome the guest node
        // did not already log is a cancelled task, nothing to report.
        let checks: Vec<_> = self
            .live_guest_nodes()
            .into_iter()
            .map(|node| {
                self.inner
                    .runtime
                    .spawn(async move { node.check_connection().await })
            })
            .collect();
        for check in checks {
            let _ = check.await;
        }
    }

    /// Close every cached connection. Use if the network changed and we know
    /// that existing connections must be dead. You may want to follow up with
    /// `check_connections` to redial the connections that were recently in use.
    pub fn close_connections(&self) {
        for node in self.live_guest_nodes() {
            node.drop_connection();
        }
    }

    /// Starts a loopback proxy on one port, serving all shares this client has
    /// joined. Requests get routed by their `Host` header, using the scheme
    /// `http://<app>.<share>.wa.localhost:<port>`.
    pub async fn start_host_routed_proxy(
        &self,
        port: u16,
        auth_mode: ProxyAuthMode,
    ) -> Result<Arc<HostRoutedProxy>> {
        let authenticator = match auth_mode {
            ProxyAuthMode::AppCookie { cookie } => reverse_proxy::Authenticator::for_cookie(cookie),
            ProxyAuthMode::Pairing { token_lifetime } => reverse_proxy::Authenticator::for_pairing(
                pairing::CookieIssuer::restore_or_mint(&*self.inner.secrets, token_lifetime)?,
            ),
        };
        let client = self.clone();
        self.on_runtime(async move {
            let (port, listeners) = loopback::bind_port(port)
                .await
                .map_err(SdkError::host_node)?;
            let serve = {
                let client = client.clone();
                let authenticator = authenticator.clone();
                move |listener| {
                    client.inner.runtime.spawn(reverse_proxy::accept_loop(
                        listener,
                        client.clone(),
                        reverse_proxy::RouteSource::HostHeader,
                        Some(authenticator.clone()),
                    ))
                }
            };
            Ok(Arc::new(HostRoutedProxy {
                client,
                authenticator,
                bound_port: BoundPort::serve(port, listeners, serve),
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

    /// Starts the loopback forward proxy for desktop clients, with a port per
    /// share that only processes of this OS user may connect to (see
    /// `forward_proxy::ShareSource::Fixed`). macOS, Linux and Windows only.
    pub fn start_per_share_forward_proxy(&self) -> Arc<PerShareForwardProxy> {
        Arc::new(PerShareForwardProxy {
            client: self.clone(),
            bound_ports: Mutex::new(HashMap::new()),
        })
    }

    /// Starts the loopback forward proxy for the shares' forward-proxy apps,
    /// on one port for all shares; `0` picks a free one. A share's webview
    /// uses it as its HTTP proxy for the share's proxied origins and answers
    /// its 407 with the share's [`CredentialRoutedForwardProxy::credential`]. For mobile client
    /// apps, which can give each share's webview a credential of its own.
    pub async fn start_credential_routed_forward_proxy(
        &self,
        port: u16,
    ) -> Result<Arc<CredentialRoutedForwardProxy>> {
        let credential_issuer = Arc::new(forward_proxy::CredentialIssuer::default());
        let client = self.clone();
        self.on_runtime(async move {
            let (port, listeners) = loopback::bind_port(port)
                .await
                .map_err(SdkError::host_node)?;
            let serve = {
                let client = client.clone();
                let credential_issuer = credential_issuer.clone();
                move |listener| {
                    client.inner.runtime.spawn(forward_proxy::accept_loop(
                        listener,
                        client.clone(),
                        forward_proxy::ShareSource::Credential(credential_issuer.clone()),
                    ))
                }
            };
            Ok(Arc::new(CredentialRoutedForwardProxy {
                client,
                credential_issuer,
                bound_port: Arc::new(BoundPort::serve(port, listeners, serve)),
                loopback_forwarders: Default::default(),
            }))
        })
        .await
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

    /// Whether this device trusts each `https` origin of the share's
    /// forward-proxy apps, as the platform's verifier decides, by a TLS
    /// handshake through a tunnel to each. For the desktop app, whose
    /// browsers never tell it about a certificate error; the mobile apps
    /// learn it from their webviews. Origins that don't parse are left out.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub async fn check_origin_trust(&self, share: ShareId) -> Result<Vec<OriginTrust>> {
        let mut origins: Vec<wire::Origin> = Vec::new();
        let apps = self.row(&share)?.read_apps().map_err(SdkError::storage)?;
        let https_origins = apps
            .iter()
            .filter(|app| app.mode == AppProxyMode::ForwardProxy)
            .flat_map(|app| &app.proxied_origins)
            .filter_map(|origin| origin.parse::<wire::Origin>().ok())
            .filter(|origin| origin.is_https());
        for origin in https_origins {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        if origins.is_empty() {
            return Ok(Vec::new());
        }
        let node = match self.restore_guest_node(share.as_str()).await? {
            Lookup::Live(node) => node,
            Lookup::Dead(state) => {
                return Err(SdkError::HostNode(format!(
                    "the share is {}",
                    state.as_str()
                )));
            }
            Lookup::Unknown => return Err(SdkError::NoSuchShare(share.to_string())),
        };
        certificates::check_origins(&node, origins).await
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

    /// The guest nodes restored so far.
    fn live_guest_nodes(&self) -> Vec<Arc<GuestNode>> {
        self.inner
            .guest_nodes
            .lock()
            .expect("unpoisoned")
            .values()
            .filter_map(|cell| cell.get().cloned())
            .collect()
    }

    fn row(&self, share: &ShareId) -> Result<storage::Row> {
        self.inner
            .db
            .find_row(share.as_str())
            .map_err(SdkError::storage)?
            .ok_or_else(|| SdkError::NoSuchShare(share.to_string()))
    }

    /// Restore the guest node of a share, by id or label. The first call
    /// restores its transport, which may reach the hub. Later calls find it
    /// cached. A terminal answer from the transport is recorded in the store,
    /// so the share is dead from then on without dialling again.
    pub(crate) async fn restore_guest_node(&self, key: &str) -> Result<Lookup> {
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

pub(crate) struct NoObserver;

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

/// How a browser authenticates to a proxy port. The port is reachable by every
/// process on the device, so the proxy demands a cookie on every request. The
/// mode says who mints it and how it reaches the browser.
#[derive(Clone, Debug, uniffi::Enum)]
pub enum ProxyAuthMode {
    /// The app mints a cookie and injects it directly into the webview that
    /// talks to the proxy. The proxy only checks that the cookie is present.
    AppCookie { cookie: RequiredCookie },
    /// The proxy pairs with the user's browser to write the cookie. The app
    /// generates a registration token and a URL like
    /// http://wa.localhost:<port>/pair?token=T&share=S&app=A. When the user
    /// follows the URL, the proxy checks the token, then redirects to the app.
    Pairing { token_lifetime: Duration },
}

/// A running loopback proxy on one port; the app and share are in the
/// `Host` header. Dropping it stops accepting connections.
#[derive(uniffi::Object)]
pub struct HostRoutedProxy {
    client: Client,
    authenticator: reverse_proxy::Authenticator,
    bound_port: BoundPort,
}

#[uniffi::export]
impl HostRoutedProxy {
    pub fn port(&self) -> u16 {
        self.bound_port.port
    }

    /// Where to send the browser for a given shared app. Uses the pairing
    /// endpoint with [`ProxyAuthMode::Pairing`], the app's base URL with
    /// [`ProxyAuthMode::AppCookie`].
    pub fn browse_url(&self, share: ShareId, app_id: String) -> Result<String> {
        let label = self.share_label(&share)?;
        Ok(match self.authenticator.issuer() {
            Some(issuer) => {
                reverse_proxy::pair_url(self.port(), &issuer.mint_token(), &label, &app_id)
            }
            None => self.app_origin(&label, &app_id),
        })
    }

    /// The URL where a given shared app is served, without any pairing logic.
    /// With [`ProxyAuthMode::AppCookie`], this is the domain for the injected
    /// cookie.
    pub fn base_url(&self, share: ShareId, app_id: String) -> Result<String> {
        let label = self.share_label(&share)?;
        Ok(self.app_origin(&label, &app_id))
    }
}

impl HostRoutedProxy {
    fn app_origin(&self, label: &str, app_id: &str) -> String {
        format!(
            "http://{app_id}.{label}.{}:{}",
            reverse_proxy::PROXY_DOMAIN,
            self.port()
        )
    }

    fn share_label(&self, share: &ShareId) -> Result<String> {
        Ok(self
            .client
            .row(share)?
            .read_names()
            .map_err(SdkError::storage)?
            .2)
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

    /// Binds every app's port again, on the same port numbers. iOS reclaims
    /// the listening sockets of a suspended app, so call this whenever the
    /// app comes back to the foreground.
    pub async fn rebind(&self) {
        let cells: Vec<_> = self
            .bound_ports
            .lock()
            .expect("unpoisoned")
            .values()
            .cloned()
            .collect();
        let _ = self
            .client
            .on_runtime(async move {
                for cell in cells {
                    if let Some(bound) = cell.get() {
                        bound.rebind().await;
                    }
                }
                Ok(())
            })
            .await;
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
                            && let Ok(bound) = loopback::bind_port(port).await
                        {
                            return Ok(bound);
                        }
                        let (port, listeners) =
                            loopback::bind_port(0).await.map_err(SdkError::host_node)?;
                        row.write_app_port(&app, port).map_err(SdkError::storage)?;
                        Ok((port, listeners))
                    })
                    .await?;
                let route_source = reverse_proxy::RouteSource::Fixed { share, app: app_id };
                let authenticator = self
                    .required_cookie
                    .clone()
                    .map(reverse_proxy::Authenticator::for_cookie);
                let client = self.client.clone();
                let serve = move |listener| {
                    client.inner.runtime.spawn(reverse_proxy::accept_loop(
                        listener,
                        client.clone(),
                        route_source.clone(),
                        authenticator.clone(),
                    ))
                };
                Ok::<_, SdkError>(BoundPort::serve(port, listeners, serve))
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

/// A running loopback forward proxy, serving every share's forward-proxy
/// apps on one port. Dropping it stops accepting connections, and the
/// shares' credentials end with it.
#[derive(uniffi::Object)]
pub struct CredentialRoutedForwardProxy {
    client: Client,
    credential_issuer: Arc<forward_proxy::CredentialIssuer>,
    bound_port: Arc<BoundPort>,
    /// Each share's loopback forwarder while it lives.
    loopback_forwarders: Arc<tokio::sync::Mutex<HashMap<ShareId, Weak<LoopbackForwarder>>>>,
}

#[uniffi::export]
impl CredentialRoutedForwardProxy {
    pub fn port(&self) -> u16 {
        self.bound_port.port
    }

    /// The realm of the proxy's 407s. The client app answers a challenge with
    /// the share's credential only if it names this realm and the proxy's
    /// host. (Android's callback gives no port, so the realm is what tells the
    /// proxy apart from a shared app on `localhost`.)
    pub fn realm(&self) -> String {
        forward_proxy::REALM.to_owned()
    }

    /// What a share's webview answers the proxy's 407s with. Minted lazily,
    /// valid as long as this proxy runs.
    pub fn credential(&self, share: ShareId) -> Result<ProxyCredential> {
        self.client.row(&share)?;
        Ok(ProxyCredential {
            username: share.to_string(),
            password: self.credential_issuer.mint_password(&share),
        })
    }

    /// Binds the proxy's port, and those of every live loopback forwarder,
    /// again, on the same port numbers. iOS reclaims the listening sockets of a
    /// suspended app, so call this whenever the app comes back to the
    /// foreground.
    pub async fn rebind(&self) {
        let own = self.bound_port.clone();
        let forwarders = self.loopback_forwarders.clone();
        let _ = self
            .client
            .on_runtime(async move {
                own.rebind().await;
                let live: Vec<_> = forwarders
                    .lock()
                    .await
                    .values()
                    .filter_map(Weak::upgrade)
                    .collect();
                for forwarder in live {
                    for bound in &forwarder.bound_ports {
                        bound.rebind().await;
                    }
                }
                Ok(())
            })
            .await;
    }

    /// For WebViews that don't send loopback hosts to a proxy (iOS), which
    /// breaks normal forward-proxy mode whenver a shared app uses
    /// localhost:port-style addresses. Instead, this attempts to bind the same
    /// ports as the shared app, then proxies requests reaching those ports.
    /// This can clash, in which case we fail with [`SdkError::PortInUse`].
    pub async fn forward_loopback_origins(&self, share: ShareId) -> Result<Arc<LoopbackForwarder>> {
        let client = self.client.clone();
        let forwarders = self.loopback_forwarders.clone();
        self.client
            .on_runtime(async move {
                let mut forwarders = forwarders.lock().await;
                if let Some(live) = forwarders.get(&share).and_then(Weak::upgrade) {
                    return Ok(live);
                }
                forwarders.retain(|_, forwarder| forwarder.strong_count() > 0);
                let apps = client.row(&share)?.read_apps().map_err(SdkError::storage)?;
                // Released by its drop if a port fails to bind.
                let mut forwarder = LoopbackForwarder {
                    cookie: RequiredCookie {
                        name: loopback_forwarding::COOKIE_NAME.to_owned(),
                        value: secrets::mint_secret(),
                    },
                    bound_ports: Vec::new(),
                };
                for (port, hosts) in loopback_forwarding::loopback_ports(&apps) {
                    if let Some(holder) = forwarders.iter().find_map(|(holder, other)| {
                        other
                            .upgrade()
                            .is_some_and(|other| other.ports().contains(&port))
                            .then_some(holder)
                    }) {
                        let name = client
                            .row(holder)?
                            .read_names()
                            .map_err(SdkError::storage)?
                            .1;
                        return Err(SdkError::PortInUse(format!(
                            "Port {port} is in use by an app of the share {name}."
                        )));
                    }
                    let (_, listeners) = loopback::bind_port(port).await.map_err(|e| {
                        tracing::warn!(
                            port,
                            error = format!("{e:#}"),
                            "could not bind a loopback port"
                        );
                        SdkError::PortInUse(format!(
                            "Port {port} is already in use on this device."
                        ))
                    })?;
                    let forwarded = Arc::new(loopback_forwarding::ForwardedPort {
                        client: client.clone(),
                        share: share.clone(),
                        port,
                        hosts,
                        cookie: forwarder.cookie.clone(),
                        share_origins: loopback_forwarding::share_origins(&apps),
                    });
                    let runtime = client.inner.runtime.clone();
                    let serve = move |listener| {
                        runtime.spawn(loopback_forwarding::accept_loop(
                            listener,
                            forwarded.clone(),
                        ))
                    };
                    forwarder
                        .bound_ports
                        .push(BoundPort::serve(port, listeners, serve));
                }
                let forwarder = Arc::new(forwarder);
                forwarders.insert(share, Arc::downgrade(&forwarder));
                Ok(forwarder)
            })
            .await
    }
}

impl Drop for CredentialRoutedForwardProxy {
    fn drop(&mut self) {
        self.bound_port.release();
    }
}

/// A running loopback forward proxy with a port per share, for desktop
/// clients (see [`Client::start_per_share_forward_proxy`]). Dropping it stops
/// accepting connections.
#[derive(uniffi::Object)]
pub struct PerShareForwardProxy {
    client: Client,
    /// Each share's port, bound by whoever asks for it first.
    bound_ports: Mutex<HashMap<ShareId, Arc<OnceCell<BoundPort>>>>,
}

#[uniffi::export]
impl PerShareForwardProxy {
    /// The share's port on `127.0.0.1`, for its browser profile's proxy
    /// setting. Binds it when on first use, using the remembered port from the
    /// last run if available (will try for a few seconds, as an earlier run of
    /// the app may still be exiting), otherwise a new port. If the share's
    /// browser got started using a a different port, it must be restarted.
    pub async fn share_port(&self, share: ShareId) -> Result<u16> {
        let cell = self
            .bound_ports
            .lock()
            .expect("unpoisoned")
            .entry(share.clone())
            .or_default()
            .clone();
        let bound = cell
            .get_or_try_init(|| async {
                let row = self.client.row(&share)?;
                let (port, listeners) = self
                    .client
                    .on_runtime(async move {
                        if let Some(port) =
                            row.read_forward_proxy_port().map_err(SdkError::storage)?
                            && let Ok(bound) = loopback::rebind_port(port).await
                        {
                            return Ok(bound);
                        }
                        let (port, listeners) =
                            loopback::bind_port(0).await.map_err(SdkError::host_node)?;
                        row.write_forward_proxy_port(port)
                            .map_err(SdkError::storage)?;
                        Ok((port, listeners))
                    })
                    .await?;
                let client = self.client.clone();
                let serve = move |listener| {
                    client.inner.runtime.spawn(forward_proxy::accept_loop(
                        listener,
                        client.clone(),
                        forward_proxy::ShareSource::Fixed(share.clone()),
                    ))
                };
                Ok::<_, SdkError>(BoundPort::serve(port, listeners, serve))
            })
            .await?;
        Ok(bound.port)
    }

    /// Closes the share's port. Used when we leave a share. Already accepted
    /// connections run until the browser closes them.
    pub fn release(&self, share: ShareId) {
        let cell = self.bound_ports.lock().expect("unpoisoned").remove(&share);
        if let Some(bound) = cell.as_ref().and_then(|cell| cell.get()) {
            bound.release();
        }
    }
}

impl Drop for PerShareForwardProxy {
    fn drop(&mut self) {
        for cell in self.bound_ports.lock().expect("unpoisoned").values() {
            if let Some(bound) = cell.get() {
                bound.release();
            }
        }
    }
}

/// A share's Basic credential for the [`CredentialRoutedForwardProxy`].
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProxyCredential {
    pub username: String,
    pub password: String,
}

/// A share's `http` loopback origins, forwarded to its host node while this
/// lives (see [`CredentialRoutedForwardProxy::forward_loopback_origins`]). Dropping the last
/// reference releases the ports.
#[derive(uniffi::Object)]
pub struct LoopbackForwarder {
    cookie: RequiredCookie,
    bound_ports: Vec<BoundPort>,
}

#[uniffi::export]
impl LoopbackForwarder {
    /// The cookie every forwarded request must carry. The client app sets it
    /// in the share's data store for `localhost` and `127.0.0.1`, `HttpOnly`
    /// so pages can't read or delete it.
    pub fn cookie(&self) -> RequiredCookie {
        self.cookie.clone()
    }

    /// The ports being forwarded.
    pub fn ports(&self) -> Vec<u16> {
        self.bound_ports.iter().map(|bound| bound.port).collect()
    }
}

impl Drop for LoopbackForwarder {
    fn drop(&mut self) {
        for bound in &self.bound_ports {
            bound.release();
        }
    }
}

/// A port bound on each loopback address, with an accept loop per address.
/// It remembers how to serve the port, so that it can bind it again.
struct BoundPort {
    port: u16,
    /// Starts the accept loop for one of the port's listeners.
    serve: Box<dyn Fn(tokio::net::TcpListener) -> tokio::task::JoinHandle<()> + Send + Sync>,
    accept_loops: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl BoundPort {
    /// Serves listeners just bound to `port`, one accept loop each.
    fn serve(
        port: u16,
        listeners: Vec<tokio::net::TcpListener>,
        serve: impl Fn(tokio::net::TcpListener) -> tokio::task::JoinHandle<()> + Send + Sync + 'static,
    ) -> Self {
        let accept_loops = listeners.into_iter().map(&serve).collect();
        BoundPort {
            port,
            serve: Box::new(serve),
            accept_loops: Mutex::new(accept_loops),
        }
    }

    /// Binds the port again and serves the new listeners. If it can't be bound
    /// again, it stays unserved, with a warning.
    async fn rebind(&self) {
        let old = std::mem::take(&mut *self.accept_loops.lock().expect("unpoisoned"));
        for accept_loop in &old {
            accept_loop.abort();
        }
        for accept_loop in old {
            let _ = accept_loop.await;
        }
        match loopback::bind_port(self.port).await {
            Ok((_, listeners)) => {
                let new = listeners.into_iter().map(&self.serve).collect();
                *self.accept_loops.lock().expect("unpoisoned") = new;
            }
            Err(e) => tracing::warn!(
                port = self.port,
                error = format!("{e:#}"),
                "could not bind a loopback port again"
            ),
        }
    }

    /// Stops the accept loops, which closes the port.
    fn release(&self) {
        for accept_loop in self.accept_loops.lock().expect("unpoisoned").iter() {
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

    /// Deletes a client's data dir once it can. Windows doesn't delete open
    /// files, and a dropped client's database closes only after its runtime
    /// has dropped the tasks it aborted.
    fn remove_scratch_dir(dir: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(e) = std::fs::remove_dir_all(dir) {
            assert!(
                std::time::Instant::now() < deadline,
                "could not remove {}: {e}",
                dir.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
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
        remove_scratch_dir(&data_dir);
    }

    #[tokio::test]
    async fn a_client_can_be_dropped_inside_another_runtime() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let proxy = client
            .start_host_routed_proxy(
                0,
                ProxyAuthMode::Pairing {
                    token_lifetime: Duration::from_secs(60),
                },
            )
            .await
            .unwrap();
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
        remove_scratch_dir(&data_dir);
    }

    #[tokio::test]
    async fn per_app_ports_survive_a_new_proxy() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let share = stored_share(&client);

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
        remove_scratch_dir(&data_dir);
    }

    #[tokio::test]
    async fn per_app_ports_are_served_again_after_a_rebind() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let share = stored_share(&client);
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let proxy = client.start_per_app_proxy(Some(cookie));
        let base = proxy.base_url(share, "echo".into()).await.unwrap();
        let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
        proxy.rebind().await;
        // Any answer will do: the port is listening.
        let req = hyper::Request::get("/").body(String::new()).unwrap();
        assert_eq!(send(port, req).await.status(), hyper::StatusCode::FORBIDDEN);
        drop(proxy);
        drop(client);
        remove_scratch_dir(&data_dir);
    }

    #[tokio::test]
    async fn a_required_cookie_is_demanded_on_every_request() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let proxy = client
            .start_host_routed_proxy(
                0,
                ProxyAuthMode::AppCookie {
                    cookie: cookie.clone(),
                },
            )
            .await
            .unwrap();
        let without = hyper::Request::get("/")
            .header("host", "echo.nope.wa.localhost")
            .body(String::new())
            .unwrap();
        assert_eq!(
            send(proxy.port(), without).await.status(),
            hyper::StatusCode::FORBIDDEN
        );
        let with = hyper::Request::get("/")
            .header("host", "echo.nope.wa.localhost")
            .header("cookie", format!("{}={}", cookie.name, cookie.value))
            .body(String::new())
            .unwrap();
        // Past the check: the share is what's unknown now.
        assert_eq!(
            send(proxy.port(), with).await.status(),
            hyper::StatusCode::NOT_FOUND
        );
        // There is no pairing to be had under an app's own cookie: the
        // browser goes straight to the app.
        let share = stored_share(&client);
        assert_eq!(
            proxy.browse_url(share.clone(), "echo".into()).unwrap(),
            proxy.base_url(share, "echo".into()).unwrap()
        );
        let pair = hyper::Request::get("/pair?token=x&share=nope&app=echo")
            .header("host", "wa.localhost")
            .body(String::new())
            .unwrap();
        assert_eq!(
            send(proxy.port(), pair).await.status(),
            hyper::StatusCode::NOT_FOUND
        );
        drop(proxy);
        drop(client);
        remove_scratch_dir(&data_dir);
    }

    #[tokio::test]
    async fn a_browse_url_pairs_the_browser_once() {
        let data_dir = scratch_dir();
        let client = client_in(&data_dir);
        let share = stored_share(&client);
        let auth_mode = ProxyAuthMode::Pairing {
            token_lifetime: Duration::from_secs(60),
        };
        let proxy = client
            .start_host_routed_proxy(0, auth_mode.clone())
            .await
            .unwrap();
        let port = proxy.port();
        let host = format!("wa.localhost:{port}");
        let browse_url = proxy.browse_url(share.clone(), "echo".into()).unwrap();
        let path_and_query = browse_url.strip_prefix(&format!("http://{host}")).unwrap();
        let pair = |cookie: Option<&str>| {
            let mut req = hyper::Request::get(path_and_query).header("host", &host);
            if let Some(cookie) = cookie {
                req = req.header("cookie", cookie);
            }
            req.body(String::new()).unwrap()
        };

        // First visit: the cookie arrives with the redirect to the app.
        let paired = send(port, pair(None)).await;
        assert_eq!(paired.status(), hyper::StatusCode::FOUND);
        assert_eq!(
            paired.headers()["location"],
            format!("http://echo.rt.wa.localhost:{port}/")
        );
        let set_cookie = paired.headers()["set-cookie"].to_str().unwrap().to_owned();
        assert!(
            set_cookie.contains("; Domain=wa.localhost; "),
            "{set_cookie}"
        );
        assert!(
            set_cookie.contains("; HttpOnly; SameSite=Lax"),
            "{set_cookie}"
        );
        let cookie = set_cookie.split(';').next().unwrap().to_owned();

        // The token is spent, the cookie is what counts from now on. A
        // browser that has neither gets an error page.
        let spent = send(port, pair(None)).await;
        assert_eq!(spent.status(), hyper::StatusCode::FORBIDDEN);
        assert_eq!(spent.headers()["content-type"], "text/html; charset=utf-8");
        let again = send(port, pair(Some(&cookie))).await;
        assert_eq!(again.status(), hyper::StatusCode::FOUND);
        assert!(!again.headers().contains_key("set-cookie"));

        // A paired browser gets past the check on the app hosts; the app
        // itself never sees the cookie, so an unknown share is all it finds.
        let app = hyper::Request::get("/")
            .header("host", format!("echo.nope.wa.localhost:{port}"))
            .header("cookie", &cookie)
            .body(String::new())
            .unwrap();
        assert_eq!(send(port, app).await.status(), hyper::StatusCode::NOT_FOUND);

        // The pairing outlives the proxy: the next one honours the same cookie.
        drop(proxy);
        let proxy = client.start_host_routed_proxy(0, auth_mode).await.unwrap();
        let port = proxy.port();
        let app = hyper::Request::get("/")
            .header("host", format!("echo.nope.wa.localhost:{port}"))
            .header("cookie", &cookie)
            .body(String::new())
            .unwrap();
        assert_eq!(send(port, app).await.status(), hyper::StatusCode::NOT_FOUND);
        drop(proxy);
        drop(client);
        remove_scratch_dir(&data_dir);
    }

    /// One HTTP/1.1 request to a proxy port, on a connection of its own. The
    /// proxy closes the connection after the response. Our end is a task on
    /// the test's runtime, which `remove_scratch_dir` blocks, and the proxy's
    /// end holds the client, and with it the database, which Windows can't
    /// delete while it is open.
    async fn send(
        port: u16,
        mut req: hyper::Request<String>,
    ) -> hyper::Response<hyper::body::Incoming> {
        req.headers_mut().insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("close"),
        );
        let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .unwrap();
        tokio::spawn(conn);
        sender.send_request(req).await.unwrap()
    }

    /// Adds a share the store knows, without a host node behind it, as "rt".
    /// The row is dropped on return. It keeps the database open, and Windows
    /// can't delete the data dir while it is.
    fn stored_share(client: &Client) -> ShareId {
        let row = client.inner.db.new_row().unwrap();
        row.write_deduped_hostname("rt").unwrap();
        row.mark_complete().unwrap();
        row.share_id().unwrap()
    }

    #[test]
    fn proxied_targets_take_origins_apart() {
        let app = |origins: &[&str]| SharedApp {
            id: "a".into(),
            name: "a".into(),
            kind: AppKind::Web,
            mode: AppProxyMode::ForwardProxy,
            start_url: None,
            proxied_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
        };
        let target = |host: &str, port, https, loopback| ProxiedTarget {
            host: host.into(),
            port,
            https,
            loopback,
        };
        assert_eq!(
            proxied_targets(app(&["http://localhost:4200", "https://sso.internal"])),
            Some(vec![
                target("localhost", 4200, false, true),
                target("sso.internal", 443, true, false),
            ])
        );
        assert_eq!(proxied_targets(app(&["http://a:1", "nonsense"])), None);
    }

    #[test]
    fn labels_are_dns_safe() {
        assert_eq!(host_slug("Bob's Apps").as_deref(), Some("bobs-apps"));
        assert_eq!(host_slug("!!!"), None);
        assert_eq!(host_slug(&"a".repeat(70)).unwrap().len(), 63);
    }
}
