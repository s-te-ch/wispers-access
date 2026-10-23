//! The per-share config file.
//!
//! These files are generated once, then edited by the user. After that, waserver
//! only reads them, at start and on `reload`.

use serde::Deserialize;
use std::path::Path;
use wispers_access_wire::Origin;

pub const FILENAME: &str = "share.toml";

/// A validated `share.toml`, as read by [`ShareConfig::parse`].
#[derive(Clone, Debug)]
pub struct ShareConfig {
    /// Display name, shown to guests.
    pub name: String,
    /// The shared apps, in the order found in the config file.
    pub apps: Vec<AppConfig>,
    /// Certificates for guests to trust for the share's `https` origins, a
    /// CA's or a self-signed server's, one PEM certificate each, re-encoded
    /// from the file.
    pub ca_certificates: Vec<String>,
    /// Legacy transport selection, used up to waserver 0.4, since moved to the
    /// state DB. If present here and in the state DB, newer versions of
    /// waserver will use the field to auto-migrate.
    pub legacy_transport: Option<LegacyTransport>,
}

/// The `[transport]` section of a 0.4 share file.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LegacyTransport {
    WispersConnect {
        #[serde(default)]
        backend: Option<String>,
    },
    Iroh {},
}

/// Use the wire protocol's definition of AppKind.
pub use wispers_access_wire::AppKind;

/// A validated `[[app]]` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    /// Stable identifier, used by guest nodes when accessing a shared app.
    pub id: String,
    /// Display name, shown to guest nodes.
    pub name: String,
    /// Custom integrations (e.g. Jellyfin) use specific identifiers here,
    /// so the clients can find the shared apps they know how to work with.
    /// `web` is any web app browsed as is.
    pub kind: AppKind,
    /// How guest nodes reach the app.
    pub mode: AppProxyMode,
}

/// How guest nodes reach an app, with what the mode needs. The wire's
/// [`AppProxyMode`](wispers_access_wire::AppProxyMode) explains the modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppProxyMode {
    /// Requests go to `upstream`, normalised to `host:port`.
    ReverseProxy { upstream: String },
    /// Guests browse at `start_url` and tunnel connections to `origins`, which
    /// include `start_url`'s origin.
    ForwardProxy {
        start_url: String,
        origins: Vec<Origin>,
    },
}

impl AppConfig {
    /// The upstream of an app in reverse-proxy mode.
    pub fn upstream(&self) -> Option<&str> {
        match &self.mode {
            AppProxyMode::ReverseProxy { upstream } => Some(upstream),
            AppProxyMode::ForwardProxy { .. } => None,
        }
    }
}

impl AppProxyMode {
    /// The mode's name, as in the file and on the wire.
    pub fn wire(&self) -> wispers_access_wire::AppProxyMode {
        match self {
            AppProxyMode::ReverseProxy { .. } => wispers_access_wire::AppProxyMode::ReverseProxy,
            AppProxyMode::ForwardProxy { .. } => wispers_access_wire::AppProxyMode::ForwardProxy,
        }
    }
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
    #[error("app {app}: {reason}")]
    InvalidApp { app: String, reason: String },
    #[error("ca_certificates entry {number}: {reason}")]
    InvalidCaCertificate { number: usize, reason: String },
}

