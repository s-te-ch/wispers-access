mod storage;

use anyhow::{Context, Result};
use bytes::Bytes;
use clap::{Parser, Subcommand};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, OnceCell};
use wispers_connect as wc;

/// Body type used in responses we send back to the peer. Boxed so we can return
/// either the upstream's streamed body or a locally-generated error body.
type BoxedBody = BoxBody<Bytes, std::io::Error>;

#[derive(Parser)]
#[command(name = "waclient", version)]
#[command(about = "Wispers Access client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Join a Wispers Accept share.
    Join {
        /// Invite code for the share (`wax_…`), produced by `waserver invite`.
        invite_code: String,
    },
    Serve {
        port: u16,
    },
}

fn main() -> Result<()> {
    // Restrict default file mode to user-only. Safe to do as the first thing.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }
    // De-conflict rustls.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Join { invite_code } => join(&invite_code).await,
        Command::Serve { port } => serve(port).await,
    }
}

async fn join(invite_code: &str) -> Result<()> {
    // Parse the invite code.
    let (registration_token, activation_code, backend) = parse_wax_code(invite_code)?;

    // Create a new DB row.
    let db = storage::DB::new()?;
    let row = db.new_row()?;

    // Register & activate the Wispers node. If the invite named a self-hosted
    // backend, use override_hub_addr().
    let ns = wc::NodeStorage::new(row.clone());
    if let Some(backend) = backend.as_deref() {
        println!("Using Wispers Connect backend: {}", backend);
        ns.override_hub_addr(backend);
    }
    let mut node = ns.restore_or_init_node().await?;
    println!("Registering Wispers node...");
    node.register(registration_token).await?;
    println!("Activating Wispers node...");
    node.activate(activation_code).await?;

    // Determine display & host names, deduping the host name if necessary.
    let group_info = node.group_info().await?;
    let cg_id = group_info.id.to_string();
    row.write_connectivity_group_id(&cg_id)?;
    row.write_backend(backend.as_deref())?;
    let display_name = group_info.name.unwrap_or_else(|| cg_id.clone());
    row.write_display_name(&display_name)?;
    let hostname = host_slug(&display_name).unwrap_or_else(|| cg_id.clone());
    let hostname = row.write_deduped_hostname(&hostname, &cg_id)?;

    // Mark the row complete, so it doesn't get cleaned up at next start.
    row.mark_complete()?;
    // TODO: There's a problem here: if activation works but something fails
    // afterwards, we should revoke the node and not just kill the registration.

    println!(
        "Joined share: {}\n  Hostname: {}\n  Connectivity group: {}\n  Node: {}\n",
        display_name,
        hostname,
        node.connectivity_group_id().unwrap(),
        node.node_number().unwrap(),
    );
    Ok(())
}

/// Parses a `wax_<token>_<activation_code>[_<backend>]` invite code into its
/// (registration_token, activation_code, backend) parts. The optional backend
/// part is a base32-encoded URL.
fn parse_wax_code(code: &str) -> Result<(&str, &str, Option<String>)> {
    let rest = code
        .trim()
        .strip_prefix("wax_")
        .context("invalid invite code (expected wax_<token>_<code>)")?;
    let mut parts = rest.splitn(3, '_');
    let token = parts.next().unwrap_or("");
    let activation = parts.next().unwrap_or("");
    if token.is_empty() || activation.is_empty() {
        anyhow::bail!("invalid invite code (expected wax_<token>_<code>)");
    }
    let backend = parts.next().map(decode_backend).transpose()?;
    Ok((token, activation, backend))
}

/// Decodes the invite's base32 backend field back to its URL, erroring when
/// the field is present but unusable. Only an `https://` URL is accepted. A
/// plaintext or bogus hub is refused outright.
fn decode_backend(encoded: &str) -> Result<String> {
    let bytes = data_encoding::BASE32_NOPAD
        .decode(encoded.to_uppercase().as_bytes())
        .context("invite's backend field is not valid base32")?;
    let url = String::from_utf8(bytes).context("invite's backend URL is not valid UTF-8")?;
    if !url.starts_with("https://") {
        anyhow::bail!("invite's backend URL must be https:// (got {url:?})");
    }
    Ok(url)
}

