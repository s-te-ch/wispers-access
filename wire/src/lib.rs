//! The Wispers Access wire contract between guests and `waserver`.
//!
//! Every stream a guest opens starts with one [`StreamType`] byte:
//!
//! - [`StreamType::Data`] for the data plane. One length-prefixed
//!   [`HttpPreamble`] naming the share, then a raw HTTP/1.1 request for that
//!   share's app, answered in raw HTTP.
//! - [`StreamType::Ctrl`] for the control plane. A raw HTTP/1.1 request to
//!   waserver's guest API  (the `/v1/...` routes below), answered in raw HTTP.
//!
//! Streams whose first byte is an ASCII letter are raw HTTP requests for
//! the default share from clients that predate this framing. Every stream
//! ends in a FIN from the side that wrote last.
//!
//! The JSON types below are the contract. Fields are only ever added, with
//! a default for readers that predate them; a field is never renamed or
//! given a new meaning. Unknown fields are ignored.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

//-- Invite codes --------------------------------------------------------------

/// Transports for peer-to-peer communication between guest nodes and server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    WispersConnect,
    Iroh,
    Tailscale,
}

impl Transport {
    /// The short tag in a version-1 invite code.
    pub fn tag(self) -> &'static str {
        match self {
            Transport::WispersConnect => "wc",
            Transport::Iroh => "iroh",
            Transport::Tailscale => "ts",
        }
    }

    /// The full name, as in `circle.toml`.
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
/// Connect. Byte strings are lowercase hex, URLs are lowercase unpadded base32.
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
    #[error("not an invite code (expected wax1_<transport>_… or wax_<token>_<code>)")]
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
    ///
    /// Wispers Connect invites are written in version 0 until the store
    /// apps parse version 1.
    pub fn to_code(&self) -> String {
        match self {
            Invite::WispersConnect {
                registration_token,
                activation_code,
                backend,
            } => {
                let base = format!("wax_{registration_token}_{activation_code}");
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
/// text. The server keeps only its SHA-256.
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

/// The first byte of a stream determines whether it belongs to the data plane
/// or the control plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamType {
    Data = 0x00,
    Ctrl = 0x01,
}

/// What the first byte of a stream says about the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstByte {
    Typed(StreamType),
    LegacyHttp,
    Unknown(u8),
}

impl From<u8> for FirstByte {
    fn from(first: u8) -> Self {
        match first {
            0x00 => FirstByte::Typed(StreamType::Data),
            0x01 => FirstByte::Typed(StreamType::Ctrl),
            // Every HTTP method starts with an ASCII letter.
            b'A'..=b'Z' | b'a'..=b'z' => FirstByte::LegacyHttp,
            other => FirstByte::Unknown(other),
        }
    }
}

//-- DATA streams --------------------------------------------------------------

/// Preamble of a DATA stream: which of the circle's shares the request that
/// follows is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpPreamble {
    pub share_id: String,
}

//-- The guest API, over CTRL streams ------------------------------------------

/// `GET /v1/circle` returns metadata for the circle, as JSON [`CircleInfo`].
/// The response carries an `ETag` of the config hash - a request with a matching
/// `If-None-Match` gets 304 and no body.
pub const CIRCLE_PATH: &str = "/v1/circle";

/// `GET /v1/events` returns a long-lived `text/event-stream`. Each event is
/// named after its type and carries that type as JSON data:
///
/// - [`EVENT_CIRCLE_CHANGED`] with [`CircleChanged`]: the circle config has
///   changed. Refresh it with `GET /v1/circle`.
pub const EVENTS_PATH: &str = "/v1/events";

pub const EVENT_CIRCLE_CHANGED: &str = "circle-changed";

/// `POST /v1/activation` (iroh only): redeems an invite code and binds the
/// guest's key to the invite. After this, the server know which identity is
/// associated with the guest node.
///
/// The body is an [`Activation`]. On success, returns 200 with the same
/// [`CircleInfo`] body and `ETag` as `GET /v1/circle`. On failure, returns an
/// [`ApiError`] carrying an [`ActivationError`], after which the server closes
/// the connection with [`CloseCode::ActivationFailed`].
pub const ACTIVATION_PATH: &str = "/v1/activation";

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
    /// This endpoint ID is already a member; served as that member.
    AlreadyMember,
}

