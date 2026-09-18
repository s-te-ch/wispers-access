//! The server side of the wire protocol (see the `wispers-access-wire` crate).
//! DATA streams go to the proxy for the share they name, CTRL streams to the
//! guest API, and raw HTTP from clients that predate the framing to the default
//! share.

use crate::config::CircleConfig;
use crate::guest_api;
use crate::http::{self, Target};
use anyhow::{Context, Result};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::broadcast;
use tracing::warn;
use wire::{FirstByte, HttpPreamble, StreamType};
use wispers_access_wire as wire;

/// What one stream is served against: the config as of when it was opened
/// (a reload applies from the next stream on) and the channel that carries
/// a new config hash on every reload, for guests holding an events stream.
#[derive(Clone)]
pub struct StreamContext {
    pub config: Arc<CircleConfig>,
    pub events: broadcast::Sender<u64>,
}

/// Serves one stream a guest opened.
pub async fn handle<S>(mut stream: S, ctx: StreamContext, user_id: Option<String>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut first = [0u8; 1];
    match stream.read_exact(&mut first).await {
        Ok(_) => {}
        // A stream opened and closed without a byte carries no request.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
        Err(e) => return Err(e).context("reading stream type"),
    }
    match FirstByte::from(first[0]) {
        FirstByte::LegacyHttp => {
            let target = match ctx.config.default_share() {
                Some(share) => Target::Upstream(share.upstream.as_str().into()),
                None => Target::NoShares,
            };
            // The byte was the start of the request; hand it back.
            http::serve(Prefixed::new(first.to_vec(), stream), target, user_id).await
        }
        FirstByte::Typed(StreamType::Data) => {
            let preamble: HttpPreamble = wire::read_message(&mut stream)
                .await
                .context("reading DATA preamble")?;
            let target = share_target(&ctx.config, &preamble.share_id);
            http::serve(stream, target, user_id).await
        }
        FirstByte::Typed(StreamType::Ctrl) => {
            guest_api::serve(stream, ctx.config, ctx.events, user_id).await
        }
        FirstByte::Unknown(byte) => {
            warn!(byte, "unknown stream type; closing");
            stream.shutdown().await.ok();
            Ok(())
        }
    }
}

fn share_target(config: &CircleConfig, share_id: &str) -> Target {
    match config.shares.iter().find(|s| s.id == share_id) {
        Some(share) => Target::Upstream(share.upstream.as_str().into()),
        None if config.shares.is_empty() => Target::NoShares,
        None => {
            warn!(
                share = share_id,
                "request for a share this circle does not have"
            );
            Target::UnknownShare {
                config_hash: config.config_hash(),
            }
        }
    }
}