impl ShareConfig {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Read(path.display().to_string(), e))?;
        Self::parse(&text)
    }

    /// Parse, normalise, and validate.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let file: ShareFile = toml::from_str(text)?;
        if file.name.trim().is_empty() {
            return Err(Error::EmptyName);
        }
        let mut apps: Vec<AppConfig> = Vec::with_capacity(file.apps.len());
        for entry in file.apps {
            let app = parse_app(entry)?;
            if apps.iter().any(|a| a.id == app.id) {
                return Err(Error::DuplicateAppId(app.id));
            }
            apps.push(app);
        }
        let ca_certificates = file
            .ca_certificates
            .iter()
            .enumerate()
            .map(|(i, pem)| {
                parse_ca_certificate(pem).map_err(|reason| Error::InvalidCaCertificate {
                    number: i + 1,
                    reason,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ShareConfig {
            name: file.name,
            apps,
            ca_certificates,
            legacy_transport: file.legacy_transport,
        })
    }

    /// Finds and returns the AppConfig for the given ID.
    pub fn find_app(&self, app_id: &str) -> Option<&AppConfig> {
        self.apps.iter().find(|app| app.id == app_id)
    }

    /// Whether a forward-proxy app lists an origin at `host:port`. The scheme
    /// doesn't count: a tunnel carries opaque bytes, so the host can't tell
    /// `http` from `https` anyway.
    pub fn has_proxied_origin(&self, host: &str, port: u16) -> bool {
        self.apps.iter().any(|app| match &app.mode {
            AppProxyMode::ForwardProxy { origins, .. } => origins
                .iter()
                .any(|o| o.host().eq_ignore_ascii_case(host) && o.port() == port),
            AppProxyMode::ReverseProxy { .. } => false,
        })
    }

    /// The app used for requests that name no app: the first in reverse-proxy
    /// mode, the only mode the clients sending such requests know.
    ///
    /// TODO: Clean this up once old clients are gone and everyone specifies the
    /// app to talk to.
    pub fn default_app(&self) -> Option<&AppConfig> {
        self.apps.iter().find(|app| app.upstream().is_some())
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
            h.write(s.mode.wire().as_str().as_bytes());
            match &s.mode {
                AppProxyMode::ReverseProxy { upstream } => h.write(upstream.as_bytes()),
                AppProxyMode::ForwardProxy { start_url, origins } => {
                    h.write(start_url.as_bytes());
                    h.write(origins.len().to_string().as_bytes());
                    for origin in origins {
                        h.write(origin.to_string().as_bytes());
                    }
                }
            }
        }
        for pem in &self.ca_certificates {
            h.write(pem.as_bytes());
        }
        h.finish()
    }
}

/// `share.toml` as written, before [`ShareConfig::parse`] validates it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShareFile {
    name: String,
    #[serde(default, rename = "app")]
    apps: Vec<AppEntry>,
    /// Inline PEM, one certificate per entry, roots or intermediates.
    #[serde(default)]
    ca_certificates: Vec<String>,
    #[serde(default, rename = "transport")]
    legacy_transport: Option<LegacyTransport>,
}

/// One `[[app]]` block as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppEntry {
    id: String,
    /// Defaults to the ID.
    #[serde(default)]
    name: String,
    /// Defaults to `web`.
    #[serde(default)]
    kind: AppKind,
    /// Defaults to reverse-proxy.
    #[serde(default)]
    mode: ModeName,
    /// Reverse-proxy mode: `host:port`, or `:port` for localhost.
    upstream: Option<String>,
    /// Forward-proxy mode: the URL guests open first.
    start_url: Option<String>,
    /// Forward-proxy mode: every origin guests reach through the share,
    /// `scheme://host[:port]`.
    proxied_origins: Option<Vec<String>>,
}

/// The `mode` of an `[[app]]` block. Strict, unlike the wire's mode: a typo
/// must not parse.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ModeName {
    #[default]
    ReverseProxy,
    ForwardProxy,
}

/// Validates one `[[app]]` block and fills in its defaults.
fn parse_app(entry: AppEntry) -> Result<AppConfig, Error> {
    let id = entry.id;
    if !is_valid_id(&id) {
        return Err(Error::InvalidAppId(id));
    }
    let invalid = |reason: &str| Error::InvalidApp {
        app: id.clone(),
        reason: reason.to_owned(),
    };
    let mode = match entry.mode {
        ModeName::ReverseProxy => {
            if entry.start_url.is_some() || entry.proxied_origins.is_some() {
                return Err(invalid(
                    "start_url and proxied_origins need mode = \"forward-proxy\"",
                ));
            }
            let upstream = entry
                .upstream
                .ok_or_else(|| invalid("upstream is missing"))?;
            let upstream = parse_upstream(&upstream).map_err(|reason| Error::InvalidUpstream {
                app: id.clone(),
                reason,
            })?;
            AppProxyMode::ReverseProxy { upstream }
        }
        ModeName::ForwardProxy => {
            if entry.upstream.is_some() {
                return Err(invalid("upstream does not apply to forward-proxy mode"));
            }
            if entry.kind != AppKind::Web {
                return Err(invalid("forward-proxy mode needs kind = \"web\""));
            }
            let (Some(start_url), Some(origins)) = (entry.start_url, entry.proxied_origins) else {
                return Err(invalid(
                    "forward-proxy mode needs start_url and proxied_origins",
                ));
            };
            parse_forward_proxy(&start_url, &origins).map_err(|reason| invalid(&reason))?
        }
    };
    let name = if entry.name.trim().is_empty() {
        id.clone()
    } else {
        entry.name
    };
    Ok(AppConfig {
        id,
        name,
        kind: entry.kind,
        mode,
    })
}

