//! Management of the browser profiles used to access apps using
//! forward-proxying.

use crate::browsers::Browser;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};
use wispers_access_sdk::ShareId;

/// Manages every share's profiles, in `<root>/<share>/<browser>`. Each browser gets a
/// directory of its own, as none can safely open another's.
pub struct ProfileManager {
    root: PathBuf,
    /// Shares whose profile was restarted since one of their apps was last
    /// opened, for the share's pane to say why.
    restarted: Mutex<HashSet<ShareId>>,
}

impl ProfileManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            restarted: Mutex::new(HashSet::new()),
        }
    }

    /// Opens `url` in the share's profile of the first installed browser,
    /// pointed at `port`. A running profile opens it in a new tab.
    pub fn open(&self, share: &ShareId, port: u16, url: &str) -> io::Result<()> {
        let (browser, executable) = Browser::first_installed().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Needs {} installed", Browser::NAMES),
            )
        })?;
        self.restarted.lock().expect("unpoisoned").remove(share);
        self.profile(share, browser)
            .start(&executable, port, Some(url))
    }

    /// Restarts each of the share's running profiles that was started with
    /// another port than `port`, its tabs restored. Its traffic would
    /// otherwise go to whoever holds the old port now, which may be another
    /// user's process.
    pub fn restart_if_port_moved(&self, share: &ShareId, port: u16) -> io::Result<()> {
        for browser in Browser::ALL {
            let profile = self.profile(share, browser);
            if profile.started_port().is_none_or(|started| started == port) || !profile.is_running()
            {
                continue;
            }
            tracing::info!(%share, ?browser, port, "the share's port moved, restarting its profile");
            let executable = browser.executable().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("{browser:?} is gone"))
            })?;
            profile.quit()?;
            profile.start(&executable, port, None)?;
            self.restarted
                .lock()
                .expect("unpoisoned")
                .insert(share.clone());
        }
        Ok(())
    }

    /// Whether [`Self::restart_if_port_moved`] restarted a profile of the
    /// share since its apps were last opened.
    pub fn was_restarted(&self, share: &ShareId) -> bool {
        self.restarted.lock().expect("unpoisoned").contains(share)
    }

    /// Quits the share's running profiles and deletes them, with their
    /// cookies and storage.
    pub fn remove(&self, share: &ShareId) -> io::Result<()> {
        for browser in Browser::ALL {
            let profile = self.profile(share, browser);
            if profile.is_running() {
                profile.quit()?;
            }
        }
        match std::fs::remove_dir_all(self.root.join(share.as_str())) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    fn profile(&self, share: &ShareId, browser: Browser) -> Profile {
        Profile {
            browser,
            dir: self
                .root
                .join(share.as_str())
                .join(browser.profile_dir_name()),
        }
    }
}

/// One share's profile in one browser.
struct Profile {
    browser: Browser,
    dir: PathBuf,
}

/// The file in a profile's directory that holds the port it was last
/// started with.
const PORT_FILE: &str = "wispers-access-port";

/// How long a quitting profile gets before restarting or deleting it fails.
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

