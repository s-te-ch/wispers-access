//! Finding the installed browsers that can run a share's profile (see
//! `profiles`), on each OS.

use std::path::PathBuf;

/// The supported browsers, in the order the first installed one is picked.
/// Each takes a profile directory and a proxy on its command line, which
/// Safari doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Chromium,
    Firefox,
}

impl Browser {
    pub const ALL: [Browser; 5] = [
        Browser::Chrome,
        Browser::Edge,
        Browser::Brave,
        Browser::Chromium,
        Browser::Firefox,
    ];

    /// For the user, when none is installed.
    pub const NAMES: &str = "Chrome, Edge, Brave, Chromium or Firefox";

    /// The first installed browser, with its executable.
    pub fn first_installed() -> Option<(Browser, PathBuf)> {
        Self::ALL
            .into_iter()
            .find_map(|browser| Some((browser, browser.executable()?)))
    }

    /// The name of its profile directories, one in each share's.
    pub fn profile_dir_name(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Edge => "edge",
            Browser::Brave => "brave",
            Browser::Chromium => "chromium",
            Browser::Firefox => "firefox",
        }
    }

    pub fn is_firefox(self) -> bool {
        self == Browser::Firefox
    }

    /// The browser's executable, if it's installed. On macOS that is the
    /// binary inside the app bundle: `open -a` would hand the arguments to
    /// a running instance of the user's own profile, or drop them.
    #[cfg(target_os = "macos")]
    pub fn executable(self) -> Option<PathBuf> {
        let (bundle, binary) = match self {
            Browser::Chrome => ("Google Chrome", "Google Chrome"),
            Browser::Edge => ("Microsoft Edge", "Microsoft Edge"),
            Browser::Brave => ("Brave Browser", "Brave Browser"),
            Browser::Chromium => ("Chromium", "Chromium"),
            Browser::Firefox => ("Firefox", "firefox"),
        };
        let user_apps =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications"));
        [Some(PathBuf::from("/Applications")), user_apps]
            .into_iter()
            .flatten()
            .map(|apps| apps.join(format!("{bundle}.app/Contents/MacOS/{binary}")))
            .find(|path| path.is_file())
    }

    /// The browser's executable, if it's installed: per machine under the
    /// program files directories, or per user under local app data.
    #[cfg(target_os = "windows")]
    pub fn executable(self) -> Option<PathBuf> {
        let relative = match self {
            Browser::Chrome => r"Google\Chrome\Application\chrome.exe",
            Browser::Edge => r"Microsoft\Edge\Application\msedge.exe",
            Browser::Brave => r"BraveSoftware\Brave-Browser\Application\brave.exe",
            Browser::Chromium => r"Chromium\Application\chrome.exe",
            Browser::Firefox => r"Mozilla Firefox\firefox.exe",
        };
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|dir| PathBuf::from(dir).join(relative))
            .find(|path| path.is_file())
    }

    /// The browser's executable, if it's installed: the first of its usual
    /// command names on the `PATH`.
    #[cfg(target_os = "linux")]
    pub fn executable(self) -> Option<PathBuf> {
        let names: &[&str] = match self {
            Browser::Chrome => &["google-chrome-stable", "google-chrome"],
            Browser::Edge => &["microsoft-edge-stable", "microsoft-edge"],
            Browser::Brave => &["brave-browser", "brave"],
            Browser::Chromium => &["chromium", "chromium-browser"],
            Browser::Firefox => &["firefox"],
        };
        let path = std::env::var_os("PATH")?;
        names.iter().find_map(|name| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
    }

    /// The directory the browser's Snap can write to, if it's installed as
    /// one. A copy of the browser installed some other way can use it too, so
    /// this doesn't say which copy runs.
    #[cfg(target_os = "linux")]
    pub fn snap_dir(self) -> Option<PathBuf> {
        let name = match self {
            Browser::Brave => "brave",
            Browser::Chromium => "chromium",
            Browser::Firefox => "firefox",
            Browser::Chrome | Browser::Edge => return None,
        };
        if !std::path::Path::new("/snap").join(name).is_dir() {
            return None;
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("snap").join(name).join("common"))
    }

    /// Snaps are Linux only.
    #[cfg(not(target_os = "linux"))]
    pub fn snap_dir(self) -> Option<PathBuf> {
        None
    }
}
