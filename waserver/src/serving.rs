//! Serving logic - handles hub connection and incoming P2P connections.

use crate::http;
use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use wispers_connect as wc;

#[derive(Clone)]
pub struct ServingHandle {
    inner: Arc<Inner>,
}

struct Inner {
    wc_handle: RwLock<Option<wc::ServingHandle>>,
    wcbe_client: wcbe::Client,
    local_port: u16,
}

impl ServingHandle {
    pub fn new(local_port: u16, api_key: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                wc_handle: RwLock::new(None),
                wcbe_client: wcbe::Client::new(api_key),
                local_port,
            }),
        }
    }

    async fn set_wc_handle(&self, handle: wc::ServingHandle) {
        *self.inner.wc_handle.write().await = Some(handle);
    }

    async fn wc_handle(&self) -> Option<wc::ServingHandle> {
        self.inner.wc_handle.read().await.clone()
    }
}

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
    let serving_handle = ServingHandle::new(port, &cfg.api_key);
    let ipc_server = ipc::Server::bind(share).await?;
    tokio::spawn(ipc_server.run(serving_handle.clone()));

    // Connect to the hub.
    let (wc_handle, session, mut incoming) = node
        .start_serving()
        .await
        .context("starting Wispers node serving loop")?;
    serving_handle.set_wc_handle(wc_handle).await;
    println!("Connected to hub");

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
            eprintln!("Failed to accept QUIC connection: {}", e);
            return;
        }
    };
    let peer = conn.peer_node_number;
    println!("QUIC connection from {} accepted", peer);

    loop {
        match conn.accept_stream().await {
            Ok(stream) => {
                tokio::spawn(http::handle_quic_stream(stream, port));
            }
            Err(e) => {
                eprintln!("QUIC connection with {} closed: {}", peer, e);
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
            println!("Session ended normally");
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
