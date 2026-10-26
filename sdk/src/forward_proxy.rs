//! The forward proxy running on the loopback device, the guest node's side of
//! forward-proxy mode.
//!
//! A share's webview uses it as its HTTP proxy for the share's proxied origins
//! (see `AppProxyMode` in the wire crate). Every connection the browser tunnels
//! with CONNECT, and every request for a plain `http` URL, becomes one
//! FORWARD_PROXY stream to the share's host node. How a port tells its share
//! apart, and who may connect to it, depends on its [`ShareSource`].

use crate::loopback::{
    BoxedBody, empty_body, error_response, gone_response, is_protocol_upgrade_request,
    means_browser_went_away, relay_upgraded_connection, strip_hop_by_hop_headers,
};
use crate::secrets::constant_time_eq;
use crate::socket_owner;
use crate::transports::{Stream, TerminalState, TransportError};
use crate::{Client, Lookup, ShareId, SharedApp};
use anyhow::{Context, Result};
use data_encoding::BASE64;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::client::conn::http1 as http1_client;
use hyper::server::conn::http1 as http1_server;
use hyper::{HeaderMap, Method, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, warn};
use wire::{AppProxyMode, ForwardProxyError, ForwardProxyReply, Origin};
use wispers_access_wire as wire;

/// The realm of the proxy's challenges, used so the webview can tell them from
/// the challenges of a shared app.
pub const REALM: &str = "Wispers Access";

/// Mints the shares' credentials and matches requests against them. They
/// live in memory only, as long as the proxy.
#[derive(Default)]
pub struct CredentialIssuer {
    /// Each share's password. The username is the share's ID.
    passwords: Mutex<HashMap<ShareId, String>>,
}

impl CredentialIssuer {
    /// Mints the share's password, or returns the one minted before.
    pub fn mint_password(&self, share: &ShareId) -> String {
        self.passwords
            .lock()
            .expect("unpoisoned")
            .entry(share.clone())
            .or_insert_with(crate::secrets::mint_secret)
            .clone()
    }

    /// The share whose credential the request's `Proxy-Authorization` header
    /// carries, if it's one minted here.
    fn match_presented_credentials(&self, headers: &HeaderMap) -> Option<ShareId> {
        let value = headers
            .get(hyper::header::PROXY_AUTHORIZATION)?
            .to_str()
            .ok()?;
        let (scheme, encoded) = value.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let decoded = BASE64.decode(encoded.trim().as_bytes()).ok()?;
        let (username, password) = std::str::from_utf8(&decoded).ok()?.split_once(':')?;
        let passwords = self.passwords.lock().expect("unpoisoned");
        let (share, expected) = passwords
            .iter()
            .find(|(share, _)| share.as_str() == username)?;
        constant_time_eq(password, expected).then(|| share.clone())
    }
}

/// Where a forward-proxy port's requests get their share from.
#[derive(Clone)]
pub enum ShareSource {
    /// Each request is for the share whose credential it carries, one of
    /// these: one port serves every share.
    Credential(Arc<CredentialIssuer>),
    /// Every request on the port is for this share. With no credential to
    /// tell the share's browser apart, only processes of this OS user may
    /// connect, and targets the share doesn't proxy are reached directly.
    Fixed(ShareId),
}

/// Serves every connection to the port, until cancelled.
pub async fn accept_loop(listener: TcpListener, client: Client, share_source: ShareSource) {
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let client = client.clone();
                let share_source = share_source.clone();
                tokio::spawn(async move {
                    if let ShareSource::Fixed(share) = &share_source
                        && !connected_by_this_user(&tcp_stream, share).await
                    {
                        return;
                    }
                    if let Err(e) = handle_connection(tcp_stream, client, share_source).await {
                        warn!(error = format!("{e:#}"), "connection error");
                    }
                });
            }
            Err(e) => {
                warn!(error = format!("{e:#}"), "accept error");
                // A reclaimed socket (iOS) fails every accept; don't spin.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Whether the connection comes from a process of this OS user. A connection
/// whose owner can't be told is refused as well.
async fn connected_by_this_user(tcp_stream: &TcpStream, share: &ShareId) -> bool {
    let (Ok(local), Ok(peer)) = (tcp_stream.local_addr(), tcp_stream.peer_addr()) else {
        return false;
    };
    let owner = tokio::task::spawn_blocking(move || socket_owner::is_same_user(local, peer)).await;
    match owner {
        Ok(Ok(true)) => true,
        Ok(Ok(false)) => {
            warn!(%share, %peer, "refused a connection from another user");
            false
        }
        Ok(Err(e)) => {
            warn!(%share, %peer, error = %e, "refused a connection whose owner is unknown");
            false
        }
        Err(_) => false,
    }
}

async fn handle_connection(
    tcp_stream: TcpStream,
    client: Client,
    share_source: ShareSource,
) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| {
        handle_request(req, client.clone(), share_source.clone())
    });
    let served = http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        // A CONNECT, or a 101 to a plain request, hands the browser socket
        // over for raw relaying.
        .with_upgrades()
        .await;
    match served {
        Ok(()) => Ok(()),
        Err(e) if means_browser_went_away(&e) => Ok(()),
        Err(e) => Err(e).context("HTTP/1 connection error"),
    }
}

