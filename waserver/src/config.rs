//! The per-share config file.
//!
//! These files are generated once, then edited by the user. After that, waserver
//! only reads them, at start and on `reload`.

use serde::Deserialize;
use std::path::Path;

pub const FILENAME: &str = "share.toml";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareConfig {
    /// Display name, shown to guests.
    pub name: String,
    #[serde(default)]
    pub transport: TransportConfig,
    /// The shared apps, in the order found in the config file.
    #[serde(default, rename = "app")]
    pub apps: Vec<AppConfig>,
}

/// Peer-to-peer transport config.
///
/// Each transport type can have its own parameters. In the file, that looks
/// like this:
///
/// ```toml
/// [transport]
/// kind = "wispers-connect"
/// backend = "https://hub.example"
/// ```
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TransportConfig {
    WispersConnect {
        /// Optional base URL of a self-hosted backend.
        #[serde(default)]
        backend: Option<String>,
    },
    Iroh {},
}

/// If not specified the transport defaults to Wispers Connect with the managed backend.
impl Default for TransportConfig {
    fn default() -> Self {
        TransportConfig::WispersConnect { backend: None }
    }
}

impl TransportConfig {
    pub fn kind(&self) -> TransportKind {
        match self {
            TransportConfig::WispersConnect { .. } => TransportKind::WispersConnect,
            TransportConfig::Iroh {} => TransportKind::Iroh,
        }
    }
}

/// The transport names, as on the command line and in status output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum TransportKind {
    WispersConnect,
    Iroh,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::WispersConnect => "wispers-connect",
            TransportKind::Iroh => "iroh",
        }
    }
}

/// Use the wire protocol's definition of AppKind.
pub use wispers_access_wire::AppKind;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// Stable identifier, used by guest nodes when accessing a shared app.
    pub id: String,
    /// Display name, defaults to the ID.
    #[serde(default)]
    pub name: String,
    /// Custom integrations (e.g. Jellyfin) use a specific identifiers here,
    /// so the clients can find the shared apps they know how to work with.
    /// `web` (the default) is any web app browsed as is.
    #[serde(default)]
    pub kind: AppKind,
    /// `host:port`, or `:port` for localhost.
    pub upstream: String,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("cannot read {0}: {1}")]
    Read(String, std::io::Error),
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error("share name is empty")]
    EmptyName,
    #[error("app {0}: invalid id (use letters, digits, '-' or '_')")]
    InvalidAppId(String),
    #[error("app {0}: duplicate id")]
    DuplicateAppId(String),
    #[error("app {app}: invalid upstream: {reason}")]
    InvalidUpstream { app: String, reason: String },
}

impl ShareConfig {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Read(path.display().to_string(), e))?;
        Self::parse(&text)
    }

    /// Parse, normalise, and validate.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut cfg: ShareConfig = toml::from_str(text)?;
        if cfg.name.trim().is_empty() {
            return Err(Error::EmptyName);
        }
        for i in 0..cfg.apps.len() {
            let id = cfg.apps[i].id.clone();
            if !is_valid_id(&id) {
                return Err(Error::InvalidAppId(id));
            }
            if cfg.apps[..i].iter().any(|s| s.id == id) {
                return Err(Error::DuplicateAppId(id));
            }
            let app = &mut cfg.apps[i];
            if app.name.trim().is_empty() {
                app.name = id.clone();
            }
            app.upstream = parse_upstream(&app.upstream)
                .map_err(|reason| Error::InvalidUpstream { app: id, reason })?;
        }
        Ok(cfg)
    }

    /// Finds and returns the AppConfig for the given ID.
    pub fn find_app(&self, app_id: &str) -> Option<&AppConfig> {
        self.apps.iter().find(|app| app.id == app_id)
    }

    /// The app used for requests that name no app.
    ///
    /// TODO: Clean this up once old clients are gone and everyone specifies the
    /// app to talk to.
    pub fn default_app(&self) -> Option<&AppConfig> {
        self.apps.first()
    }

    /// Hash of the (observable parts of the) config, used to detect config
    /// updates without having to compare the entire data structure.
    pub fn config_hash(&self) -> u64 {
        let mut h = Fnv1a::new();
        h.write(self.name.as_bytes());
        for s in &self.apps {
            h.write(s.id.as_bytes());
            h.write(s.name.as_bytes());
            h.write(s.kind.as_str().as_bytes());
            h.write(s.upstream.as_bytes());
        }
        h.finish()
    }
}

