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
    /// Join a Wispers Access share.
    Join {
        /// Invite code for the share (`wax_…`), produced by `waserver invite`.
        invite_code: String,
    },
    Serve {
        port: u16,
    },
    /// Show all joined shares and their state.
    List,
    /// Remove a share from this device, deregistering from its hub when possible.
    Remove {
        /// The share's hostname, as shown by `waclient list`.
        share: String,
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
        Command::List => list().await,
        Command::Remove { share } => remove(&share).await,
    }
}

async fn join(invite_code: &str) -> Result<()> {
    // Parse the invite code.
    let (registration_token, activation_code, backend) = parse_wax_code(invite_code)?;

    // Create a new DB row.
    let db = storage::DB::new()?;
    let row = db.new_row()?;

    // Register the Wispers node. If the invite named a self-hosted backend,
    // use override_hub_addr().
    let ns = wc::NodeStorage::new(row.clone());
    if let Some(backend) = backend.as_deref() {
        println!("Using Wispers Connect backend: {}", backend);
        ns.override_hub_addr(backend);
    }
    let mut node = ns.restore_or_init_node().await?;
    println!("Registering Wispers node...");
    node.register(registration_token).await?;

    // From here on the hub holds a registration that consumes quota, so
    // a failed join must log the node out again (revoke + deregister) rather
    // than orphan the registration. This is best-effort.
    if let Err(e) = finish_join(&mut node, &row, activation_code, backend.as_deref()).await {
        match node.logout().await {
            Ok(()) => eprintln!("Join failed; deregistered from the hub again."),
            Err(le) => eprintln!("Join failed; could not deregister from the hub either ({le})."),
        }
        let _ = row.delete_row();
        return Err(e);
    }
    Ok(())
}

/// The steps of `join` after registration: activation and local bookkeeping.
/// Any failure here makes `join` roll the registration back.
async fn finish_join(
    node: &mut wc::Node,
    row: &storage::Row,
    activation_code: &str,
    backend: Option<&str>,
) -> Result<()> {
    println!("Activating Wispers node...");
    node.activate(activation_code).await?;

    // Determine display & host names, deduping the host name if necessary.
    let group_info = node.group_info().await?;
    let cg_id = group_info.id.to_string();
    row.write_connectivity_group_id(&cg_id)?;
    row.write_backend(backend)?;
    let display_name = group_info.name.unwrap_or_else(|| cg_id.clone());
    row.write_display_name(&display_name)?;
    let hostname = host_slug(&display_name).unwrap_or_else(|| cg_id.clone());
    let hostname = row.write_deduped_hostname(&hostname, &cg_id)?;

    // Mark the row complete, so it doesn't get cleaned up at next start.
    row.mark_complete()?;

    println!(
        "Joined share: {}\n  Hostname: {}\n  Connectivity group: {}\n  Node: {}\n",
        display_name,
        hostname,
        node.connectivity_group_id().unwrap(),
        node.node_number().unwrap(),
    );
    Ok(())
}

async fn list() -> Result<()> {
    use std::io::Write;
    use tabwriter::TabWriter;

    let db = storage::DB::new()?;
    let rows = db.get_all_rows()?;
    if rows.is_empty() {
        println!("No shares joined. Use 'waclient join <invite_code>'.");
        return Ok(());
    }
    let mut tw = TabWriter::new(std::io::stdout().lock()).padding(2);
    writeln!(&mut tw, "Hostname\tName\tStatus")?;
    for row in rows {
        let (_, display_name, hostname) = row.read_names()?;
        let state = match row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            Some(state) => state.describe(),
            None => "ok",
        };
        writeln!(&mut tw, "{}\t{}\t{}", hostname, display_name, state)?;
    }
    tw.flush()?;
    Ok(())
}

