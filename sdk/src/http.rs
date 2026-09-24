//! The loopback HTTP proxy.
//!
//! The user's browser talks to `http://<app>.<share>.localhost:<port>`, and
//! every request becomes one DATA stream to the share's host node.

use crate::transports::{TerminalState, TransportError};
use crate::{Client, Lookup, ShareId};
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

/// Bind a port on both loopback addresses, `127.0.0.1` and `::1`, since a
/// browser may resolve `localhost` to either. Port `0` picks a free one.
/// Returns the port and its listeners.
///
/// Only a machine without an IPv6 loopback gets IPv4 alone, with a warning: a
/// port that is taken on one address is an error, or with port `0` another try.
pub async fn bind_loopback_port(port: u16) -> Result<(u16, Vec<TcpListener>)> {
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

/// How a listener knows which app a request is for.
#[derive(Clone)]
pub enum Route {
    /// From the `Host` header: `<app>.<share>.localhost`.
    FromHost,
    /// The listener serves this one app.
    Fixed { share: ShareId, app: String },
}

/// Serves every connection against the client's shares, until cancelled.
pub async fn accept_loop(listener: TcpListener, client: Client, route: Route) {
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let client = client.clone();
                let route = route.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(tcp_stream, client, route).await {
                        warn!(error = format!("{e:#}"), "connection error");
                    }
                });
            }
            Err(e) => {
                warn!(error = format!("{e:#}"), "accept error");
            }
        }
    }
}

async fn handle_connection(tcp_stream: TcpStream, client: Client, route: Route) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service =
        hyper::service::service_fn(move |req| forward(req, client.clone(), route.clone()));
    let served = http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        // Allow a 101 to hand the browser socket over for raw relaying (WebSocket).
        .with_upgrades()
        .await;
    match served {
        Ok(()) => Ok(()),
        // The client gave up on the response (a reload, a closed tab) and
        // closed its socket. Routine for a browser, not a fault.
        Err(e) if peer_went_away(&e) => Ok(()),
        Err(e) => Err(e).context("HTTP/1 connection error"),
    }
}

/// Body type used in responses we send back to the peer. Boxed so we can return
/// either the upstream's streamed body or a locally-generated error body.
type BoxedBody = BoxBody<Bytes, std::io::Error>;