impl ActivationError {
    pub fn status(self) -> u16 {
        match self {
            ActivationError::Malformed => 400,
            ActivationError::InviteUnknown | ActivationError::Revoked => 403,
            ActivationError::InviteExpired => 410,
            ActivationError::InviteConsumed | ActivationError::AlreadyMember => 409,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ActivationError::Malformed => "malformed",
            ActivationError::InviteUnknown => "invite-unknown",
            ActivationError::InviteExpired => "invite-expired",
            ActivationError::InviteConsumed => "invite-consumed",
            ActivationError::Revoked => "revoked",
            ActivationError::AlreadyMember => "already-member",
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

/// Guest-facing metadata for a circle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircleInfo {
    /// Hash of everything below, to make it easy to detect changes.
    pub config_hash: ConfigHash,
    /// Display name of the circle.
    pub name: String,
    pub transport: String,
    pub shares: Vec<Share>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// Stable key.
    pub id: String,
    /// Display name.
    pub name: String,
    #[serde(default)]
    pub kind: ShareKind,
}

/// What kind of app is being shared: a generic web app, or one of the apps that
/// integrating clients know how to talk to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShareKind {
    #[default]
    Web,
    // Values below are aspirational.
    Jellyfin,
    Immich,
}

impl ShareKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ShareKind::Web => "web",
            ShareKind::Jellyfin => "jellyfin",
            ShareKind::Immich => "immich",
        }
    }
}

/// Data of a `circle-changed` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircleChanged {
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
/// circle dead for good, so a guest should only act on them only when they
/// arrive on an established (i.e. trusted) connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseCode {
    /// Orderly close by either side.
    Closing = 0,
    /// The server does not know this endpoint ID. Terminal.
    Unknown = 1,
    /// This endpoint ID was revoked. Terminal.
    Revoked = 2,
    /// The activation on this connection failed.
    ActivationFailed = 3,
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
            CloseCode::ActivationFailed => b"activation-failed",
        }
    }

    pub fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(CloseCode::Closing),
            1 => Some(CloseCode::Unknown),
            2 => Some(CloseCode::Revoked),
            3 => Some(CloseCode::ActivationFailed),
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