/// Free-form name -> DNS-label-safe slug, or None if nothing usable remains.
fn host_slug(name: &str) -> Option<String> {
    // Remove apostrophes, so "Bob's app" becomes "bobs-app", not "bob-s-app".
    let cleaned = name.replace(['\'', '’'], "");
    // Slugify, but keep it to 63 chars, to produce a legal DNS label.
    let mut s = slug::slugify(cleaned);
    s.truncate(63);
    // Truncation can leave a trailing '-'.
    let s = s.trim_end_matches('-');
    (!s.is_empty()).then(|| s.to_string())
}

async fn serve(port: u16) -> Result<()> {
    // Start a stream factory for all known shares.
    let db = storage::DB::new()?;
    let rows = db.get_all_rows()?;
    let mut nodes = Vec::new();
    let mut hostname_map: HashMap<String, String> = HashMap::new();
    println!("Available shares:");
    for row in rows {
        let (cg_id, _, hostname) = row.read_names()?;
        let backend = row.read_backend()?;
        println!("  http://{}.localhost:{}", hostname, port);
        hostname_map.insert(hostname.clone(), cg_id.clone());
        let ns = wc::NodeStorage::new(row);
        if let Some(backend) = backend.as_deref() {
            ns.override_hub_addr(backend);
        }
        let node = ns.restore_or_init_node().await?;
        nodes.push((cg_id, node));
    }
    let stream_factory = Arc::new(StreamFactory::new(nodes, hostname_map));

    // Bind to local port.
    let bind_addr = format!("localhost:{}", port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {}", &bind_addr))?;
    println!("Listening on {}", bind_addr);

    // Serve.
    // TODO: we also need to handle revocation.
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let stream_factory = stream_factory.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(tcp_stream, stream_factory).await {
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

async fn handle_connection(
    tcp_stream: TcpStream,
    stream_factory: Arc<StreamFactory>,
) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| forward(req, stream_factory.clone()));
    http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        // Allow a 101 to hand the browser socket over for raw relaying (WebSocket).
        .with_upgrades()
        .await
        .context("HTTP/1 connection error")
}

async fn forward(
    mut req: hyper::Request<Incoming>,
    stream_factory: Arc<StreamFactory>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    // Determine upstream server...
    let Ok(host) = extract_host(&req) else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "missing host header",
        ));
    };
    let Ok(share) = extract_share(&host) else {
        return Ok(error_response(StatusCode::NOT_FOUND, "unknown host"));
    };

    // ... and open a stream to it.
    let fwd_stream = match stream_factory.open_stream(&share).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[{}] open_stream failed: {:#}", share, e);
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
            eprintln!("[{}] client handshake failed: {:#}", share, e);
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
            eprintln!("[{}] send_request failed: {:#}", share, e);
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };

    // Successful upgrade: hand both raw byte streams to a relay task and return
    // the 101 to the browser with its handshake headers intact.
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        match peer_upgrade {
            Some(peer_upgrade) => {
                let upstream_upgrade = hyper::upgrade::on(&mut resp);
                tokio::spawn(splice_upgrade(peer_upgrade, upstream_upgrade));
            }
            None => eprintln!("[{}] server returned 101 without an upgrade request", share),
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

fn extract_share(host: &str) -> Result<String> {
    if let Some((share, rest)) = host.split_once('.')
        && rest.starts_with("localhost")
    {
        Ok(share.to_owned())
    } else if host.starts_with("localhost") {
        Ok("".to_string())
    } else {
        anyhow::bail!("unknown host");
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

struct StreamFactory {
    nodes: HashMap<String /* connectivity_group_id */, wc::Node>,
    hostname_map: HashMap<String /* hostname */, String /* connectivity_group_id */>,
    pool: Mutex<HashMap<String, PoolEntry>>,
}

impl StreamFactory {
    fn new(nodes: Vec<(String, wc::Node)>, hostname_map: HashMap<String, String>) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
            hostname_map,
            pool: Mutex::new(HashMap::new()),
        }
    }

    async fn open_stream(&self, host: &str) -> Result<wc::QuicStream> {
        let mut cg_id = host;
        if let Some(mapped) = self.hostname_map.get(host) {
            cg_id = mapped;
        }
        let Some(node) = self.nodes.get(cg_id) else {
            anyhow::bail!("Unknown host {}", host);
        };
        // Open a stream with a single retry. This covers the case when the
        // connection has died and needed reestablishing.
        match self.try_open_stream(cg_id, node).await {
            Ok(s) => Ok(s),
            Err(e) => {
                eprintln!(
                    "[{}] open_stream attempt 1 failed, retrying once: {:#}",
                    cg_id, e
                );
                self.try_open_stream(cg_id, node).await
            }
        }
    }

    async fn try_open_stream(&self, cg_id: &str, node: &wc::Node) -> Result<wc::QuicStream> {
        // Get the cell under lock.
        let cell = {
            let mut pool = self.pool.lock().await;
            let pool_entry = pool.entry(cg_id.to_owned()).or_insert_with(|| PoolEntry {
                cell: Arc::new(OnceCell::new()),
            });
            pool_entry.cell.clone()
        };
        // Get or establish the connection.
        let conn = cell
            .get_or_try_init(|| async {
                eprintln!("[{}] establishing QUIC connection", cg_id);
                node.connect_quic(1).await.map(Arc::new)
            })
            .await?
            .clone();
        // Open a stream. If this fails, the underlying connection has broken
        // and we should remove it from the pool. There could be several threads
        // trying this, so make sure the cell hasn't changed.
        match conn.open_stream().await {
            Ok(stream) => Ok(stream),
            Err(e) => {
                eprintln!(
                    "[{}] conn.open_stream failed, evicting connection: {:#}",
                    cg_id, e
                );
                let mut pool = self.pool.lock().await;
                if let Some(entry) = pool.get(cg_id)
                    && Arc::ptr_eq(&entry.cell, &cell)
                {
                    pool.remove(cg_id);
                }
                Err(e.into())
            }
        }
    }
}

struct PoolEntry {
    cell: Arc<OnceCell<Arc<wc::QuicConnection>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wax_code() {
        assert_eq!(
            parse_wax_code("wax_ab12cd_1-xyz789").unwrap(),
            ("ab12cd", "1-xyz789", None)
        );
    }

    #[test]
    fn tolerates_pasted_whitespace() {
        assert_eq!(
            parse_wax_code("  wax_ab12cd_1-xyz789\n").unwrap(),
            ("ab12cd", "1-xyz789", None)
        );
    }

    #[test]
    fn parses_wax_code_with_backend() {
        let url = "https://myhub.example.com";
        let enc = data_encoding::BASE32_NOPAD
            .encode(url.as_bytes())
            .to_lowercase();
        assert_eq!(
            parse_wax_code(&format!("wax_ab12cd_1-xyz789_{}", enc)).unwrap(),
            ("ab12cd", "1-xyz789", Some(url.to_owned()))
        );
    }

    #[test]
    fn rejects_malformed_codes() {
        assert!(parse_wax_code("ab12cd/1-xyz789").is_err()); // old test format
        assert!(parse_wax_code("wax_ab12cd").is_err()); // missing activation code
        assert!(parse_wax_code("wax__1-xyz789").is_err()); // empty token
        assert!(parse_wax_code("wax_ab12cd_").is_err()); // empty activation code
        assert!(parse_wax_code("").is_err());
    }

    #[test]
    fn rejects_code_with_bad_backend_field() {
        // A present-but-undecodable / non-https backend fails the whole code
        // shut, rather than falling back to the managed hub — and says why.
        let http = data_encoding::BASE32_NOPAD
            .encode(b"http://evil.example.com")
            .to_lowercase();
        let err = parse_wax_code(&format!("wax_ab12cd_1-xyz789_{}", http)).unwrap_err();
        assert!(err.to_string().contains("https"), "got: {err}");
        assert!(parse_wax_code("wax_ab12cd_1-xyz789_!!notbase32").is_err());
    }
}
