//! `init` and `deinit`.

use crate::config::{self, TransportConfig};
use crate::ipc;
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use std::future::Future;
use std::pin::Pin;

pub async fn up(
    api_key: &str,
    circle: &str,
    display_name: &str,
    transport: &TransportConfig,
) -> Result<()> {
    let dir = storage::CircleDir::new(circle)?;
    if dir.exists() {
        anyhow::bail!("Circle {} already exists", circle);
    }

    // Nothing may be left behind on failure: `init` refuses to run on an
    // existing directory, and an orphaned group would consume quota.
    let mut rollback = Rollback::new();
    match create(&mut rollback, &dir, api_key, display_name, transport).await {
        Ok(()) => {
            println!(
                "Circle {} initialised. Add its shares to {} and run `waserver serve {}`.",
                circle,
                dir.config_path().display(),
                circle
            );
            Ok(())
        }
        Err(e) => {
            rollback.run().await;
            Err(e)
        }
    }
}

/// The steps of `init`, each pushing its undo before the next runs: the
/// backend group, the circle directory with config and state, the node and
/// its registration.
async fn create(
    rollback: &mut Rollback,
    dir: &storage::CircleDir,
    api_key: &str,
    display_name: &str,
    transport: &TransportConfig,
) -> Result<()> {
    let TransportConfig::WispersConnect { backend } = transport;
    let backend = backend.as_deref();
    let wcbe_client = wcbe::Client::new(api_key, &wcbe::api_base(backend));

    let cg_id = wcbe_client
        .add_connectivity_group(display_name)
        .await
        .map_err(explain_group_quota)?;
    rollback.push("connectivity group", {
        let (client, cg_id) = (wcbe_client.clone(), cg_id.clone());
        async move { client.remove_connectivity_group(&cg_id).await }
    });

    let state = dir.create(&config::render_template(display_name, transport))?;
    rollback.push("circle directory", {
        let dir = dir.clone();
        async move { dir.delete().map_err(Into::into) }
    });
    state.set_wispers_connect_state(&storage::WispersConnectState {
        api_key: api_key.to_owned(),
        connectivity_group_id: cg_id.clone(),
    })?;

    // Create the serving Wispers node and register it with the backend. The
    // registration goes away with the group, so it needs no undo of its own.
    let node_storage = wispers_connect::NodeStorage::new(state);
    if let Some(backend) = backend {
        node_storage.override_hub_addr(backend);
    }
    let mut node = node_storage.restore_or_init_node().await?;
    let token = wcbe_client
        .get_registration_token(&cg_id, Some("Server"), None /* metadata */)
        .await?;
    node.register(&token).await.context("registration failed")?;
    Ok(())
}

/// Undo actions for the steps of `init` that have succeeded so far, run in
/// reverse when a later step fails. Each failure to undo is reported and the
/// rest still run.
struct Rollback {
    steps: Vec<(&'static str, Undo)>,
}

type Undo = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

impl Rollback {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn push(
        &mut self,
        what: &'static str,
        undo: impl Future<Output = Result<()>> + Send + 'static,
    ) {
        self.steps.push((what, Box::pin(undo)));
    }

    async fn run(self) {
        for (what, undo) in self.steps.into_iter().rev() {
            if let Err(e) = undo.await {
                eprintln!("Init failed; could not undo the {what} either ({e:#}).");
            }
        }
    }
}

pub async fn down(circle: &str) -> Result<()> {
    let dir = storage::CircleDir::new(circle)?;
    let cfg = dir.load_config()?;
    let wcs = dir.open_state()?.wispers_connect_state()?;

    // Refuse to tear down a circle while its server is still running.
    if let Ok(mut client) = ipc::Client::connect(circle).await {
        // A reachable socket means a daemon is serving this circle. Name its
        // shares in the message if it answers promptly, but don't hang on a
        // wedged daemon.
        let shares_hint = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.request(&ipc::Request::Status),
        )
        .await
        {
            Ok(Ok(ipc::Response::Success {
                data: ipc::ResponseData::Status(status),
                ..
            })) => format!(
                " serving {}",
                status
                    .shares
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        anyhow::bail!(
            "circle '{}' has a running server{}; stop it first with `waserver stop {}`",
            circle,
            shares_hint,
            circle
        );
    }

    // Remove the Wispers connectivity group. This deregisters all nodes.
    if let Some(wcs) = wcs {
        let TransportConfig::WispersConnect { backend } = &cfg.transport;
        let wcbe_client = wcbe::Client::new(&wcs.api_key, &wcbe::api_base(backend.as_deref()));
        wcbe_client
            .remove_connectivity_group(&wcs.connectivity_group_id)
            .await?;
    }
    // Remove the directory: config file and state database.
    dir.delete()?;
    Ok(())
}

/// Group creation is where the plan's connectivity-group quota bites.
/// Make the error actionable.
fn explain_group_quota(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<wcbe::QuotaExceeded>() {
        Some(q) if q.quota == "groups_per_domain" => anyhow::anyhow!(
            "cannot create a new circle: your plan's connectivity-group quota \
             is used up ({} of {}). Delete an unused circle with `waserver \
             deinit <circle>` or upgrade your plan.",
            q.current,
            q.limit
        ),
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_quota_error_names_the_way_out() {
        let quota = anyhow::Error::new(wcbe::QuotaExceeded {
            quota: "groups_per_domain".to_owned(),
            limit: 3,
            current: 3,
        });
        let msg = format!("{}", explain_group_quota(quota));
        assert!(msg.contains("3 of 3"), "{msg}");
        assert!(msg.contains("waserver deinit"), "{msg}");

        // Other errors pass through untouched.
        let other = anyhow::Error::new(wcbe::QuotaExceeded {
            quota: "nodes_per_group".to_owned(),
            limit: 12,
            current: 12,
        });
        let msg = format!("{}", explain_group_quota(other));
        assert!(!msg.contains("waserver deinit"), "{msg}");
    }
}