async fn handle_request(
    req: hyper::Request<Incoming>,
    client: Client,
    share_source: ShareSource,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    let share = match &share_source {
        // Neither WebView nor WebKit sends the credential before it's
        // challenged, so every new connection starts here.
        ShareSource::Credential(credential_issuer) => {
            match credential_issuer.match_presented_credentials(req.headers()) {
                Some(share) => share,
                None => return Ok(challenge()),
            }
        }
        ShareSource::Fixed(share) => share.clone(),
    };
    let Some((host, port)) = target(&req) else {
        // Not a request for a proxy. Close, so that nothing in its body can
        // be read as the next request.
        let mut resp = error_response(
            StatusCode::BAD_REQUEST,
            "only CONNECT and http:// requests are proxied",
        );
        resp.headers_mut().insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("close"),
        );
        return Ok(resp);
    };

    // Only the share's own origins go to its host node, checked here so a
    // webview of one share can't reach another's, and nothing is opened for
    // the rest.
    let apps = match client.row(&share).map(|row| row.read_apps()) {
        Ok(Ok(apps)) => apps,
        Ok(Err(e)) => {
            warn!(%share, error = format!("{e:#}"), "could not read the share's apps");
            return Ok(unavailable_response());
        }
        Err(e) => {
            warn!(%share, error = format!("{e:#}"), "could not read the share");
            return Ok(error_response(StatusCode::FORBIDDEN, "unknown share"));
        }
    };
    if !lists_origin(&apps, &host, port) {
        return Ok(match share_source {
            // The share's browser profile goes through this port for
            // everything, as the command line can't name the origins.
            ShareSource::Fixed(_) => go_direct(req, &host, port).await,
            ShareSource::Credential(_) => error_response(
                StatusCode::FORBIDDEN,
                &format!("{host}:{port} is not proxied by this share"),
            ),
        });
    }

    let node = match client.restore_guest_node(share.as_str()).await {
        Ok(Lookup::Live(node)) => node,
        Ok(Lookup::Dead(state)) => return Ok(gone_response(state)),
        Ok(Lookup::Unknown) => {
            return Ok(error_response(StatusCode::FORBIDDEN, "unknown share"));
        }
        Err(e) => {
            warn!(%share, error = format!("{e:#}"), "could not restore the share");
            return Ok(unavailable_response());
        }
    };
    let stream = match open_tunnel(&node, &host, port).await {
        Ok(stream) => stream,
        Err(refusal) => return Ok(refusal.into_response(&host, port)),
    };
    Ok(if req.method() == Method::CONNECT {
        tunnel(req, stream)
    } else {
        forward(req, stream, node.label()).await
    })
}

/// Connects to a target the share doesn't proxy from this machine itself,
/// so the share's browser profile works like any other browser outside the
/// share's origins. It doesn't use the machine's own proxy, if any.
async fn go_direct(
    req: hyper::Request<Incoming>,
    host: &str,
    port: u16,
) -> hyper::Response<BoxedBody> {
    // Hosts are as in `Origin::host`: IPv6 literals in brackets.
    let name = host.trim_start_matches('[').trim_end_matches(']');
    let tcp = match TcpStream::connect((name, port)).await {
        Ok(tcp) => tcp,
        Err(e) => {
            debug!(host, port, error = %e, "direct connection failed");
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("could not reach {host}:{port}"),
            );
        }
    };
    let stream: Stream = Box::new(tcp);
    if req.method() == Method::CONNECT {
        tunnel(req, stream)
    } else {
        forward(req, stream, "direct").await
    }
}

/// The 407 that makes a webview send its share's credential.
fn challenge() -> hyper::Response<BoxedBody> {
    let mut resp = error_response(
        StatusCode::PROXY_AUTHENTICATION_REQUIRED,
        "proxy authentication required",
    );
    resp.headers_mut().insert(
        hyper::header::PROXY_AUTHENTICATE,
        hyper::header::HeaderValue::from_str(&format!("Basic realm=\"{REALM}\""))
            .expect("the realm is a valid header value"),
    );
    resp
}

