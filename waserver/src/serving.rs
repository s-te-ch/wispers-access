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
    wcbe_client: wcbe::Client,
    connectivity_group_id: String,
    local_port: u16,
}

impl ServingHandle {
    pub fn new(local_port: u16, api_key: &str, cg_id: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                wc_handle: RwLock::new(None),
                wcbe_client: wcbe::Client::new(api_key),
                connectivity_group_id: cg_id.to_owned(),
                local_port,
            }),
        }
    }

    pub fn local_port(&self) -> u16 {
        self.inner.local_port
    }

    pub async fn connected_to_hub(&self) -> bool {
        self.inner.wc_handle.read().await.is_some()
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
        let code = handle.generate_activation_code().await?;
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
    }

    async fn wc_handle(&self) -> Option<wc::ServingHandle> {
        self.inner.wc_handle.read().await.clone()
    }
}

/// Run the serving loop. The caller is responsible for initialising logging
/// (foreground or background) before invoking this.
pub async fn serve(share: &str, port: u16) -> Result<()> {
    let store = storage::ShareStateStore::new(share)?;
    let Some(cfg) = store.load_share_config()? else {
        anyhow::bail!("Share {} is not initialised", share);
    };
    let node_storage = wc::NodeStorage::new(store);
    let node = node_storage.restore_or_init_node().await?;
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
    let serving_handle = ServingHandle::new(port, &cfg.api_key, &cg_id);
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

    // Main serving loop. Accept connections from the peer nodes or from IPC.
    loop {
        tokio::select! {
            // Incoming QUIC connection.
            Some(result) = incoming.quic.recv() => {
                tokio::spawn(handle_quic_conn(result, port));
            },
            // Session end.
            result = &mut session_task => break handle_session_end(result),
        }
    }
}

async fn handle_quic_conn(r: Result<wc::QuicConnection, wc::P2pError>, port: u16) {
    let conn = match r {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to accept QUIC connection: {}", e);
            return;
        }
    };
    let peer = conn.peer_node_number;
    info!(peer, "QUIC connection accepted");

    loop {
        match conn.accept_stream().await {
            Ok(stream) => {
                tokio::spawn(async move {
                    if let Err(e) = http::handle_quic_stream(stream, port).await {
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