async fn forward(
    mut req: hyper::Request<Incoming>,
    client: Client,
    route: Route,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    // Determine the share and app...
    let (share, app) = match route {
        Route::Fixed { share, app } => (share.to_string(), app),
        Route::FromHost => {
            let Ok(host) = extract_host(&req) else {
                return Ok(error_response(
                    StatusCode::BAD_REQUEST,
                    "missing host header",
                ));
            };
            match extract_target(&host) {
                Ok((share, app)) => (share, app),
                Err(e) => return Ok(error_response(StatusCode::NOT_FOUND, &e.to_string())),
            }
        }
    };
    let share = match client.guest_node(&share).await {
        Ok(Lookup::Live(share)) => share,
        Ok(Lookup::Dead(state)) => return Ok(gone(state)),
        Ok(Lookup::Unknown) => {
            return Ok(error_response(
                StatusCode::NOT_FOUND,
                &format!("unknown share '{}'", share),
            ));
        }
        Err(e) => {
            warn!(
                share,
                error = format!("{e:#}"),
                "could not restore the share"
            );
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };

    // ... and open a DATA stream to it, naming the app.
    let fwd_stream = match share.open_data_stream(&app).await {
        Ok(s) => s,
        Err(TransportError::Terminal(state)) => return Ok(gone(state)),
        Err(TransportError::Transient(e)) => {
            warn!(
                share = share.label(),
                error = format!("{e:#}"),
                "could not open a stream"
            );
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };
    let fwd_io = TokioIo::new(fwd_stream);
    let (mut sender, conn) = match http1_client::handshake(fwd_io).await {
        Ok(hs) => hs,
        Err(e) => {
            warn!(
                share = share.label(),
                error = format!("{e:#}"),
                "client handshake failed"
            );
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            warn!(error = format!("{e:#}"), "upstream connection error");
        }
    });

    // An Upgrade request (e.g. WebSocket) keeps its handshake headers and, on a
    // 101, becomes a raw byte relay. Capture the browser-side upgrade now, before
    // the request is consumed; it resolves once we send the 101 back.
    let upgrade = is_upgrade_request(req.headers());
    let peer_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));

    // Rewrite the query.
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers, upgrade);
    if !upgrade {
        // One QUIC stream per request: force close so hyper FINs the stream on both
        // ends after the single response, instead of holding it open in keep-alive.
        // Otherwise the stream is never finished or dropped, quiche never collects
        // it, and the peer never returns MAX_STREAMS credit — so open_stream
        // eventually blocks and requests hang under load. An upgrade is exempt: it
        // deliberately holds one stream open for the socket's lifetime, then FINs.
        parts.headers.insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("close"),
        );
    }
    let rewritten = hyper::Request::from_parts(parts, body);

    // Forward to upstream and get response.
    let mut resp = match sender.send_request(rewritten).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                share = share.label(),
                error = format!("{e:#}"),
                "sending the request failed"
            );
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };

    // The host node no longer has this app: our list is stale. Refresh it in
    // the background and pass the 404 on as it is.
    if resp.status() == StatusCode::NOT_FOUND
        && resp
            .headers()
            .get(ERROR_HEADER)
            .is_some_and(|v| v == "app-not-found")
    {
        info!(
            share = share.label(),
            app, "app is gone; refreshing the app list"
        );
        let share = share.clone();
        tokio::spawn(async move {
            if let Err(crate::guest_node::RefreshError::Transient(e)) = share.refresh().await {
                warn!(
                    share = share.label(),
                    error = format!("{e:#}"),
                    "could not refresh the app list"
                );
            }
        });
    }

    // Successful upgrade: hand both raw byte streams to a relay task and return
    // the 101 to the browser with its handshake headers intact.
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        match peer_upgrade {
            Some(peer_upgrade) => {
                let upstream_upgrade = hyper::upgrade::on(&mut resp);
                tokio::spawn(splice_upgrade(peer_upgrade, upstream_upgrade));
            }
            None => warn!(
                share = share.label(),
                "the app returned 101 without an upgrade request"
            ),
        }
        let (mut parts, _body) = resp.into_parts();
        strip_hop_by_hop_headers(&mut parts.headers, true);
        return Ok(hyper::Response::from_parts(parts, empty_body()));
    }

    // Rewrite the response on the way back.
    let (mut parts, body) = resp.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers, false);
    let body: BoxedBody = body.map_err(std::io::Error::other).boxed();
    Ok(hyper::Response::from_parts(parts, body))
}

/// Relay raw bytes both ways between the browser side and the QUIC-stream side
/// after a successful protocol upgrade. Each direction ends at its own EOF — a
/// half-close on one side is forwarded as a FIN while the opposite direction
/// keeps flowing — so this returns only once both directions have closed. Both
/// `Upgraded` halves carry any bytes hyper buffered past the handshake, so
/// nothing is lost.
async fn splice_upgrade(peer: hyper::upgrade::OnUpgrade, upstream: hyper::upgrade::OnUpgrade) {
    let (peer, upstream) = match tokio::try_join!(peer, upstream) {
        Ok(pair) => pair,
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                "upgrade handshake did not complete"
            );
            return;
        }
    };
    let mut peer = TokioIo::new(peer);
    let mut upstream = TokioIo::new(upstream);
    if let Err(e) = tokio::io::copy_bidirectional(&mut peer, &mut upstream).await {
        warn!(error = format!("{e:#}"), "upgraded relay error");
    }
}

