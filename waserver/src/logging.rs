//! Per-share log files: the tracing setup that writes them, and `waserver
//! logs`, which reads them. Foreground mode writes to stderr and a daily-
//! rotated file, the daemonized mode writes only to the file. In daemon mode we
//! install a panic hook that funnels panic messages through tracing.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
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
    std::fs::create_dir_all(&dir).with_context(|| format!("creating log dir {}", dir.display()))?;
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

//-- `waserver logs` -----------------------------------------------------------

/// Prints the logs of one share, or of every share with the share's name in
/// front of each line. With `follow`, keeps printing as the servers log.
pub fn print(follow: bool, share: Option<&str>) -> Result<()> {
    use std::io::{self, Write};

    let names = match share {
        Some(share) => vec![share.to_owned()],
        None => {
            let mut names = crate::storage::list_shares()?;
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
        // The reader went away (`waserver logs | head`). Not an error.
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
    path: PathBuf,
    file: std::io::BufReader<std::fs::File>,
    later: std::collections::VecDeque<PathBuf>,
    /// A line the writer has not finished yet.
    partial: Vec<u8>,
}

impl LogTail {
    /// `None` for a share without logs.
    fn open(share: &str) -> Result<Option<Self>> {
        let mut later = std::collections::VecDeque::from(list_log_files(share)?);
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

    /// Queues any files the daily rotation created after the current one.
    fn pick_up_rotated_files(&mut self) -> Result<()> {
        for path in list_log_files(&self.share)? {
            if path > self.path && !self.later.contains(&path) {
                self.later.push_back(path);
            }
        }
        Ok(())
    }

    fn open_file(path: &Path) -> Result<std::io::BufReader<std::fs::File>> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(std::io::BufReader::new(file))
    }
}

//-- The log files -------------------------------------------------------------

const LOG_FILE_PREFIX: &str = "waserver.log.";

/// List existing log files for a share, sorted oldest-first. The daily
/// appender writes `waserver.log.YYYY-MM-DD`, which sorts lexicographically
/// in chronological order. Returns an empty vec if the log dir doesn't
/// exist yet.
pub fn list_log_files(share: &str) -> Result<Vec<PathBuf>> {
    let dir = log_dir(share)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("read log dir {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(LOG_FILE_PREFIX))
        })
        .collect();
    files.sort();
    Ok(files)
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
