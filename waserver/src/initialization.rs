//! Share initialisation & tear-down

use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};

/// Initialise a new app share.
pub async fn up(
    api_key: &str,
    share: &str,
    display_name: &str,
    backend: Option<&str>,
) -> Result<()> {
    // Check for valid name & non-existence.
    if !is_valid_share_name(share) {
        anyhow::bail!("Invalid share name '{}'", share);
    }
    let store = storage::ShareStateStore::new(share)?;
    if store.load_share_config()?.is_some() {
        anyhow::bail!("Share {} already exists", share);
    }

    // Create a Wispers connectivity group for the app share.
    let wcbe_client = wcbe::Client::new(api_key, &wcbe::api_base(backend));
    let cg_id = wcbe_client.add_connectivity_group(display_name).await?;

    // Write the ShareConfig.
    let cfg = storage::ShareConfig::new(api_key, &cg_id, backend);
    store.save_share_config(&cfg)?;

    // Create the serving Wispers node.
    let node_storage = wispers_connect::NodeStorage::new(store);
    if let Some(backend) = backend {
        node_storage.override_hub_addr(backend);
    }
    let mut node = node_storage.restore_or_init_node().await?;

    // Register the node with the Wispers backend.
    let token = wcbe_client
        .get_registration_token(&cg_id, Some("Server"), None /* metadata */)
        .await?;
    node.register(&token).await.context("registration failed")?;

    Ok(())
}

pub async fn down(share: &str) -> Result<()> {
    let store = storage::ShareStateStore::new(share)?;
    let Some(cfg) = store.load_share_config()? else {
        anyhow::bail!("Could not find share '{}'", share);
    };

    // Refuse to tear down a share while its server is still running.
    if let Ok(mut client) = ipc::Client::connect(share).await {
        // A reachable socket means a daemon is serving this share. Name its
        // port in the message if it answers promptly, but don't hang on a
        // wedged daemon.
        let port_hint = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.request(&ipc::Request::Status),
        )
        .await
        {
            Ok(Ok(ipc::Response::Success {
                data: ipc::ResponseData::Status(status),
                ..
            })) => format!(" on {}", status.upstream),
            _ => String::new(),
        };
        anyhow::bail!(
            "share '{}' has a running server{}; stop it first with `waserver stop {}`",
            share,
            port_hint,
            share
        );
    }

    // Remove the Wispers connectivity group. This deregisters all nodes.
    let wcbe_client = wcbe::Client::new(&cfg.api_key, &wcbe::api_base(cfg.backend.as_deref()));
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
