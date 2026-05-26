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
use std::fs;
use std::path::PathBuf;
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
        /// Share name.
        share: String,
        /// Invite code for the share, produced by waserver.
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
        Command::Join { share, invite_code } => join(&share, &invite_code).await,
        Command::Serve { port } => serve(port.clone()).await,
    }
}

async fn join(share: &str, invite_code: &str) -> Result<()> {
    let Some((registration_token, activation_code)) = invite_code.split_once('/') else {
        anyhow::bail!("invalid invite code");
    };
    let storage = get_node_storage(share)?;
    if let Some(_) = storage.read_registration()? {
        anyhow::bail!("found existing registration for share {}", share);
    }
    let mut node = storage.restore_or_init_node().await?;
    println!("Registering Wispers node...");
    node.register(registration_token).await?;
    println!("Activating Wispers node...");
    node.activate(activation_code).await?;
    println!(
        "Joined share {}\n  Connectivity group ID: {}\n  Node number: {}\n",
        share,
        node.connectivity_group_id().unwrap().to_string(),
        node.node_number().unwrap(),
    );

    Ok(())
}

async fn serve(port: u16) -> Result<()> {
    // Start a stream factory for all known shares.
    let shares = list_shares()?;
    let mut nodes = Vec::new();
    for share in &shares {
        let node = load_node(share).await?;
        nodes.push((share.clone(), node));
    }
    let stream_factory = Arc::new(StreamFactory::new(nodes));

    // Bind to local port.
    let bind_addr = format!("localhost:{}", port);
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind to {}", &bind_addr))?;
    println!("Listening on {}", bind_addr);

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

async fn load_node(share: &str) -> Result<wc::Node> {
    let ns = get_node_storage(share)?;
    if !ns.read_registration()?.is_some() {
        anyhow::bail!("Share {} doesn't have a registered node", share);
    }
    let node = ns.restore_or_init_node().await?;
    Ok(node)
}

async fn handle_connection(
    tcp_stream: TcpStream,
    stream_factory: Arc<StreamFactory>,
) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| forward(req, stream_factory.clone()));
    http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        .await
        .context("HTTP/1 connection error")
}

async fn forward(
    req: hyper::Request<Incoming>,
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
    let Ok(fwd_stream) = stream_factory.open_stream(&share).await else {
        return Ok(error_response(
            StatusCode::BAD_GATEWAY,
            "Wispers Access server unavailable",
        ));
    };
    let fwd_io = quic_stream_io::wrap(fwd_stream);
    let handshake = http1_client::handshake(fwd_io).await;
    let Ok((mut sender, conn)) = handshake else {
        return Ok(error_response(
            StatusCode::BAD_GATEWAY,
            "Wispers Access server unavailable",
        ));
    };
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("upstream connection error: {:#}", e);
        }
    });

    // Rewrite the query.
    let (mut parts, body) = req.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers);
    let rewritten = hyper::Request::from_parts(parts, body);

    // Forward to upstream and get response.
    let resp = sender
        .send_request(rewritten)
        .await
        .context("upstream send_request");
    let Ok(resp) = resp else {
        return Ok(error_response(
            StatusCode::BAD_GATEWAY,
            "Wispers Access server unavailable",
        ));
    };

    // Rewrite the response on the way back.
    let (mut parts, body) = resp.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers);
    let body: BoxedBody = body
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        .boxed();
    Ok(hyper::Response::from_parts(parts, body))
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
/// handled by hyper itself, so we leave it alone.
//
// Note: To be fully compliant, this should also process the `Connection`
// header's value to remove the headers it names. De facto, `keep-alive` and
// `close` are the only headers getting set.
fn strip_hop_by_hop_headers(headers: &mut hyper::HeaderMap) {
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

fn get_node_storage(share: &str) -> Result<wc::NodeStorage> {
    let dir = get_storage_dir()?;
    let dir = dir.join(share);
    let store = wc::FileNodeStateStore::new(dir);
    let storage = wc::NodeStorage::new(store);
    Ok(storage)
}

fn list_shares() -> Result<Vec<String>> {
    let dir = get_storage_dir()?;
    let shares: Vec<String> = if !dir.exists() {
        Vec::new()
    } else {
        fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    };
    Ok(shares)
}

fn get_storage_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("waclient"))
}

struct StreamFactory {
    nodes: HashMap<String, wc::Node>,
    pool: Mutex<HashMap<String, PoolEntry>>,
}

impl StreamFactory {
    fn new(nodes: Vec<(String, wc::Node)>) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
            pool: Mutex::new(HashMap::new()),
        }
    }

    async fn open_stream(&self, share: &str) -> Result<wc::QuicStream> {
        let Some(node) = self.nodes.get(share) else {
            anyhow::bail!("Unknown share {}", share);
        };
        // Open a stream with a single retry. This covers the case when the
        // connection has died and needed reestablishing.
        match self.try_open_stream(share, &node).await {
            Ok(s) => Ok(s),
            Err(_) => self.try_open_stream(share, &node).await,
        }
    }

    async fn try_open_stream(&self, share: &str, node: &wc::Node) -> Result<wc::QuicStream> {
        // Get the cell under lock.
        let cell = {
            let mut pool = self.pool.lock().await;
            let pool_entry = pool.entry(share.to_owned()).or_insert_with(|| PoolEntry {
                cell: Arc::new(OnceCell::new()),
            });
            pool_entry.cell.clone()
        };
        // Get or establish the connection.
        let conn = cell
            .get_or_try_init(|| async { node.connect_quic(1).await.map(Arc::new) })
            .await?
            .clone();
        // Open a stream. If this fails, the underlying connection has broken
        // and we should remove it from the pool. There could be several threads
        // trying this, so make sure the cell hasn't changed.
        match conn.open_stream().await {
            Ok(stream) => Ok(stream),
            Err(e) => {
                let mut pool = self.pool.lock().await;
                if let Some(entry) = pool.get(share)
                    && Arc::ptr_eq(&entry.cell, &cell)
                {
                    pool.remove(share);
                }
                Err(e.into())
            }
        }
    }
}

struct PoolEntry {
    cell: Arc<OnceCell<Arc<wc::QuicConnection>>>,
}
