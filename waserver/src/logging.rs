//! Per-share tracing setup. Foreground mode writes to stderr and a daily-
//! rotated file; the daemonized mode writes only to the file. In daemon mode
//! we install a panic hook that funnels panic messages through tracing.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Once;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Returned by the init functions. Drop this *after* the program is done. When
/// the guard is dropped, the non-blocking writer flushes its buffer and joins
/// its worker thread.
pub struct LoggingHandle {
    #[allow(dead_code)]
    file_guard: WorkerGuard,
}

/// Log to stderr (pretty/colorised) and to a daily-rotated file in
/// [`log_dir`]. Reads `RUST_LOG` for filtering; defaults to `info`.
pub fn init_foreground(share: &str) -> Result<LoggingHandle> {
    let (writer, file_guard) = file_writer(share)?;

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(writer);

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(false)
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter())
        .with(stderr_layer)
        .with(file_layer)
        .init();

    install_panic_hook();
    Ok(LoggingHandle { file_guard })
}

/// Daemon mode: log only to the daily-rotated file. Panics are caught by a
/// hook that routes them through tracing, since the daemon's real stderr is
/// detached.
pub fn init_background(share: &str) -> Result<LoggingHandle> {
    let (writer, file_guard) = file_writer(share)?;

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(writer);

    tracing_subscriber::registry()
        .with(env_filter())
        .with(file_layer)
        .init();

    install_panic_hook();
    Ok(LoggingHandle { file_guard })
}

fn file_writer(share: &str) -> Result<(NonBlocking, WorkerGuard)> {
    let dir = log_dir(share)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating log dir {}", dir.display()))?;
    let appender = tracing_appender::rolling::daily(&dir, "waserver.log");
    Ok(tracing_appender::non_blocking(appender))
}

/// Replace the default panic hook with one that logs through tracing. The
/// default hook writes to stderr, which is detached/silent in daemon mode.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            tracing::error!(location = %location, "panic: {}", payload);
            prev(info);
        }));
    });
}

fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Platform-appropriate log directory for a given share.
///
/// - macOS:   `~/Library/Logs/waserver/<share>/`
/// - Windows: `%LOCALAPPDATA%\waserver\Logs\<share>\`
/// - Linux:   `$XDG_STATE_HOME/waserver/<share>/` (defaults to
///   `~/.local/state/waserver/<share>/`)
pub fn log_dir(share: &str) -> Result<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()
            .context("could not determine home directory")?
            .join("Library/Logs/waserver")
    } else if cfg!(target_os = "windows") {
        dirs::data_local_dir()
            .context("could not determine LocalAppData directory")?
            .join("waserver/Logs")
    } else {
        dirs::state_dir()
            .context("could not determine XDG state directory")?
            .join("waserver")
    };
    Ok(base.join(share))
}