async fn remove(share: &str) -> Result<()> {
    let db = storage::DB::new()?;
    let row = db
        .find_row(share)?
        .with_context(|| format!("no share '{}' (see 'waclient list')", share))?;

    // Deregistering is best-effort: for a removed share the hub already rejects
    // us, and for a revoked one logout cleanly retires the zombie registration.
    let backend = row.read_backend()?;
    let ns = wc::NodeStorage::new(row.clone());
    if let Some(backend) = backend.as_deref() {
        ns.override_hub_addr(backend);
    }
    match ns.restore_or_init_node().await {
        Ok(mut node) => match node.logout().await {
            Ok(()) => println!("Deregistered from the hub."),
            Err(e) => println!("Could not deregister from the hub ({e}); removing locally anyway."),
        },
        Err(e) => println!("Could not restore the node ({e}); removing locally anyway."),
    }
    row.delete_row()?;
    println!("Share '{}' removed from this device.", share);
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
    // Start a stream factory for all known shares. One dead or unreachable
    // share must not take the others down: it's reported and skipped, and a
    // terminal rejection is persisted so the share is never dialed again.
    let db = storage::DB::new()?;
    let rows = db.get_all_rows()?;
    let mut nodes = Vec::new();
    let mut hostname_map: HashMap<String, String> = HashMap::new();
    let mut dead: HashMap<String, TerminalState> = HashMap::new();
    println!("Available shares:");
    for row in rows {
        let (cg_id, display_name, hostname) = row.read_names()?;
        hostname_map.insert(hostname.clone(), cg_id.clone());
        if let Some(state) = row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            report_dead_share(&display_name, &hostname, state);
            dead.insert(cg_id, state);
            continue;
        }
        let backend = row.read_backend()?;
        let ns = wc::NodeStorage::new(row.clone());
        if let Some(backend) = backend.as_deref() {
            ns.override_hub_addr(backend);
        }
        match ns.restore_or_init_node().await {
            Ok(node) if matches!(node.state(), wc::NodeState::Revoked) => {
                let state = TerminalState::Revoked;
                let _ = row.write_terminal_state(state.as_str());
                report_dead_share(&display_name, &hostname, state);
                dead.insert(cg_id, state);
            }
            Ok(node) => {
                println!("  http://{}.localhost:{}", hostname, port);
                nodes.push((cg_id, ShareNode { node, row }));
            }
            Err(e) => {
                if let Some(state) = terminal_from_node_err(&e) {
                    let _ = row.write_terminal_state(state.as_str());
                    report_dead_share(&display_name, &hostname, state);
                    dead.insert(cg_id, state);
                } else {
                    eprintln!(
                        "  {} — temporarily unavailable ({e}), not serving it this run",
                        hostname
                    );
                }
            }
        }
    }
    let stream_factory = Arc::new(StreamFactory::new(nodes, hostname_map, dead));

    // Bind to local port.
    let bind_addr = format!("localhost:{}", port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {}", bind_addr))?;
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
        Err(OpenError::Terminal(state)) => {
            // Terminal is deliberate, not an outage: 410 tells the reader that
            // retrying won't help, unlike the 502 below.
            return Ok(error_response(
                StatusCode::GONE,
                &format!(
                    "This app is no longer available on this device — {}.",
                    state.describe()
                ),
            ));
        }
        Err(OpenError::Other(e)) => {
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

/// Why a share is permanently unusable. `Removed` = the hub rejected our
/// credentials outright (share deleted server-side); `Revoked` = this device
/// was revoked from the share's roster.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Removed,
    Revoked,
}

impl TerminalState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Revoked => "revoked",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "removed" => Some(Self::Removed),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Removed => "the share was removed on the server side",
            Self::Revoked => "this device's access was revoked",
        }
    }
}

fn terminal_from_node_err(e: &wc::NodeStateError) -> Option<TerminalState> {
    if e.is_unauthenticated() || e.is_not_found() {
        return Some(TerminalState::Removed);
    }
    if e.is_revoked() {
        return Some(TerminalState::Revoked);
    }
    None
}

