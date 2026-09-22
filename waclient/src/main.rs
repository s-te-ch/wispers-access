mod iroh_transport;
mod shares;
mod storage;
mod transports;
mod wispers_connect_transport;

use anyhow::{Context, Result};
use bytes::Bytes;
use clap::{Parser, Subcommand};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use shares::{Lookup, Share, ShareRegistry};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use transports::{TerminalState, TransportError};
use wispers_access_wire as wire;

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
    /// Serve every joined share's apps on localhost, as
    /// `http://<app>.<share>.localhost:<port>`.
    Serve { port: u16 },
    /// Show all joined shares, their apps and their state.
    List,
    /// Remove a share from this device, deregistering from its hub when possible.
    Remove {
        /// The share's label, as shown by `waclient list`.
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
    let invite = wire::Invite::parse(invite_code)?;
    let db = storage::DB::new()?;
    let row = db.new_row()?;
    // The row is the share's; the transport writes its part of it and the
    // host node's answer fills the rest. A failed join leaves no row behind.
    let result = async {
        let info = transports::join(invite, &row).await?;
        record_join(&row, &info)
    }
    .await;
    if result.is_err() {
        let _ = row.delete_row();
    }
    result
}

/// The local bookkeeping of any `join`, once the host node has answered with
/// the share: names, the app list, and marking the row complete so it
/// survives the next start.
fn record_join(row: &storage::Row, info: &wire::ShareInfo) -> Result<()> {
    let share_id = row.share_id()?;
    row.write_share_info(info)?;
    let display_name = if info.name.is_empty() {
        share_id.to_string()
    } else {
        info.name.clone()
    };
    row.write_display_name(&display_name)?;
    let hostname = host_slug(&display_name).unwrap_or_else(|| share_id.to_string());
    let hostname = row.write_deduped_hostname(&hostname)?;
    row.mark_complete()?;

    println!(
        "Joined share: {}\n  Label: {}\n  Apps: {}\n  Share id: {}\n",
        display_name,
        hostname,
        describe_apps(&info.apps),
        share_id,
    );
    Ok(())
}

fn describe_apps(apps: &[wire::App]) -> String {
    if apps.is_empty() {
        return "none yet".to_owned();
    }
    apps.iter()
        .map(|s| {
            if s.name == s.id {
                s.id.clone()
            } else {
                format!("{} ({})", s.id, s.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
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
    writeln!(&mut tw, "Share\tName\tApps\tStatus")?;
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
        let apps = row
            .read_apps()?
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            &mut tw,
            "{}\t{}\t{}\t{}",
            hostname,
            display_name,
            if apps.is_empty() { "-" } else { &apps },
            state
        )?;
    }
    tw.flush()?;
    Ok(())
}

async fn remove(share: &str) -> Result<()> {
    let db = storage::DB::new()?;
    let row = db
        .find_row(share)?
        .with_context(|| format!("no share '{}' (see 'waclient list')", share))?;

    transports::leave(&row).await?;
    row.delete_row()?;
    println!("Share '{}' removed from this device.", share);
    Ok(())
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
    // Load every known share. One dead or unreachable share must not take
    // the others down: it's reported and skipped, and a terminal rejection
    // is persisted so the share is never dialed again.
    let db = storage::DB::new()?;
    let mut registry = ShareRegistry::default();
    println!("Available apps (as last seen; refreshed in the background):");
    for row in db.get_all_rows()? {
        let (share_id, display_name, label) = row.read_names()?;
        if let Some(state) = row
            .read_terminal_state()?
            .as_deref()
            .and_then(TerminalState::parse)
        {
            report_dead_share(&display_name, &label, state);
            registry.insert_dead(label, share_id, state);
            continue;
        }
        match transports::restore(row.clone()).await {
            Ok(transport) => {
                let share = Share::new(label, share_id, display_name, row, transport);
                println!(
                    "  {} ({}) via {}:",
                    share.display_name(),
                    share.label(),
                    share.describe_transport()
                );
                print_app_urls(share.label(), &share.apps()?, port);
                registry.insert(share);
            }
            Err(TransportError::Terminal(state)) => {
                let _ = row.write_terminal_state(state.as_str());
                report_dead_share(&display_name, &label, state);
                registry.insert_dead(label, share_id, state);
            }
            Err(TransportError::Transient(e)) => {
                eprintln!(
                    "  {} — temporarily unavailable ({e:#}), not serving it this run",
                    label
                );
            }
        }
    }
    let registry = Arc::new(registry);

    // Ask every live share's host node whether the app list changed since the
    // last run. Best effort and off the startup path: an unreachable host node
    // just leaves the stored list in place.
    for share in registry.iter() {
        let share = share.clone();
        tokio::spawn(async move {
            match share.refresh().await {
                Ok(Some(info)) => {
                    println!("Updated app list for {} ({}):", info.name, share.label());
                    print_app_urls(share.label(), &info.apps, port);
                }
                Ok(None) => {}
                Err(e) => eprintln!(
                    "[{}] could not refresh the app list: {:#}",
                    share.label(),
                    e
                ),
            }
        });
    }

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

fn print_app_urls(hostname: &str, apps: &[wire::App], port: u16) {
    if apps.is_empty() {
        println!("    (no apps yet)");
    }
    for app in apps {
        println!(
            "    {:<16} http://{}.{}.localhost:{}",
            app.name, app.id, hostname, port
        );
    }
}

fn report_dead_share(display_name: &str, label: &str, state: TerminalState) {
    eprintln!(
        "  {} ('{}') is no longer available — {}.",
        label,
        display_name,
        state.describe()
    );
    eprintln!("    Run 'waclient remove {}' to clean it up.", label);
}

async fn handle_connection(tcp_stream: TcpStream, registry: Arc<ShareRegistry>) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| forward(req, registry.clone()));
    http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        // Allow a 101 to hand the browser socket over for raw relaying (WebSocket).
        .with_upgrades()
        .await
        .context("HTTP/1 connection error")
}

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

/// Why a share is permanently unusable. `Removed` = the hub rejected our
/// credentials outright (share deleted on the host node); `Revoked` = this
/// device was revoked from the share's roster.
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

    #[test]
    fn app_descriptions_skip_redundant_names() {
        let apps = vec![
            wire::App {
                id: "echo".into(),
                name: "echo".into(),
                kind: wire::AppKind::Web,
            },
            wire::App {
                id: "jf".into(),
                name: "Jellyfin".into(),
                kind: wire::AppKind::Jellyfin,
            },
        ];
        assert_eq!(describe_apps(&apps), "echo, jf (Jellyfin)");
        assert_eq!(describe_apps(&[]), "none yet");
    }
}
