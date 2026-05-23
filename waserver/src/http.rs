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

/// Body type used in responses we send back to the peer. Boxed so we can return
/// either the upstream's streamed body or a locally-generated error body.
type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// Serve HTTP/1 over a single QUIC stream, forwarding to the local port.
pub async fn handle_quic_stream(
    stream: wispers_connect::QuicStream,
    local_port: u16,
) -> Result<()> {
    let io = quic_stream_io::wrap(stream);
    let service = hyper::service::service_fn(move |req| forward(req, local_port));
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
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    match try_forward(req, local_port).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("forward error: {:#}", e);
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
            eprintln!("upstream connection error: {}", e);
        }
    });

    // Rewrite the request before forwarding.
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    // TODO: X-Forwarded-For / X-Forwarded-Proto based on the peer node identity.
    let forwarded = hyper::Request::from_parts(parts, body);

    let upstream_resp = sender
        .send_request(forwarded)
        .await
        .context("upstream send_request")?;

    // Rewrite the response on the way back.
    let (mut parts, body) = upstream_resp.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    let body: BoxedBody = body
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        .boxed();
    Ok(hyper::Response::from_parts(parts, body))
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
