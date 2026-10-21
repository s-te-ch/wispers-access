//! The Wispers Access wire contract between guest nodes and the host node
//! (`waserver`).
//!
//! Every stream a guest opens starts with one [`StreamType`] byte:
//!
//! - [`StreamType::ReverseProxy`] for apps in reverse-proxy mode, on the data
//!   plane. One length-prefixed [`ReverseProxyPreamble`] naming the app, then a
//!   raw HTTP/1.1 request for that app, answered in raw HTTP.
//! - [`StreamType::ForwardProxy`] for apps in forward-proxy mode, on the data
//!   plane. One length-prefixed [`ForwardProxyPreamble`] naming a `host:port`
//!   from the share's proxied origins, answered by one [`ForwardProxyReply`].
//!   After a successful reply, raw bytes both ways: one tunnelled connection.
//! - [`StreamType::Ctrl`] for the control plane. A raw HTTP/1.1 request to the
//!   host node's guest API (the `/v1/...` routes below), answered in raw HTTP.
//!
//! Streams whose first byte is an ASCII letter are raw HTTP requests for the
//! default app from clients that predate this framing. Every stream ends in a
//! FIN from the side that wrote last.
//!
//! The JSON types below are the contract. Fields are only ever added, with a
//! default for readers that predate them; a field is never renamed or given a
//! new meaning. Unknown fields are ignored.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

//-- Invite codes --------------------------------------------------------------

/// Transports for peer-to-peer communication between guest nodes and the host node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    WispersConnect,
    Iroh,
    Tailscale,
}

/// Parses the full name, as in `share.toml`.
impl std::str::FromStr for Transport {
    type Err = UnknownTransport;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "wispers-connect" => Ok(Transport::WispersConnect),
            "iroh" => Ok(Transport::Iroh),
            "tailscale" => Ok(Transport::Tailscale),
            other => Err(UnknownTransport(other.to_owned())),
        }
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("unknown transport {0:?}")]
pub struct UnknownTransport(String);

impl Transport {
    /// The short tag in a version-1 invite code.
    pub fn tag(self) -> &'static str {
        match self {
            Transport::WispersConnect => "wc",
            Transport::Iroh => "iroh",
            Transport::Tailscale => "ts",
        }
    }

    /// The full name, as in `share.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::WispersConnect => "wispers-connect",
            Transport::Iroh => "iroh",
            Transport::Tailscale => "tailscale",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "wc" => Some(Transport::WispersConnect),
            "iroh" => Some(Transport::Iroh),
            "ts" => Some(Transport::Tailscale),
            _ => None,
        }
    }
}

/// An invite, as carried by an invite code.
///
/// Version 1 is `wax1_<transport tag>_<field>[_<field>…]`. Version 0 is
/// `wax_<registration-token>_<activation-code>[_<backend>]`, for Wispers
/// Connect.Byte strings are lowercase hex, URLs are lowercase unpadded base32.
///
/// TODO: Nothing emits wax_ since 2026-10. Clean up the code accepting this
/// format.
#[derive(Clone, PartialEq, Eq)]
pub enum Invite {
    WispersConnect {
        registration_token: String,
        activation_code: String,
        /// Self-hosted Wispers Connect backend, `https://` only.
        backend: Option<String>,
    },
    Iroh {
        endpoint_id: EndpointId,
        /// Presented on first contact to bind the guest's key to the invite.
        secret: InviteSecret,
    },
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum InviteError {
    #[error("not an invite code (expected wax1_<transport>_…)")]
    NotAnInvite,
    /// A transport this parser does not know or was built without.
    #[error("unsupported transport {0:?}")]
    UnsupportedTransport(String),
    #[error("malformed invite: {0}")]
    Malformed(&'static str),
}

impl Invite {
    pub fn parse(code: &str) -> Result<Invite, InviteError> {
        let code = code.trim();
        if let Some(rest) = code.strip_prefix("wax1_") {
            let mut fields = rest.split('_');
            let tag = fields.next().unwrap_or("");
            if tag.is_empty() {
                return Err(InviteError::NotAnInvite);
            }
            match Transport::from_tag(tag) {
                Some(Transport::WispersConnect) => parse_wispers_connect(fields),
                Some(Transport::Iroh) => parse_iroh(fields),
                Some(Transport::Tailscale) | None => {
                    // Known, but no invite is defined for it.
                    Err(InviteError::UnsupportedTransport(tag.to_owned()))
                }
            }
        } else if let Some(rest) = code.strip_prefix("wax_") {
            parse_wispers_connect(rest.split('_'))
        } else {
            Err(InviteError::NotAnInvite)
        }
    }

    /// The code string for this invite.
    pub fn to_code(&self) -> String {
        match self {
            Invite::WispersConnect {
                registration_token,
                activation_code,
                backend,
            } => {
                let base = format!(
                    "wax1_{}_{registration_token}_{activation_code}",
                    Transport::WispersConnect.tag()
                );
                match backend {
                    Some(backend) => format!("{base}_{}", encode_url(backend)),
                    None => base,
                }
            }
            Invite::Iroh {
                endpoint_id,
                secret,
            } => format!("wax1_{}_{endpoint_id}_{secret}", Transport::Iroh.tag()),
        }
    }

    pub fn transport(&self) -> Transport {
        match self {
            Invite::WispersConnect { .. } => Transport::WispersConnect,
            Invite::Iroh { .. } => Transport::Iroh,
        }
    }
}

fn parse_wispers_connect<'a>(
    mut fields: impl Iterator<Item = &'a str>,
) -> Result<Invite, InviteError> {
    let registration_token = fields.next().unwrap_or("");
    let activation_code = fields.next().unwrap_or("");
    if registration_token.is_empty() {
        return Err(InviteError::Malformed("missing registration token"));
    }
    if activation_code.is_empty() {
        return Err(InviteError::Malformed("missing activation code"));
    }
    let backend = fields.next().map(decode_backend).transpose()?;
    Ok(Invite::WispersConnect {
        registration_token: registration_token.to_owned(),
        activation_code: activation_code.to_owned(),
        backend,
    })
}

/// Only an `https://` backend is accepted: a plaintext or bogus hub fails
/// the whole code rather than falling back to the managed hub.
fn decode_backend(field: &str) -> Result<String, InviteError> {
    let url = decode_url(field).ok_or(InviteError::Malformed("backend is not a base32 URL"))?;
    if !url.starts_with("https://") {
        return Err(InviteError::Malformed("backend URL must be https://"));
    }
    Ok(url)
}

fn parse_iroh<'a>(mut fields: impl Iterator<Item = &'a str>) -> Result<Invite, InviteError> {
    let endpoint_id = fields
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| InviteError::Malformed("endpoint id must be 64 hex digits"))?;
    let secret = fields
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| InviteError::Malformed("secret must be 32 hex digits"))?;
    Ok(Invite::Iroh {
        endpoint_id,
        secret,
    })
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Invite::WispersConnect { backend, .. } => f
                .debug_struct("WispersConnect")
                .field("registration_token", &"<redacted>")
                .field("activation_code", &"<redacted>")
                .field("backend", backend)
                .finish(),
            Invite::Iroh { endpoint_id, .. } => f
                .debug_struct("Iroh")
                .field("endpoint_id", &endpoint_id.to_string())
                .field("secret", &"<redacted>")
                .finish(),
        }
    }
}

