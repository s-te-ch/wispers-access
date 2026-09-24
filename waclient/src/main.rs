use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wispers_access_sdk as sdk;

#[derive(Parser)]
#[command(name = "waclient", version)]
#[command(about = "Wispers Access client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Join a Wispers Access share.
    Join {
        /// Invite code for the share (`wax_…`), produced by `waserver invite`.
        invite_code: String,
    },
    /// Serve every joined share's apps on localhost, as
    /// `http://<app>.<share>.localhost:<port>`.
    Serve { port: u16 },
    /// Show all joined shares, their apps and their state.
    List,
    /// Remove a share from this device, deregistering from its hub when possible.
    Remove {
        /// The share's label, as shown by `waclient list`.
        share: String,
    },
}

fn main() -> Result<()> {
    // Restrict default file mode to user-only. Safe to do as the first thing.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }

    let cli = Cli::parse();
    init_logging();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    let client = sdk::Client::new_with_runtime(
        sdk::ClientConfig {
            data_dir: data_dir()?.to_string_lossy().into_owned(),
            secrets: None,
            observer: None,
        },
        tokio::runtime::Handle::current(),
    )?;
    match command {
        Command::Join { invite_code } => join(&client, &invite_code).await,
        Command::Serve { port } => serve(&client, port).await,
        Command::List => list(&client),
        Command::Remove { share } => remove(&client, &share).await,
    }
}

/// The SDK's own lines at `info`, its dependencies only when they complain.
/// `RUST_LOG` overrides.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,wispers_access_sdk=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .init();
}

fn data_dir() -> Result<std::path::PathBuf> {
    let config_dir = dirs::config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("waclient"))
}

async fn join(client: &sdk::Client, invite_code: &str) -> Result<()> {
    let share = client.join(invite_code.to_owned()).await?;
    println!(
        "Joined share: {}\n  Label: {}\n  Apps: {}\n  Share id: {}\n",
        share.name,
        share.label,
        describe_apps(&share.apps),
        share.id,
    );
    Ok(())
}

fn describe_apps(apps: &[sdk::App]) -> String {
    if apps.is_empty() {
        return "none yet".to_owned();
    }
    apps.iter()
        .map(|s| {
            if s.name == s.id {
                s.id.clone()
            } else {
                format!("{} ({})", s.id, s.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn list(client: &sdk::Client) -> Result<()> {
    use std::io::Write;
    use tabwriter::TabWriter;

    let shares = client.shares()?;
    if shares.is_empty() {
        println!("No shares joined. Use 'waclient join <invite_code>'.");
        return Ok(());
    }
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);
    writeln!(&mut tw, "Share\tName\tApps\tStatus")?;
    for share in shares {
        let apps = share
            .apps
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            &mut tw,
            "{}\t{}\t{}\t{}",
            share.label,
            share.name,
            if apps.is_empty() { "-" } else { &apps },
            describe_state(share.state)
        )?;
    }
    tw.flush()?;
    Ok(())
}

fn describe_state(state: sdk::ShareState) -> &'static str {
    match state {
        sdk::ShareState::Live => "ok",
        sdk::ShareState::Removed => "the share was removed by its host node",
        sdk::ShareState::Revoked => "this device's access was revoked",
    }
}

async fn remove(client: &sdk::Client, share: &str) -> Result<()> {
    let found = client
        .share(share.to_owned())?
        .with_context(|| format!("no share '{}' (see 'waclient list')", share))?;
    client.leave(found.id).await?;
    println!("Share '{}' removed from this device.", share);
    Ok(())
}

async fn serve(client: &sdk::Client, port: u16) -> Result<()> {
    let proxy = client.start_host_routed_proxy(port, None).await?;
    println!("Listening on localhost:{}", proxy.port());

    // Every share as last seen. Report but don't serve dead ones.
    println!("Available apps (as last seen; refreshed in the background):");
    let shares = client.shares()?;
    for share in &shares {
        if share.state != sdk::ShareState::Live {
            report_dead_share(share);
            continue;
        }
        println!(
            "  {} ({}) via {}:",
            share.name,
            share.label,
            share.transport.as_str()
        );
        print_app_urls(&proxy, share);
    }

    // Ask every live share's host node whether the config changed since the
    // last run. Best effort and off the startup path - an unreachable host node
    // just leaves the stored copy in place.
    for share in shares
        .into_iter()
        .filter(|s| s.state == sdk::ShareState::Live)
    {
        let client = client.clone();
        let proxy = proxy.clone();
        tokio::spawn(async move {
            match client.refresh(share.id.clone()).await {
                Ok(Some(share)) if share.state != sdk::ShareState::Live => {
                    report_dead_share(&share)
                }
                Ok(Some(share)) => {
                    println!("Updated app list for {} ({}):", share.name, share.label);
                    print_app_urls(&proxy, &share);
                }
                Ok(None) => {}
                Err(e) => eprintln!("[{}] could not refresh the share: {:#}", share.label, e),
            }
        });
    }

    // Wait forever to let the proxy do its thing.
    std::future::pending().await
}

fn print_app_urls(proxy: &sdk::HostRoutedProxy, share: &sdk::Share) {
    if share.apps.is_empty() {
        println!("    (no apps yet)");
    }
    for app in &share.apps {
        let url = proxy
            .base_url(share.id.clone(), app.id.clone())
            .unwrap_or_else(|e| format!("({e})"));
        println!("    {:<16} {}", app.name, url);
    }
}

fn report_dead_share(share: &sdk::Share) {
    eprintln!(
        "  {} ('{}') is no longer available — {}.",
        share.label,
        share.name,
        describe_state(share.state)
    );
    eprintln!("    Run 'waclient remove {}' to clean it up.", share.label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_descriptions_skip_redundant_names() {
        let apps = vec![
            sdk::App {
                id: "echo".into(),
                name: "echo".into(),
                kind: sdk::AppKind::Web,
            },
            sdk::App {
                id: "jf".into(),
                name: "Jellyfin".into(),
                kind: sdk::AppKind::Jellyfin,
            },
        ];
        assert_eq!(describe_apps(&apps), "echo, jf (Jellyfin)");
        assert_eq!(describe_apps(&[]), "none yet");
    }
}
