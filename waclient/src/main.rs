mod http;
mod iroh_transport;
mod shares;
mod storage;
mod transports;
mod wispers_connect_transport;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use shares::{Share, ShareRegistry};
use std::sync::Arc;
use transports::{TerminalState, TransportError};
use wispers_access_wire as wire;

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
    // De-conflict rustls.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Join { invite_code } => join(&invite_code).await,
        Command::Serve { port } => serve(port).await,
        Command::List => list().await,
        Command::Remove { share } => remove(&share).await,
    }
}

async fn join(invite_code: &str) -> Result<()> {
    let invite = wire::Invite::parse(invite_code)?;
    let db = storage::DB::new()?;
    let row = db.new_row()?;
    // The row is the share's; the transport writes its part of it and the
    // host node's answer fills the rest. A failed join leaves no row behind.
    let result = async {
        let info = transports::join(invite, &row).await?;
        record_join(&row, &info)
    }
    .await;
    if result.is_err() {
        let _ = row.delete_row();
    }
    result
}

/// The local bookkeeping of any `join`, once the host node has answered with
/// the share: names, the app list, and marking the row complete so it
/// survives the next start.
fn record_join(row: &storage::Row, info: &wire::ShareInfo) -> Result<()> {
    let share_id = row.share_id()?;
    row.write_share_info(info)?;
    let display_name = if info.name.is_empty() {
        share_id.to_string()
    } else {
        info.name.clone()
    };
    row.write_display_name(&display_name)?;
    let hostname = host_slug(&display_name).unwrap_or_else(|| share_id.to_string());
    let hostname = row.write_deduped_hostname(&hostname)?;
    row.mark_complete()?;

    println!(
        "Joined share: {}\n  Label: {}\n  Apps: {}\n  Share id: {}\n",
        display_name,
        hostname,
        describe_apps(&info.apps),
        share_id,
    );
    Ok(())
}

fn describe_apps(apps: &[wire::App]) -> String {
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

async fn list() -> Result<()> {
    use std::io::Write;
    use tabwriter::TabWriter;

    let db = storage::DB::new()?;
    let rows = db.get_all_rows()?;
    if rows.is_empty() {
        println!("No shares joined. Use 'waclient join <invite_code>'.");
        return Ok(());
    }
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);
    writeln!(&mut tw, "Share\tName\tApps\tStatus")?;
    for row in rows {
        let (_, display_name, hostname) = row.read_names()?;
        let state = match row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            Some(state) => state.describe(),
            None => "ok",
        };
        let apps = row
            .read_apps()?
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            &mut tw,
            "{}\t{}\t{}\t{}",
            hostname,
            display_name,
            if apps.is_empty() { "-" } else { &apps },
            state
        )?;
    }
    tw.flush()?;
    Ok(())
}

async fn remove(share: &str) -> Result<()> {
    let db = storage::DB::new()?;
    let row = db
        .find_row(share)?
        .with_context(|| format!("no share '{}' (see 'waclient list')", share))?;

    transports::leave(&row).await?;
    row.delete_row()?;
    println!("Share '{}' removed from this device.", share);
    Ok(())
}

/// Free-form name -> DNS-label-safe slug, or None if nothing usable remains.
fn host_slug(name: &str) -> Option<String> {
    // Remove apostrophes, so "Bob's app" becomes "bobs-app", not "bob-s-app".
    let cleaned = name.replace(['\'', '’'], "");
    // Slugify, but keep it to 63 chars, to produce a legal DNS label.
    let mut s = slug::slugify(cleaned);
    s.truncate(63);
    // Truncation can leave a trailing '-'.
    let s = s.trim_end_matches('-');
    (!s.is_empty()).then(|| s.to_string())
}

async fn serve(port: u16) -> Result<()> {
    // Load every known share. One dead or unreachable share must not take the
    // others down - report it and skip it. Terminal rejections (revocations)
    // get persisted so we never dial the share again.
    let db = storage::DB::new()?;
    let mut registry = ShareRegistry::default();
    println!("Available apps (as last seen; refreshed in the background):");
    for row in db.get_all_rows()? {
        let (share_id, display_name, label) = row.read_names()?;
        if let Some(state) = row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            report_dead_share(&display_name, &label, state);
            registry.insert_dead(label, share_id, state);
            continue;
        }
        match transports::restore(row.clone()).await {
            Ok(transport) => {
                let share = Share::new(label, share_id, display_name, row, transport);
                println!(
                    "  {} ({}) via {}:",
                    share.display_name(),
                    share.label(),
                    share.describe_transport()
                );
                print_app_urls(share.label(), &share.apps()?, port);
                registry.insert(share);
            }
            Err(TransportError::Terminal(state)) => {
                let _ = row.write_terminal_state(state.as_str());
                report_dead_share(&display_name, &label, state);
                registry.insert_dead(label, share_id, state);
            }
            Err(TransportError::Transient(e)) => {
                eprintln!(
                    "  {} — temporarily unavailable ({e:#}), not serving it this run",
                    label
                );
            }
        }
    }
    let registry = Arc::new(registry);

    // Ask every live share's host node whether the app list changed since the
    // last run. Best effort and off the startup path: an unreachable host node
    // just leaves the stored list in place.
    for share in registry.iter() {
        let share = share.clone();
        tokio::spawn(async move {
            match share.refresh().await {
                Ok(Some(info)) => {
                    println!("Updated app list for {} ({}):", info.name, share.label());
                    print_app_urls(share.label(), &info.apps, port);
                }
                Ok(None) => {}
                Err(e) => eprintln!(
                    "[{}] could not refresh the app list: {:#}",
                    share.label(),
                    e
                ),
            }
        });
    }

    http::serve(port, registry).await
}

fn print_app_urls(hostname: &str, apps: &[wire::App], port: u16) {
    if apps.is_empty() {
        println!("    (no apps yet)");
    }
    for app in apps {
        println!(
            "    {:<16} http://{}.{}.localhost:{}",
            app.name, app.id, hostname, port
        );
    }
}

fn report_dead_share(display_name: &str, label: &str, state: TerminalState) {
    eprintln!(
        "  {} ('{}') is no longer available — {}.",
        label,
        display_name,
        state.describe()
    );
    eprintln!("    Run 'waclient remove {}' to clean it up.", label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_descriptions_skip_redundant_names() {
        let apps = vec![
            wire::App {
                id: "echo".into(),
                name: "echo".into(),
                kind: wire::AppKind::Web,
            },
            wire::App {
                id: "jf".into(),
                name: "Jellyfin".into(),
                kind: wire::AppKind::Jellyfin,
            },
        ];
        assert_eq!(describe_apps(&apps), "echo, jf (Jellyfin)");
        assert_eq!(describe_apps(&[]), "none yet");
    }
}