/// An iroh endpoint ID: an Ed25519 public key, 64 hex digits in text, the
/// form iroh itself prints and parses.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointId(pub [u8; 32]);

/// The one-time secret in an iroh invite, 16 random bytes, 32 hex digits in
/// text. The host node keeps only its SHA-256.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InviteSecret(pub [u8; 16]);

macro_rules! hex_bytes {
    ($name:ident, $len:expr, $what:literal) => {
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
            }
        }

        impl std::str::FromStr for $name {
            type Err = HexError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let bytes = data_encoding::HEXLOWER_PERMISSIVE
                    .decode(s.as_bytes())
                    .map_err(|_| HexError($what))?;
                let bytes: [u8; $len] = bytes.try_into().map_err(|_| HexError($what))?;
                Ok($name(bytes))
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_bytes!(EndpointId, 32, "endpoint id");
hex_bytes!(InviteSecret, 16, "invite secret");

impl std::fmt::Debug for EndpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EndpointId({self})")
    }
}

impl std::fmt::Debug for InviteSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteSecret(<redacted>)")
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("{0} is not the right number of hex digits")]
pub struct HexError(&'static str);

/// URLs in invite codes are lowercase unpadded base32 of the UTF-8 bytes, which
/// keeps them from colliding with the `_` delimiter.
fn encode_url(url: &str) -> String {
    data_encoding::BASE32_NOPAD
        .encode(url.as_bytes())
        .to_lowercase()
}

fn decode_url(field: &str) -> Option<String> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(field.to_uppercase().as_bytes())
        .ok()?;
    String::from_utf8(bytes).ok()
}

//-- Streams -------------------------------------------------------------------

/// The first byte of a stream says what the rest of it carries. Types are only
/// ever added; a host closes a stream of a type it doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamType {
    ReverseProxy = 0x00,
    Ctrl = 0x01,
    ForwardProxy = 0x02,
}

impl StreamType {
    pub fn plane(self) -> Plane {
        match self {
            StreamType::ReverseProxy | StreamType::ForwardProxy => Plane::Data,
            StreamType::Ctrl => Plane::Control,
        }
    }
}

/// Only activated guest nodes may open data-plane streams. The control plane is
/// open to every authenticated peer, so that it can activate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    Data,
    Control,
}

/// What the first byte of a stream says about the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstByte {
    Typed(StreamType),
    LegacyHttp(u8),
    Unknown(u8),
}

impl From<u8> for FirstByte {
    fn from(first: u8) -> Self {
        match first {
            0x00 => FirstByte::Typed(StreamType::ReverseProxy),
            0x01 => FirstByte::Typed(StreamType::Ctrl),
            0x02 => FirstByte::Typed(StreamType::ForwardProxy),
            // Every HTTP method starts with an ASCII letter.
            b'A'..=b'Z' | b'a'..=b'z' => FirstByte::LegacyHttp(first),
            other => FirstByte::Unknown(other),
        }
    }
}

//-- REVERSE_PROXY streams -----------------------------------------------------

