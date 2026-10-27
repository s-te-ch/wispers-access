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
        /// Invite code for the share (`wax1_…`), produced by `waserver invite`.
        invite_code: String,
    },
    /// Serve every joined share's apps on localhost.
    Serve { port: u16 },
    /// Show all joined shares, their apps and their state.
    List,
    /// Remove a share from this device, deregistering from its hub when possible.
    Remove {
        /// The share's label, as shown by `waclient list`.
        share: String,
    },
    /// Print the certificates required to use the given share's apps, so the
    /// user can import them into their browser. These certificates are needed
    /// to use forward-proxied apps if they proxy internal HTTPS services that
    /// don't have publicly valid certficiates.
    Certificates {
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
        Command::Certificates { share } => certificates(&client, &share),
    }
}

/// Set up logging so the SDK's own lines log at `info`, its dependencies only
/// when they complain. Use `RUST_LOG` to override.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Turn off rustls-platform-verifier's logging because
        // `report_untrusted_origins` runs untrusted origins through it on
        // purpose and prints its own messages for them.
        EnvFilter::new("warn,wispers_access_sdk=info,rustls_platform_verifier=off")
    });
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

fn describe_apps(apps: &[sdk::SharedApp]) -> String {
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
        sdk::ShareState::Outdated => "the host node needs a newer version of waclient",
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
    // The URLs are printed once and opened whenever the user gets to them,
    // so their pairing tokens last as long as this process does.
    let auth_mode = sdk::ProxyAuthMode::Pairing {
        token_lifetime: std::time::Duration::MAX,
    };
    let proxy = client.start_host_routed_proxy(port, auth_mode).await?;
    let forward_proxy = client.start_per_share_forward_proxy();

    // Every share as last seen. Report but don't serve dead ones.
    let shares = client.shares()?;
    let (live, dead): (Vec<_>, Vec<_>) = shares
        .iter()
        .partition(|share| share.state == sdk::ShareState::Live);
    dead.into_iter().for_each(report_dead_share);
    print_apps(&proxy, &forward_proxy, &live).await;

    // Ask every live share's host node whether the config changed since the
    // last run, then check whether this device trusts the certificates of its
    // forward-proxy apps' origins, by a TLS handshake through the share. Best
    // effort and off the startup path - an unreachable host node just leaves
    // the stored copy in place.
    for share in shares.into_iter().filter(|s| !s.state.is_terminal()) {
        let client = client.clone();
        let proxy = proxy.clone();
        let forward_proxy = forward_proxy.clone();
        tokio::spawn(async move {
            let share = match client.refresh(share.id.clone()).await {
                Ok(Some(share)) if share.state != sdk::ShareState::Live => {
                    report_dead_share(&share);
                    return;
                }
                Ok(Some(share)) => {
                    println!("\nUpdated app list for {} ({}).", share.name, share.label);
                    print_apps(&proxy, &forward_proxy, &[&share]).await;
                    share
                }
                Ok(None) => share,
                Err(e) => {
                    eprintln!("[{}] could not refresh the share: {:#}", share.label, e);
                    return;
                }
            };
            report_untrusted_origins(&client, &share).await;
        });
    }

    // Wait forever to let the proxy do its thing.
    std::future::pending().await
}

/// Link to documentation for setting up a browser for a share's proxy.
const FORWARD_PROXY_HELP: &str =
    "https://github.com/s-te-ch/wispers-access/blob/main/docs/waclient.md#forward-proxied-apps";

/// Prints the shares' apps in one block per mode, as the modes open their
/// apps differently. A mode without apps gets no block.
async fn print_apps(
    proxy: &sdk::HostRoutedProxy,
    forward_proxy: &sdk::PerShareForwardProxy,
    shares: &[&sdk::Share],
) {
    let in_mode = |forward: bool| {
        shares.iter().map(move |share| {
            let apps: Vec<_> = share
                .apps
                .iter()
                .filter(|app| (app.mode == sdk::AppProxyMode::ForwardProxy) == forward)
                .collect();
            (*share, apps)
        })
    };
    // A share without apps goes with the reverse-proxy apps, as the default.
    let reverse: Vec<_> = in_mode(false)
        .filter(|(share, apps)| !apps.is_empty() || share.apps.is_empty())
        .collect();
    let forward: Vec<_> = in_mode(true).filter(|(_, apps)| !apps.is_empty()).collect();
    if reverse.is_empty() && forward.is_empty() {
        println!();
        println!("No live shares to serve.");
    }
    if !reverse.is_empty() {
        println!();
        println!("## Reverse-proxied apps");
        println!("(Each URL pairs your system browser on first use)");
        for (share, apps) in reverse {
            if apps.is_empty() {
                println!("* {} - no apps yet", describe_share(share));
                continue;
            }
            println!("* {}", describe_share(share));
            for app in apps {
                print_reverse_proxy_app(proxy, share, app);
            }
        }
    }
    if !forward.is_empty() {
        println!();
        println!("## Forward-proxied apps");
        println!(
            "(Configure your browser to use a share's proxy to use its apps. See {FORWARD_PROXY_HELP} for instructions)"
        );
        for (share, apps) in forward {
            let port = match forward_proxy.share_port(share.id.clone()).await {
                Ok(port) => port,
                Err(e) => {
                    println!("* {} - no proxy port ({e})", describe_share(share));
                    continue;
                }
            };
            println!(
                "* {}, proxy 127.0.0.1:{port}{}",
                describe_share(share),
                describe_certificates(share)
            );
            for app in apps {
                let start_url = app.start_url.as_deref().unwrap_or("no start URL");
                println!("  * {} - {}", app.name, start_url);
            }
        }
    }
}