/// Parses a forward-proxy app's start URL and origins, checking that the
/// origins cover the start URL, so that they are all a guest node can reach.
fn parse_forward_proxy(start_url: &str, origins: &[String]) -> Result<AppProxyMode, String> {
    let origins = origins
        .iter()
        .map(|o| {
            let origin = o
                .parse::<Origin>()
                .map_err(|e| format!("invalid origin {o:?} in proxied_origins: {e}"))?;
            check_scheme(&origin)?;
            Ok(origin)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let url = url::Url::parse(start_url.trim()).map_err(|e| format!("invalid start_url: {e}"))?;
    let start_origin = Origin::of(&url).map_err(|e| format!("invalid start_url: {e}"))?;
    if !origins.contains(&start_origin) {
        return Err(format!(
            "the origin of start_url ({start_origin}) is not in proxied_origins"
        ));
    }
    Ok(AppProxyMode::ForwardProxy {
        start_url: url.to_string(),
        origins,
    })
}

/// Checks the scheme rule for proxied origins: a loopback name takes `http`,
/// every other name `https`. Guest nodes on some platforms can't open an
/// origin that breaks this rule.
fn check_scheme(origin: &Origin) -> Result<(), String> {
    match (origin.is_loopback(), origin.is_https()) {
        (true, true) => Err(format!(
            "{origin} in proxied_origins must be http: iOS guest nodes can't reach https on a loopback name"
        )),
        (false, false) => Err(format!(
            "{origin} in proxied_origins must be https: Android guest nodes block plain http to hosts other than localhost"
        )),
        _ => Ok(()),
    }
}

/// Parses a `ca_certificates` entry, which must hold one PEM certificate that
/// parses as X.509, and re-encodes it. Re-encoding keeps anything else pasted
/// into the entry from reaching guest nodes, such as a key under a label the PEM
/// parser skips.
fn parse_ca_certificate(pem: &str) -> Result<String, String> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::{PemObject, SectionKind};
    let sections = <(SectionKind, Vec<u8>)>::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("not valid PEM: {e}"))?;
    let der = match sections.as_slice() {
        [(SectionKind::Certificate, der)] => der,
        [] => return Err("no PEM certificate found".to_owned()),
        [(kind, _)] => return Err(format!("expected a certificate, found {kind:?}")),
        _ => return Err("more than one PEM section, use one entry per certificate".to_owned()),
    };
    rustls::RootCertStore::empty()
        .add(CertificateDer::from(der.as_slice()))
        .map_err(|e| format!("not an X.509 certificate: {e}"))?;
    Ok(pem_certificate(der))
}

