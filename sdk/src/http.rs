//! The loopback HTTP proxy.
//!
//! The user's browser talks to `http://<app>.<share>.wa.localhost:<port>`, and
//! every request becomes one DATA stream to the share's host node. The bare
//! `http://wa.localhost:<port>` is used for browser pairing (see
//! [`crate::pairing`]).

use crate::pairing::CookieIssuer;
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
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
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

/// The host-routed proxy's domain. Every browser resolves `*.localhost` to
/// loopback, and the pairing cookie needs a dotted parent to be scoped to.
/// Apps get served at `<app>.<share>.wa.localhost`.
pub const PROXY_DOMAIN: &str = "wa.localhost";

/// Authenticates requests to a proxy port. Demands `cookie` on every request,
/// rejects with 403 otherwise. When in "pairing" mode, also issues the cookie.
#[derive(Clone)]
pub struct Authenticator {
    cookie: RequiredCookie,
    issuer: Option<Arc<CookieIssuer>>,
}

impl Authenticator {
    /// Create an authenticator for app-cookie mode.
    pub fn for_cookie(cookie: RequiredCookie) -> Self {
        Self {
            cookie,
            issuer: None,
        }
    }

    /// Create an authenticator for pairing mode.
    pub fn for_pairing(issuer: CookieIssuer) -> Self {
        Self {
            cookie: issuer.required_cookie().clone(),
            issuer: Some(Arc::new(issuer)),
        }
    }

    /// True if the request passes authentication.
    fn admits(&self, req: &hyper::Request<Incoming>) -> bool {
        self.cookie.is_presented_in(req.headers())
    }

    /// Removes the cookie from the request, so the proxied app can't read it.
    fn scrub(&self, req: &mut hyper::Request<Incoming>) {
        self.cookie.strip_from(req.headers_mut());
    }

    /// What issues the cookie to browsers, in pairing mode.
    pub fn issuer(&self) -> Option<&CookieIssuer> {
        self.issuer.as_deref()
    }
}

/// A cookie the proxy demands on every request.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RequiredCookie {
    pub name: String,
    pub value: String,
}

impl RequiredCookie {
    /// Whether the request's `Cookie` headers carry the cookie, compared in
    /// time that does not depend on where the values differ.
    fn is_presented_in(&self, headers: &hyper::HeaderMap) -> bool {
        headers
            .get_all(hyper::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(name, value)| name == self.name && constant_time_eq(value, &self.value))
    }

    /// Removes the cookie from the request's `Cookie` headers, so the app
    /// behind the proxy can't read it.
    fn strip_from(&self, headers: &mut hyper::HeaderMap) {
        use hyper::header::{COOKIE, HeaderValue};

        // A `name=value` pair of a `Cookie` header, ours by name.
        let is_ours = |pair: &str| {
            pair.split_once('=')
                .is_some_and(|(name, _)| name == self.name)
        };
        // One `Cookie` header without our pair, or nothing if that was all
        // it carried.
        let without_ours = |header: &HeaderValue| -> Option<HeaderValue> {
            let rest = header
                .to_str()
                .ok()?
                .split(';')
                .map(str::trim)
                .filter(|pair| !is_ours(pair))
                .collect::<Vec<_>>()
                .join("; ");
            if rest.is_empty() {
                return None;
            }
            HeaderValue::from_str(&rest).ok()
        };

        let kept: Vec<HeaderValue> = headers
            .get_all(COOKIE)
            .iter()
            .filter_map(without_ours)
            .collect();
        headers.remove(COOKIE);
        for value in kept {
            headers.append(COOKIE, value);
        }
    }
}

pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Where a listener's requests get their route from.
#[derive(Clone)]
pub enum RouteSource {
    /// The `Host` header: `<app>.<share>.wa.localhost`, or the proxy's own
    /// `wa.localhost`.
    HostHeader,
    /// Every request goes to this one app.
    Fixed { share: ShareId, app: String },
}

