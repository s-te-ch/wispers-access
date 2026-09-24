//! The loopback HTTP proxy.
//!
//! The user's browser talks to `http://<app>.<share>.localhost:<port>`, and
//! every request becomes one DATA stream to the share's host node.

use crate::shares::{Lookup, ShareRegistry};
use crate::transports::{TerminalState, TransportError};
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

/// Binds the loopback port and serves every connection against `registry`.
/// Returns only when the port cannot be bound.
pub async fn serve(port: u16, registry: Arc<ShareRegistry>) -> Result<()> {
    let bind_addr = format!("localhost:{}", port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {}", bind_addr))?;
    println!("Listening on {}", bind_addr);
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let registry = registry.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(tcp_stream, registry).await {
                        eprintln!("Connection error: {:#}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept error: {:#}", e);
            }
        }
    }
}

async fn handle_connection(tcp_stream: TcpStream, registry: Arc<ShareRegistry>) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| forward(req, registry.clone()));
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
    registry: Arc<ShareRegistry>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    // Determine the share and app...
    let Ok(host) = extract_host(&req) else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "missing host header",
        ));
    };
    let (app, share) = match extract_target(&host) {
        Ok(target) => target,
        Err(e) => return Ok(error_response(StatusCode::NOT_FOUND, &e.to_string())),
    };
    let share = match registry.get(&share) {
        Lookup::Live(share) => share,
        Lookup::Dead(state) => return Ok(gone(state)),
        Lookup::Unknown => {
            return Ok(error_response(
                StatusCode::NOT_FOUND,
                &format!("unknown share '{}' (see 'waclient list')", share),
            ));
        }
    };

    // ... and open a DATA stream to it, naming the app.
    let fwd_stream = match share.open_data_stream(&app).await {
        Ok(s) => s,
        Err(TransportError::Terminal(state)) => return Ok(gone(state)),
        Err(TransportError::Transient(e)) => {
            eprintln!("[{}] open_stream failed: {:#}", share.label(), e);
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
            eprintln!("[{}] client handshake failed: {:#}", share.label(), e);
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            eprintln!("upstream connection error: {:#}", e);
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
            eprintln!("[{}] send_request failed: {:#}", share.label(), e);
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
        eprintln!(
            "[{}] app '{}' is gone; refreshing the app list",
            share.label(),
            app
        );
        let share = share.clone();
        tokio::spawn(async move {
            if let Err(e) = share.refresh().await {
                eprintln!(
                    "[{}] could not refresh the app list: {:#}",
                    share.label(),
                    e
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
            None => eprintln!(
                "[{}] the app returned 101 without an upgrade request",
                share.label()
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
            eprintln!("upgrade handshake did not complete: {:#}", e);
            return;
        }
    };
    let mut peer = TokioIo::new(peer);
    let mut upstream = TokioIo::new(upstream);
    if let Err(e) = tokio::io::copy_bidirectional(&mut peer, &mut upstream).await {
        eprintln!("upgraded relay error: {:#}", e);
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

/// `<app>.<share>.localhost[:port]` → (app, share).
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
            Ok(((*app).to_owned(), (*share).to_owned()))
        }
        _ => anyhow::bail!(
            "unknown host {}: apps are served at http://<app>.<share>.localhost:<port> (see 'waclient list')",
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

    #[test]
    fn targets_are_app_dot_share() {
        assert_eq!(
            extract_target("echo.round-trip.localhost:8000").unwrap(),
            ("echo".to_owned(), "round-trip".to_owned())
        );
        assert_eq!(
            extract_target("echo.round-trip.localhost").unwrap(),
            ("echo".to_owned(), "round-trip".to_owned())
        );
        // One label is not enough, and neither is a foreign host.
        assert!(extract_target("round-trip.localhost:8000").is_err());
        assert!(extract_target("localhost:8000").is_err());
        assert!(extract_target("echo.round-trip.example.com").is_err());
        assert!(extract_target(".round-trip.localhost").is_err());
        assert!(extract_target("a.b.c.localhost").is_err());
    }
}
