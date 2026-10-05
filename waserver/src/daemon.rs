//! Running a share's server in the background: `waserver start`.

use crate::ActionOutcome;
use crate::ipc;
use anyhow::{Context, Result};

#[cfg(unix)]
pub fn start_daemon() -> Result<()> {
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
pub fn start_daemon() -> Result<()> {
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
pub async fn start_share(share: &str) -> Result<ActionOutcome> {
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