/// Serves every connection against the client's shares, until cancelled.
pub async fn accept_loop(
    listener: TcpListener,
    client: Client,
    route_source: RouteSource,
    authenticator: Option<Authenticator>,
) {
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let client = client.clone();
                let route_source = route_source.clone();
                let authenticator = authenticator.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_connection(tcp_stream, client, route_source, authenticator).await
                    {
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

async fn handle_connection(
    tcp_stream: TcpStream,
    client: Client,
    route_source: RouteSource,
    authenticator: Option<Authenticator>,
) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| {
        forward(
            req,
            client.clone(),
            route_source.clone(),
            authenticator.clone(),
        )
    });
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
    route_source: RouteSource,
    authenticator: Option<Authenticator>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    // Determine the share and app...
    let (share, app) = match route_source {
        RouteSource::Fixed { share, app } => (share.to_string(), app),
        RouteSource::HostHeader => {
            let Ok(host) = extract_host(&req) else {
                return Ok(error_response(
                    StatusCode::BAD_REQUEST,
                    "missing host header",
                ));
            };
            match parse_host(&host) {
                Ok(Route::App { share, app }) => (share, app),
                Ok(Route::Proxy) => {
                    return Ok(answer_own_host(&req, &host, authenticator.as_ref()));
                }
                Err(e) => return Ok(error_response(StatusCode::NOT_FOUND, &e.to_string())),
            }
        }
    };

    // Only the user's own browser gets past here, and the app itself never
    // sees the cookie.
    if let Some(authenticator) = &authenticator {
        if !authenticator.admits(&req) {
            return Ok(error_response(StatusCode::FORBIDDEN, "forbidden"));
        }
        authenticator.scrub(&mut req);
    }

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

/// A request to the proxy's own host, `wa.localhost`. Pairing, when the
/// proxy pairs browsers, and nothing else.
fn answer_own_host(
    req: &hyper::Request<Incoming>,
    host: &str,
    authenticator: Option<&Authenticator>,
) -> hyper::Response<BoxedBody> {
    match (req.uri().path(), authenticator.and_then(|a| a.issuer())) {
        ("/pair", Some(issuer)) => answer_pair(req, host, issuer),
        _ => error_response(
            StatusCode::NOT_FOUND,
            &format!("apps are served at http://<app>.<share>.{PROXY_DOMAIN}:<port>"),
        ),
    }
}

/// The pairing URL for a given token and app. Answered by [`answer_pair`].
pub fn pair_url(port: u16, token: &str, share: &str, app: &str) -> String {
    format!("http://{PROXY_DOMAIN}:{port}/pair?token={token}&share={share}&app={app}")
}

/// Answers `GET /pair?token=…&share=…&app=…[&path=…]`, `host` being the
/// request's `Host` header, port included. A paired browser is redirected to
/// the app unconditionally, an unpaired one with a live token gets
/// the cookie and a redirect. Any gets told it is not paired.
fn answer_pair(
    req: &hyper::Request<Incoming>,
    host: &str,
    issuer: &CookieIssuer,
) -> hyper::Response<BoxedBody> {
    let params = parse_query(req.uri().query().unwrap_or(""));
    let (Some(share), Some(app)) = (params.get("share"), params.get("app")) else {
        return error_response(StatusCode::BAD_REQUEST, "missing share or app");
    };
    if !is_dns_label(share) || !is_dns_label(app) {
        return error_response(StatusCode::BAD_REQUEST, "invalid share or app");
    }
    let path = params.get("path").copied().unwrap_or("/");
    if !path.starts_with('/') {
        return error_response(StatusCode::BAD_REQUEST, "path must be absolute");
    }
    // Prepend app and share (and optionally append the path) to get the
    // redirect location.
    let location = format!("http://{app}.{share}.{host}{path}");
    if issuer.required_cookie().is_presented_in(req.headers()) {
        return redirect(&location, None);
    }
    if params.get("token").is_some_and(|t| issuer.consume_token(t)) {
        return redirect(&location, Some(issuer.set_cookie_header()));
    }
    error_response(
        StatusCode::FORBIDDEN,
        "This browser is not paired with Wispers Access. Open the app from Wispers Access again to pair it.",
    )
}

/// Parse the query's `name=value` pairs, without decoding them. Our values are
/// tokens and DNS labels, and `path` is passed on encoded as it came.
fn parse_query(query: &str) -> HashMap<&str, &str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .collect()
}