impl Profile {
    /// Starts the browser on the profile, which opens `url` or, without one,
    /// restores the last session. If the profile is running already, the new
    /// process hands `url` to it and exits, and the port stays the one the
    /// profile was started with, so its port file stays as it is too.
    fn start(&self, executable: &Path, port: u16, url: Option<&str>) -> io::Result<()> {
        let running = self.is_running();
        if !running {
            std::fs::create_dir_all(&self.dir)?;
            std::fs::write(self.dir.join(PORT_FILE), port.to_string())?;
        }
        let mut command = Command::new(executable);
        if self.browser.is_firefox() {
            if !running {
                std::fs::write(self.dir.join("user.js"), firefox_prefs(port))?;
            }
            command.arg("-profile").arg(&self.dir);
        } else {
            command
                .arg(self.chromium_profile_arg())
                .arg(format!("--proxy-server=127.0.0.1:{port}"))
                // Chromium sends loopback names to the proxy only with this.
                .arg("--proxy-bypass-list=<-loopback>")
                .arg("--no-first-run")
                .arg("--no-default-browser-check");
            if url.is_none() {
                command.arg("--restore-last-session");
            }
        }
        command.args(url);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reaped on a thread of its own, as a browser that started an instance
        // runs as long as the user keeps it open.
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    /// The port the profile was last started with, `None` if never.
    fn started_port(&self) -> Option<u16> {
        std::fs::read_to_string(self.dir.join(PORT_FILE))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn is_running(&self) -> bool {
        self.main_processes(&processes()).next().is_some()
    }

    /// Asks the profile's browser to quit, and waits until it has. Windows
    /// has no signal for that, so there it is killed, which may lose its
    /// last few seconds of writes.
    fn quit(&self) -> io::Result<()> {
        for process in self.main_processes(&processes()) {
            if process.kill_with(Signal::Term).is_none() {
                process.kill();
            }
        }
        let deadline = Instant::now() + QUIT_TIMEOUT;
        while self.is_running() {
            if Instant::now() > deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{:?} didn't quit the share's profile", self.browser),
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    /// The browser processes running the profile, told apart by the profile
    /// argument it was started with. Helper processes are left out: they
    /// quit with their browser.
    fn main_processes<'a>(&'a self, system: &'a System) -> impl Iterator<Item = &'a Process> {
        let profile_arg = self.chromium_profile_arg();
        let mut firefox_args = OsString::from("-profile ");
        firefox_args.push(&self.dir);
        system.processes().values().filter(move |process| {
            let cmd = process.cmd();
            if self.browser.is_firefox() {
                has_arguments(cmd, firefox_args.as_encoded_bytes())
                    && !has_arguments(cmd, b"-contentproc")
            } else {
                has_arguments(cmd, profile_arg.as_encoded_bytes())
                    && !has_argument_prefix(cmd, b"--type=")
            }
        })
    }

    fn chromium_profile_arg(&self) -> OsString {
        let mut arg = OsString::from("--user-data-dir=");
        arg.push(&self.dir);
        arg
    }
}

/// Every process, with its command line.
fn processes() -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    system
}

/// Whether the command line holds `wanted` as a whole argument, or as a run
/// of whole arguments like `-profile <dir>`. The arguments are joined with
/// spaces and matched at space boundaries rather than one by one, because
/// Chromium on Linux rewrites its argv into one space-separated string (its
/// process title), so `/proc/<pid>/cmdline` holds a single argument. A
/// profile directory with a space in its name can't be told apart there.
fn has_arguments(cmd: &[OsString], wanted: &[u8]) -> bool {
    let joined = join(cmd);
    argument_starts(&joined, wanted)
        .any(|start| joined.get(start + wanted.len()).is_none_or(|b| *b == b' '))
}

/// Whether an argument of the command line starts with `prefix`.
fn has_argument_prefix(cmd: &[OsString], prefix: &[u8]) -> bool {
    let joined = join(cmd);
    argument_starts(&joined, prefix).next().is_some()
}

fn join(cmd: &[OsString]) -> Vec<u8> {
    cmd.iter()
        .map(|arg| arg.as_encoded_bytes())
        .collect::<Vec<_>>()
        .join(&b' ')
}

/// The offsets in `joined` at which an argument starting with `wanted` begins.
fn argument_starts<'a>(joined: &'a [u8], wanted: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    (0..=joined.len().saturating_sub(wanted.len()))
        .filter(move |&i| (i == 0 || joined[i - 1] == b' ') && joined[i..].starts_with(wanted))
}

/// The Firefox profile's preferences, written before each start. Firefox
/// sends loopback names to the proxy only with `allow_hijacking_localhost`.
/// The profile always restores its last session, so that a restart for a
/// moved port keeps its tabs. The rest skips the screens of a first start.
fn firefox_prefs(port: u16) -> String {
    format!(
        r#"// Written by Wispers Access each time it starts this profile.
user_pref("network.proxy.type", 1);
user_pref("network.proxy.http", "127.0.0.1");
user_pref("network.proxy.http_port", {port});
user_pref("network.proxy.ssl", "127.0.0.1");
user_pref("network.proxy.ssl_port", {port});
user_pref("network.proxy.no_proxies_on", "");
user_pref("network.proxy.allow_hijacking_localhost", true);
user_pref("security.enterprise_roots.enabled", true);
user_pref("browser.startup.page", 3);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn arguments_are_found_one_by_one_and_in_chromiums_single_string() {
        let dir = "--user-data-dir=/home/u/.local/share/app/profiles/s/chrome";
        let separate = args(&[
            "/opt/chrome",
            dir,
            "--proxy-server=127.0.0.1:1",
            "https://x/",
        ]);
        let single = args(&[&format!(
            "/opt/chrome {dir} --proxy-server=127.0.0.1:1 https://x/"
        )]);
        let helper = args(&[&format!("/opt/chrome --type=renderer {dir} --lang=en")]);
        let other_profile = args(&[&format!("/opt/chrome {dir}2 https://x/")]);
        for cmd in [&separate, &single] {
            assert!(has_arguments(cmd, dir.as_bytes()));
            assert!(!has_argument_prefix(cmd, b"--type="));
        }
        assert!(has_arguments(&helper, dir.as_bytes()));
        assert!(has_argument_prefix(&helper, b"--type="));
        assert!(!has_arguments(&other_profile, dir.as_bytes()));
        assert!(!has_arguments(&separate, b"user-data-dir=/home/u"));
    }

    #[test]
    fn firefox_profile_pair_is_found() {
        let cmd = args(&[
            "/usr/lib/firefox/firefox",
            "-profile",
            "/home/u/p",
            "https://x/",
        ]);
        assert!(has_arguments(&cmd, b"-profile /home/u/p"));
        assert!(!has_arguments(&cmd, b"-profile /home/u"));
        assert!(!has_arguments(&cmd, b"-contentproc"));
    }
}
