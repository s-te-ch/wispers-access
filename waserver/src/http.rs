//! HTTP forwarding with (optional) header rewriting.

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use tokio::net::TcpStream;
use tracing::{info, warn};

/// Body type used in responses we send back to the peer. Boxed so we can return
/// either the upstream's streamed body or a locally-generated error body.
type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// Serve HTTP/1 over a single QUIC stream, forwarding to the local port.
pub async fn handle_quic_stream(
    stream: wispers_connect::QuicStream,
    local_port: u16,
    user_id: Option<String>,
) -> Result<()> {
    let io = quic_stream_io::wrap(stream);
    let service =
        hyper::service::service_fn(move |req| forward(req, local_port, user_id.clone()));
    http1_server::Builder::new()
        .serve_connection(io, service)
        .await
        .context("HTTP/1 connection error")
}

/// Service entry point. Always succeeds at the service level. Upstream failures
/// are converted to 5xx responses rather than connection errors.
async fn forward(
    req: hyper::Request<Incoming>,
    local_port: u16,
    user_id: Option<String>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    match try_forward(req, local_port, user_id).await {
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
    req: hyper::Request<Incoming>,
    local_port: u16,
    user_id: Option<String>,
) -> Result<hyper::Response<BoxedBody>> {
    // Open upstream on the localhost.
    let upstream = TcpStream::connect(("127.0.0.1", local_port))
        .await
        .with_context(|| format!("connect to upstream 127.0.0.1:{}", local_port))?;

    let (mut sender, conn) = http1_client::handshake(TokioIo::new(upstream))
        .await
        .context("upstream handshake")?;
    // Drive the connection I/O in its own task.
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            warn!(error = %e, "upstream connection error");
        }
    });

    // Rewrite the request before forwarding.
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    inject_identity(&mut parts.headers, user_id.as_deref());
    // "forwarding" appearing means the request was fully received from the peer
    // over its QUIC stream and is now going to the local app.
    let uri_for_log = parts.uri.clone();
    info!(method = %parts.method, uri = %parts.uri, "forwarding to upstream");
    let forwarded = hyper::Request::from_parts(parts, body);

    let upstream_resp = sender
        .send_request(forwarded)
        .await
        .context("upstream send_request")?;
    info!(status = %upstream_resp.status(), uri = %uri_for_log, "upstream responded");

    // Rewrite the response on the way back.
    let (mut parts, body) = upstream_resp.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    let body: BoxedBody = body
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        .boxed();
    Ok(hyper::Response::from_parts(parts, body))
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
/// handled by hyper itself, so we leave it alone.
//
// Note: To be fully compliant, this should also process the `Connection`
// header's value to remove the headers it names. De facto, `keep-alive` and
// `close` are the only headers getting set.
fn strip_hop_by_hop(headers: &mut hyper::HeaderMap) {
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "upgrade",
    ] {
        headers.remove(name);
    }
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
