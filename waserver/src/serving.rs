//! Serving logic - handles hub connection and incoming P2P connections.

use crate::storage;
use crate::wcbe;
use anyhow::Result;
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
    // TODO:
    // - Open IPC (UDS or local port)
    // - Serving loop
    
    Ok(())
}
