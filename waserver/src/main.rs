mod config;
mod daemon;
mod editing;
mod guest_api;
mod http;
mod initialization;
mod invites;
mod ipc;
mod iroh_transport;
mod logging;
mod protocol;
mod serving;
mod status;
mod storage;
mod wcbe;
mod wispers_connect_transport;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "waserver", version)]
#[command(about = "Wispers Access server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialise a new share.
    Init {
        /// Wispers Connect API key (can also be set via WC_API_KEY env var).
        /// Required when using Wispers Connect, unused otherwise.
        #[arg(long, env = "WC_API_KEY", hide_env_values = true)]
        api_key: Option<String>,
        /// Optional override for the Wispers Connect backend (e.g.
        /// `https://myhub.example.com`. Allows using a self-hosted backend
        #[arg(long, env = "WC_BACKEND")]
        backend: Option<String>,
        /// Which peer-to-peer transport library to use for this share.
        #[arg(long, default_value = "iroh")]
        transport: storage::TransportKind,
        /// Share identifier (use letters, digits, '-' or '_').
        share: String,
        /// Human readable name of the share, shown to users.
        display_name: String,
    },
    /// De-initialise a share. Irreversible.
    Deinit {
        /// Name of the share.
        share: String,
    },
    /// Run the server for the given share in the foreground.
    Serve { share: String },
    /// Run the server for a share in the background.
    Start {
        /// Share ID. All shares if omitted.
        share: Option<String>,
    },
    /// Stop a share's server.
    Stop {
        /// Share ID. All shares if omitted.
        share: Option<String>,
    },
    /// Reload a share's configuration (`share.toml`). A broken configuration
    /// leaves the running config in place.
    Reload {
        /// Share ID. All shares if omitted.
        share: Option<String>,
    },
    /// Open a share's configuration (`share.toml`) in your editor ($VISUAL or
    /// $EDITOR) and reload the server with it.
    Edit {
        /// Share ID.
        share: String,
    },
    /// Shows the status of all shares, or a detailed view of one share.
    Status {
        /// ID of a share to show in detail.
        share: Option<String>,
        /// Write stable, machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print a share's server logs to stdout.
    Logs {
        /// Don't stop at EOF, follow log entries as they're written.
        #[arg(short = 'f', long)]
        follow: bool,
        /// Share ID. All shares if omitted.
        share: Option<String>,
    },
    /// Generate a guest node invite code.
    Invite {
        /// Share ID.
        share: String,
        /// Name of the new node (e.g. "Alice's phone").
        node_name: String,
        /// User ID (e.g. email address) tied to the invite.
        user_id: String,
        /// Also write the invite QR code as a PNG to this path, for emailing.
        #[arg(long, value_name = "PATH")]
        png: Option<std::path::PathBuf>,
    },
    /// Revoke access.
    Revoke {
        share: String,
        /// The node's number in `waserver status`.
        number: i64,
    },
}

fn main() -> Result<()> {
    // Restrict default file mode to user-only. None of files waserver writes
    // have an obvious reason to be group- or world-readable. This is marked
    // unsafe because it changes global state, but doing so as the first thing
    // is safe.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }

    // De-conflict rustls. reqwest pulls it in via the aws-lc-rs provider
    // feature, and wispers-connect via the ring feature. We have to choose one.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    // Parse the command line.
    let cli = Cli::parse();

    // Daemonising must happen before starting tokio, so if the command is
    // `waserver start <share>`, do it now. Note that the for-all-shares version
    // (`waserver start`) stays in the foreground and spawns one `waserver start
    // <share>` for each share.
    if let Command::Start { share: Some(_) } = &cli.command {
        daemon::start_daemon()?;
    }

    // Start async mode.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    use crate::storage::TransportKind;

    match command {
        Command::Init {
            api_key,
            backend,
            transport,
            share,
            display_name,
        } => {
            let backend = initialization::normalize_backend(backend.as_deref())?;
            if transport == TransportKind::Iroh && backend.is_some() {
                anyhow::bail!("--transport iroh doesn't take --backend");
            }
            initialization::up(
                &share,
                &display_name,
                transport,
                api_key.as_deref(),
                backend.as_deref(),
            )
            .await
        }
        Command::Deinit { share } => initialization::down(&share).await,
        Command::Serve { share } => {
            let _log = logging::init_foreground(&share)?;
            serving::serve(&share).await
        }
        Command::Start { share: Some(share) } => {
            let _log = logging::init_background(&share)?;
            serving::serve(&share)
                .await
                // At this point, stderr is gone, so we write the error to the log.
                .inspect_err(|e| tracing::error!(error = format!("{e:#}"), "server failed"))
        }
        Command::Start { share: None } => on_every_share(daemon::start_share).await,
        Command::Stop { share: Some(share) } => stop(&share).await,
        Command::Stop { share: None } => on_every_share(stop_share).await,
        Command::Reload { share: Some(share) } => reload(&share).await,
        Command::Reload { share: None } => on_every_share(reload_share).await,
        Command::Edit { share } => editing::edit(&share).await,
        Command::Status { share, json } => match share {
            Some(share) => status::report_on_share(&share, json).await,
            None => status::report_on_fleet(json).await,
        },
        Command::Logs { follow, share } => logging::print(follow, share.as_deref()),
        Command::Invite {
            share,
            node_name,
            user_id,
            png,
        } => invites::invite(&share, &node_name, &user_id, png.as_deref()).await,
        Command::Revoke { share, number } => revoke(&share, number).await,
    }
}