/// Renders the `share.toml` written by `init`.
pub fn render_template(name: &str, transport: &TransportConfig) -> String {
    let mut out = String::new();
    out.push_str("# This share's config. Edit freely and apply with `waserver reload`.\n");
    out.push_str("# Reference: one [[app]] block per shared app.\n\n");
    out.push_str(&format!("name = {}\n\n", quote(name)));
    out.push_str("[transport]\n");
    out.push_str(&format!("kind = {}\n", quote(transport.kind().as_str())));
    match transport {
        TransportConfig::WispersConnect { backend: Some(b) } => {
            out.push_str(&format!("backend = {}\n", quote(b)))
        }
        TransportConfig::WispersConnect { backend: None } => out.push_str(
            "# backend = \"https://hub.example\"   # self-hosted Wispers Connect backend\n",
        ),
        TransportConfig::Iroh {} => {}
    }
    out.push_str(
        "\n# One [[app]] block per app. Keep `id` stable, guest nodes refer to it.\n\
         # `upstream` is host:port, or :port for localhost.\n\
         #\n# [[app]]\n# id = \"myapp\"\n# name = \"My App\"\n# upstream = \":3000\"\n",
    );
    out
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// Validate share or app ID. They need to be file-system and DNS-label safe.
pub fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !(s.starts_with('-') || s.starts_with('_'))
        && !(s.ends_with('-') || s.ends_with('_'))
}

/// Parse an upstream dial target in `host:port` form into a normalized
/// `host:port` string. An empty host (`:3000`) means `localhost`. It's
/// `localhost` rather than `127.0.0.1` so the dial tries both IPv4 and IPv6
/// addresses families if available. IPv6 literals would need bracket form
/// (`[::1]:3000`) and aren't handled.
pub fn parse_upstream(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("upstream is empty".to_string());
    }
    // Split off the port after the last colon so the host may itself be a
    // name like `app`.
    let Some((host, port_str)) = s.rsplit_once(':') else {
        return Err(format!("expected host:port or :port, got '{}'", s));
    };
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("invalid port '{}' (expected 1–65535)", port_str))?;
    if port == 0 {
        return Err("port 0 is not a valid upstream".to_string());
    }
    let host = if host.is_empty() { "localhost" } else { host };
    Ok(format!("{}:{}", host, port))
}

