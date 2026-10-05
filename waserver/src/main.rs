mod config;
mod guest_api;
mod http;
mod initialization;
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
    use crate::storage::TransportKind;

    match command {
        Command::Init {
            api_key,
            backend,
            transport,
            share,
            display_name,
        } => {
            let backend = normalize_backend(backend.as_deref())?;
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
        Command::Start { share: None } => on_every_share(start_share).await,
        Command::Stop { share: Some(share) } => stop(&share).await,
        Command::Stop { share: None } => on_every_share(stop_share).await,
        Command::Reload { share: Some(share) } => reload(&share).await,
        Command::Reload { share: None } => on_every_share(reload_share).await,
        Command::Status { share, json } => match share {
            Some(share) => status::report_on_share(&share, json).await,
            None => status::report_on_fleet(json).await,
        },
        Command::Logs { follow, share } => logs(follow, share.as_deref()),
        Command::Invite {
            share,
            node_name,
            user_id,
            png,
        } => invite(&share, &node_name, &user_id, png.as_deref()).await,
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

#[cfg(unix)]
fn start_daemon() -> Result<()> {
    let daemonizer = daemonize::Daemonize::new()
        // daemonize defaults to 0o027 post-fork. Set the same mask as in main().
        .umask(0o077);
    daemonizer.start().context("failed to daemonize")?;
    Ok(())
}

/// Windows has no fork, so `start` re-launches itself without a console and
/// marks the copy with this variable. The copy then runs as the daemon.
#[cfg(windows)]
const DAEMON_ENV: &str = "WASERVER_DAEMON";

#[cfg(windows)]
fn start_daemon() -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    if std::env::var_os(DAEMON_ENV).is_some() {
        return Ok(()); // We are the background copy.
    }

    // Make our std handles non-inheritable, so the daemon can't hold on to the
    // caller's pipes and make whoever reads our output wait for it to exit.
    // Nulling the child's stdio is not enough - CreateProcess hands the child
    // every inheritable handle. (std duplicates the child's own stdio
    // explicitly, so that still works.)
    {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // Best effort: absent or invalid handles just fail the call.
            unsafe {
                SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0);
            }
        }
    }

    let exe = std::env::current_exe().context("failed to get current executable path")?;
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(DAEMON_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("failed to spawn background process")?;

    // The background process has been spawned. Exit the starting process.
    std::process::exit(0);
}