/// Outcome of a per-share action as used by `on_every_share`.
enum ActionOutcome {
    NotRunning,
    Done(String),
}

/// Runs `action` on every share, in name order, one line per share. Tries
/// them all before failing on any share's error.
async fn on_every_share(action: impl AsyncFn(&str) -> Result<ActionOutcome>) -> Result<()> {
    let mut names = storage::list_shares()?;
    names.sort();
    if names.is_empty() {
        println!("No shares.");
        return Ok(());
    }
    let mut failed = 0;
    for name in &names {
        match action(name).await {
            Ok(ActionOutcome::Done(what)) => println!("{name}: {what}"),
            Ok(ActionOutcome::NotRunning) => println!("{name}: not running"),
            Err(e) => {
                eprintln!("{name}: {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {} share(s) failed", names.len());
    }
    Ok(())
}

async fn stop(share: &str) -> Result<()> {
    match stop_share(share).await? {
        ActionOutcome::Done(_) => println!("Success!"),
        ActionOutcome::NotRunning => anyhow::bail!("cannot connect to server for share {}", share),
    }
    Ok(())
}

/// `stop` action for `on_every_share`.
async fn stop_share(share: &str) -> Result<ActionOutcome> {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        return Ok(ActionOutcome::NotRunning);
    };
    match client.request(&ipc::Request::Shutdown).await {
        Ok(ipc::Response::Success { .. }) => Ok(ActionOutcome::Done("stopped".to_owned())),
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error stopping server: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
}

async fn reload(share: &str) -> Result<()> {
    match reload_share(share).await? {
        ActionOutcome::Done(what) => println!("{what}"),
        ActionOutcome::NotRunning => anyhow::bail!("cannot connect to server for share {}", share),
    }
    Ok(())
}

/// `reload` action for `on_every_share`. pub(crate) because `edit` uses it to
/// apply the new config.
pub(crate) async fn reload_share(share: &str) -> Result<ActionOutcome> {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        return Ok(ActionOutcome::NotRunning);
    };
    match client.request(&ipc::Request::Reload).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Reload(r),
            ..
        }) => {
            let ids: Vec<&str> = r.apps.iter().map(|s| s.id.as_str()).collect();
            let verdict = if r.changed { "Reloaded" } else { "No change" };
            Ok(ActionOutcome::Done(format!(
                "{verdict}. Serving {} app(s): {}",
                ids.len(),
                ids.join(", ")
            )))
        }
        Ok(ipc::Response::Success { .. }) => {
            anyhow::bail!("unexpected response from server");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("reload failed, running config kept: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
}

/// Revokes a guest node's access.
async fn revoke(share: &str, number: i64) -> Result<()> {
    let dir = storage::ShareDir::new(share)?;
    match dir.open_state()?.transport()? {
        storage::TransportKind::WispersConnect => {
            let node_number = i32::try_from(number).context("not a node number")?;
            wispers_connect_transport::revoke(share, dir, node_number).await
        }
        storage::TransportKind::Iroh => iroh_transport::revoke(share, dir, number).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_share_init() {
        // The default needs neither a transport nor a key.
        let cli = Cli::try_parse_from(["waserver", "init", "team", "Awesome Team"]).unwrap();
        match cli.command {
            Command::Init {
                share,
                display_name,
                api_key,
                transport,
                ..
            } => {
                assert_eq!(share, "team");
                assert_eq!(display_name, "Awesome Team");
                assert_eq!(api_key, None);
                assert_eq!(transport, storage::TransportKind::Iroh);
            }
            _ => panic!("parsed the wrong command"),
        }
        match Cli::try_parse_from([
            "waserver",
            "init",
            "--transport",
            "wispers-connect",
            "--api-key",
            "k",
            "f",
            "F",
        ])
        .unwrap()
        .command
        {
            Command::Init {
                api_key, transport, ..
            } => {
                assert_eq!(api_key.as_deref(), Some("k"));
                assert_eq!(transport, storage::TransportKind::WispersConnect);
            }
            _ => panic!("parsed the wrong command"),
        }
    }
}
