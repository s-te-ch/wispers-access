//! Shared logic for proxy implementations for serving on the loopback device.

use crate::secrets::constant_time_eq;
use crate::transports::TerminalState;
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::TcpListener;
use tracing::warn;

/// Bind a port on both loopback addresses, `127.0.0.1` and `::1`, since a
/// browser may resolve `localhost` to either. Port `0` picks a free one.
/// Returns the port and its listeners.
///
/// Only a machine without an IPv6 loopback gets IPv4 alone. Otherwise, not
/// being able to bind the same port on both addresses is an error.
pub async fn bind_port(port: u16) -> Result<(u16, Vec<TcpListener>)> {
    for _ in 0..PORT_TRIES {
        let ipv4_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let v4_listener = TcpListener::bind(ipv4_addr)
            .await
            .with_context(|| format!("failed to bind to {ipv4_addr}"))?;
        let bound_port = v4_listener.local_addr()?.port();
        let ipv6_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), bound_port);
        match TcpListener::bind(ipv6_addr).await {
            Ok(v6_listener) => return Ok((bound_port, vec![v4_listener, v6_listener])),
            Err(e) if no_ipv6_loopback(&e) => {
                warn!(error = format!("{e:#}"), "no IPv6 loopback");
                return Ok((bound_port, vec![v4_listener]));
            }
            Err(_) if port == 0 => {
                // The auto-assigned port for IPv4 isn't available on IPv6. Retry.
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("no port free on both loopback addresses after {PORT_TRIES} tries")
}

/// How often port `0` is retried when the IPv4 pick is taken on `::1`.
const PORT_TRIES: usize = 8;

/// Whether binding `::1` failed because the machine has no IPv6 loopback,
/// rather than because the port is taken.
fn no_ipv6_loopback(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
    )
}

/// A cookie a loopback proxy demands on every request, so that only the client
/// app's own WebViews get through: every process on the device can connect to
/// the port.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RequiredCookie {
    pub name: String,
    pub value: String,
}

impl RequiredCookie {
    /// Whether the request's `Cookie` headers carry the cookie, compared in
    /// time that does not depend on where the values differ.
    pub(crate) fn is_presented_in(&self, headers: &hyper::HeaderMap) -> bool {
        headers
            .get_all(hyper::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(name, value)| name == self.name && constant_time_eq(value, &self.value))
    }
}

/// Removes every cookie whose name passes `criterion` from the request's
/// `Cookie` headers, so the shared app can't read them.
pub(crate) fn strip_cookies(headers: &mut hyper::HeaderMap, criterion: impl Fn(&str) -> bool) {
    use hyper::header::{COOKIE, HeaderValue};

    // One `Cookie` header without the stripped `name=value` pairs, or
    // nothing if that was all it carried.
    let without_stripped = |header: &HeaderValue| -> Option<HeaderValue> {
        let rest = header
            .to_str()
            .ok()?
            .split(';')
            .map(str::trim)
            .filter(|pair| {
                !pair
                    .split_once('=')
                    .is_some_and(|(name, _)| criterion(name))
            })
            .collect::<Vec<_>>()
            .join("; ");
        if rest.is_empty() {
            return None;
        }
        HeaderValue::from_str(&rest).ok()
    };

    let kept: Vec<HeaderValue> = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(without_stripped)
        .collect();
    headers.remove(COOKIE);
    for value in kept {
        headers.append(COOKIE, value);
    }
}

/// The body of a proxy's responses to the browser. Boxed, so it can be either
/// a shared app's body, streamed from the host node, or one the proxy made.
pub type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// After a shared app answered an upgrade request (WebSocket) with 101, relay
/// raw bytes both ways between the browser's connection and the stream to the
/// host node. Each direction ends at its own EOF: a half-close on one side is
/// passed on as a FIN while the other direction keeps flowing, so this returns
/// only once both have closed. Both `Upgraded` halves carry any bytes hyper
/// buffered past the handshake, so nothing is lost.
pub async fn relay_upgraded_connection(
    browser: hyper::upgrade::OnUpgrade,
    host_node: hyper::upgrade::OnUpgrade,
) {
    let (browser, host_node) = match tokio::try_join!(browser, host_node) {
        Ok(pair) => pair,
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                "upgrade handshake did not complete"
            );
            return;
        }
    };
    let mut browser = TokioIo::new(browser);
    let mut host_node = TokioIo::new(host_node);
    if let Err(e) = tokio::io::copy_bidirectional(&mut browser, &mut host_node).await {
        warn!(error = format!("{e:#}"), "upgraded relay error");
    }
}