/// 64-bit FNV-1a hash, deterministic across processes and Rust versions, unlike
/// `DefaultHasher`. Fields are separated so shifting bytes between them changes
/// the hash.
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Fnv1a(0xcbf29ce484222325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes.iter().chain(std::iter::once(&0u8)) {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
name = "Family"

[transport]
kind = "wispers-connect"

[[app]]
id = "jellyfin"
name = "Jellyfin"
kind = "jellyfin"
upstream = "127.0.0.1:8096"

[[app]]
id = "photos"
upstream = ":2283"
"#;

    #[test]
    fn parses_and_normalises() {
        let cfg = ShareConfig::parse(FULL).unwrap();
        assert_eq!(cfg.name, "Family");
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect { backend: None }
        );
        assert_eq!(cfg.apps.len(), 2);
        assert_eq!(cfg.default_app().unwrap().id, "jellyfin");
        assert_eq!(cfg.apps[0].kind, AppKind::Jellyfin);
        // Name defaults to the id, an empty host means localhost.
        assert_eq!(cfg.apps[1].name, "photos");
        assert_eq!(cfg.apps[1].upstream, "localhost:2283");
        assert_eq!(cfg.apps[1].kind, AppKind::Web);
    }

    #[test]
    fn transport_defaults_to_wispers_connect_and_apps_may_be_absent() {
        let cfg = ShareConfig::parse("name = \"x\"\n").unwrap();
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect { backend: None }
        );
        assert!(cfg.apps.is_empty());
        assert!(cfg.default_app().is_none());
    }

    #[test]
    fn transport_settings_live_with_their_transport() {
        let text = "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nbackend = \"https://h.example\"\n";
        let cfg = ShareConfig::parse(text).unwrap();
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect {
                backend: Some("https://h.example".to_owned())
            }
        );
        // A setting that belongs to no transport, or the old flat form, is rejected.
        assert!(
            ShareConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nrelay = \"r\"\n"
            )
            .is_err()
        );
        assert!(ShareConfig::parse("name = \"x\"\ntransport = \"wispers-connect\"\n").is_err());
        assert!(ShareConfig::parse("name = \"x\"\nbackend = \"https://h.example\"\n").is_err());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(matches!(
            ShareConfig::parse("name = \"\"\n"),
            Err(Error::EmptyName)
        ));
        assert!(matches!(
            ShareConfig::parse("name = \"x\"\n[transport]\nkind = \"tailscale\"\n"),
            Err(Error::Parse(_))
        ));
        // A setting from another transport is a parse error.
        assert!(matches!(
            ShareConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"iroh\"\nbackend = \"https://h\"\n"
            ),
            Err(Error::Parse(_))
        ));
        assert_eq!(
            ShareConfig::parse("name = \"x\"\n[transport]\nkind = \"iroh\"\n")
                .unwrap()
                .transport,
            TransportConfig::Iroh {}
        );
        assert!(matches!(
            ShareConfig::parse("name = \"x\"\nbogus = 1\n"),
            Err(Error::Parse(_))
        ));
        let dup = "name = \"x\"\n[[app]]\nid = \"a\"\nupstream = \":1\"\n[[app]]\nid = \"a\"\nupstream = \":2\"\n";
        assert!(matches!(
            ShareConfig::parse(dup),
            Err(Error::DuplicateAppId(id)) if id == "a"
        ));
        let bad_id = "name = \"x\"\n[[app]]\nid = \"a b\"\nupstream = \":1\"\n";
        assert!(matches!(
            ShareConfig::parse(bad_id),
            Err(Error::InvalidAppId(_))
        ));
        let bad_port = "name = \"x\"\n[[app]]\nid = \"a\"\nupstream = \"app\"\n";
        assert!(matches!(
            ShareConfig::parse(bad_port),
            Err(Error::InvalidUpstream { app, .. }) if app == "a"
        ));
        // An unknown kind is a contract violation, not a free-form label.
        let bad_kind = "name = \"x\"\n[[app]]\nid = \"a\"\nkind = \"plex\"\nupstream = \":1\"\n";
        assert!(matches!(ShareConfig::parse(bad_kind), Err(Error::Parse(_))));
    }

    #[test]
    fn hash_tracks_what_guests_see() {
        let a = ShareConfig::parse(FULL).unwrap();
        let same = ShareConfig::parse(&format!("{}\n# a comment\n", FULL)).unwrap();
        assert_eq!(a.config_hash(), same.config_hash());
        let renamed = ShareConfig::parse(&FULL.replace("Jellyfin", "Movies")).unwrap();
        assert_ne!(a.config_hash(), renamed.config_hash());
        let reordered = ShareConfig::parse(&FULL.replace("photos", "aphotos")).unwrap();
        assert_ne!(a.config_hash(), reordered.config_hash());
    }

    #[test]
    fn template_round_trips() {
        let transport = TransportConfig::WispersConnect {
            backend: Some("https://h.example".to_owned()),
        };
        let cfg = ShareConfig::parse(&render_template("It's \"Demo\"", &transport)).unwrap();
        assert_eq!(cfg.name, "It's \"Demo\"");
        assert_eq!(cfg.transport, transport);
        // The app block is commented out, so nothing is served until edited.
        assert!(cfg.apps.is_empty());

        let managed =
            ShareConfig::parse(&render_template("x", &TransportConfig::default())).unwrap();
        assert_eq!(managed.transport, TransportConfig::default());
    }

    #[test]
    fn parses_upstream_forms() {
        assert_eq!(parse_upstream("app:3000").unwrap(), "app:3000");
        assert_eq!(parse_upstream("127.0.0.1:8080").unwrap(), "127.0.0.1:8080");
        assert_eq!(parse_upstream(":3000").unwrap(), "localhost:3000");
        assert_eq!(parse_upstream("  app:3000\n").unwrap(), "app:3000");
    }

    #[test]
    fn rejects_bad_upstream() {
        assert!(parse_upstream("").is_err());
        assert!(parse_upstream("8080").is_err()); // bare port: not a form other tools accept
        assert!(parse_upstream("app").is_err()); // no port
        assert!(parse_upstream("app:").is_err()); // empty port
        assert!(parse_upstream("app:abc").is_err()); // non-numeric port
        assert!(parse_upstream("app:0").is_err()); // port 0
        assert!(parse_upstream("app:99999").is_err()); // out of range
    }

    #[test]
    fn ids_are_label_safe() {
        assert!(is_valid_id("my-app_2"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("-lead"));
        assert!(!is_valid_id("trail_"));
        assert!(!is_valid_id("has space"));
        assert!(!is_valid_id("dot.dot"));
    }
}
