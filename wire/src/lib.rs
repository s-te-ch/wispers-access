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

/// The first byte of a stream.
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

/// `GET /v1/circle`: the circle as the guest may know it, as JSON
/// [`CircleInfo`]. The response carries an `ETag` of the config hash; a
/// request with a matching `If-None-Match` gets 304 and no body.
pub const CIRCLE_PATH: &str = "/v1/circle";

/// `GET /v1/events`: a long-lived `text/event-stream`. Each event is named
/// after its type and carries that type as JSON data:
///
/// - [`EVENT_CIRCLE_CHANGED`] with [`CircleChanged`]: the config changed;
///   fetch it with a conditional `GET /v1/circle`.
pub const EVENTS_PATH: &str = "/v1/events";

pub const EVENT_CIRCLE_CHANGED: &str = "circle-changed";

/// What a guest may know about a circle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircleInfo {
    /// Hash of everything below. Equal hashes mean an unchanged circle.
    pub config_hash: ConfigHash,
    /// Display name of the circle.
    pub name: String,
    /// The transport this circle rides, e.g. `wispers-connect`.
    pub transport: String,
    pub shares: Vec<Share>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// Stable key; what a guest persists and sends in [`HttpPreamble`].
    pub id: String,
    /// Display name.
    pub name: String,
    #[serde(default)]
    pub kind: ShareKind,
}

/// What a share is: a generic web app, or one of the apps that integrating
/// clients know how to talk to. Extended per integration; a value is never
/// renamed. The same vocabulary as waserver's config file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShareKind {
    #[default]
    Web,
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

/// Data of a `circle-changed` event. Carries no diff: the guest fetches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CircleChanged {
    pub config_hash: ConfigHash,
}

/// A 64-bit hash, carried as 16 hex digits so that JavaScript readers keep
/// every bit. Doubles as the `ETag` of `GET /v1/circle`.
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

//-- Framing -------------------------------------------------------------------

/// Upper bound on one framed message. Preambles are small; anything larger
/// is a broken or hostile peer.
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
    async fn preamble_round_trips() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let preamble = HttpPreamble {
            share_id: "jf".into(),
        };
        write_stream_type(&mut a, StreamType::Data).await.unwrap();
        write_message(&mut a, &preamble).await.unwrap();

        let mut first = [0u8; 1];
        b.read_exact(&mut first).await.unwrap();
        assert_eq!(
            FirstByte::from(first[0]),
            FirstByte::Typed(StreamType::Data)
        );
        let back: HttpPreamble = read_message(&mut b).await.unwrap();
        assert_eq!(back, preamble);
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
