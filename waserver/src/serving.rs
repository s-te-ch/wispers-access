//! Serving logic - handles hub connection and incoming P2P connections.

use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::Result;
use tokio::io::AsyncWriteExt;
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

    loop {
        tokio::select! {
            // New IPC connection.
            result = ipc_server.accept() => {
                match result {
                    Ok(stream) => {
                        let (_, mut writer) = stream.into_split();
                        if let Err(e) = writer.write_all(b"hello\n").await {
                            eprintln!("Failed to write response: {}", e);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to accept daemon connection: {}", e);
                    }
                }
            }
        }
    }
}
