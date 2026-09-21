//! Serving: the daemon's startup sequence and the per-circle state shared
//! by the transports, the IPC server and the stream handlers. A transport
//! (`wispers_connect_transport.rs`, `iroh_transport.rs`) contributes a
//! `bind` that returns its node or endpoint and a `run` that drives its loop;
//! `serve` calls them in order and owns everything in between.

use crate::config::{CircleConfig, TransportConfig};
use crate::ipc;
use crate::iroh_transport;
use crate::protocol;
use crate::storage;
use crate::wispers_connect_transport;
use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use wispers_access_wire as wire;

pub async fn serve(circle: &str) -> Result<()> {
    let dir = storage::CircleDir::new(circle)?;
    let cfg = dir.load_config()?;
    match cfg.default_share() {
        Some(s) => info!(share = %s.id, upstream = %s.upstream, "default share"),
        None => warn!("no shares configured; guests get 503 until `waserver reload`"),
    }
    let state = dir.open_state()?;
    let server: Arc<dyn Server> = match cfg.transport.clone() {
        TransportConfig::WispersConnect { backend } => {
            Arc::new(wispers_connect_transport::bind(circle, state.clone(), backend).await?)
        }
        TransportConfig::Iroh {} => Arc::new(iroh_transport::bind(circle, state.clone()).await?),
    };
    let handle = ServingHandle::new(dir, cfg, state, server.clone());
    let ipc_server = match ipc::Server::bind(circle).await {
        Ok(ipc_server) => ipc_server,
        Err(e) => {
            // Let the transport go down properly rather than drop it.
            let _ = server.shutdown().await;
            return Err(e);
        }
    };
    let mut ipc_task = tokio::spawn(ipc_server.run(handle.clone()));
    let reason = server.run(handle).await?;
    if reason == ExitReason::Stopped {
        // `waserver stop` ended the loop. Give some time to answer the request.
        let _ = tokio::time::timeout(IPC_REPLY_GRACE, &mut ipc_task).await;
    }
    Ok(())
}

/// A circle's server, using the appropriate transport through dynamic dispatch.
/// Created by that transport's `bind` function.
pub trait Server: Send + Sync {
    /// Serves until the transport stops or a signal arrives.
    fn run(&self, handle: ServingHandle) -> BoxFuture<'_, Result<ExitReason>>;

    /// Mints an invite for a new guest.
    fn invite<'a>(
        &'a self,
        node_name: &'a str,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<wire::Invite>>;

    /// Marks a guest revoked. Called by `ServingHandle::revoke_guest`, which
    /// then closes the guest's live connections.
    fn revoke_guest(&self, number: i64) -> Result<storage::GuestNode>;

    /// Stops serving. Called by `ServingHandle::shutdown`, which already tells
    /// every live connection to close beforehand.
    fn shutdown(&self) -> BoxFuture<'_, Result<()>>;
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why a Server's `run` loop returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// A shutdown signal. Nobody is waiting for a reply.
    Signal,
    /// Stopped from inside (`waserver stop`) or the session ended.
    Stopped,
}

/// How long the exit waits for the IPC server to answer the `stop` that
/// ended the loop.
const IPC_REPLY_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct ServingHandle {
    inner: Arc<Inner>,
}

struct Inner {
    server: Arc<dyn Server>,
    /// The circle's state database, which activation writes to.
    db: storage::StateDb,

    // Time or creation and time since being reachable, respectively.
    started_at: chrono::DateTime<chrono::Utc>,
    reachable_since: RwLock<Option<chrono::DateTime<chrono::Utc>>>,

    /// Live connections from guests, keyed by a per-connection ID.
    connections: RwLock<HashMap<u64, GuestConnection>>,
    next_connection_id: AtomicU64,

    /// Config location.
    dir: storage::CircleDir,
    /// The config as last loaded. Swapped whole on `reload`.
    config: RwLock<Arc<CircleConfig>>,
    /// Notifies connected guests of config updates.
    events: broadcast::Sender<u64>,
}

/// Outcome of `reload`-ing the config.
pub struct ReloadOutcome {
    pub changed: bool,
    pub config: Arc<CircleConfig>,
}

struct GuestConnection {
    number: i32,
    user_id: String,
    connected_since: chrono::DateTime<chrono::Utc>,
    /// Closes the connection with a contract code, on transports that can.
    closer: Option<Closer>,
}

/// Callback type for closing a live connection.
pub type Closer = Box<dyn Fn(wire::CloseCode) + Send + Sync>;

/// One guest's live connection state, for status reporting. Multiple QUIC
/// connections from the same guest are aggregated into one entry.
pub struct ConnectedGuest {
    /// The node number (Wispers Connect) or guest number (iroh).
    pub number: i32,
    pub user_id: String,
    pub connected_since: chrono::DateTime<chrono::Utc>,
}