fn terminal_from_p2p_err(e: &wc::P2pError) -> Option<TerminalState> {
    match e {
        wc::P2pError::Revoked => Some(TerminalState::Revoked),
        wc::P2pError::Hub(h) if h.is_unauthenticated() || h.is_not_found() => {
            Some(TerminalState::Removed)
        }
        _ => None,
    }
}

fn report_dead_share(display_name: &str, hostname: &str, state: TerminalState) {
    eprintln!(
        "  {} ('{}') is no longer available — {}.",
        hostname,
        display_name,
        state.describe()
    );
    eprintln!("    Run 'waclient remove {}' to clean it up.", hostname);
}

enum OpenError {
    Terminal(TerminalState),
    Other(anyhow::Error),
}

struct ShareNode {
    node: wc::Node,
    row: storage::Row,
}

struct StreamFactory {
    nodes: HashMap<String /* connectivity_group_id */, ShareNode>,
    hostname_map: HashMap<String /* hostname */, String /* connectivity_group_id */>,
    // Shares the hub has terminally rejected (at startup or mid-session): they
    // answer 410 instead of being dialed.
    dead: Mutex<HashMap<String /* connectivity_group_id */, TerminalState>>,
    pool: Mutex<HashMap<String, PoolEntry>>,
}

impl StreamFactory {
    fn new(
        nodes: Vec<(String, ShareNode)>,
        hostname_map: HashMap<String, String>,
        dead: HashMap<String, TerminalState>,
    ) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
            hostname_map,
            dead: Mutex::new(dead),
            pool: Mutex::new(HashMap::new()),
        }
    }

    async fn open_stream(&self, host: &str) -> Result<wc::QuicStream, OpenError> {
        let mut cg_id = host;
        if let Some(mapped) = self.hostname_map.get(host) {
            cg_id = mapped;
        }
        if let Some(state) = self.dead.lock().await.get(cg_id) {
            return Err(OpenError::Terminal(*state));
        }
        let Some(share) = self.nodes.get(cg_id) else {
            return Err(OpenError::Other(anyhow::anyhow!("Unknown host {}", host)));
        };
        // Open a stream with a single retry. This covers the case when the
        // connection has died and needed reestablishing. A terminal rejection
        // is not retried — it can only repeat.
        match self.try_open_stream(cg_id, share).await {
            Ok(s) => Ok(s),
            Err(OpenError::Other(e)) => {
                eprintln!(
                    "[{}] open_stream attempt 1 failed, retrying once: {:#}",
                    cg_id, e
                );
                self.try_open_stream(cg_id, share).await
            }
            Err(terminal) => Err(terminal),
        }
    }

    async fn try_open_stream(
        &self,
        cg_id: &str,
        share: &ShareNode,
    ) -> Result<wc::QuicStream, OpenError> {
        // Get the cell under lock.
        let cell = {
            let mut pool = self.pool.lock().await;
            let pool_entry = pool.entry(cg_id.to_owned()).or_insert_with(|| PoolEntry {
                cell: Arc::new(OnceCell::new()),
            });
            pool_entry.cell.clone()
        };
        // Get or establish the connection.
        let conn = match cell
            .get_or_try_init(|| async {
                eprintln!("[{}] establishing QUIC connection", cg_id);
                share.node.connect_quic(1).await.map(Arc::new)
            })
            .await
        {
            Ok(conn) => conn.clone(),
            Err(e) => {
                // A mid-session revocation/removal is forever: persist it and
                // remember it so later requests 410 without dialing.
                if let Some(state) = terminal_from_p2p_err(&e) {
                    eprintln!(
                        "[{}] share is no longer available — {}",
                        cg_id,
                        state.describe()
                    );
                    let _ = share.row.write_terminal_state(state.as_str());
                    self.dead.lock().await.insert(cg_id.to_owned(), state);
                    return Err(OpenError::Terminal(state));
                }
                return Err(OpenError::Other(e.into()));
            }
        };
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
                Err(OpenError::Other(e.into()))
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