fn describe_share(share: &sdk::Share) -> String {
    format!("{} ({})", share.name, share.label)
}

fn print_reverse_proxy_app(proxy: &sdk::HostRoutedProxy, share: &sdk::Share, app: &sdk::SharedApp) {
    if app.mode == sdk::AppProxyMode::Unknown {
        println!("  * {} - needs a newer version of waclient", app.name);
        return;
    }
    let url = proxy
        .browse_url(share.id.clone(), app.id.clone())
        .unwrap_or_else(|e| format!("({e})"));
    println!("  * {} - {}", app.name, url);
}

/// The share's own certificates, as a suffix for its line. Empty when the
/// share has none and its servers have to be trusted already.
fn describe_certificates(share: &sdk::Share) -> String {
    match certificate_subjects(share).as_slice() {
        [] => String::new(),
        [subject] => format!(", using custom certificate from {subject}"),
        subjects => format!(", using custom certificates from {}", subjects.join(", ")),
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

/// Says which of the share's `https` origins this device's certificate store
/// doesn't trust, and how to get the share's certificates. Nothing when every
/// origin is trusted or out of reach.
async fn report_untrusted_origins(client: &sdk::Client, share: &sdk::Share) {
    if share.state != sdk::ShareState::Live {
        return;
    }
    let checked = match client.check_origin_trust(share.id.clone()).await {
        Ok(checked) => checked,
        Err(e) => {
            eprintln!(
                "[{}] could not check the share's certificates: {e:#}",
                share.label
            );
            return;
        }
    };
    let untrusted: Vec<&str> = checked
        .iter()
        .filter(|checked| matches!(checked.verdict, sdk::TrustVerdict::Untrusted { .. }))
        .map(|checked| checked.origin.trim_start_matches("https://"))
        .collect();
    if untrusted.is_empty() {
        return;
    }
    eprintln!(
        "[{}] This device's certificate store doesn't trust {}.",
        share.label,
        untrusted.join(", ")
    );
    let subjects = certificate_subjects(share);
    let label = &share.label;
    match subjects.len() {
        0 => eprintln!(
            "[{label}] The share has no certificate for it, so whoever runs the share has to fix this."
        ),
        1 => eprintln!(
            "[{label}] The share comes with a certificate of {}. Save it with `waclient certificates {label} > {label}.pem`, then {}.",
            subjects[0],
            import_hint("it", &format!("{label}.pem")),
        ),
        _ => eprintln!(
            "[{label}] The share comes with certificates of {}. Save them with `waclient certificates {label} > {label}.pem`, then {}.",
            subjects.join(", "),
            import_hint("each", &format!("{label}.pem")),
        ),
    }
}

/// How to make the browsers trust `which` of the share's certificates, saved
/// in `file`. Chrome and Firefox on macOS and Windows trust the user's store,
/// on Linux only their profile's.
#[cfg(target_os = "macos")]
fn import_hint(which: &str, file: &str) -> String {
    format!(
        "add {which} to your login keychain, trusted for SSL (`security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db {file}`)"
    )
}

#[cfg(target_os = "windows")]
fn import_hint(which: &str, file: &str) -> String {
    format!("add {which} to your root certificates (`certutil -user -addstore Root {file}`)")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn import_hint(which: &str, file: &str) -> String {
    format!(
        "import {which} from {file} in your browser's certificate settings, as an authority it trusts for websites. This check reads the system's store, which Chrome and Firefox don't, so it keeps saying so"
    )
}

fn certificates(client: &sdk::Client, share: &str) -> Result<()> {
    let found = client
        .share(share.to_owned())?
        .with_context(|| format!("no share '{}' (see 'waclient list')", share))?;
    if found.ca_certificates.is_empty() {
        eprintln!("Share '{}' comes with no certificates.", share);
    }
    for pem in &found.ca_certificates {
        print!("{}", pem.trim_end());
        println!();
    }
    Ok(())
}

/// The subjects of the share's certificates, quoted.
fn certificate_subjects(share: &sdk::Share) -> Vec<String> {
    share
        .ca_certificates
        .iter()
        .filter_map(|pem| sdk::describe_certificate(pem.clone()))
        .map(|description| format!("“{}”", description.subject))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_descriptions_skip_redundant_names() {
        let apps = vec![
            sdk::SharedApp {
                id: "echo".into(),
                name: "echo".into(),
                kind: sdk::AppKind::Web,
                mode: sdk::AppProxyMode::ReverseProxy,
                start_url: None,
                proxied_origins: Vec::new(),
            },
            sdk::SharedApp {
                id: "jf".into(),
                name: "Jellyfin".into(),
                kind: sdk::AppKind::Jellyfin,
                mode: sdk::AppProxyMode::ReverseProxy,
                start_url: None,
                proxied_origins: Vec::new(),
            },
        ];
        assert_eq!(describe_apps(&apps), "echo, jf (Jellyfin)");
        assert_eq!(describe_apps(&[]), "none yet");
    }
}