/// Preamble of a REVERSE_PROXY stream: which of the share's apps the request
/// that follows is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseProxyPreamble {
    pub app_id: String,
}

//-- FORWARD_PROXY streams -----------------------------------------------------

/// Preamble of a FORWARD_PROXY stream, indicating where to connect. It names no
/// app, because the guest node's proxy serves a whole share and can't tell which app
/// a browser connection belongs to. The host checks `(host, port)` against the
/// union of the share's proxied origins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardProxyPreamble {
    /// In the form of [`Origin::host`].
    pub host: String,
    pub port: u16,
}

/// The host's answer to a [`ForwardProxyPreamble`]. An error signals the end of
/// the stream. Otherwise, the host is now connected to the target, and the
/// stream carries raw bytes from here on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardProxyReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ForwardProxyError>,
}

/// Why a FORWARD_PROXY stream was refused. Values are only ever added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForwardProxyError {
    /// The target is not one of the share's proxied origins.
    NotAllowed,
    /// The host node could not connect to the target.
    Unreachable,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

//-- The guest API, over CTRL streams ------------------------------------------

/// `GET /v1/share` returns metadata for the share, as JSON [`ShareInfo`].
/// The response carries an `ETag` of the config hash - a request with a matching
/// `If-None-Match` gets 304 and no body.
pub const SHARE_PATH: &str = "/v1/share";

/// `GET /v1/events` returns a long-lived `text/event-stream`. Each event is
/// named after its type and carries that type as JSON data:
///
/// - [`EVENT_SHARE_CHANGED`] with [`ShareChanged`]: the share's config has
///   changed. Refresh it with `GET /v1/share`.
pub const EVENTS_PATH: &str = "/v1/events";

pub const EVENT_SHARE_CHANGED: &str = "share-changed";

/// `POST /v1/activation` (iroh only): redeems an invite code and binds the
/// guest's key to the invite. After this, the host knows which identity is
/// associated with the guest node.
///
/// The body is an [`Activation`]. On success, returns 200 with the same
/// [`ShareInfo`] body and `ETag` as `GET /v1/share`. On failure, returns an
/// [`ApiError`] carrying an [`ActivationError`]; an unbound connection is
/// then closed with [`CloseCode::Unknown`], which is what the key still is.
pub const ACTIVATION_PATH: &str = "/v1/activation";

/// `DELETE /v1/guest` (iroh only): the calling guest leaves the share. The
/// host forgets it and answers 204.
pub const GUEST_PATH: &str = "/v1/guest";

/// Body of `POST /v1/activation`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activation {
    pub secret: InviteSecret,
}

impl std::fmt::Debug for Activation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Activation { secret: <redacted> }")
    }
}

/// Body of every guest API error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// The `User-Agent` value for an SDK of the given `version`.
pub fn client_user_agent(version: &str) -> String {
    format!("{CLIENT_PRODUCT}/{version}")
}

/// Extracts the version from a user agent string as created by
/// [`client_user_agent`].
pub fn client_version(user_agent: &str) -> Option<semver::Version> {
    let first = user_agent.split_whitespace().next()?;
    let version = first.strip_prefix(CLIENT_PRODUCT)?.strip_prefix('/')?;
    semver::Version::parse(version).ok()
}

/// The product token used by [`client_user_agent`] and [`client_version`].
const CLIENT_PRODUCT: &str = "wispers-access-sdk";

/// The `error` sent with a 426 Upgrade Required.
pub const CLIENT_TOO_OLD: &str = "client-too-old";

/// Why an activation was refused, the `error` of its [`ApiError`]. Values are
/// only ever added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationError {
    /// The body is not an [`Activation`].
    Malformed,
    /// No invite with this secret.
    InviteUnknown,
    /// Past its validity.
    InviteExpired,
    /// Redeemed by another endpoint ID. (The same endpoint ID redeeming
    /// again gets 200: a lost response is retried, not punished.)
    InviteConsumed,
    /// This endpoint ID was revoked; a revoked key is never re-bound.
    Revoked,
    /// This endpoint ID is already a guest node.
    AlreadyGuest,
}

impl ActivationError {
    pub fn status(self) -> u16 {
        match self {
            ActivationError::Malformed => 400,
            ActivationError::InviteUnknown | ActivationError::Revoked => 403,
            ActivationError::InviteExpired => 410,
            ActivationError::InviteConsumed | ActivationError::AlreadyGuest => 409,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ActivationError::Malformed => "malformed",
            ActivationError::InviteUnknown => "invite-unknown",
            ActivationError::InviteExpired => "invite-expired",
            ActivationError::InviteConsumed => "invite-consumed",
            ActivationError::Revoked => "revoked",
            ActivationError::AlreadyGuest => "already-guest",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(code.to_owned())).ok()
    }
}

impl From<ActivationError> for ApiError {
    fn from(e: ActivationError) -> Self {
        ApiError {
            error: e.as_str().to_owned(),
        }
    }
}

