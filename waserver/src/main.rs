mod http;
mod initialization;
mod ipc;
mod logging;
mod serving;
mod status;
mod storage;
mod wcbe;

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
    /// Initialise a new application share
    Init {
        /// Wispers Connect API key (can also be set via WC_API_KEY env var).
        #[arg(long, env = "WC_API_KEY", hide_env_values = true)]
        api_key: String,
        /// Base URL of a custom Wispers Connect backend (e.g.
        /// `https://myhub.example.com`). Omit to use the managed backend. Must
        /// be https. Pinned into the share and carried in its invite codes.
        #[arg(long, env = "WC_BACKEND")]
        backend: Option<String>,
        /// Name of the application share.
        share: String,
        /// Display name of the application share.
        display_name: String,
    },
    /// De-initialise an existing application share.
    Deinit {
        /// Name of the application share.
        share: String,
    },
    /// Runs the server proxying <name> in the foreground.
    Serve {
        /// Name of the application share.
        share: String,
        /// Upstream to proxy: `host:port`, or a bare `port` (host defaults to
        /// localhost). E.g. `3000`, `127.0.0.1:8080`, or `app:3000` (a Docker
        /// compose service name).
        #[arg(value_parser = parse_upstream)]
        upstream: String,
    },
    /// Runs the server in the background.
    Start {
        /// Name of the application share.
        share: String,
        /// Upstream to proxy: `host:port`, or a bare `port` (host defaults to
        /// localhost). E.g. `3000`, `127.0.0.1:8080`, or `app:3000` (a Docker
        /// compose service name).
        #[arg(value_parser = parse_upstream)]
        upstream: String,
    },
    /// Stops a server that is running in the background.
    Stop {
        /// Name of the application share.
        share: String,
    },
    /// Shows the status of all shares, or a detailed view of one share.
    Status {
        /// Name of an application share to show in detail. Omit for a
        /// one-line-per-share fleet overview.
        share: Option<String>,
        /// Output a JSON document instead of the human-readable rendering.
        /// The JSON shape is the stable interface.
        #[arg(long)]
        json: bool,
    },
    /// Prints the logs of the given server to stdout.
    Logs {
        /// Don't stop at EOF. Instead, wait for more logs to be written.
        #[arg(short = 'f', long)]
        follow: bool,
        share: String,
    },
    /// Generates an guest device invite code.
    Invite {
        /// Name of the application share.
        share: String,
        /// Name of the new node.
        node_name: String,
        /// User identification (e.g. email address) of the user.
        user_id: String,
        /// Also write the invite QR code as a PNG (for emailing) to this path.
        #[arg(long, value_name = "PATH")]
        png: Option<std::path::PathBuf>,
    },
    /// Revoke access.
    Revoke {
        /// Name of the application share.
        share: String,
        /// Node number whose access to revoke.
        node_number: i32,
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

    // Daemonising must happen before starting tokio.
    if let Command::Start { .. } = &cli.command {
        start_daemon()?;
    }

    // Start async mode.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Init {
            api_key,
            backend,
            share,
            display_name,
        } => {
            let backend = normalize_backend(backend.as_deref())?;
            initialization::up(&api_key, &share, &display_name, backend.as_deref()).await
        }
        Command::Deinit { share } => initialization::down(&share).await,
        Command::Serve { share, upstream } => {
            let _log = logging::init_foreground(&share)?;
            serving::serve(&share, upstream).await
        }
        Command::Start { share, upstream } => {
            let _log = logging::init_background(&share)?;
            serving::serve(&share, upstream).await
        }
        Command::Stop { share } => stop(&share).await,
        Command::Status { share, json } => status::run(share.as_deref(), json).await,
        Command::Logs { follow, share } => logs(follow, &share),
        Command::Invite {
            share,
            node_name,
            user_id,
            png,
        } => invite(&share, &node_name, &user_id, png.as_deref()).await,
        Command::Revoke { share, node_number } => revoke(&share, &node_number).await,
    }
}

#[cfg(unix)]
fn start_daemon() -> Result<()> {
    let daemonizer = daemonize::Daemonize::new()
        // The daemonize crate defaults to 0o027 post-fork, which would loosen
        // the 0o077 we set in main().
        .umask(0o077);
    daemonizer.start().context("failed to daemonize")?;
    Ok(())
}

#[cfg(windows)]
fn start_daemon() -> Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // Re-launch ourselves with s/start/serve/.
    let exe = std::env::current_exe().context("failed to get current executable path")?;
    let args: Vec<String> = std::iter::once("serve".to_string())
        .chain(std::env::args().skip(2)) // Skip 'waserver start'.
        .collect();

    std::process::Command::new(exe)
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("failed to spawn background process")?;

    // The background process has been spawned. Exit the starting process.
    std::process::exit(0);
}

