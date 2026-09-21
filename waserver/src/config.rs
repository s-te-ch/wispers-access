//! The per-circle config file.
//!
//! These files are generated once, then edited by the user. After this they're
//! only read, at start and on `reload`.

use serde::Deserialize;
use std::path::Path;

pub const FILENAME: &str = "circle.toml";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircleConfig {
    /// Display name, shown to guests.
    pub name: String,
    /// The transport for this circle, with that transport's own settings.
    #[serde(default)]
    pub transport: TransportConfig,
    /// The apps served to this circle, in the order found in the config file.
    #[serde(default, rename = "share")]
    pub shares: Vec<ShareConfig>,
}

/// Peer-to-peer communications transport types.
///
/// Each type can have its own parameters. In the file that looks like this:
///
/// ```toml
/// [transport]
/// kind = "wispers-connect"
/// backend = "https://hub.example"
///     ```
///
/// Parameters used with the wrong type result in parse errors.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TransportConfig {
    WispersConnect {
        /// Base URL of a self-hosted backend. `None` = the managed backend.
        #[serde(default)]
        backend: Option<String>,
    },
    Iroh {},
}

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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShareConfig {
    /// Stable key.
    pub id: String,
    /// Display name, defaults to the ID.
    #[serde(default)]
    pub name: String,
    /// Which app this is. `web` (the default) is any web app browsed as is.
    /// Other values target specific integrators, like Jellyfin clients.
    #[serde(default)]
    pub kind: ShareKind,
    /// `host:port`, or `:port` for localhost.
    pub upstream: String,
}

/// The share kinds are the wire crate's: the config file and the messages
/// guests receive use one vocabulary.
pub use wispers_access_wire::ShareKind;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("cannot read {0}: {1}")]
    Read(String, std::io::Error),
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error("circle name is empty")]
    EmptyName,
    #[error("share {0}: invalid id (use letters, digits, '-' or '_')")]
    InvalidShareId(String),
    #[error("share {0}: duplicate id")]
    DuplicateShareId(String),
    #[error("share {share}: invalid upstream: {reason}")]
    InvalidUpstream { share: String, reason: String },
}

impl CircleConfig {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Read(path.display().to_string(), e))?;
        Self::parse(&text)
    }

    /// Parses, normalises (share names, upstream form) and validates.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut cfg: CircleConfig = toml::from_str(text)?;
        if cfg.name.trim().is_empty() {
            return Err(Error::EmptyName);
        }
        for i in 0..cfg.shares.len() {
            let id = cfg.shares[i].id.clone();
            if !is_valid_id(&id) {
                return Err(Error::InvalidShareId(id));
            }
            if cfg.shares[..i].iter().any(|s| s.id == id) {
                return Err(Error::DuplicateShareId(id));
            }
            let share = &mut cfg.shares[i];
            if share.name.trim().is_empty() {
                share.name = id.clone();
            }
            share.upstream = parse_upstream(&share.upstream)
                .map_err(|reason| Error::InvalidUpstream { share: id, reason })?;
        }
        Ok(cfg)
    }

    /// The share used for requests that name no share.
    pub fn default_share(&self) -> Option<&ShareConfig> {
        self.shares.first()
    }

    /// Hash of everything a guest can observe about the share list. Used by
    /// guest nodes to detect whether they need to update their cached view of
    /// the circle config.
    pub fn config_hash(&self) -> u64 {
        let mut h = Fnv1a::new();
        h.write(self.name.as_bytes());
        for s in &self.shares {
            h.write(s.id.as_bytes());
            h.write(s.name.as_bytes());
            h.write(s.kind.as_str().as_bytes());
            h.write(s.upstream.as_bytes());
        }
        h.finish()
    }
}

/// Renders the `circle.toml` that `init` writes: the circle's name and
/// transport, and a commented-out share block to copy from.
pub fn render_template(name: &str, transport: &TransportConfig) -> String {
    let mut out = String::new();
    out.push_str("# This circle's config. Edit freely and apply with `waserver reload`.\n");
    out.push_str("# Reference: one [[share]] block per app served to the circle.\n\n");
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
        "\n# One [[share]] block per app. `id` is what guests persist: keep it stable\n\
         # and rename via `name`. `upstream` is host:port, or :port for localhost.\n\
         # `kind` is web (default), jellyfin or immich; it tells integrating clients\n\
         # which app this is.\n\
         # The first share is the default for clients that predate circles.\n\
         #\n# [[share]]\n# id = \"myapp\"\n# name = \"My App\"\n# kind = \"jellyfin\"\n# upstream = \":3000\"\n",
    );
    out
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// Circle and share IDs, file-system and DNS-label safe.
pub fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !(s.starts_with('-') || s.starts_with('_'))
        && !(s.ends_with('-') || s.ends_with('_'))
}

/// Parse an upstream dial target in `host:port` form into a normalized
/// `host:port` string. An empty host (`:3000`) means `localhost`, as in
/// other tools' bind/dial syntax. `localhost` rather than `127.0.0.1` so the
/// dial tries both address families — modern Node dev servers (e.g. Vite)
/// often listen on `::1` only. A non-numeric or out-of-range port is
/// rejected. IPv6 literals would need bracket form (`[::1]:3000`) and aren't
/// handled here.
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