/// Where a proxy request goes: the authority of a CONNECT, or the host and
/// port of an absolute `http` URL. `None` for anything else, such as a request
/// in origin form. The host is lowercased, as in [`Origin::host`].
fn target(req: &hyper::Request<Incoming>) -> Option<(String, u16)> {
    let uri = req.uri();
    let authority = uri.authority()?;
    let port = if req.method() == Method::CONNECT {
        authority.port_u16()?
    } else if uri.scheme_str() == Some("http") {
        authority.port_u16().unwrap_or(80)
    } else {
        return None;
    };
    Some((authority.host().to_ascii_lowercase(), port))
}

/// Whether a forward-proxy app of the share lists an origin at `host:port`.
/// An origin this build can't parse is listed nowhere.
fn lists_origin(apps: &[SharedApp], host: &str, port: u16) -> bool {
    apps.iter()
        .filter(|app| app.mode == AppProxyMode::ForwardProxy)
        .flat_map(|app| &app.proxied_origins)
        .filter_map(|origin| origin.parse::<Origin>().ok())
        .any(|origin| origin.host() == host && origin.port() == port)
}

/// Why a FORWARD_PROXY stream to `host:port` didn't get connected.
#[derive(Debug)]
pub(crate) enum TunnelRefusal {
    /// The share was removed, or this guest node revoked.
    Gone(TerminalState),
    /// Our app list is stale: the host node no longer proxies the origin.
    NotAllowed,
    /// The host node couldn't connect to the origin.
    Unreachable,
    /// No stream, no answer, or an answer this SDK doesn't know.
    Unavailable,
}

impl TunnelRefusal {
    /// The response that tells the browser why it can't reach `host:port`.
    pub(crate) fn into_response(self, host: &str, port: u16) -> hyper::Response<BoxedBody> {
        match self {
            TunnelRefusal::Gone(state) => gone_response(state),
            TunnelRefusal::NotAllowed => error_response(
                StatusCode::FORBIDDEN,
                &format!("{host}:{port} is not proxied by this share"),
            ),
            TunnelRefusal::Unreachable => error_response(
                StatusCode::BAD_GATEWAY,
                &format!("the share's host could not reach {host}:{port}"),
            ),
            TunnelRefusal::Unavailable => unavailable_response(),
        }
    }
}

/// A reason shown next to the origin, so the origin is "it".
impl std::fmt::Display for TunnelRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TunnelRefusal::Gone(_) => "the share is no longer available",
            TunnelRefusal::NotAllowed => "the share no longer proxies it",
            TunnelRefusal::Unreachable => "the share's host could not reach it",
            TunnelRefusal::Unavailable => "the share's host is unavailable",
        })
    }
}

/// A FORWARD_PROXY stream to `host:port` that the host node has connected.
pub(crate) async fn open_tunnel(
    node: &crate::guest_node::GuestNode,
    host: &str,
    port: u16,
) -> Result<Stream, TunnelRefusal> {
    let mut stream = match node.open_forward_proxy_stream(host, port).await {
        Ok(stream) => stream,
        Err(TransportError::Terminal(state)) => return Err(TunnelRefusal::Gone(state)),
        Err(TransportError::Transient(e)) => {
            warn!(
                share = node.label(),
                error = format!("{e:#}"),
                "could not open a stream"
            );
            return Err(TunnelRefusal::Unavailable);
        }
    };
    let reply: ForwardProxyReply = match wire::read_message(&mut stream).await {
        Ok(reply) => reply,
        Err(e) => {
            warn!(
                share = node.label(),
                error = format!("{e:#}"),
                "no answer to a FORWARD_PROXY stream"
            );
            return Err(TunnelRefusal::Unavailable);
        }
    };
    match reply.error {
        None => Ok(stream),
        Some(ForwardProxyError::NotAllowed) => Err(TunnelRefusal::NotAllowed),
        Some(ForwardProxyError::Unreachable) => Err(TunnelRefusal::Unreachable),
        Some(ForwardProxyError::Unknown) => Err(TunnelRefusal::Unavailable),
    }
}