/// Extract host from the request. Works with both HTTP 1 & 2.
fn extract_host(req: &hyper::Request<Incoming>) -> Result<String> {
    // Get the host header (or the "authority" in HTTP/2 lingo).
    let host = req.uri().authority().map(|a| a.as_str()).or_else(|| {
        req.headers()
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
    });
    let Some(host) = host else {
        anyhow::bail!("missing host header");
    };
    Ok(host.to_owned())
}

/// The header waserver sets on errors it generated itself, so a 404 from
/// the proxy can be told from one the app sent.
const ERROR_HEADER: &str = "x-wispers-access-error";

/// `<app>.<share>.localhost[:port]` → (share, app).
fn extract_target(host: &str) -> Result<(String, String)> {
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    match host.split('.').collect::<Vec<_>>().as_slice() {
        [app, share, "localhost"] if !app.is_empty() && !share.is_empty() => {
            Ok(((*share).to_owned(), (*app).to_owned()))
        }
        _ => anyhow::bail!(
            "unknown host {}: apps are served at http://<app>.<share>.localhost:<port>",
            host
        ),
    }
}

/// Remove HTTP/1 hop-by-hop headers (RFC 7230 §6.1). `Transfer-Encoding` is
/// handled by hyper itself, so we leave it alone. On an Upgrade exchange,
/// `Connection` and `Upgrade` are preserved — they carry the handshake.
//
// Note: To be fully compliant, this should also process the `Connection`
// header's value to remove the headers it names. De facto, `keep-alive` and
// `close` are the only headers getting set.
fn strip_hop_by_hop_headers(headers: &mut hyper::HeaderMap, is_upgrade: bool) {
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

/// True if this is an HTTP/1.1 Upgrade request (e.g. WebSocket): a `Connection`
/// header listing the `upgrade` token plus an `Upgrade` header naming the target
/// protocol.
fn is_upgrade_request(headers: &hyper::HeaderMap) -> bool {
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

fn empty_body() -> BoxedBody {
    Full::new(Bytes::new())
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// Terminal is deliberate, not an outage: 410 tells the reader that retrying
/// won't help, unlike a 502.
fn gone(state: TerminalState) -> hyper::Response<BoxedBody> {
    error_response(
        StatusCode::GONE,
        &format!(
            "This app is no longer available on this device — {}.",
            state.describe()
        ),
    )
}

fn error_response(status: hyper::StatusCode, msg: &str) -> hyper::Response<BoxedBody> {
    let body: BoxedBody = Full::new(Bytes::copy_from_slice(msg.as_bytes()))
        .map_err(|never: Infallible| match never {})
        .boxed();
    hyper::Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
        .expect("static error response is always valid")
}

/// Whether a hyper connection error just means the other side stopped
/// reading or writing - it closed mid-message, or the transport reported the
/// stream reset or stopped underneath hyper.
fn peer_went_away(e: &hyper::Error) -> bool {
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
        assert!(bind_loopback_port(taken).await.is_err());
        // Letting the SDK pick still yields a port bound on both.
        let (port, listeners) = bind_loopback_port(0).await.unwrap();
        assert_ne!(port, taken);
        assert_eq!(listeners.len(), 2);
    }

    #[test]
    fn targets_are_app_dot_share() {
        assert_eq!(
            extract_target("echo.round-trip.localhost:8000").unwrap(),
            ("round-trip".to_owned(), "echo".to_owned())
        );
        assert_eq!(
            extract_target("echo.round-trip.localhost").unwrap(),
            ("round-trip".to_owned(), "echo".to_owned())
        );
        // One label is not enough, and neither is a foreign host.
        assert!(extract_target("round-trip.localhost:8000").is_err());
        assert!(extract_target("localhost:8000").is_err());
        assert!(extract_target("echo.round-trip.example.com").is_err());
        assert!(extract_target(".round-trip.localhost").is_err());
        assert!(extract_target("a.b.c.localhost").is_err());
    }
}
