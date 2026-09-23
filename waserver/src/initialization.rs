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

    // Make sure we roll back failed init runs, so retries don't get stuck.
    let mut rollback = Rollback::new();

    // Generate the `share.toml` contents.
    let config_text = config::render_template(display_name, transport);

    // Run the transport-specific parts.
    let result = match transport {
        TransportConfig::WispersConnect { backend } => {
            wispers_connect_transport::init(
                &mut rollback,
                &dir,
                &config_text,
                api_key,
                display_name,
                backend.as_deref(),
            )
            .await
        }
        TransportConfig::Iroh {} => iroh_transport::init(&mut rollback, &dir, &config_text),
    };

    // Handle success/error.
    match result {
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

pub async fn down(share: &str) -> Result<()> {
    let dir = storage::ShareDir::new(share)?;
    let cfg = dir.load_config()?;
    let wcs = dir.open_state()?.wispers_connect_state()?;

    // Refuse to tear down a share while its server is still running.
    if let Ok(mut client) = ipc::Client::connect(share).await {
        // A reachable socket means a daemon is serving this share. Name its
        // apps in the message if it answers promptly, but don't hang on a
        // wedged daemon.
        let status = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.request(&ipc::Request::Status),
        )
        .await;

        let apps_hint = match status {
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

    // Run transport-specific deinit.
    match &cfg.transport {
        TransportConfig::WispersConnect { backend } => {
            wispers_connect_transport::deinit(wcs, backend.as_deref()).await?
        }
        TransportConfig::Iroh {} => {
            // iroh has nothing beyond what dir.delete() below removes.
        }
    }
    // Remove the directory incl. config file and state database.
    dir.delete()?;
    Ok(())
}

/// Undo stack that allows rolling back `init` steps if a later one failed.
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
