//! HTTP forwarding with (optional) header rewriting.

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::TcpStream;
use tracing::{info, warn};

/// Body type used in responses we send back to the peer. Boxed so we can return
/// either the upstream's streamed body or a locally-generated error body.
type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// Serve HTTP/1 over a single QUIC stream, forwarding to the upstream address.
pub async fn handle_quic_stream(
    stream: wispers_connect::QuicStream,
    upstream: Arc<str>,
    user_id: Option<String>,
) -> Result<()> {
    let io = TokioIo::new(stream);
    let service =
        hyper::service::service_fn(move |req| forward(req, upstream.clone(), user_id.clone()));
    http1_server::Builder::new()
        .serve_connection(io, service)
        // Allow a 101 to hand the QUIC stream over for raw relaying (WebSocket).
        .with_upgrades()
        .await
        .context("HTTP/1 connection error")
}

/// Service entry point. Always succeeds at the service level. Upstream failures
/// are converted to 5xx responses rather than connection errors.
async fn forward(
    req: hyper::Request<Incoming>,
    upstream: Arc<str>,
    user_id: Option<String>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    match try_forward(req, upstream, user_id).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            warn!(error = format!("{:#}", e), "forward error");
            Ok(error_response(
                hyper::StatusCode::BAD_GATEWAY,
                "upstream unavailable",
            ))
        }
    }
}

async fn try_forward(
    mut req: hyper::Request<Incoming>,
    upstream: Arc<str>,
    user_id: Option<String>,
) -> Result<hyper::Response<BoxedBody>> {
    // Open a connection to the upstream app. `upstream` is `host:port`; tokio
    // resolves DNS names (e.g. a Docker compose service) and parses IP:port.
    let tcp = TcpStream::connect(upstream.as_ref())
        .await
        .with_context(|| format!("connect to upstream {}", upstream))?;

    let (mut sender, conn) = http1_client::handshake(TokioIo::new(tcp))
        .await
        .context("upstream handshake")?;
    // Drive the connection I/O in its own task. `with_upgrades` keeps it alive to
    // hand over the raw stream on a 101 instead of treating that as an error.
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            warn!(error = %e, "upstream connection error");
        }
    });

    // An Upgrade request (e.g. WebSocket) keeps its handshake headers and, on a
    // 101, becomes a raw byte relay. Capture the peer-side upgrade now, before
    // the request is consumed; it resolves once we send the 101 back.
    let upgrade = is_upgrade_request(req.headers());
    let peer_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));

    // Rewrite the request before forwarding.
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop(&mut parts.headers, upgrade);
    inject_identity(&mut parts.headers, user_id.as_deref());
    // "forwarding" appearing means the request was fully received from the peer
    // over its QUIC stream and is now going to the local app.
    let uri_for_log = parts.uri.clone();
    info!(method = %parts.method, uri = %parts.uri, "forwarding to upstream");
    let forwarded = hyper::Request::from_parts(parts, body);

    let mut upstream_resp = sender
        .send_request(forwarded)
        .await
        .context("upstream send_request")?;
    info!(status = %upstream_resp.status(), uri = %uri_for_log, "upstream responded");

    // Successful upgrade: hand both raw byte streams to a relay task and return
    // the 101 to the peer with its handshake headers intact.
    if upstream_resp.status() == hyper::StatusCode::SWITCHING_PROTOCOLS {
        match peer_upgrade {
            Some(peer_upgrade) => {
                let upstream_upgrade = hyper::upgrade::on(&mut upstream_resp);
                tokio::spawn(splice_upgrade(peer_upgrade, upstream_upgrade));
            }
            None => warn!("upstream returned 101 without an upgrade request; not relaying"),
        }
        let (mut parts, _body) = upstream_resp.into_parts();
        strip_hop_by_hop(&mut parts.headers, true);
        return Ok(hyper::Response::from_parts(parts, empty_body()));
    }

    // Rewrite the response on the way back.
    let (mut parts, body) = upstream_resp.into_parts();
    strip_hop_by_hop(&mut parts.headers, false);
    let body: BoxedBody = body.map_err(std::io::Error::other).boxed();
    Ok(hyper::Response::from_parts(parts, body))
}

/// Relay raw bytes both ways between the peer (QUIC) side and the upstream
/// (local app) side after a successful protocol upgrade. Each direction ends at
/// its own EOF — a half-close on one side is forwarded as a FIN while the
/// opposite direction keeps flowing — so this returns only once both directions
/// have closed. Both `Upgraded` halves carry any bytes hyper buffered past the
/// handshake, so nothing is lost.
async fn splice_upgrade(peer: hyper::upgrade::OnUpgrade, upstream: hyper::upgrade::OnUpgrade) {
    let (peer, upstream) = match tokio::try_join!(peer, upstream) {
        Ok(pair) => pair,
        Err(e) => {
            warn!(error = %e, "upgrade handshake did not complete");
            return;
        }
    };
    let mut peer = TokioIo::new(peer);
    let mut upstream = TokioIo::new(upstream);
    match tokio::io::copy_bidirectional(&mut peer, &mut upstream).await {
        Ok((peer_to_upstream, upstream_to_peer)) => {
            info!(
                peer_to_upstream,
                upstream_to_peer, "upgraded connection closed"
            )
        }
        Err(e) => warn!(error = %e, "upgraded relay error"),
    }
}

/// Authoritative identity header injected on every forwarded request, naming the
/// guest behind the connection.
const IDENTITY_HEADER: &str = "x-wispers-access-user";

/// Set the identity header from the resolved peer identity. Always strips any
/// incoming value first.
fn inject_identity(headers: &mut hyper::HeaderMap, user_id: Option<&str>) {
    headers.remove(IDENTITY_HEADER);
    if let Some(user_id) = user_id {
        match hyper::header::HeaderValue::from_str(user_id) {
            Ok(value) => {
                headers.insert(IDENTITY_HEADER, value);
            }
            Err(_) => warn!(%user_id, "user_id is not a valid header value; omitting identity"),
        }
    }
}

/// Remove HTTP/1 hop-by-hop headers (RFC 7230 §6.1). `Transfer-Encoding` is
/// handled by hyper itself, so we leave it alone. On an Upgrade exchange,
/// `Connection` and `Upgrade` are preserved — they carry the handshake.
//
// Note: To be fully compliant, this should also process the `Connection`
// header's value to remove the headers it names. De facto, `keep-alive` and
// `close` are the only headers getting set.
fn strip_hop_by_hop(headers: &mut hyper::HeaderMap, is_upgrade: bool) {
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