async fn stop(share: &str) -> Result<()> {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        anyhow::bail!("cannot connect to server for share {}", share);
    };
    match client.request(&ipc::Request::Shutdown).await {
        Ok(ipc::Response::Success { .. }) => {
            println!("Success!");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error stopping server: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
    Ok(())
}

fn logs(follow: bool, share: &str) -> Result<()> {
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};

    let mut stdout = io::stdout().lock();
    let paths = logging::list_log_files(share)?;
    let mut paths = VecDeque::from(paths);
    let Some(mut path) = paths.pop_front() else {
        eprintln!("No logs for share {}", share);
        return Ok(());
    };
    let mut file =
        std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = [0u8; 8192];
    loop {
        // Copy the entire file to stdout.
        loop {
            let n = file
                .read(&mut buf)
                .with_context(|| format!("read {}", path.display()))?;
            if n == 0 {
                break;
            }
            if let Err(e) = stdout.write_all(&buf[..n]) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(e.into());
            }
        }
        stdout.flush().ok();

        // End of file. If there's another file in the list, continue with that.
        if let Some(next) = paths.pop_front() {
            path = next;
            file =
                std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
            continue;
        }

        // If we're not in follow mode we're done here.
        if !follow {
            break;
        }

        // Check for newer log files due to log rotation.
        for new_path in logging::list_log_files(share)? {
            if new_path > path {
                paths.push_back(new_path);
            }
        }
        if let Some(next) = paths.pop_front() {
            path = next;
            file =
                std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
            continue;
        }

        // Sleep before retry.
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

async fn invite(
    share: &str,
    node_name: &str,
    user_id: &str,
    png: Option<&std::path::Path>,
) -> Result<()> {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        anyhow::bail!("cannot connect to server for share {}", share);
    };
    // The share's pinned backend (if any) gets baked into the invite so the
    // guest client knows which hub to join.
    let backend = storage::ShareStateStore::new(share)?
        .load_share_config()?
        .and_then(|c| c.backend);
    let req = ipc::Request::GetInvite {
        node_name: node_name.to_owned(),
        user_id: user_id.to_owned(),
    };
    match client.request(&req).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Invite(invite),
            ..
        }) => {
            let code = compose_wax_code(
                &invite.registration_token,
                &invite.activation_code,
                backend.as_deref(),
            );
            let qr = qrcode::QrCode::new(code.as_bytes()).context("cannot build QR code")?;
            println!("Invite code (valid for 24 hours):\n\n  {}\n", code);
            println!("{}", render_qr_ansi(&qr));
            if let Some(path) = png {
                let img = qr
                    .render::<image::Luma<u8>>()
                    .min_dimensions(360, 360)
                    .build();
                img.save(path)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                println!("QR code written to {}", path.display());
            }
        }
        Ok(ipc::Response::Success { .. }) => {
            anyhow::bail!("unexpected response from server");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error generating invite: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
    Ok(())
}

/// Render a QR code to a terminal string that scans regardless of terminal theme.
///
/// `qrcode`'s `unicode::Dense1x2` renderer draws modules in the terminal's
/// *foreground* colour on its *background*, so on a dark terminal the QR comes out
/// inverted (light modules on dark) and scanners — which expect dark-on-light —
/// reject it. Here every module gets an explicit truecolour black/white, so it is
/// always dark-on-light. `▀` (upper half block) packs two module rows per line: the
/// glyph's foreground is the top module, its background the bottom one. (`--png`
/// stays the colour-independent fallback for terminals that strip ANSI.)
fn render_qr_ansi(qr: &qrcode::QrCode) -> String {
    const QUIET: usize = 4; // standard quiet zone, in modules
    const BLACK: &str = "0;0;0";
    const WHITE: &str = "255;255;255";

    let w = qr.width();
    let modules = qr.to_colors();
    let size = w + 2 * QUIET;
    // Dark module at (col x, row y)? The quiet-zone border is light.
    let dark = |x: usize, y: usize| -> bool {
        if x < QUIET || y < QUIET || x >= QUIET + w || y >= QUIET + w {
            return false;
        }
        matches!(modules[(y - QUIET) * w + (x - QUIET)], qrcode::Color::Dark)
    };

    let mut out = String::new();
    let mut y = 0;
    while y < size {
        for x in 0..size {
            let fg = if dark(x, y) { BLACK } else { WHITE };
            let bg = if y + 1 < size && dark(x, y + 1) {
                BLACK
            } else {
                WHITE
            };
            out.push_str(&format!("\x1b[38;2;{fg}m\x1b[48;2;{bg}m\u{2580}"));
        }
        out.push_str("\x1b[0m\n"); // reset colours at end of each line
        y += 2;
    }
    out
}