/// Guest-facing metadata for a share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareInfo {
    /// Hash of everything below, to make it easy to detect changes.
    pub config_hash: ConfigHash,
    /// Display name of the share.
    pub name: String,
    pub transport: String,
    pub apps: Vec<SharedApp>,
    /// CA certificates for the share's `https` origins, one PEM certificate
    /// each. Guests trust them for those origins only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ca_certificates: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedApp {
    /// Stable key.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Absent means `web`, and so does a kind this build does not know: a
    /// host may add kinds before its guests learn them.
    #[serde(default, deserialize_with = "AppKind::deserialize_or_web")]
    pub kind: AppKind,
    /// The proxy mode used for this app, defaults to reverse-proxy. Unknown
    /// values get read as [`AppProxyMode::Unknown`] and are a sign that the
    ///  client app needs to update.
    #[serde(default)]
    pub mode: AppProxyMode,
    /// The URL guest nodes open first in forward-proxy mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_url: Option<String>,
    /// In forward-proxy mode, the proxied origins guest nodes send through the
    /// share. Each should be parseable as an [`Origin`], failure indicates a
    /// need to update the client app.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxied_origins: Vec<String>,
}

/// What kind of app is being shared: a generic web app, or one of the apps that
/// integrating clients know how to talk to. Strict on its own (a config file
/// must not silently accept a typo); the guest-facing [`SharedApp`] reads unknown
/// kinds as `web`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppKind {
    #[default]
    Web,
    // Values below are aspirational.
    Jellyfin,
    Immich,
}

impl AppKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AppKind::Web => "web",
            AppKind::Jellyfin => "jellyfin",
            AppKind::Immich => "immich",
        }
    }

    /// The kind named by `s`, or `web` when this build does not know it.
    pub fn parse_or_web(s: &str) -> AppKind {
        serde_json::from_value(serde_json::Value::String(s.to_owned())).unwrap_or_default()
    }

    fn deserialize_or_web<'de, D: serde::Deserializer<'de>>(d: D) -> Result<AppKind, D::Error> {
        let s = String::deserialize(d)?;
        Ok(AppKind::parse_or_web(&s))
    }
}

/// How guest nodes reach an app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppProxyMode {
    /// The client app reverse-proxies http://<app>.<share>.wa.localhost:11235
    /// to the host node's upstream. The browser talks HTTP to the local proxy
    /// and takes it for the app's server. Suitable for an app that lives on one
    /// origin and works at a URL other than its own. Requires much less plumbin
    /// to get working with the browser side.
    #[default]
    ReverseProxy,
    /// The client offers a forward proxy to the browser, which uses `CONNECT`
    /// requests to talk to the proxied app. This allows for multiple origins.
    /// The browser opens the app's real URL (`start_url`) and can connect to
    /// all allowlisted origins, with TLS support. Because internal servers
    /// often use self-signed certs, the share's `ca_certificates` are used to
    /// verify them. This is more work to get working with the browser side.
    ForwardProxy,
    /// A mode this build does not know.
    #[serde(other)]
    Unknown,
}

impl AppProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AppProxyMode::ReverseProxy => "reverse-proxy",
            AppProxyMode::ForwardProxy => "forward-proxy",
            AppProxyMode::Unknown => "unknown",
        }
    }

    /// The mode named by `s`, [`AppProxyMode::Unknown`] when this build does not
    /// know it.
    pub fn parse(s: &str) -> AppProxyMode {
        serde_json::from_value(serde_json::Value::String(s.to_owned()))
            .unwrap_or(AppProxyMode::Unknown)
    }
}

/// A web origin, `scheme://host[:port]`, as listed in an app's proxied origins.
/// The scheme is `http` or `https` and sets the default port. Parsing
/// normalises the host to lowercase (punycode for international names) and
/// drops default ports.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("{0}")]
pub struct OriginError(&'static str);

impl Origin {
    /// The origin of a URL, which must be `http` or `https`.
    pub fn of(url: &url::Url) -> Result<Origin, OriginError> {
        let https = match url.scheme() {
            "http" => false,
            "https" => true,
            _ => return Err(OriginError("the scheme must be http or https")),
        };
        let host = url
            .host_str()
            .ok_or(OriginError("the host is missing"))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .expect("http and https have default ports");
        Ok(Origin { https, host, port })
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    /// A lowercase DNS name, an IPv4 literal, or an IPv6 literal in brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port, the scheme's default if none was given.
    pub fn port(&self) -> u16 {
        self.port
    }

    fn default_port(&self) -> u16 {
        if self.https { 443 } else { 80 }
    }
}

impl std::str::FromStr for Origin {
    type Err = OriginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url =
            url::Url::parse(s.trim()).map_err(|_| OriginError("expected scheme://host[:port]"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(OriginError("an origin has no user name or password"));
        }
        // The URL parser turns an empty path into "/", so a trailing slash is
        // accepted.
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(OriginError("an origin has no path, query or fragment"));
        }
        Origin::of(&url)
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.https { "https" } else { "http" };
        write!(f, "{scheme}://{}", self.host)?;
        if self.port != self.default_port() {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

/// Data of a `share-changed` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareChanged {
    pub config_hash: ConfigHash,
}

/// A 64-bit hash, carried as 16 hex digits so JavaScript doesn't get confused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ConfigHash(pub u64);

impl ConfigHash {
    /// The strong entity tag form, `"0123456789abcdef"`.
    pub fn etag(self) -> String {
        format!("\"{self}\"")
    }

    /// Parses an `If-None-Match` value: one or more entity tags, or `*`.
    pub fn matches_if_none_match(self, header: &str) -> bool {
        let mine = self.etag();
        header
            .split(',')
            .map(str::trim)
            .any(|tag| tag == "*" || tag == mine || tag.strip_prefix("W/") == Some(&mine))
    }
}

impl std::fmt::Display for ConfigHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for ConfigHash {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(s, 16).map(ConfigHash)
    }
}

impl Serialize for ConfigHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ConfigHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

//-- Connections ---------------------------------------------------------------

/// The ALPN of an iroh connection.
pub const ALPN: &[u8] = b"wispers-access/1";

/// QUIC application close codes.
///
/// [`CloseCode::Unknown`] and [`CloseCode::Revoked`] make the guest mark the
/// share dead for good, so a guest should only act on them only when they
/// arrive on an established (i.e. trusted) connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseCode {
    /// Orderly close by either side.
    Closing = 0,
    /// The host does not know this endpoint ID. Terminal.
    Unknown = 1,
    /// This endpoint ID was revoked. Terminal.
    Revoked = 2,
}

impl CloseCode {
    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn reason(self) -> &'static [u8] {
        match self {
            CloseCode::Closing => b"closing",
            CloseCode::Unknown => b"unknown",
            CloseCode::Revoked => b"revoked",
        }
    }

    pub fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(CloseCode::Closing),
            1 => Some(CloseCode::Unknown),
            2 => Some(CloseCode::Revoked),
            _ => None, // Unknown, treat as [`CloseCode::Closing`].
        }
    }
}

