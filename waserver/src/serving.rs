//! Serving logic - handles hub connection and incoming P2P connections.

use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use wispers_connect as wc;

pub async fn serve(share: &str, _port: &u16) -> Result<()> {
    let store = storage::ShareStateStore::new(share)?;
    let Some(cfg) = store.load_share_config()? else {
        anyhow::bail!("Share {} is not initialised", share);
    };
    let _wcbe_client = wcbe::Client::new(&cfg.api_key);
    let node_storage = wc::NodeStorage::new(store);
    let node = node_storage.restore_or_init_node().await?;
    if !node.is_registered() {
        anyhow::bail!("Wispers Connect node is not registered");
    }
    let ipc_server = ipc::Server::bind(share).await?;
    let mut connect_task = connect_to_hub(node);

    // Wait for the hub connection, but already accept IPC requests.
    let (handle, session, mut incoming) = loop {
        tokio::select! {
            result = &mut connect_task => {
                match result {
                    Ok(Ok(triple)) => break triple,
                    Ok(Err(e)) => return Err(e),
                    Err(e) => return Err(anyhow::anyhow!("Connect task panicked: {}", e)),
                }
            }
            result = ipc_server.accept() => handle_ipc_conn(result).await,
        }
    };
    println!("Connected to hub");

    // Run the Wispers serving session.
    let mut session_task = tokio::spawn(async move { session.run().await });

    // Main serving loop. Accept connections from the peer nodes or from IPC.
    loop {
        tokio::select! {
            // Incoming QUIC connection.
            Some(result) = incoming.quic.recv() => {
                tokio::spawn(handle_quic_conn(result));
            },
            // Incoming IPC connection.
            result = ipc_server.accept() => handle_ipc_conn(result).await,
            // Session end.
            result = &mut session_task => break handle_session_end(result),
        }
    }
}

fn connect_to_hub(
    node: wc::Node,
) -> tokio::task::JoinHandle<
    Result<(
        wc::ServingHandle,
        wc::ServingSession,
        wc::IncomingConnections,
    )>,
> {
    tokio::spawn(async move {
        node.start_serving()
            .await
            .context("failed to start serving")
    })
}

async fn handle_ipc_conn(r: Result<ipc::IpcStream>) {
    let stream = match r {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to accept daemon connection: {}", e);
            return;
        }
    };
    let (reader, _) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut req = String::new();
    match reader.read_line(&mut req).await {
        Ok(_) => {
            println!("Received IPC request: {}", req);
        }
        Err(e) => {
            eprintln!("Failed to read from IPC client: {}", e);
        }
    }
}

async fn handle_quic_conn(r: Result<wc::QuicConnection, wc::P2pError>) {
    let _conn = match r {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to accept QUIC connection: {}", e);
            return;
        }
    };
    println!("QUIC connection accepted");
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
