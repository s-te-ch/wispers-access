//! Share initialisation & tear-down

use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use wispers_connect;

/// Initialise a new app share.
pub async fn up(api_key: &str, share: &str) -> Result<()> {
    // Check for valid name & non-existence.
    if !is_valid_share_name(share) {
        anyhow::bail!("Invalid share name '{}'", share);
    }
    let store = storage::ShareStateStore::new(share)?;
    if let Some(_) = store.load_share_config()? {
        anyhow::bail!("Share {} already exists", share);
    }

    // Create a Wispers connectivity group for the app share.
    let wcbe_client = wcbe::Client::new(api_key);
    let cg_id = wcbe_client.add_connectivity_group(share).await?;

    // Write the ShareConfig.
    let cfg = storage::ShareConfig::new(&api_key, &cg_id);
    store.save_share_config(&cfg)?;

    // Create the serving Wispers node.
    let node_storage = wispers_connect::NodeStorage::new(store);
    let mut node = node_storage.restore_or_init_node().await?;

    // Register the node with the Wispers backend.
    let token = wcbe_client
        .get_registration_token(&cg_id, "Server".into())
        .await?;
    node.register(&token).await.context("registration failed")?;

    Ok(())
}

pub async fn down(share: &str) -> Result<()> {
    let store = storage::ShareStateStore::new(share)?;
    let Some(cfg) = store.load_share_config()? else {
        anyhow::bail!("Could not find share '{}'", share);
    };
    // Remove the Wispers connectivity group. This deregisters all nodes.
    let wcbe_client = wcbe::Client::new(&cfg.api_key);
    wcbe_client
        .remove_connectivity_group(&cfg.connectivity_group_id)
        .await?;
    // Remove the store directory. This removes both the share config and the
    // node state.
    store.delete()?;
    Ok(())
}

fn is_valid_share_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64  // pick your limit
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !(s.starts_with('-') || s.starts_with('_'))
        && !(s.ends_with('-') || s.ends_with('_'))
}
