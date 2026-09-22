//! `init` and `deinit`.

use crate::config::{self, TransportConfig};
use crate::ipc;
use crate::iroh_transport;
use crate::storage;
use crate::wispers_connect_transport;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

pub async fn up(
    api_key: Option<&str>,
    share: &str,
    display_name: &str,
    transport: &TransportConfig,
) -> Result<()> {
    let dir = storage::ShareDir::new(share)?;
    if dir.exists() {
        anyhow::bail!("Share {} already exists", share);
    }

    // Nothing may be left behind on failure: `init` refuses to run on an
    // existing directory, and an orphaned group would consume quota.
    let mut rollback = Rollback::new();
    match create_share(&mut rollback, &dir, api_key, display_name, transport).await {
        Ok(()) => {
            println!(
                "Share {} initialised. Add its apps to {} and run `waserver serve {}`.",
                share,
                dir.config_path().display(),
                share
            );
            Ok(())
        }
        Err(e) => {
            rollback.run().await;
            Err(e)
        }
    }
}

/// Creates the share the way its transport needs: the directory with
/// config and state, plus whatever the transport keeps beyond this machine.
async fn create_share(
    rollback: &mut Rollback,
    dir: &storage::ShareDir,
    api_key: Option<&str>,
    display_name: &str,
    transport: &TransportConfig,
) -> Result<()> {
    let config_text = config::render_template(display_name, transport);
    match transport {
        TransportConfig::WispersConnect { backend } => {
            wispers_connect_transport::init(
                rollback,
                dir,
                &config_text,
                api_key,
                display_name,
                backend.as_deref(),
            )
            .await
        }
        TransportConfig::Iroh {} => iroh_transport::init(dir, &config_text),
    }
}

/// Undo actions for the steps of `init` that have succeeded so far, run in
/// reverse when a later step fails. Each failure to undo is reported and the
/// rest still run.
pub(crate) struct Rollback {
    steps: Vec<(&'static str, Undo)>,
}

type Undo = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

impl Rollback {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub(crate) fn push(
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

pub async fn down(share: &str) -> Result<()> {
    let dir = storage::ShareDir::new(share)?;
    let cfg = dir.load_config()?;
    let wcs = dir.open_state()?.wispers_connect_state()?;

    // Refuse to tear down a share while its server is still running.
    if let Ok(mut client) = ipc::Client::connect(share).await {
        // A reachable socket means a daemon is serving this share. Name its
        // apps in the message if it answers promptly, but don't hang on a
        // wedged daemon.
        let apps_hint = match tokio::time::timeout(
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
                    .apps
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        anyhow::bail!(
            "share '{}' has a running server{}; stop it first with `waserver stop {}`",
            share,
            apps_hint,
            share
        );
    }

    // Whatever the transport keeps beyond this machine goes first, so a
    // failure there leaves the share intact to try again.
    match &cfg.transport {
        TransportConfig::WispersConnect { backend } => {
            wispers_connect_transport::deinit(wcs, backend.as_deref()).await?
        }
        // Nothing beyond this machine: guests that are offline now will
        // find the endpoint gone, which is all they can know.
        TransportConfig::Iroh {} => {}
    }
    // Remove the directory: config file and state database.
    dir.delete()?;
    Ok(())
}
