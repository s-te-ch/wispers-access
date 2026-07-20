//! Serving logic - handles hub connection and incoming P2P connections.

use crate::http;
use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use wispers_connect as wc;

#[derive(Clone)]
pub struct ServingHandle {
    inner: Arc<Inner>,
}

struct Inner {
    wc_handle: RwLock<Option<wc::ServingHandle>>,
    connected_since: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    started_at: chrono::DateTime<chrono::Utc>,
    wcbe_client: wcbe::Client,
    connectivity_group_id: String,
    upstream: Arc<str>,
}

impl ServingHandle {
    pub fn new(upstream: Arc<str>, api_key: &str, cg_id: &str, api_base: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                wc_handle: RwLock::new(None),
                connected_since: RwLock::new(None),
                started_at: chrono::Utc::now(),
                wcbe_client: wcbe::Client::new(api_key, api_base),
                connectivity_group_id: cg_id.to_owned(),
                upstream,
            }),
        }
    }

    /// The upstream address (`host:port`) this server proxies to.
    pub fn upstream(&self) -> &str {
        &self.inner.upstream
    }

    pub async fn connected_to_hub(&self) -> bool {
        self.inner.wc_handle.read().await.is_some()
    }

    pub fn started_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.inner.started_at
    }

    pub async fn connected_since(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.inner.connected_since.read().await
    }

    pub async fn get_registration_token(&self, node_name: &str, user_id: &str) -> Result<String> {
        let metadata = wcbe::NodeMetadata {
            user_id: user_id.to_owned(),
        };
        self.inner
            .wcbe_client
            .get_registration_token(
                &self.inner.connectivity_group_id,
                Some(node_name),
                Some(&metadata),
            )
            .await
    }

    pub async fn get_activation_code(&self) -> Result<String> {
        let Some(handle) = self.wc_handle().await else {
            anyhow::bail!("Not connected to hub");
        };
        // Invites are delivered out-of-band (chat, email, QR), so use the
        // long-lived profile; the interactive one expires in two minutes.
        let code = handle
            .generate_activation_code_with_ttl(wc::TtlProfile::Asynchronous)
            .await?;
        Ok(code.format())
    }

    pub async fn shutdown(&self) -> Result<()> {
        match self.wc_handle().await {
            Some(handle) => handle.shutdown().await.context("shutdown failed"),
            None => Ok(()),
        }
    }

    async fn set_wc_handle(&self, handle: wc::ServingHandle) {
        *self.inner.wc_handle.write().await = Some(handle);
        *self.inner.connected_since.write().await = Some(chrono::Utc::now());
    }

    async fn wc_handle(&self) -> Option<wc::ServingHandle> {
        self.inner.wc_handle.read().await.clone()
    }
}

pub async fn serve(share: &str, upstream: String) -> Result<()> {
    let upstream: Arc<str> = upstream.into();
    let store = storage::ShareStateStore::new(share)?;
    let Some(cfg) = store.load_share_config()? else {
        anyhow::bail!("Share {} is not initialised", share);
    };
    let node_storage = wc::NodeStorage::new(store);
    if let Some(backend) = cfg.backend.as_deref() {
        node_storage.override_hub_addr(backend);
    }
    let node = Arc::new(node_storage.restore_or_init_node().await?);
    if !node.is_registered() {
        anyhow::bail!("Wispers Connect node is not registered");
    }

    // Start serving IPC requests to handle local requests.
    let cg_id = match node.connectivity_group_id() {
        Some(id) => id.to_string(),
        None => {
            anyhow::bail!("server node not registered");
        }
    };
    let serving_handle = ServingHandle::new(
        upstream.clone(),
        &cfg.api_key,
        &cg_id,
        &wcbe::api_base(cfg.backend.as_deref()),
    );
    let ipc_server = ipc::Server::bind(share).await?;
    tokio::spawn(ipc_server.run(serving_handle.clone()));

    // Connect to the hub.
    let (wc_handle, session, mut incoming) = node
        .start_serving()
        .await
        .context("starting Wispers node serving loop")?;
    serving_handle.set_wc_handle(wc_handle).await;
    info!("Connected to hub");

    // Run the Wispers serving session.
    let mut session_task = tokio::spawn(async move { session.run().await });

    // Resolves on SIGTERM/SIGINT so we tear the hub session down cleanly
    // instead of being killed mid-flight (e.g. by a container supervisor).
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    // Main serving loop. Accept connections from the peer nodes or from IPC.
    loop {
        tokio::select! {
            // Incoming QUIC connection.
            Some(result) = incoming.quic.recv() => {
                tokio::spawn(handle_quic_conn(result, upstream.clone(), node.clone()));
            },
            // Session end.
            result = &mut session_task => break handle_session_end(result),
            // Shutdown signal: stop the session the same way `waserver stop`
            // does, drain it, then exit cleanly. This stop was intended, so we
            // report success (exit 0) regardless of how the session terminates.
            _ = &mut shutdown => {
                info!("shutdown signal received; shutting down gracefully");
                if let Err(e) = serving_handle.shutdown().await {
                    warn!(error = format!("{:#}", e), "error during graceful shutdown");
                }
                let _ = (&mut session_task).await;
                break Ok(());
            }
        }
    }
}

async fn handle_quic_conn(
    r: Result<wc::QuicConnection, wc::P2pError>,
    upstream: Arc<str>,
    node: Arc<wc::Node>,
) {
    let conn = match r {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to accept QUIC connection: {}", e);
            return;
        }
    };
    let peer = conn.peer_node_number;
    info!(peer, "QUIC connection accepted");

    // Resolve the peer's identity once per connection.
    let user_id = resolve_identity(&node, peer).await;
    match &user_id {
        Some(uid) => info!(peer, user_id = %uid, "resolved peer identity"),
        None => warn!(
            peer,
            "no identity resolved; forwarding without identity header"
        ),
    }

    loop {
        match conn.accept_stream().await {
            Ok(stream) => {
                let user_id = user_id.clone();
                let upstream = upstream.clone();
                tokio::spawn(async move {
                    if let Err(e) = http::handle_quic_stream(stream, upstream, user_id).await {
                        error!(error = format!("{:#}", e), "QUIC stream handler error");
                    }
                });
            }
            Err(e) => {
                warn!(peer, error = %e, "QUIC connection closed");
                break;
            }
        }
    }
}

/// Best-effort lookup of the peer's `user_id` from its node metadata, via the hub.
async fn resolve_identity(node: &wc::Node, peer: i32) -> Option<String> {
    let group = match node.group_info().await {
        Ok(g) => g,
        Err(e) => {
            warn!(peer, error = %e, "could not fetch group info for identity");
            return None;
        }
    };
    let info = group.nodes.iter().find(|n| n.node_number == peer)?;
    let metadata: wcbe::NodeMetadata = serde_json::from_str(&info.metadata).ok()?;
    Some(metadata.user_id).filter(|s| !s.is_empty())
}

fn handle_session_end(
    result: Result<Result<(), wc::ServingError>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(Ok(())) => {
            info!("Session ended normally");
        }
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!("Session error: {}", e));
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Session task panicked: {}", e));
        }
    }
    Ok(())
}

/// Resolves when the process receives a shutdown signal (`SIGTERM` or `SIGINT`)
/// If the handlers can't be installed, this never resolves, so the server
/// keeps running rather than shutting down spuriously.
async fn shutdown_signal() {
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