impl ServingHandle {
    pub(crate) fn new(
        dir: storage::CircleDir,
        config: CircleConfig,
        db: storage::StateDb,
        server: Arc<dyn Server>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                server,
                db,
                reachable_since: RwLock::new(None),
                connections: RwLock::new(HashMap::new()),
                next_connection_id: AtomicU64::new(0),
                started_at: chrono::Utc::now(),
                dir,
                config: RwLock::new(Arc::new(config)),
                events: broadcast::channel(16).0,
            }),
        }
    }

    /// The config as currently served.
    pub async fn config(&self) -> Arc<CircleConfig> {
        self.inner.config.read().await.clone()
    }

    /// Context necessary for handling a stream.
    pub async fn stream_context(&self) -> protocol::StreamContext {
        protocol::StreamContext {
            config: self.config().await,
            events: self.inner.events.clone(),
            db: self.inner.db.clone(),
        }
    }

    /// Re-reads `circle.toml`, but only replaces the running config if the
    /// file passes validation.
    pub async fn reload(&self) -> Result<ReloadOutcome> {
        let fresh = Arc::new(self.inner.dir.load_config()?);
        // Hold the write lock only for the swap; new streams read this lock.
        let changed = {
            let mut current = self.inner.config.write().await;
            let changed = fresh.config_hash() != current.config_hash();
            if changed {
                *current = fresh.clone();
            }
            changed
        };
        if changed {
            info!(
                shares = ?fresh.shares.iter().map(|s| &s.id).collect::<Vec<_>>(),
                "config reloaded"
            );
            let _ = self.inner.events.send(fresh.config_hash());
        }
        Ok(ReloadOutcome {
            changed,
            config: fresh,
        })
    }

    /// Whether guests can reach this server.
    pub async fn reachable(&self) -> bool {
        self.inner.reachable_since.read().await.is_some()
    }

    pub async fn reachable_since(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.inner.reachable_since.read().await
    }

    pub async fn set_reachable(&self) {
        *self.inner.reachable_since.write().await = Some(chrono::Utc::now());
    }

    pub fn started_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.inner.started_at
    }

    /// The guests with a live P2P connection right now, one entry per node
    /// (earliest `connected_since` when a guest holds several connections).
    pub async fn connected_guests(&self) -> Vec<ConnectedGuest> {
        let connections = self.inner.connections.read().await;
        let mut by_number: HashMap<i32, ConnectedGuest> = HashMap::new();
        for p in connections.values() {
            by_number
                .entry(p.number)
                .and_modify(|s| {
                    s.connected_since = s.connected_since.min(p.connected_since);
                })
                .or_insert(ConnectedGuest {
                    number: p.number,
                    user_id: p.user_id.clone(),
                    connected_since: p.connected_since,
                });
        }
        let mut guests: Vec<ConnectedGuest> = by_number.into_values().collect();
        guests.sort_by_key(|g| g.number);
        guests
    }

    /// Tracks a live connection for `status` and, with a closer, for
    /// `revoke` and shutdown.
    pub async fn register_connection(
        &self,
        number: i32,
        user_id: String,
        closer: Option<Closer>,
    ) -> u64 {
        let id = self
            .inner
            .next_connection_id
            .fetch_add(1, Ordering::Relaxed);
        self.inner.connections.write().await.insert(
            id,
            GuestConnection {
                number,
                user_id,
                connected_since: chrono::Utc::now(),
                closer,
            },
        );
        id
    }

    pub async fn unregister_connection(&self, id: u64) {
        self.inner.connections.write().await.remove(&id);
    }

    /// Mints an invite for a new guest.
    pub async fn invite(&self, node_name: &str, user_id: &str) -> Result<wire::Invite> {
        self.inner.server.invite(node_name, user_id).await
    }

    /// Marks the guest revoked and closes its live connections with the
    /// `revoked` code, so it learns at once.
    pub async fn revoke_guest(&self, number: i64) -> Result<storage::GuestNode> {
        let guest = self.inner.server.revoke_guest(number)?;
        self.close_connections(|c| i64::from(c.number) == number, wire::CloseCode::Revoked)
            .await;
        Ok(guest)
    }

    /// Stops serving. When this is called, live connections have already been
    /// told to close first, so no need to close them in an orderly fashion.
    pub async fn shutdown(&self) -> Result<()> {
        self.close_connections(|_| true, wire::CloseCode::Closing)
            .await;
        self.inner.server.shutdown().await
    }

    async fn close_connections(
        &self,
        which: impl Fn(&GuestConnection) -> bool,
        code: wire::CloseCode,
    ) {
        for c in self.inner.connections.read().await.values() {
            if which(c)
                && let Some(close) = &c.closer
            {
                close(code);
            }
        }
    }
}

//-- Shared plumbing -----------------------------------------------------------

/// Resolves when the process receives a shutdown signal (`SIGTERM` or `SIGINT`)
/// If the handlers can't be installed, this never resolves, so the server
/// keeps running rather than shutting down spuriously.
pub(crate) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGTERM handler");
                return std::future::pending::<()>().await;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGINT handler");
                return std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            _ = term.recv() => info!("received SIGTERM"),
            _ = int.recv() => info!("received SIGINT"),
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!(error = %e, "failed to listen for Ctrl-C");
            std::future::pending::<()>().await
        }
    }
}