fn pem_certificate(der: &[u8]) -> String {
    let base64 = data_encoding::BASE64.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in base64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// Renders the `share.toml` written by `init`.
pub fn render_template(name: &str) -> String {
    format!(
        "# This share's config. Edit freely and apply with `waserver reload`.\n\
         # Reference: one [[app]] block per shared app.\n\n\
         name = {}\n\n\
         # One [[app]] block per app. Keep `id` stable, guest nodes refer to it.\n\
         # `upstream` is host:port, or :port for localhost.\n\
         #\n# [[app]]\n# id = \"myapp\"\n# name = \"My App\"\n# upstream = \":3000\"\n\
         #\n\
         # An app made of several origins (single sign-on, an API on its own host,\n\
         # a dev setup) can use forward-proxy mode instead: guest nodes browse it at its\n\
         # real URL, and requests to the listed origins go through the share.\n\
         # Origins on localhost take http, all others https.\n\
         #\n# [[app]]\n# id = \"dev\"\n# mode = \"forward-proxy\"\n\
         # start_url = \"http://localhost:4200/\"\n\
         # proxied_origins = [\"http://localhost:4200\", \"http://localhost:1337\"]\n",
        quote(name)
    )
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
/// addresses families if available (see `upstream::connect`). IPv6
/// literals would need bracket form (`[::1]:3000`) and aren't handled.
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
        assert!(cfg.legacy_transport.is_none());
        assert_eq!(cfg.apps.len(), 2);
        assert_eq!(cfg.default_app().unwrap().id, "jellyfin");
        assert_eq!(cfg.apps[0].kind, AppKind::Jellyfin);
        // Name defaults to the id, an empty host means localhost.
        assert_eq!(cfg.apps[1].name, "photos");
        assert_eq!(cfg.apps[1].upstream(), Some("localhost:2283"));
        assert_eq!(cfg.apps[1].kind, AppKind::Web);
    }

    const FORWARD_PROXY: &str = r#"
name = "Dev"

[[app]]
id = "oss"
name = "OSS Directory"
mode = "forward-proxy"
start_url = "http://localhost:4200/directory"
proxied_origins = ["http://localhost:4200", "http://LOCALHOST:1337/", "https://sso.internal:443"]

[[app]]
id = "grafana"
upstream = ":3000"
"#;

    #[test]
    fn parses_forward_proxy_apps() {
        let cfg = ShareConfig::parse(FORWARD_PROXY).unwrap();
        let AppProxyMode::ForwardProxy { start_url, origins } = &cfg.apps[0].mode else {
            panic!("{:?}", cfg.apps[0].mode);
        };
        assert_eq!(start_url, "http://localhost:4200/directory");
        let origins: Vec<String> = origins.iter().map(ToString::to_string).collect();
        assert_eq!(
            origins,
            [
                "http://localhost:4200",
                "http://localhost:1337",
                "https://sso.internal"
            ]
        );
        assert_eq!(cfg.apps[0].upstream(), None);
        // Legacy clients only know reverse-proxy apps.
        assert_eq!(cfg.default_app().unwrap().id, "grafana");
    }

    #[test]
    fn proxied_origins_match_by_host_and_port() {
        let cfg = ShareConfig::parse(FORWARD_PROXY).unwrap();
        assert!(cfg.has_proxied_origin("localhost", 4200));
        assert!(cfg.has_proxied_origin("LocalHost", 1337));
        // The default port of the scheme.
        assert!(cfg.has_proxied_origin("sso.internal", 443));
        assert!(!cfg.has_proxied_origin("sso.internal", 80));
        assert!(!cfg.has_proxied_origin("localhost", 4201));
        // Upstreams of reverse-proxy apps aren't tunnel targets.
        assert!(!cfg.has_proxied_origin("localhost", 3000));
    }

    #[test]
    fn modes_need_their_own_fields_only() {
        let app = |fields: &str| {
            ShareConfig::parse(&format!("name = \"x\"\n[[app]]\nid = \"a\"\n{fields}"))
        };
        let fp = "mode = \"forward-proxy\"\n";
        let start = "start_url = \"http://localhost:4200/\"\n";
        let origins = "proxied_origins = [\"http://localhost:4200\"]\n";
        assert!(app(&format!("{fp}{start}{origins}")).is_ok());
        let invalid = |fields: String| match app(&fields) {
            Err(Error::InvalidApp { app, reason }) => {
                assert_eq!(app, "a");
                reason
            }
            other => panic!("{fields}: {other:?}"),
        };
        assert!(invalid(String::new()).contains("upstream is missing"));
        assert!(invalid(format!("upstream = \":1\"\n{start}")).contains("forward-proxy"));
        assert!(invalid(format!("{fp}{start}{origins}upstream = \":1\"\n")).contains("upstream"));
        assert!(invalid(format!("{fp}{start}")).contains("needs start_url and proxied_origins"));
        assert!(invalid(format!("{fp}{origins}")).contains("needs start_url and proxied_origins"));
        assert!(
            invalid(format!("{fp}{start}{origins}kind = \"jellyfin\"\n"))
                .contains("kind = \"web\"")
        );
        // The start URL's origin must be listed, so the list is everything
        // a guest node can reach.
        let other_port = "start_url = \"http://localhost:4201/\"\n";
        assert!(
            invalid(format!("{fp}{other_port}{origins}"))
                .contains("origin of start_url (http://localhost:4201) is not in proxied_origins")
        );
        let with_path = "proxied_origins = [\"http://localhost:4200/app\"]\n";
        assert!(invalid(format!("{fp}{start}{with_path}")).contains("invalid origin"));
        assert!(
            invalid(format!(
                "{fp}start_url = \"ftp://localhost:4200/\"\n{origins}"
            ))
            .contains("invalid start_url")
        );
        // A misspelt mode is a parse error, not an unknown mode.
        assert!(matches!(
            app(&format!("mode = \"forward_proxy\"\n{start}{origins}")),
            Err(Error::Parse(_))
        ));
    }

    #[test]
    fn loopback_origins_take_http_and_named_ones_https() {
        let app = |origins: &str| {
            ShareConfig::parse(&format!(
                "name = \"x\"\n[[app]]\nid = \"a\"\nmode = \"forward-proxy\"\n\
                 start_url = \"http://localhost:4200/\"\n\
                 proxied_origins = [\"http://localhost:4200\", {origins}]\n"
            ))
        };
        assert!(app("\"http://127.0.0.1:1337\", \"https://sso.example.org\"").is_ok());
        let reason = |origins: &str| match app(origins) {
            Err(Error::InvalidApp { reason, .. }) => reason,
            other => panic!("{origins}: {other:?}"),
        };
        assert!(
            reason("\"https://localhost:8443\"")
                .contains("https://localhost:8443 in proxied_origins must be http")
        );
        assert!(reason("\"https://[::1]:8443\"").contains("must be http"));
        assert!(
            reason("\"http://intranet.example.org\"")
                .contains("http://intranet.example.org in proxied_origins must be https")
        );
        assert!(reason("\"http://192.168.1.10:8080\"").contains("must be https"));
    }

    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBjDCCATGgAwIBAgIUUrDdcVlMx/ZhY/3RH5YdY3c7suswCgYIKoZIzj0EAwIw
GjEYMBYGA1UEAwwPV2lzcGVycyBUZXN0IENBMCAXDTI2MTAyMTA3MTkwMloYDzIx
MjYwOTI3MDcxOTAyWjAaMRgwFgYDVQQDDA9XaXNwZXJzIFRlc3QgQ0EwWTATBgcq
hkjOPQIBBggqhkjOPQMBBwNCAAQTkrl0yVJak/l38mKlLyqFUuSvFC5CBFRCURUV
35e523TJYXvQfodnfHzKNvvsdJUSFEmwDSmgfVFQztL7K22Bo1MwUTAdBgNVHQ4E
FgQUcdx/0yv8Ebwn+sk6vjsERVq00ZQwHwYDVR0jBBgwFoAUcdx/0yv8Ebwn+sk6
vjsERVq00ZQwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNJADBGAiEA4zZs
vc2iX9+shlPG22U1caCAIsYtnzW20taM+Le/5RECIQDlZ0JplVXedwTeX3Rx5an+
kkFn94FcM4MSTZeO+pCqSw==
-----END CERTIFICATE-----
";

    /// A self-signed server's certificate, for `app.internal` (`CA:FALSE`).
    const SELF_SIGNED_SERVER: &str = "-----BEGIN CERTIFICATE-----
MIIBmzCCAUGgAwIBAgIUNekUhbU6QaFLS2kkIv+Ozz/V36wwCgYIKoZIzj0EAwIw
FzEVMBMGA1UEAwwMYXBwLmludGVybmFsMCAXDTI2MTAyMzA5MzM0MloYDzIxMjYw
OTI5MDkzMzQyWjAXMRUwEwYDVQQDDAxhcHAuaW50ZXJuYWwwWTATBgcqhkjOPQIB
BggqhkjOPQMBBwNCAATX+3iIAh6XDLmaHuXcA3IldmagVHfyqydnuwVkmmTMnqoq
xqa9/PQrCSiGmIrxYCDzLP9f4na0DCLs9oEgQXRzo2kwZzAdBgNVHQ4EFgQUU3Jh
ZkvPrXD6gsuCtLFjNSNGie4wHwYDVR0jBBgwFoAUU3JhZkvPrXD6gsuCtLFjNSNG
ie4wFwYDVR0RBBAwDoIMYXBwLmludGVybmFsMAwGA1UdEwEB/wQCMAAwCgYIKoZI
zj0EAwIDSAAwRQIgaMP9Btas0cwOZQJxNvAETcILKvq3ewa50vB39vZCKQgCIQCh
LmgiKMekczkYKWj8heM50uEnUsNDYlCkP926zkjP5g==
-----END CERTIFICATE-----
";

    fn with_certificates(entries: &[&str]) -> Result<ShareConfig, Error> {
        let list: Vec<String> = entries.iter().map(|e| format!("'''\n{e}'''")).collect();
        ShareConfig::parse(&format!(
            "name = \"x\"\nca_certificates = [{}]\n",
            list.join(", ")
        ))
    }

    #[test]
    fn ca_certificates_are_checked_and_re_encoded() {
        let cfg = with_certificates(&[TEST_CA, SELF_SIGNED_SERVER]).unwrap();
        assert_eq!(cfg.ca_certificates, [TEST_CA, SELF_SIGNED_SERVER]);
        assert_ne!(
            cfg.config_hash(),
            ShareConfig::parse("name = \"x\"\n").unwrap().config_hash()
        );

        // Only the certificate is passed on: a section under a label the PEM
        // parser doesn't know is dropped, not forwarded.
        let smuggled = format!(
            "{TEST_CA}-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
        );
        let cfg = with_certificates(&[&smuggled]).unwrap();
        assert_eq!(cfg.ca_certificates, [TEST_CA]);

        let reason = |entries: &[&str]| match with_certificates(entries) {
            Err(Error::InvalidCaCertificate { number, reason }) => (number, reason),
            other => panic!("{other:?}"),
        };
        let (number, why) = reason(&[TEST_CA, "not PEM\n"]);
        assert_eq!(number, 2);
        assert!(why.contains("no PEM certificate"), "{why}");
        let (_, why) = reason(&[&format!("{TEST_CA}{TEST_CA}")]);
        assert!(why.contains("one entry per certificate"), "{why}");
        let (_, why) = reason(&["-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n"]);
        assert!(why.contains("expected a certificate"), "{why}");
        let (_, why) = reason(&["-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"]);
        assert!(why.contains("not an X.509 certificate"), "{why}");
    }

    #[test]
    fn apps_may_be_absent() {
        let cfg = ShareConfig::parse("name = \"x\"\n").unwrap();
        assert!(cfg.apps.is_empty());
        assert!(cfg.default_app().is_none());
    }

    #[test]
    fn legacy_transport_section_still_parses() {
        let text = "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nbackend = \"https://h.example\"\n";
        let cfg = ShareConfig::parse(text).unwrap();
        assert_eq!(
            cfg.legacy_transport,
            Some(LegacyTransport::WispersConnect {
                backend: Some("https://h.example".to_owned())
            })
        );
        let cfg = ShareConfig::parse("name = \"x\"\n[transport]\nkind = \"iroh\"\n").unwrap();
        assert_eq!(cfg.legacy_transport, Some(LegacyTransport::Iroh {}));
        // Strict as it always was: no unknown settings, kinds or flat forms.
        assert!(
            ShareConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"wispers-connect\"\nrelay = \"r\"\n"
            )
            .is_err()
        );
        assert!(
            ShareConfig::parse(
                "name = \"x\"\n[transport]\nkind = \"iroh\"\nbackend = \"https://h\"\n"
            )
            .is_err()
        );
        assert!(ShareConfig::parse("name = \"x\"\n[transport]\nkind = \"tailscale\"\n").is_err());
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

        let fp = ShareConfig::parse(FORWARD_PROXY).unwrap();
        let fewer =
            ShareConfig::parse(&FORWARD_PROXY.replace(", \"https://sso.internal:443\"", ""))
                .unwrap();
        assert_ne!(fp.config_hash(), fewer.config_hash());
        let moved = ShareConfig::parse(&FORWARD_PROXY.replace("/directory", "/other")).unwrap();
        assert_ne!(fp.config_hash(), moved.config_hash());
    }

    #[test]
    fn template_round_trips() {
        let cfg = ShareConfig::parse(&render_template("It's \"Demo\"")).unwrap();
        assert_eq!(cfg.name, "It's \"Demo\"");
        // The app block is commented out, so nothing is served until edited.
        assert!(cfg.apps.is_empty());
        assert!(cfg.legacy_transport.is_none());
        // The forward-proxy example is valid once uncommented.
        let template = render_template("x");
        let example: Vec<&str> = template[template.rfind("# [[app]]").unwrap()..]
            .lines()
            .map(|l| l.trim_start_matches("# "))
            .collect();
        let cfg = ShareConfig::parse(&format!("name = \"x\"\n{}\n", example.join("\n"))).unwrap();
        assert!(matches!(
            cfg.apps[0].mode,
            AppProxyMode::ForwardProxy { .. }
        ));
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