/// Answers a CONNECT with 200 and relays the browser's connection over the
/// stream once hyper hands it over, until both directions have closed.
fn tunnel(mut req: hyper::Request<Incoming>, mut stream: Stream) -> hyper::Response<BoxedBody> {
    let browser = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        match browser.await {
            Ok(browser) => {
                let mut browser = TokioIo::new(browser);
                if let Err(e) = tokio::io::copy_bidirectional(&mut browser, &mut stream).await {
                    debug!(error = %e, "tunnel ended with an error");
                }
            }
            Err(e) => warn!(error = %e, "CONNECT did not complete"),
        }
    });
    hyper::Response::new(empty_body())
}

/// Sends a request for an `http` URL down the stream in origin form, as the
/// app expects it, and its response back. On a 101, the connection is relayed
/// from then on.
pub(crate) async fn forward(
    mut req: hyper::Request<Incoming>,
    stream: Stream,
    share: &str,
) -> hyper::Response<BoxedBody> {
    let (mut sender, conn) = match http1_client::handshake(TokioIo::new(stream)).await {
        Ok(hs) => hs,
        Err(e) => {
            warn!(share, error = format!("{e:#}"), "client handshake failed");
            return unavailable_response();
        }
    };
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            warn!(error = format!("{e:#}"), "upstream connection error");
        }
    });

    let upgrade = is_protocol_upgrade_request(req.headers());
    let browser_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));
    let (mut parts, body) = req.into_parts();
    parts.uri = match parts.uri.path_and_query() {
        Some(path_and_query) => path_and_query.as_str().parse().expect("a path is a URI"),
        None => hyper::Uri::from_static("/"),
    };
    // `Proxy-Authorization` goes with the hop-by-hop headers.
    parts.headers.remove("proxy-connection");
    strip_hop_by_hop_headers(&mut parts.headers, upgrade);
    if !upgrade {
        // One request per stream, as on REVERSE_PROXY streams, so the stream
        // is finished after the response (see `reverse_proxy::forward`).
        parts.headers.insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("close"),
        );
    }
    let mut resp = match sender
        .send_request(hyper::Request::from_parts(parts, body))
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            warn!(
                share,
                error = format!("{e:#}"),
                "sending the request failed"
            );
            return unavailable_response();
        }
    };

    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        match browser_upgrade {
            Some(browser_upgrade) => {
                let host_node_upgrade = hyper::upgrade::on(&mut resp);
                tokio::spawn(relay_upgraded_connection(
                    browser_upgrade,
                    host_node_upgrade,
                ));
            }
            None => warn!(share, "the app returned 101 without an upgrade request"),
        }
        let (mut parts, _body) = resp.into_parts();
        strip_hop_by_hop_headers(&mut parts.headers, true);
        return hyper::Response::from_parts(parts, empty_body());
    }
    let (mut parts, body) = resp.into_parts();
    strip_hop_by_hop_headers(&mut parts.headers, false);
    let body: BoxedBody = body.map_err(std::io::Error::other).boxed();
    hyper::Response::from_parts(parts, body)
}