/// True if s is DNS safe, i.e. can be used as a hostname label.
fn is_dns_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn redirect(location: &str, set_cookie: Option<String>) -> hyper::Response<BoxedBody> {
    let mut builder = hyper::Response::builder()
        .status(StatusCode::FOUND)
        .header(hyper::header::LOCATION, location);
    if let Some(cookie) = set_cookie {
        builder = builder.header(hyper::header::SET_COOKIE, cookie);
    }
    match builder.body(empty_body()) {
        Ok(response) => response,
        // Only a path with characters no header may carry gets here.
        Err(_) => error_response(StatusCode::BAD_REQUEST, "invalid path"),
    }
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

/// Where a request goes, as its `Host` header says.
enum Route {
    /// `<app>.<share>.wa.localhost`, an app of a share.
    App { share: String, app: String },
    /// `wa.localhost` itself, the proxy's own pages.
    Proxy,
}

/// `<app>.<share>.wa.localhost[:port]` or `wa.localhost[:port]`.
fn parse_host(host: &str) -> Result<Route> {
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    if host == PROXY_DOMAIN {
        return Ok(Route::Proxy);
    }
    let labels = host
        .strip_suffix(PROXY_DOMAIN)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .map(|prefix| prefix.split('.').collect::<Vec<_>>());
    match labels.as_deref() {
        Some([app, share]) if !app.is_empty() && !share.is_empty() => Ok(Route::App {
            share: (*share).to_owned(),
            app: (*app).to_owned(),
        }),
        _ => anyhow::bail!(
            "unknown host {host}: apps are served at http://<app>.<share>.{PROXY_DOMAIN}:<port>"
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
    fn the_required_cookie_is_found_among_others() {
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::COOKIE,
            "a=1; __wispers_proxy_auth=s3cret".parse().unwrap(),
        );
        assert!(cookie.is_presented_in(&headers));
        headers.insert(
            hyper::header::COOKIE,
            "__wispers_proxy_auth=s3cre".parse().unwrap(),
        );
        assert!(!cookie.is_presented_in(&headers));
        assert!(!cookie.is_presented_in(&hyper::HeaderMap::new()));
    }

    #[test]
    fn the_required_cookie_is_stripped_and_the_others_kept() {
        let cookie = RequiredCookie {
            name: "__wispers_proxy_auth".into(),
            value: "s3cret".into(),
        };
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            "a=1; __wispers_proxy_auth=s3cret; b=2".parse().unwrap(),
        );
        headers.append(
            hyper::header::COOKIE,
            "__wispers_proxy_auth=other".parse().unwrap(),
        );
        cookie.strip_from(&mut headers);
        let left: Vec<_> = headers.get_all(hyper::header::COOKIE).iter().collect();
        assert_eq!(left, ["a=1; b=2"]);
    }

    #[test]
    fn only_labels_and_absolute_paths_make_a_pair_target() {
        assert!(is_dns_label("round-trip"));
        assert!(!is_dns_label(""));
        assert!(!is_dns_label("a.b"));
        assert!(!is_dns_label("evil.com/"));
        let params = parse_query("token=t&share=s&app=a&path=/x?y=1");
        assert_eq!(params["path"], "/x?y=1");
        assert_eq!(params.len(), 4);
    }

    fn target(host: &str) -> Result<(String, String)> {
        match parse_host(host)? {
            Route::App { share, app } => Ok((share, app)),
            Route::Proxy => anyhow::bail!("the proxy's own host"),
        }
    }

    #[test]
    fn targets_are_app_dot_share_under_the_proxy_domain() {
        assert_eq!(
            target("echo.round-trip.wa.localhost:8000").unwrap(),
            ("round-trip".to_owned(), "echo".to_owned())
        );
        assert_eq!(
            target("echo.round-trip.wa.localhost").unwrap(),
            ("round-trip".to_owned(), "echo".to_owned())
        );
        assert!(matches!(parse_host("wa.localhost:8000"), Ok(Route::Proxy)));
        assert!(matches!(parse_host("wa.localhost"), Ok(Route::Proxy)));
        // One label is not enough, and neither is a foreign host or the old shape.
        assert!(target("round-trip.wa.localhost:8000").is_err());
        assert!(target("localhost:8000").is_err());
        assert!(target("echo.round-trip.localhost").is_err());
        assert!(target("echo.round-trip.example.com").is_err());
        assert!(target(".round-trip.wa.localhost").is_err());
        assert!(target("a.b.c.wa.localhost").is_err());
        assert!(target("evilwa.localhost").is_err());
    }
}