/// A stream with bytes already read from it put back in front.
struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            buf.put_slice(&self.prefix[self.pos..self.pos + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ShareKind;
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use wire::{CircleInfo, ConfigHash};

    const CONFIG: &str = r#"
name = "Family"
[[share]]
id = "jf"
name = "Jellyfin"
kind = "jellyfin"
upstream = "UPSTREAM"
[[share]]
id = "photos"
upstream = ":1"
"#;

    fn context(config: &str) -> StreamContext {
        StreamContext {
            config: Arc::new(CircleConfig::parse(config).unwrap()),
            events: broadcast::channel(4).0,
        }
    }

    fn context_with_upstream(upstream: &str) -> StreamContext {
        context(&CONFIG.replace("UPSTREAM", upstream))
    }

    /// An upstream that answers every request with the identity header it
    /// received, so a test can see both the routing and the injection.
    async fn echo_upstream() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(|req: hyper::Request<_>| async move {
                        let who = req
                            .headers()
                            .get(http::IDENTITY_HEADER)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-")
                            .to_owned();
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(who))))
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(tcp), service)
                        .await;
                });
            }
        });
        format!("127.0.0.1:{}", addr.port())
    }

    /// Runs the dispatcher on one end of a pipe, writes `request` into the
    /// other, half-closes it and returns everything the dispatcher wrote back.
    async fn exchange(ctx: StreamContext, user_id: Option<&str>, request: Vec<u8>) -> Vec<u8> {
        let (server, mut client) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(handle(server, ctx, user_id.map(str::to_owned)));
        client.write_all(&request).await.unwrap();
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server_task.await.unwrap().unwrap();
        response
    }

    fn data_stream(share_id: &str, request: &[u8]) -> Vec<u8> {
        let body = serde_json::to_vec(&HttpPreamble {
            share_id: share_id.to_owned(),
        })
        .unwrap();
        let mut out = vec![StreamType::Data as u8];
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(request);
        out
    }

    fn ctrl_stream(request: &str) -> Vec<u8> {
        let mut out = vec![StreamType::Ctrl as u8];
        out.extend_from_slice(request.as_bytes());
        out
    }

    /// Splits an HTTP/1.1 response into its lowercased head and its body.
    fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
        let text = String::from_utf8_lossy(raw);
        let end = text.find("\r\n\r\n").expect("response has headers");
        (text[..end].to_lowercase(), raw[end + 4..].to_vec())
    }

    const GET: &[u8] = b"GET /x HTTP/1.1\r\nHost: a\r\n\r\n";

    #[tokio::test]
    async fn legacy_request_goes_to_the_default_share_with_identity() {
        let upstream = echo_upstream().await;
        let response = exchange(
            context_with_upstream(&upstream),
            Some("alice"),
            GET.to_vec(),
        )
        .await;
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        assert!(text.ends_with("alice"), "{text}");
    }

    #[tokio::test]
    async fn data_stream_is_routed_by_share_id() {
        let upstream = echo_upstream().await;
        let response = exchange(
            context_with_upstream(&upstream),
            None,
            data_stream("jf", GET),
        )
        .await;
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        // No identity resolved: the upstream saw no header.
        assert!(text.ends_with('-'), "{text}");
    }

    #[tokio::test]
    async fn unknown_share_gets_a_typed_404_with_the_config_hash() {
        let ctx = context_with_upstream("127.0.0.1:1");
        let hash = ctx.config.config_hash();
        let response = exchange(ctx, None, data_stream("gone", GET)).await;
        let (head, _) = split_response(&response);
        assert!(head.starts_with("http/1.1 404"), "{head}");
        assert!(
            head.contains(&format!("{}: share-not-found", http::ERROR_HEADER)),
            "{head}"
        );
        assert!(
            head.contains(&format!("{}: {:016x}", http::CONFIG_HASH_HEADER, hash)),
            "{head}"
        );
    }

    #[tokio::test]
    async fn no_shares_gets_a_503() {
        let response = exchange(context("name = \"x\"\n"), None, GET.to_vec()).await;
        let (head, _) = split_response(&response);
        assert!(head.starts_with("http/1.1 503"), "{head}");
        assert!(
            head.contains(&format!("{}: no-shares", http::ERROR_HEADER)),
            "{head}"
        );
    }

    #[tokio::test]
    async fn get_circle_returns_the_info_with_an_etag() {
        let ctx = context_with_upstream("127.0.0.1:1");
        let hash = ConfigHash(ctx.config.config_hash());
        let request = format!("GET {} HTTP/1.1\r\nHost: w\r\n\r\n", wire::CIRCLE_PATH);
        let response = exchange(ctx, None, ctrl_stream(&request)).await;
        let (head, body) = split_response(&response);
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(head.contains("content-type: application/json"), "{head}");
        assert!(head.contains(&format!("etag: {}", hash.etag())), "{head}");
        let info: CircleInfo = serde_json::from_slice(&body).unwrap();
        assert_eq!(info.config_hash, hash);
        assert_eq!(info.name, "Family");
        assert_eq!(info.transport, "wispers-connect");
        assert_eq!(info.shares.len(), 2);
        assert_eq!(info.shares[0].id, "jf");
        assert_eq!(info.shares[0].kind, ShareKind::Jellyfin);
        assert_eq!(info.shares[1].name, "photos");
        assert_eq!(info.shares[1].kind, ShareKind::Web);
        // Upstreams never reach a guest.
        assert!(!String::from_utf8_lossy(&body).contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn get_circle_is_conditional() {
        let ctx = context_with_upstream("127.0.0.1:1");
        let etag = ConfigHash(ctx.config.config_hash()).etag();
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: w\r\nIf-None-Match: {}\r\n\r\n",
            wire::CIRCLE_PATH,
            etag
        );
        let response = exchange(ctx, None, ctrl_stream(&request)).await;
        let (head, body) = split_response(&response);
        assert!(head.starts_with("http/1.1 304"), "{head}");
        assert!(head.contains(&format!("etag: {etag}")), "{head}");
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn unknown_routes_and_methods_are_rejected() {
        let ctx = context_with_upstream("127.0.0.1:1");
        let response = exchange(
            ctx.clone(),
            None,
            ctrl_stream("GET /v1/nope HTTP/1.1\r\nHost: w\r\n\r\n"),
        )
        .await;
        assert!(split_response(&response).0.starts_with("http/1.1 404"));
        let request = format!("DELETE {} HTTP/1.1\r\nHost: w\r\n\r\n", wire::CIRCLE_PATH);
        let response = exchange(ctx, None, ctrl_stream(&request)).await;
        assert!(split_response(&response).0.starts_with("http/1.1 405"));
    }

    #[tokio::test]
    async fn events_stream_delivers_config_changes() {
        let ctx = context_with_upstream("127.0.0.1:1");
        let events = ctx.events.clone();
        let (server, mut client) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(handle(server, ctx, None));
        let request = format!("GET {} HTTP/1.1\r\nHost: w\r\n\r\n", wire::EVENTS_PATH);
        client.write_all(&ctrl_stream(&request)).await.unwrap();

        // Headers and the opening comment arrive before any event.
        let opening = read_until(&mut client, ": connected\n\n").await;
        let (head, _) = split_response(opening.as_bytes());
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(head.contains("content-type: text/event-stream"), "{head}");

        events.send(0xabc).unwrap();
        let event = read_until(&mut client, "\n\n").await;
        assert!(event.contains("event: circle-changed\n"), "{event}");
        assert!(
            event.contains("data: {\"config_hash\":\"0000000000000abc\"}\n"),
            "{event}"
        );

        // The guest going away ends the stream on the next write, which is
        // an event here and a heartbeat in production. Not an error.
        drop(client);
        events.send(0xdef).unwrap();
        server_task.await.unwrap().unwrap();
    }

    async fn read_until(client: &mut tokio::io::DuplexStream, marker: &str) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let n = client.read(&mut chunk).await.unwrap();
                assert!(n > 0, "stream ended before {marker:?}");
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains(marker) {
                    break;
                }
            }
        })
        .await
        .expect("marker arrives in time");
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn unknown_type_and_empty_streams_are_closed_quietly() {
        let ctx = context_with_upstream("127.0.0.1:1");
        assert!(exchange(ctx.clone(), None, vec![0x7f]).await.is_empty());
        assert!(exchange(ctx, None, Vec::new()).await.is_empty());
    }

    #[tokio::test]
    async fn prefixed_reader_replays_then_delegates() {
        let (mut a, b) = tokio::io::duplex(64);
        a.write_all(b"world").await.unwrap();
        drop(a);
        let mut p = Prefixed::new(b"hello ".to_vec(), b);
        let mut out = String::new();
        p.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }
}