//-- Framing -------------------------------------------------------------------

/// Upper bound on one framed message. Anything larger is a broken or hostile
/// peer.
pub const MAX_MESSAGE_LEN: u32 = 64 * 1024;

#[derive(thiserror::Error, Debug)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("message of {0} bytes exceeds the {MAX_MESSAGE_LEN} byte limit")]
    TooLong(u32),
    #[error("malformed message: {0}")]
    Decode(#[from] serde_json::Error),
}

/// Writes the type byte that opens a stream.
pub async fn write_stream_type<W: AsyncWrite + Unpin>(
    w: &mut W,
    t: StreamType,
) -> std::io::Result<()> {
    w.write_all(&[t as u8]).await
}

/// Opens a REVERSE_PROXY stream on the guest node's side, writing the type byte and
/// the preamble naming the app.
pub async fn open_reverse_proxy_stream<W: AsyncWrite + Unpin>(
    w: &mut W,
    app_id: &str,
) -> std::io::Result<()> {
    write_stream_type(w, StreamType::ReverseProxy).await?;
    write_message(
        w,
        &ReverseProxyPreamble {
            app_id: app_id.to_owned(),
        },
    )
    .await
}

/// Opens a FORWARD_PROXY stream on the guest node's side, writing the type byte and
/// the preamble naming the target. The host answers with a
/// [`ForwardProxyReply`], to be read with [`read_message`].
pub async fn open_forward_proxy_stream<W: AsyncWrite + Unpin>(
    w: &mut W,
    host: &str,
    port: u16,
) -> std::io::Result<()> {
    write_stream_type(w, StreamType::ForwardProxy).await?;
    write_message(
        w,
        &ForwardProxyPreamble {
            host: host.to_owned(),
            port,
        },
    )
    .await
}

/// Opens a CTRL stream on the guest side.
pub async fn open_ctrl_stream<W: AsyncWrite + Unpin>(w: &mut W) -> std::io::Result<()> {
    write_stream_type(w, StreamType::Ctrl).await
}

/// Writes one message: big-endian u32 length, then the JSON bytes.
pub async fn write_message<W: AsyncWrite + Unpin, M: Serialize>(
    w: &mut W,
    msg: &M,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let len = u32::try_from(bytes.len()).expect("preambles are small");
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&bytes).await
}