fn unavailable_response() -> hyper::Response<BoxedBody> {
    error_response(StatusCode::BAD_GATEWAY, "Wispers Access server unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Setup, echo_app, send_to};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn requests_without_the_shares_credential_are_challenged() {
        let setup = Setup::new().await;
        let target = format!("127.0.0.1:{}", setup.app_port);
        let (_, head) = setup
            .send(&format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"
            ))
            .await;
        assert!(head.starts_with("http/1.1 407"), "{head}");
        assert!(
            head.contains("proxy-authenticate: basic realm=\"wispers access\""),
            "{head}"
        );
        let wrong = format!(
            "Basic {}",
            BASE64.encode(format!("{}:guess", setup.share).as_bytes())
        );
        let (_, head) = setup
            .send(&format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {wrong}\r\n\r\n"
            ))
            .await;
        assert!(head.starts_with("http/1.1 407"), "{head}");
        assert_eq!(setup.streams(), 0);
    }

    #[tokio::test]
    async fn connect_tunnels_to_a_proxied_origin() {
        let setup = Setup::new().await;
        let target = format!("127.0.0.1:{}", setup.app_port);
        let (mut tcp, head) = setup
            .send(&format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
                setup.authorization()
            ))
            .await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        // From here on, the browser talks to the app itself.
        tcp.write_all(b"GET /through HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        tcp.read_to_string(&mut response).await.unwrap();
        assert!(
            response.ends_with("GET /through proxy-authorization=false cookie=-"),
            "{response}"
        );
        assert_eq!(setup.streams(), 1);
    }

    #[tokio::test]
    async fn plain_http_requests_reach_the_app_in_origin_form() {
        let setup = Setup::new().await;
        let target = format!("127.0.0.1:{}", setup.app_port);
        let (mut tcp, head) = setup
            .send(&format!(
                "GET http://{target}/x?y=1 HTTP/1.1\r\nHost: {target}\r\n\
                 Proxy-Authorization: {}\r\nProxy-Connection: keep-alive\r\n\r\n",
                setup.authorization()
            ))
            .await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        let expected = "GET /x?y=1 proxy-authorization=false cookie=-";
        let mut body = vec![0u8; expected.len()];
        tcp.read_exact(&mut body).await.unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), expected);
    }

    #[tokio::test]
    async fn other_targets_get_a_403_without_a_stream() {
        let setup = Setup::new().await;
        let other = format!("127.0.0.1:{}", setup.app_port + 1);
        let (_, head) = setup
            .send(&format!(
                "CONNECT {other} HTTP/1.1\r\nHost: {other}\r\nProxy-Authorization: {}\r\n\r\n",
                setup.authorization()
            ))
            .await;
        assert!(head.starts_with("http/1.1 403"), "{head}");
        assert_eq!(setup.streams(), 0);
    }

    #[tokio::test]
    async fn requests_not_meant_for_a_proxy_are_rejected() {
        let setup = Setup::new().await;
        let (_, head) = setup
            .send(&format!(
                "GET /x HTTP/1.1\r\nHost: a\r\nProxy-Authorization: {}\r\n\r\n",
                setup.authorization()
            ))
            .await;
        assert!(head.starts_with("http/1.1 400"), "{head}");
        assert!(head.contains("connection: close"), "{head}");
        assert_eq!(setup.streams(), 0);
    }

    #[tokio::test]
    async fn a_removed_share_answers_410() {
        let setup = Setup::new().await;
        setup
            .client
            .row(&setup.share)
            .unwrap()
            .write_terminal_state(TerminalState::Removed.as_str())
            .unwrap();
        let target = format!("127.0.0.1:{}", setup.app_port);
        let (_, head) = setup
            .send(&format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
                setup.authorization()
            ))
            .await;
        assert!(head.starts_with("http/1.1 410"), "{head}");
        assert_eq!(setup.streams(), 0);
    }

    #[tokio::test]
    async fn same_user_ports_need_no_credential_and_send_other_targets_direct() {
        let setup = Setup::new().await;
        let proxy = setup.client.start_per_share_forward_proxy();
        let port = proxy.share_port(setup.share.clone()).await.unwrap();
        let connect =
            |target: String| format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
        let through_the_share = |mut tcp: TcpStream| async move {
            tcp.write_all(b"GET /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut response = String::new();
            tcp.read_to_string(&mut response).await.unwrap();
            response
        };

        // The share's origin, through its host node.
        let (tcp, head) = send_to(port, &connect(format!("127.0.0.1:{}", setup.app_port))).await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(through_the_share(tcp).await.contains("GET /x"));
        assert_eq!(setup.streams(), 1);

        // Anything else, straight from this machine.
        let elsewhere = echo_app().await;
        let (tcp, head) = send_to(port, &connect(format!("127.0.0.1:{elsewhere}"))).await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        assert!(through_the_share(tcp).await.contains("GET /x"));
        assert_eq!(setup.streams(), 1);

        // The port outlives the proxy.
        drop(proxy);
        let proxy = setup.client.start_per_share_forward_proxy();
        assert_eq!(proxy.share_port(setup.share.clone()).await.unwrap(), port);
    }

    #[tokio::test]
    async fn a_released_share_port_stops_listening() {
        let setup = Setup::new().await;
        let proxy = setup.client.start_per_share_forward_proxy();
        let port = proxy.share_port(setup.share.clone()).await.unwrap();
        proxy.release(setup.share.clone());
        // The listeners close once the client's runtime drops the aborted
        // accept loops.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            assert!(std::time::Instant::now() < deadline, "{port} still listens");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn origins_are_matched_by_host_and_port() {
        let app = |mode, origins: &[&str]| SharedApp {
            id: "a".into(),
            name: "a".into(),
            kind: wire::AppKind::Web,
            mode,
            start_url: None,
            proxied_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
        };
        let apps = [
            app(
                AppProxyMode::ForwardProxy,
                &["http://localhost:4200", "https://sso.internal", "nonsense"],
            ),
            app(AppProxyMode::Unknown, &["http://other:80"]),
        ];
        assert!(lists_origin(&apps, "localhost", 4200));
        assert!(lists_origin(&apps, "sso.internal", 443));
        assert!(!lists_origin(&apps, "sso.internal", 80));
        assert!(!lists_origin(&apps, "other", 80));
    }
}