/// Remove HTTP/1 hop-by-hop headers (RFC 7230 §6.1). `Transfer-Encoding` is
/// handled by hyper itself, so we leave it alone. On an Upgrade exchange,
/// `Connection` and `Upgrade` are preserved — they carry the handshake.
//
// Note: To be fully compliant, this should also process the `Connection`
// header's value to remove the headers it names. De facto, `keep-alive` and
// `close` are the only headers getting set.
pub fn strip_hop_by_hop_headers(headers: &mut hyper::HeaderMap, is_upgrade: bool) {
    for name in [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
    ] {
        headers.remove(name);
    }
    if !is_upgrade {
        headers.remove("connection");
        headers.remove("upgrade");
    }
}

/// True if this is an HTTP/1.1 protocol upgrade request (e.g. WebSocket): a
/// `Connection` header listing the `upgrade` token plus an `Upgrade` header
/// naming the target protocol.
pub fn is_protocol_upgrade_request(headers: &hyper::HeaderMap) -> bool {
    headers.contains_key(hyper::header::UPGRADE) && connection_lists_upgrade(headers)
}

fn connection_lists_upgrade(headers: &hyper::HeaderMap) -> bool {
    headers
        .get(hyper::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
}

/// The body of a response that has none, such as a 101 or a redirect.
pub fn empty_body() -> BoxedBody {
    Full::new(Bytes::new())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// The response for a share the host node removed or revoked this device
/// from.
pub fn gone_response(state: TerminalState) -> hyper::Response<BoxedBody> {
    error_response(
        StatusCode::GONE,
        &format!(
            "This app is no longer available on this device — {}.",
            state.describe()
        ),
    )
}

/// A response the proxy makes itself, with `msg` as its plain-text body.
pub fn error_response(status: hyper::StatusCode, msg: &str) -> hyper::Response<BoxedBody> {
    let body: BoxedBody = Full::new(Bytes::copy_from_slice(msg.as_bytes()))
        .map_err(|never: Infallible| match never {})
        .boxed();
    hyper::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
        .expect("static error response is always valid")
}

/// Whether an error serving the browser's connection just means the browser
/// went away (a reload, a closed tab): it closed the connection mid-message,
/// or reset it. Routine for a browser, so not worth a warning.
pub fn means_browser_went_away(e: &hyper::Error) -> bool {
    if e.is_incomplete_message() || e.is_body_write_aborted() {
        return true;
    }
    let mut source = std::error::Error::source(e);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
            );
        }
        source = err.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_port_taken_on_one_loopback_is_not_half_bound() {
        // Occupy a port on ::1 only.
        let Ok(occupant) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else {
            eprintln!("no IPv6 loopback on this machine");
            return;
        };
        let taken = occupant.local_addr().unwrap().port();
        // Asking for that port fails rather than serving IPv4 alone.
        assert!(bind_port(taken).await.is_err());
        // Letting the SDK pick still yields a port bound on both.
        let (port, listeners) = bind_port(0).await.unwrap();
        assert_ne!(port, taken);
        assert_eq!(listeners.len(), 2);
    }

    #[test]
    fn the_required_cookie_is_found_among_others() {
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::COOKIE,
            "a=1; __wispers_proxy_auth=s3cret".parse().unwrap(),
        );
        assert!(cookie.is_presented_in(&headers));
        headers.insert(
            hyper::header::COOKIE,
            "__wispers_proxy_auth=s3cre".parse().unwrap(),
        );
        assert!(!cookie.is_presented_in(&headers));
        assert!(!cookie.is_presented_in(&hyper::HeaderMap::new()));
    }

    #[test]
    fn the_required_cookie_is_stripped_and_the_others_kept() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            "a=1; __wispers_proxy_auth=s3cret; b=2".parse().unwrap(),
        );
        headers.append(
            hyper::header::COOKIE,
            "__wispers_proxy_auth=other".parse().unwrap(),
        );
        strip_cookies(&mut headers, |name| name == "__wispers_proxy_auth");
        let left: Vec<_> = headers.get_all(hyper::header::COOKIE).iter().collect();
        assert_eq!(left, ["a=1; b=2"]);
    }
}