/// Reads one message written by [`write_message`].
pub async fn read_message<R: AsyncRead + Unpin, M: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> Result<M, FrameError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len);
    if len > MAX_MESSAGE_LEN {
        return Err(FrameError::TooLong(len));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wc(token: &str, code: &str, backend: Option<&str>) -> Invite {
        Invite::WispersConnect {
            registration_token: token.into(),
            activation_code: code.into(),
            backend: backend.map(str::to_owned),
        }
    }

    #[test]
    fn transport_names_round_trip() {
        for t in [
            Transport::WispersConnect,
            Transport::Iroh,
            Transport::Tailscale,
        ] {
            assert_eq!(t.as_str().parse::<Transport>(), Ok(t));
        }
        assert!("hub".parse::<Transport>().is_err());
    }

    #[test]
    fn client_version_reads_the_sdk_token_only() {
        assert_eq!(
            client_version(&client_user_agent("0.2.0")),
            Some(semver::Version::new(0, 2, 0))
        );
        assert_eq!(
            client_version("wispers-access-sdk/1.4.0 wispers-access-ios/2.0.0"),
            Some(semver::Version::new(1, 4, 0))
        );
        assert_eq!(client_version("Mozilla/5.0"), None);
        assert_eq!(client_version("wispers-access-sdk/latest"), None);
        assert_eq!(client_version(""), None);
    }

    #[test]
    fn version_0_still_parses() {
        assert_eq!(
            Invite::parse("wax_ab12cd_1-xyz789").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        let url = "https://myhub.example.com";
        assert_eq!(
            Invite::parse(&format!("wax_ab12cd_1-xyz789_{}", encode_url(url))).unwrap(),
            wc("ab12cd", "1-xyz789", Some(url))
        );
    }

    #[test]
    fn wispers_connect_round_trips_in_version_1() {
        assert_eq!(
            wc("ab12cd", "1-xyz789", None).to_code(),
            "wax1_wc_ab12cd_1-xyz789"
        );
        assert_eq!(
            Invite::parse("wax1_wc_ab12cd_1-xyz789").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        // Pasted whitespace is tolerated.
        assert_eq!(
            Invite::parse("  wax1_wc_ab12cd_1-xyz789\n").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        let url = "https://myhub.example.com";
        let with_backend = wc("ab12cd", "1-xyz789", Some(url));
        let code = with_backend.to_code();
        assert_eq!(code, format!("wax1_wc_ab12cd_1-xyz789_{}", encode_url(url)));
        assert!(!encode_url(url).contains('_'));
        assert_eq!(Invite::parse(&code).unwrap(), with_backend);
    }

    #[test]
    fn iroh_round_trips() {
        let invite = Invite::Iroh {
            endpoint_id: EndpointId([0xab; 32]),
            secret: InviteSecret([0x01; 16]),
        };
        let code = invite.to_code();
        assert_eq!(
            code,
            format!("wax1_iroh_{}_{}", "ab".repeat(32), "01".repeat(16))
        );
        assert_eq!(Invite::parse(&code).unwrap(), invite);
        assert_eq!(invite.transport(), Transport::Iroh);
        // Uppercase hex is accepted on the way in.
        assert_eq!(
            Invite::parse(&code.to_uppercase().replace("WAX1_IROH", "wax1_iroh")).unwrap(),
            invite
        );
        // Appended fields are ignored.
        assert_eq!(Invite::parse(&format!("{code}_future")).unwrap(), invite);
    }

    #[test]
    fn malformed_iroh_fields() {
        let id = "ab".repeat(32);
        let secret = "01".repeat(16);
        assert_eq!(
            Invite::parse(&format!("wax1_iroh_{}_{secret}", "ab".repeat(31))),
            Err(InviteError::Malformed("endpoint id must be 64 hex digits"))
        );
        assert_eq!(
            Invite::parse(&format!("wax1_iroh_{id}_zz")),
            Err(InviteError::Malformed("secret must be 32 hex digits"))
        );
        assert_eq!(
            Invite::parse(&format!("wax1_iroh_{id}")),
            Err(InviteError::Malformed("secret must be 32 hex digits"))
        );
    }

    #[test]
    fn unsupported_and_malformed_codes() {
        assert_eq!(
            Invite::parse("wax1_ts_anything"),
            Err(InviteError::UnsupportedTransport("ts".into()))
        );
        assert_eq!(
            Invite::parse("wax1_plex_x"),
            Err(InviteError::UnsupportedTransport("plex".into()))
        );
        for code in ["", "wax1_", "ab12cd/1-xyz789", "wax2_wc_a_b"] {
            assert_eq!(
                Invite::parse(code),
                Err(InviteError::NotAnInvite),
                "{code:?}"
            );
        }
        for code in ["wax_ab12cd", "wax_ab12cd_", "wax1_wc_ab12cd"] {
            assert_eq!(
                Invite::parse(code),
                Err(InviteError::Malformed("missing activation code")),
                "{code:?}"
            );
        }
        assert_eq!(
            Invite::parse("wax1_wc__1-xyz789"),
            Err(InviteError::Malformed("missing registration token"))
        );
    }

    #[test]
    fn backend_must_be_https() {
        // A present-but-undecodable or non-https backend fails the whole
        // code, rather than falling back to the managed hub.
        let http = encode_url("http://evil.example.com");
        assert_eq!(
            Invite::parse(&format!("wax1_wc_ab12cd_1-xyz789_{http}")),
            Err(InviteError::Malformed("backend URL must be https://"))
        );
        assert_eq!(
            Invite::parse("wax1_wc_ab12cd_1-xyz789_!!notbase32"),
            Err(InviteError::Malformed("backend is not a base32 URL"))
        );
    }

    #[test]
    fn debug_redacts_secrets() {
        let dbg = format!("{:?}", wc("zq9v", "1-p7k2", Some("https://h")));
        assert!(!dbg.contains("zq9v") && !dbg.contains("p7k2"), "{dbg}");
        assert!(dbg.contains("https://h"));
        let dbg = format!(
            "{:?}",
            Invite::Iroh {
                endpoint_id: EndpointId([0xab; 32]),
                secret: InviteSecret([0x01; 16]),
            }
        );
        assert!(dbg.contains(&"ab".repeat(32)));
        assert!(!dbg.contains("0101"), "{dbg}");
        assert_eq!(
            format!(
                "{:?}",
                Activation {
                    secret: InviteSecret([0x01; 16])
                }
            ),
            "Activation { secret: <redacted> }"
        );
    }

    #[test]
    fn activation_shapes() {
        let body = serde_json::to_value(Activation {
            secret: InviteSecret([0x01; 16]),
        })
        .unwrap();
        assert_eq!(body, serde_json::json!({ "secret": "01".repeat(16) }));
        assert!(serde_json::from_str::<Activation>(r#"{"secret":"zz"}"#).is_err());

        let err: ApiError = ActivationError::InviteConsumed.into();
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({ "error": "invite-consumed" })
        );
        for e in [
            ActivationError::Malformed,
            ActivationError::InviteUnknown,
            ActivationError::InviteExpired,
            ActivationError::InviteConsumed,
            ActivationError::Revoked,
            ActivationError::AlreadyGuest,
        ] {
            assert_eq!(ActivationError::parse(e.as_str()), Some(e));
            assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
        }
        assert_eq!(ActivationError::parse("something-newer"), None);
        assert_eq!(ActivationError::InviteExpired.status(), 410);
    }

    #[test]
    fn close_codes_round_trip() {
        for c in [CloseCode::Closing, CloseCode::Unknown, CloseCode::Revoked] {
            assert_eq!(CloseCode::from_code(c.code().into()), Some(c));
            assert!(c.reason().is_ascii());
        }
        assert_eq!(CloseCode::from_code(3), None);
        assert_eq!(CloseCode::Unknown.code(), 1);
        assert_eq!(CloseCode::Revoked.reason(), b"revoked");
        assert_eq!(ALPN, b"wispers-access/1");
    }

    #[test]
    fn first_byte_classes() {
        assert_eq!(
            FirstByte::from(0x00),
            FirstByte::Typed(StreamType::ReverseProxy)
        );
        assert_eq!(FirstByte::from(0x01), FirstByte::Typed(StreamType::Ctrl));
        assert_eq!(
            FirstByte::from(0x02),
            FirstByte::Typed(StreamType::ForwardProxy)
        );
        for method in ["GET", "POST", "OPTIONS", "get"] {
            assert_eq!(
                FirstByte::from(method.as_bytes()[0]),
                FirstByte::LegacyHttp(method.as_bytes()[0])
            );
        }
        assert_eq!(FirstByte::from(0x03), FirstByte::Unknown(0x03));
        assert_eq!(FirstByte::from(b' '), FirstByte::Unknown(b' '));
        assert_eq!(FirstByte::from(0xff), FirstByte::Unknown(0xff));
    }

    #[test]
    fn both_proxy_types_are_data_plane() {
        assert_eq!(StreamType::ReverseProxy.plane(), Plane::Data);
        assert_eq!(StreamType::ForwardProxy.plane(), Plane::Data);
        assert_eq!(StreamType::Ctrl.plane(), Plane::Control);
    }

    // The JSON shapes are the contract; this pins them.
    #[test]
    fn message_shapes() {
        let info = ShareInfo {
            config_hash: ConfigHash(0xdead_beef),
            name: "Family".into(),
            transport: "wispers-connect".into(),
            apps: vec![
                SharedApp {
                    id: "jf".into(),
                    name: "Jellyfin".into(),
                    kind: AppKind::Jellyfin,
                    mode: AppProxyMode::ReverseProxy,
                    start_url: None,
                    proxied_origins: Vec::new(),
                },
                SharedApp {
                    id: "dev".into(),
                    name: "Dev".into(),
                    kind: AppKind::Web,
                    mode: AppProxyMode::ForwardProxy,
                    start_url: Some("http://localhost:4200/".into()),
                    proxied_origins: vec!["http://localhost:4200".into()],
                },
            ],
            ca_certificates: Vec::new(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["config_hash"], "00000000deadbeef");
        assert_eq!(json["apps"][0]["kind"], "jellyfin");
        // Reverse-proxy apps carry no forward-proxy fields, and a share
        // without CA certificates no empty list.
        assert_eq!(
            json["apps"][0],
            serde_json::json!({
                "id": "jf", "name": "Jellyfin", "kind": "jellyfin", "mode": "reverse-proxy"
            })
        );
        assert_eq!(json["apps"][1]["mode"], "forward-proxy");
        assert_eq!(json["apps"][1]["start_url"], "http://localhost:4200/");
        assert_eq!(
            json["apps"][1]["proxied_origins"],
            serde_json::json!(["http://localhost:4200"])
        );
        assert!(json.get("ca_certificates").is_none());
        assert_eq!(
            serde_json::to_value(ShareChanged {
                config_hash: ConfigHash(1)
            })
            .unwrap(),
            serde_json::json!({ "config_hash": "0000000000000001" })
        );
        // An app without a kind is a web app; unknown fields are ignored.
        let app: SharedApp =
            serde_json::from_str(r#"{"id":"x","name":"X","future_field":1}"#).unwrap();
        assert_eq!(app.kind, AppKind::Web);
        // Without a mode, as from hosts that predate modes, it's reverse-proxy.
        assert_eq!(app.mode, AppProxyMode::ReverseProxy);
        // An unknown kind reads as web: a host may add kinds before its guests.
        let app: SharedApp =
            serde_json::from_str(r#"{"id":"x","name":"X","kind":"plex"}"#).unwrap();
        assert_eq!(app.kind, AppKind::Web);
        // The kind on its own stays strict, for config files.
        assert!(serde_json::from_str::<AppKind>(r#""plex""#).is_err());
        // An unknown mode is kept as unknown, for the guest node to flag.
        let app: SharedApp =
            serde_json::from_str(r#"{"id":"x","name":"X","mode":"teleport"}"#).unwrap();
        assert_eq!(app.mode, AppProxyMode::Unknown);

        // A successful reply is an empty object, an unknown error still an error.
        assert_eq!(
            serde_json::to_value(ForwardProxyReply::default()).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(ForwardProxyReply {
                error: Some(ForwardProxyError::NotAllowed)
            })
            .unwrap(),
            serde_json::json!({ "error": "not-allowed" })
        );
        let reply: ForwardProxyReply = serde_json::from_str(r#"{"error":"later"}"#).unwrap();
        assert_eq!(reply.error, Some(ForwardProxyError::Unknown));
    }

    #[test]
    fn origins_parse_and_normalise() {
        let origin = |s: &str| s.parse::<Origin>().unwrap();
        let o = origin("http://localhost:1234");
        assert_eq!(
            (o.host(), o.port(), o.is_https()),
            ("localhost", 1234, false)
        );
        assert_eq!(o.to_string(), "http://localhost:1234");
        // Default ports are implied and dropped, case is folded, a trailing
        // slash is tolerated.
        let o = origin("HTTPS://SSO.Internal:443/");
        assert_eq!(
            (o.host(), o.port(), o.is_https()),
            ("sso.internal", 443, true)
        );
        assert_eq!(o.to_string(), "https://sso.internal");
        assert_eq!(origin("http://a.example").port(), 80);
        assert_eq!(origin("http://a.example:80"), origin("http://a.example"));
        // IP literals, IPv6 in brackets.
        assert_eq!(origin("http://127.0.0.1:8080").host(), "127.0.0.1");
        assert_eq!(origin("https://[::1]:8443").host(), "[::1]");
        assert_eq!(
            origin("https://[::1]:8443").to_string(),
            "https://[::1]:8443"
        );

        for bad in [
            "localhost:1234",
            "ftp://a.example",
            "ws://a.example",
            "http://a.example/path",
            "http://a.example/?q",
            "http://a.example/#f",
            "http://user@a.example",
            "http://",
            "",
        ] {
            assert!(bad.parse::<Origin>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn etag_forms() {
        let h = ConfigHash(0xdead_beef);
        assert_eq!(h.etag(), "\"00000000deadbeef\"");
        assert!(h.matches_if_none_match("\"00000000deadbeef\""));
        assert!(h.matches_if_none_match("\"0\", \"00000000deadbeef\""));
        assert!(h.matches_if_none_match("W/\"00000000deadbeef\""));
        assert!(h.matches_if_none_match("*"));
        assert!(!h.matches_if_none_match("\"00000000deadbeee\""));
        assert!(!h.matches_if_none_match("00000000deadbeef")); // unquoted is not a tag
        assert_eq!("00000000deadbeef".parse::<ConfigHash>().unwrap(), h);
    }

    #[tokio::test]
    async fn guest_side_openers_match_the_server_side_readers() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        open_reverse_proxy_stream(&mut a, "jf").await.unwrap();
        a.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

        let mut first = [0u8; 1];
        b.read_exact(&mut first).await.unwrap();
        assert_eq!(
            FirstByte::from(first[0]),
            FirstByte::Typed(StreamType::ReverseProxy)
        );
        let back: ReverseProxyPreamble = read_message(&mut b).await.unwrap();
        assert_eq!(back.app_id, "jf");
        let mut rest = [0u8; 16];
        b.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"GET / HTTP/1.1\r\n");

        let (mut a, mut b) = tokio::io::duplex(1024);
        open_forward_proxy_stream(&mut a, "sso.internal", 443)
            .await
            .unwrap();
        b.read_exact(&mut first).await.unwrap();
        assert_eq!(
            FirstByte::from(first[0]),
            FirstByte::Typed(StreamType::ForwardProxy)
        );
        let back: ForwardProxyPreamble = read_message(&mut b).await.unwrap();
        assert_eq!((back.host.as_str(), back.port), ("sso.internal", 443));

        let (mut a, mut b) = tokio::io::duplex(1024);
        open_ctrl_stream(&mut a).await.unwrap();
        b.read_exact(&mut first).await.unwrap();
        assert_eq!(
            FirstByte::from(first[0]),
            FirstByte::Typed(StreamType::Ctrl)
        );
    }

    #[tokio::test]
    async fn oversized_and_truncated_messages_are_errors() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(MAX_MESSAGE_LEN + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(
            read_message::<_, ReverseProxyPreamble>(&mut b).await,
            Err(FrameError::TooLong(_))
        ));

        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&8u32.to_be_bytes()).await.unwrap();
        a.write_all(&[1, 2, 3]).await.unwrap();
        drop(a);
        assert!(matches!(
            read_message::<_, ReverseProxyPreamble>(&mut b).await,
            Err(FrameError::Io(_))
        ));

        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&3u32.to_be_bytes()).await.unwrap();
        a.write_all(b"{{{").await.unwrap();
        assert!(matches!(
            read_message::<_, ReverseProxyPreamble>(&mut b).await,
            Err(FrameError::Decode(_))
        ));
    }
}