/// 64-bit FNV-1a hash. Deterministic across processes and Rust versions, unlike
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

[[share]]
id = "jellyfin"
name = "Jellyfin"
kind = "jellyfin"
upstream = "127.0.0.1:8096"

[[share]]
id = "photos"
upstream = ":2283"
"#;

    #[test]
    fn parses_and_normalises() {
        let cfg = CircleConfig::parse(FULL).unwrap();
        assert_eq!(cfg.name, "Family");
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect { backend: None }
        );
        assert_eq!(cfg.shares.len(), 2);
        assert_eq!(cfg.default_share().unwrap().id, "jellyfin");
        assert_eq!(cfg.shares[0].kind, ShareKind::Jellyfin);
        // Name defaults to the id, an empty host means localhost.
        assert_eq!(cfg.shares[1].name, "photos");
        assert_eq!(cfg.shares[1].upstream, "localhost:2283");
        assert_eq!(cfg.shares[1].kind, ShareKind::Web);
    }

    #[test]
    fn transport_defaults_to_wispers_connect_and_shares_may_be_absent() {
        let cfg = CircleConfig::parse("name = \"x\"\n").unwrap();
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect { backend: None }
        );
        assert!(cfg.shares.is_empty());
        assert!(cfg.default_share().is_none());
    }

    #[test]
    fn transport_settings_live_with_their_transport() {
        let text = "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nbackend = \"https://h.example\"\n";
        let cfg = CircleConfig::parse(text).unwrap();
        assert_eq!(
            cfg.transport,
            TransportConfig::WispersConnect {
                backend: Some("https://h.example".to_owned())
            }
        );
        // A setting that belongs to no transport, or the old flat form, is rejected.
        assert!(
            CircleConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nrelay = \"r\"\n"
            )
            .is_err()
        );
        assert!(CircleConfig::parse("name = \"x\"\ntransport = \"wispers-connect\"\n").is_err());
        assert!(CircleConfig::parse("name = \"x\"\nbackend = \"https://h.example\"\n").is_err());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(matches!(
            CircleConfig::parse("name = \"\"\n"),
            Err(Error::EmptyName)
        ));
        assert!(matches!(
            CircleConfig::parse("name = \"x\"\n[transport]\nkind = \"tailscale\"\n"),
            Err(Error::Parse(_))
        ));
        // A setting from another transport is a parse error.
        assert!(matches!(
            CircleConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"iroh\"\nbackend = \"https://h\"\n"
            ),
            Err(Error::Parse(_))
        ));
        assert_eq!(
            CircleConfig::parse("name = \"x\"\n[transport]\nkind = \"iroh\"\n")
                .unwrap()
                .transport,
            TransportConfig::Iroh {}
        );
        assert!(matches!(
            CircleConfig::parse("name = \"x\"\nbogus = 1\n"),
            Err(Error::Parse(_))
        ));
        let dup = "name = \"x\"\n[[share]]\nid = \"a\"\nupstream = \":1\"\n[[share]]\nid = \"a\"\nupstream = \":2\"\n";
        assert!(matches!(
            CircleConfig::parse(dup),
            Err(Error::DuplicateShareId(id)) if id == "a"
        ));
        let bad_id = "name = \"x\"\n[[share]]\nid = \"a b\"\nupstream = \":1\"\n";
        assert!(matches!(
            CircleConfig::parse(bad_id),
            Err(Error::InvalidShareId(_))
        ));
        let bad_port = "name = \"x\"\n[[share]]\nid = \"a\"\nupstream = \"app\"\n";
        assert!(matches!(
            CircleConfig::parse(bad_port),
            Err(Error::InvalidUpstream { share, .. }) if share == "a"
        ));
        // An unknown kind is a contract violation, not a free-form label.
        let bad_kind = "name = \"x\"\n[[share]]\nid = \"a\"\nkind = \"plex\"\nupstream = \":1\"\n";
        assert!(matches!(
            CircleConfig::parse(bad_kind),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn hash_tracks_what_guests_see() {
        let a = CircleConfig::parse(FULL).unwrap();
        let same = CircleConfig::parse(&format!("{}\n# a comment\n", FULL)).unwrap();
        assert_eq!(a.config_hash(), same.config_hash());
        let renamed = CircleConfig::parse(&FULL.replace("Jellyfin", "Movies")).unwrap();
        assert_ne!(a.config_hash(), renamed.config_hash());
        let reordered = CircleConfig::parse(&FULL.replace("photos", "aphotos")).unwrap();
        assert_ne!(a.config_hash(), reordered.config_hash());
    }

    #[test]
    fn template_round_trips() {
        let transport = TransportConfig::WispersConnect {
            backend: Some("https://h.example".to_owned()),
        };
        let cfg = CircleConfig::parse(&render_template("It's \"Demo\"", &transport)).unwrap();
        assert_eq!(cfg.name, "It's \"Demo\"");
        assert_eq!(cfg.transport, transport);
        // The share block is commented out, so nothing is served until edited.
        assert!(cfg.shares.is_empty());

        let managed =
            CircleConfig::parse(&render_template("x", &TransportConfig::default())).unwrap();
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