/// `start` action for `on_every_share`. Spawns `waserver start <share>`, which
/// daemonises itself, unless a server is running already.
async fn start_share(share: &str) -> Result<ActionOutcome> {
    if ipc::Client::connect(share).await.is_ok() {
        return Ok(ActionOutcome::Done("already running".to_owned()));
    }
    let exe = std::env::current_exe().context("failed to get current executable path")?;
    let status = std::process::Command::new(exe)
        .args(["start", share])
        .status()
        .context("failed to run `waserver start`")?;
    if !status.success() {
        anyhow::bail!("`waserver start {share}` failed ({status})");
    }
    Ok(ActionOutcome::Done("started".to_owned()))
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

/// `reload` action for `on_every_share`.
async fn reload_share(share: &str) -> Result<ActionOutcome> {
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

/// Prints the logs of one share, or of every share with the share's name in
/// front of each line. With `follow`, keeps printing as the servers log.
fn logs(follow: bool, share: Option<&str>) -> Result<()> {
    use std::io::{self, Write};

    let names = match share {
        Some(share) => vec![share.to_owned()],
        None => {
            let mut names = storage::list_shares()?;
            names.sort();
            names
        }
    };
    let mut tails = Vec::new();
    for name in &names {
        match LogTail::open(name)? {
            Some(tail) => tails.push(tail),
            None => eprintln!("No logs for share {name}"),
        }
    }
    if tails.is_empty() {
        return Ok(());
    }
    let prefixed = share.is_none();
    let mut stdout = io::stdout().lock();
    let result = (|| -> Result<()> {
        loop {
            for tail in &mut tails {
                tail.copy_complete_lines(&mut stdout, prefixed)?;
            }
            stdout.flush()?;
            if !follow {
                for tail in &mut tails {
                    tail.copy_rest(&mut stdout, prefixed)?;
                }
                return Ok(());
            }
            for tail in &mut tails {
                tail.pick_up_rotated_files()?;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    })();
    match result {
        // The reader went away (`waserver logs | head`); not an error.
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// A reader over one share's log files, oldest first, that remembers where
/// it got to.
struct LogTail {
    share: String,
    /// The file being read and those that come after it.
    path: std::path::PathBuf,
    file: std::io::BufReader<std::fs::File>,
    later: std::collections::VecDeque<std::path::PathBuf>,
    /// A line the writer has not finished yet.
    partial: Vec<u8>,
}

impl LogTail {
    /// `None` for a share without logs.
    fn open(share: &str) -> Result<Option<Self>> {
        let mut later = std::collections::VecDeque::from(logging::list_log_files(share)?);
        let Some(path) = later.pop_front() else {
            return Ok(None);
        };
        Ok(Some(Self {
            share: share.to_owned(),
            file: Self::open_file(&path)?,
            path,
            later,
            partial: Vec::new(),
        }))
    }

    /// Writes every complete line not written yet, moving on through the
    /// files as each one ends.
    fn copy_complete_lines(&mut self, out: &mut impl std::io::Write, prefixed: bool) -> Result<()> {
        use std::io::BufRead;
        loop {
            let n = self
                .file
                .read_until(b'\n', &mut self.partial)
                .with_context(|| format!("read {}", self.path.display()))?;
            if self.partial.ends_with(b"\n") {
                self.write_partial(out, prefixed)?;
            } else if n == 0 {
                // At the end of this file. The next one, if there is one,
                // starts with a new line.
                let Some(next) = self.later.pop_front() else {
                    return Ok(());
                };
                self.copy_rest(out, prefixed)?;
                self.file = Self::open_file(&next)?;
                self.path = next;
            }
        }
    }

    /// Writes an unfinished last line, when nothing more is coming.
    fn copy_rest(&mut self, out: &mut impl std::io::Write, prefixed: bool) -> Result<()> {
        if !self.partial.is_empty() {
            self.partial.push(b'\n');
            self.write_partial(out, prefixed)?;
        }
        Ok(())
    }

    fn write_partial(&mut self, out: &mut impl std::io::Write, prefixed: bool) -> Result<()> {
        if prefixed {
            write!(out, "{}: ", self.share)?;
        }
        out.write_all(&self.partial)?;
        self.partial.clear();
        Ok(())
    }

    /// Queues files the daily rotation created since the current one.
    fn pick_up_rotated_files(&mut self) -> Result<()> {
        for path in logging::list_log_files(&self.share)? {
            if path > self.path && !self.later.contains(&path) {
                self.later.push_back(path);
            }
        }
        Ok(())
    }

    fn open_file(path: &std::path::Path) -> Result<std::io::BufReader<std::fs::File>> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(std::io::BufReader::new(file))
    }
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
    let req = ipc::Request::GetInvite {
        node_name: node_name.to_owned(),
        user_id: user_id.to_owned(),
    };
    let code = match client.request(&req).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Invite(invite),
            ..
        }) => invite.code,
        Ok(ipc::Response::Success { .. }) => {
            anyhow::bail!("unexpected response from server");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error generating invite: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    };
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
    Ok(())
}

/// Render a QR code to a terminal string that scans regardless of terminal
/// theme.
///
/// `qrcode`'s `unicode::Dense1x2` renderer draws modules in the terminal's
/// *foreground* colour on its *background*, so on a dark terminal the QR comes
/// out inverted (light modules on dark) and scanners — which expect
/// dark-on-light — reject it. Here every module gets an explicit black/white,
/// so it is always dark-on-light. The colours come from the 256-colour palette,
/// not truecolour because a terminal without 24-bit support drops the
/// truecolour codes and renders every line as one solid bar in its default
/// colours, while the palette works everywhere. `▀` (upper half block) packs
/// two module rows per line: the glyph's foreground is the top module, its
/// background the bottom one. (`--png` stays the colour-independent fallback
/// for terminals that strip ANSI.)
fn render_qr_ansi(qr: &qrcode::QrCode) -> String {
    const QUIET: usize = 4; // standard quiet zone, in modules
    const BLACK: &str = "16"; // palette index of #000000
    const WHITE: &str = "231"; // palette index of #ffffff

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
            out.push_str(&format!("\x1b[38;5;{fg}m\x1b[48;5;{bg}m\u{2580}"));
        }
        out.push_str("\x1b[0m\n"); // reset colours at end of each line
        y += 2;
    }
    out
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