/// Opens a DATA stream on the guest side, writing the type byte and the
/// preamble naming the share.
pub async fn open_data_stream<W: AsyncWrite + Unpin>(
    w: &mut W,
    share_id: &str,
) -> std::io::Result<()> {
    write_stream_type(w, StreamType::Data).await?;
    write_message(
        w,
        &HttpPreamble {
            share_id: share_id.to_owned(),
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
    fn version_0_is_wispers_connect() {
        assert_eq!(
            Invite::parse("wax_ab12cd_1-xyz789").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        // Pasted whitespace is tolerated.
        assert_eq!(
            Invite::parse("  wax_ab12cd_1-xyz789\n").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        let url = "https://myhub.example.com";
        let with_backend = wc("ab12cd", "1-xyz789", Some(url));
        let code = with_backend.to_code();
        assert_eq!(code, format!("wax_ab12cd_1-xyz789_{}", encode_url(url)));
        assert!(!encode_url(url).contains('_'));
        assert_eq!(Invite::parse(&code).unwrap(), with_backend);
        // Wispers Connect keeps writing version 0 for now.
        assert_eq!(
            wc("ab12cd", "1-xyz789", None).to_code(),
            "wax_ab12cd_1-xyz789"
        );
    }

    #[test]
    fn version_1_wispers_connect_parses_too() {
        assert_eq!(
            Invite::parse("wax1_wc_ab12cd_1-xyz789").unwrap(),
            wc("ab12cd", "1-xyz789", None)
        );
        let url = "https://myhub.example.com";
        assert_eq!(
            Invite::parse(&format!("wax1_wc_ab12cd_1-xyz789_{}", encode_url(url))).unwrap(),
            wc("ab12cd", "1-xyz789", Some(url))
        );
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
            Invite::parse("wax__1-xyz789"),
            Err(InviteError::Malformed("missing registration token"))
        );
    }

    #[test]
    fn backend_must_be_https() {
        // A present-but-undecodable or non-https backend fails the whole
        // code, rather than falling back to the managed hub.
        let http = encode_url("http://evil.example.com");
        assert_eq!(
            Invite::parse(&format!("wax_ab12cd_1-xyz789_{http}")),
            Err(InviteError::Malformed("backend URL must be https://"))
        );
        assert_eq!(
            Invite::parse("wax_ab12cd_1-xyz789_!!notbase32"),
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
            ActivationError::AlreadyMember,
        ] {
            assert_eq!(ActivationError::parse(e.as_str()), Some(e));
            assert_eq!(serde_json::to_value(e).unwrap(), e.as_str());
        }
        assert_eq!(ActivationError::parse("something-newer"), None);
        assert_eq!(ActivationError::InviteExpired.status(), 410);
    }

    #[test]
    fn close_codes_round_trip() {
        for c in [
            CloseCode::Closing,
            CloseCode::Unknown,
            CloseCode::Revoked,
            CloseCode::ActivationFailed,
        ] {
            assert_eq!(CloseCode::from_code(c.code().into()), Some(c));
            assert!(c.reason().is_ascii());
        }
        assert_eq!(CloseCode::from_code(4), None);
        assert_eq!(CloseCode::Unknown.code(), 1);
        assert_eq!(CloseCode::Revoked.reason(), b"revoked");
        assert_eq!(ALPN, b"wispers-access/1");
    }

    #[test]
    fn first_byte_classes() {
        assert_eq!(FirstByte::from(0x00), FirstByte::Typed(StreamType::Data));
        assert_eq!(FirstByte::from(0x01), FirstByte::Typed(StreamType::Ctrl));
        for method in ["GET", "POST", "OPTIONS", "get"] {
            assert_eq!(FirstByte::from(method.as_bytes()[0]), FirstByte::LegacyHttp);
        }
        assert_eq!(FirstByte::from(0x02), FirstByte::Unknown(0x02));
        assert_eq!(FirstByte::from(b' '), FirstByte::Unknown(b' '));
        assert_eq!(FirstByte::from(0xff), FirstByte::Unknown(0xff));
    }

    // The JSON shapes are the contract; this pins them.
    #[test]
    fn message_shapes() {
        let info = CircleInfo {
            config_hash: ConfigHash(0xdead_beef),
            name: "Family".into(),
            transport: "wispers-connect".into(),
            shares: vec![Share {
                id: "jf".into(),
                name: "Jellyfin".into(),
                kind: ShareKind::Jellyfin,
            }],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["config_hash"], "00000000deadbeef");
        assert_eq!(json["shares"][0]["kind"], "jellyfin");
        assert_eq!(
            serde_json::to_value(CircleChanged {
                config_hash: ConfigHash(1)
            })
            .unwrap(),
            serde_json::json!({ "config_hash": "0000000000000001" })
        );
        // A share without a kind is a web app; unknown fields are ignored.
        let share: Share =
            serde_json::from_str(r#"{"id":"x","name":"X","future_field":1}"#).unwrap();
        assert_eq!(share.kind, ShareKind::Web);
        // An unknown kind is a contract violation.
        assert!(serde_json::from_str::<Share>(r#"{"id":"x","name":"X","kind":"plex"}"#).is_err());
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
        open_data_stream(&mut a, "jf").await.unwrap();
        a.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();

        let mut first = [0u8; 1];
        b.read_exact(&mut first).await.unwrap();
        assert_eq!(
            FirstByte::from(first[0]),
            FirstByte::Typed(StreamType::Data)
        );
        let back: HttpPreamble = read_message(&mut b).await.unwrap();
        assert_eq!(back.share_id, "jf");
        let mut rest = [0u8; 16];
        b.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"GET / HTTP/1.1\r\n");

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
            read_message::<_, HttpPreamble>(&mut b).await,
            Err(FrameError::TooLong(_))
        ));

        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&8u32.to_be_bytes()).await.unwrap();
        a.write_all(&[1, 2, 3]).await.unwrap();
        drop(a);
        assert!(matches!(
            read_message::<_, HttpPreamble>(&mut b).await,
            Err(FrameError::Io(_))
        ));

        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&3u32.to_be_bytes()).await.unwrap();
        a.write_all(b"{{{").await.unwrap();
        assert!(matches!(
            read_message::<_, HttpPreamble>(&mut b).await,
            Err(FrameError::Decode(_))
        ));
    }
}