/// Composes the user-facing invite code from its raw parts. The activation
/// code keeps its native `<node>-<secret>` form so the endorsing node stays
/// encoded; the `wax_` prefix makes codes recognizable and greppable.
///
/// If there's a custom backend, appends it as an additional, base32-encoded
/// field.
fn compose_wax_code(
    registration_token: &str,
    activation_code: &str,
    backend: Option<&str>,
) -> String {
    let base = format!("wax_{}_{}", registration_token, activation_code);
    match backend {
        Some(backend) => format!("{}_{}", base, encode_backend(backend)),
        None => base,
    }
}

/// base32 of a backend URL, lowercase, unpadded.
fn encode_backend(backend: &str) -> String {
    data_encoding::BASE32_NOPAD
        .encode(backend.as_bytes())
        .to_lowercase()
}

/// Validate and normalize `--backend`
fn normalize_backend(backend: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = backend else {
        return Ok(None);
    };
    let trimmed = raw.trim().trim_end_matches('/');
    // Allow both unset and empty (useful if set via env var).
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !trimmed.starts_with("https://") {
        anyhow::bail!("backend URL must start with https:// (got '{}')", raw);
    }
    if trimmed.len() <= "https://".len() {
        anyhow::bail!("backend URL has no host");
    }
    Ok(Some(trimmed.to_owned()))
}

async fn revoke(share: &str, node_number: &i32) -> Result<()> {
    println!("revoke({}, {});", share, node_number);
    // TODO: add revocation support to the library. Right now all it has is logout().
    Ok(())
}

/// Parse an upstream dial target in `[host:]port` form into a normalized
/// `host:port` string. A bare port (no colon) uses host `localhost`; an empty
/// host (e.g. `:3000`) defaults the same way. `localhost` rather than
/// `127.0.0.1` so the dial tries both address families — modern Node dev
/// servers (e.g. Vite) often listen on `::1` only. A non-numeric or
/// out-of-range port is rejected. IPv6 literals would need bracket form
/// (`[::1]:3000`) and aren't handled here.
fn parse_upstream(s: &str) -> std::result::Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("upstream is empty".to_string());
    }
    // No colon ⇒ the whole thing is a port; otherwise split off the port after
    // the last colon so the host may itself be a name like `app`.
    let (host, port_str) = s.rsplit_once(':').unwrap_or(("", s));
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("invalid port '{}' (expected 1–65535)", port_str))?;
    if port == 0 {
        return Err("port 0 is not a valid upstream".to_string());
    }
    let host = if host.is_empty() { "localhost" } else { host };
    Ok(format!("{}:{}", host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wax_code_keeps_activation_code_verbatim() {
        assert_eq!(
            compose_wax_code("ab12cd", "1-xyz789", None),
            "wax_ab12cd_1-xyz789"
        );
    }

    #[test]
    fn wax_code_appends_encoded_backend() {
        let code = compose_wax_code("ab12cd", "1-xyz789", Some("https://myhub.example.com"));
        let expected_backend = encode_backend("https://myhub.example.com");
        assert_eq!(code, format!("wax_ab12cd_1-xyz789_{}", expected_backend));
        // The backend field must not reintroduce the '_' delimiter.
        assert!(!expected_backend.contains('_'));
        // Round-trips back to the original URL.
        let decoded = data_encoding::BASE32_NOPAD
            .decode(expected_backend.to_uppercase().as_bytes())
            .unwrap();
        assert_eq!(decoded, b"https://myhub.example.com");
    }

    #[test]
    fn normalize_backend_requires_https_and_trims() {
        assert_eq!(normalize_backend(None).unwrap(), None);
        assert_eq!(
            normalize_backend(Some("https://h.example.com/")).unwrap(),
            Some("https://h.example.com".to_owned())
        );
        assert!(normalize_backend(Some("http://h.example.com")).is_err());
        assert!(normalize_backend(Some("h.example.com")).is_err());
        assert!(normalize_backend(Some("https://")).is_err());
        // Blank/empty (e.g. an unset dashboard env) is managed, not an error.
        assert_eq!(normalize_backend(Some("")).unwrap(), None);
        assert_eq!(normalize_backend(Some("   ")).unwrap(), None);
    }

    #[test]
    fn parses_upstream_forms() {
        assert_eq!(parse_upstream("8080").unwrap(), "localhost:8080");
        assert_eq!(parse_upstream("app:3000").unwrap(), "app:3000");
        assert_eq!(parse_upstream("127.0.0.1:8080").unwrap(), "127.0.0.1:8080");
        assert_eq!(parse_upstream(":3000").unwrap(), "localhost:3000");
        assert_eq!(parse_upstream("  app:3000\n").unwrap(), "app:3000");
    }

    #[test]
    fn rejects_bad_upstream() {
        assert!(parse_upstream("").is_err());
        assert!(parse_upstream("app").is_err()); // no port
        assert!(parse_upstream("app:").is_err()); // empty port
        assert!(parse_upstream("app:abc").is_err()); // non-numeric port
        assert!(parse_upstream("app:0").is_err()); // port 0
        assert!(parse_upstream("app:99999").is_err()); // out of range
    }
}
