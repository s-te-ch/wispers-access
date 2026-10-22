//! Loopback forwarding, a workaround for the limitations of iOS WebViews.
//!
//! Normally, in forward-proxy mode, a WebView (or browser tab) sends all its
//! requests through the proxy (or at least we're able to configure it to do
//! so). On iOS, the WebKit insists on skipping the proxy for loopback names.
//! This breaks primarily in-development apps that host backends for example on
//! `localhost:1234`.
//!
//! The workaround is to identify the share's lookback origins, try and bind the
//! same ports on the device's own loopback, then forward requests reaching
//! those ports. The LoopbackForwarder implements this.

use crate::forward_proxy::{forward, open_tunnel};
use crate::loopback::{
    BoxedBody, RequiredCookie, error_response, gone_response, means_browser_went_away,
    strip_cookies,
};
use crate::{Client, Lookup, ShareId, SharedApp};
use anyhow::{Context, Result};
use hyper::StatusCode;
use hyper::body::Incoming;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tracing::warn;
use wire::{AppProxyMode, Origin};
use wispers_access_wire as wire;

/// The name of the forwarding cookie.
pub const COOKIE_NAME: &str = "__wispers_loopback_auth";

/// The ports of the share's `http` loopback origins, each with the hosts the
/// origins on it name (`localhost`, `127.0.0.1`, `[::1]`). Origins that don't
/// parse are left out, as they are everywhere.
pub fn loopback_ports(apps: &[SharedApp]) -> BTreeMap<u16, Vec<String>> {
    let mut ports: BTreeMap<u16, Vec<String>> = BTreeMap::new();
    let origins = apps
        .iter()
        .filter(|app| app.mode == AppProxyMode::ForwardProxy)
        .flat_map(|app| &app.proxied_origins)
        .filter_map(|origin| origin.parse::<Origin>().ok())
        .filter(|origin| origin.is_loopback() && !origin.is_https());
    for origin in origins {
        let hosts = ports.entry(origin.port()).or_default();
        if !hosts.iter().any(|host| host == origin.host()) {
            hosts.push(origin.host().to_owned());
        }
    }
    ports
}

/// Every origin of the share's forward-proxy apps (except those that don't parse).
pub fn share_origins(apps: &[SharedApp]) -> Vec<Origin> {
    apps.iter()
        .filter(|app| app.mode == AppProxyMode::ForwardProxy)
        .flat_map(|app| &app.proxied_origins)
        .filter_map(|origin| origin.parse::<Origin>().ok())
        .collect()
}

/// Serves every connection to one forwarded port, until cancelled.
pub async fn accept_loop(listener: TcpListener, port: Arc<ForwardedPort>) {
    loop {
        match listener.accept().await {
            Ok((tcp_stream, _)) => {
                let port = port.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(tcp_stream, port).await {
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

/// One forwarded port of a share: what its requests need checked and where
/// they go.
pub struct ForwardedPort {
    pub client: Client,
    pub share: ShareId,
    pub port: u16,
    /// The hosts the share's loopback origins on this port name.
    pub hosts: Vec<String>,
    pub cookie: RequiredCookie,
    /// All the share's origins, loopback or not, which let a request in
    /// without the cookie (see `is_from_the_share`).
    pub share_origins: Vec<Origin>,
}

async fn handle_connection(tcp_stream: TcpStream, port: Arc<ForwardedPort>) -> Result<()> {
    let tcp_stream = TokioIo::new(tcp_stream);
    let service = hyper::service::service_fn(move |req| handle_request(req, port.clone()));
    let served = http1_server::Builder::new()
        .serve_connection(tcp_stream, service)
        // A 101 hands the browser socket over for raw relaying (WebSocket).
        .with_upgrades()
        .await;
    match served {
        Ok(()) => Ok(()),
        Err(e) if means_browser_went_away(&e) => Ok(()),
        Err(e) => Err(e).context("HTTP/1 connection error"),
    }
}

async fn handle_request(
    mut req: hyper::Request<Incoming>,
    port: Arc<ForwardedPort>,
) -> Result<hyper::Response<BoxedBody>, Infallible> {
    // Only the share's own WebViews get past here, and the app itself never
    // sees the cookie.
    if !port.cookie.is_presented_in(req.headers()) && !is_from_the_share(&req, &port.share_origins)
    {
        return Ok(error_response(StatusCode::FORBIDDEN, "forbidden"));
    }
    strip_cookies(req.headers_mut(), |name| name == port.cookie.name);

    let node = match port.client.restore_guest_node(port.share.as_str()).await {
        Ok(Lookup::Live(node)) => node,
        Ok(Lookup::Dead(state)) => return Ok(gone_response(state)),
        Ok(Lookup::Unknown) => {
            return Ok(error_response(StatusCode::FORBIDDEN, "unknown share"));
        }
        Err(e) => {
            warn!(share = %port.share, error = format!("{e:#}"), "could not restore the share");
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "Wispers Access server unavailable",
            ));
        }
    };
    let host = tunnel_host(&req, &port.hosts);
    let stream = match open_tunnel(&node, host, port.port).await {
        Ok(stream) => stream,
        Err(resp) => return Ok(*resp),
    };
    Ok(forward(req, stream, node.label()).await)
}

/// Whether the request's `Origin` header names one of the share's origins.
/// Browsers send cookies on a cross-origin `fetch` or XHR only if the page
/// asks for it, so a page on one loopback origin calling an API on another
/// gets here without the cookie. They do always send `Origin` then, and a web
/// page can't set it. Other apps on the device can, so this lets in what the
/// cookie alone would refuse to browsers, nothing more.
fn is_from_the_share(req: &hyper::Request<Incoming>, share_origins: &[Origin]) -> bool {
    req.headers()
        .get(hyper::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<Origin>().ok())
        .is_some_and(|origin| share_origins.contains(&origin))
}

/// Which of the port's hosts the request is for: the one its `Host` header
/// names, else the first. The share can list `localhost` and `127.0.0.1` on
/// the same port, and they may well be different servers on the host.
fn tunnel_host<'a>(req: &hyper::Request<Incoming>, hosts: &'a [String]) -> &'a str {
    let named = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|host| host.parse::<hyper::http::uri::Authority>().ok());
    named
        .and_then(|authority| {
            hosts
                .iter()
                .find(|host| host.eq_ignore_ascii_case(authority.host()))
        })
        .unwrap_or(&hosts[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Setup, send_to};
    use crate::{SdkError, transports::TerminalState};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A port that is free now.
    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn requests_with_the_cookie_reach_the_host_nodes_loopback() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        assert_eq!(forwarder.ports(), [port]);
        let cookie = forwarder.cookie();
        let (mut tcp, head) = send_to(
            port,
            &format!(
                "GET /api?x=1 HTTP/1.1\r\nHost: localhost:{port}\r\n\
                 Cookie: a=1; {}={}\r\nConnection: close\r\n\r\n",
                cookie.name, cookie.value
            ),
        )
        .await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        let mut body = String::new();
        tcp.read_to_string(&mut body).await.unwrap();
        // The app sees its own cookies, not ours.
        assert_eq!(body, "GET /api?x=1 proxy-authorization=false cookie=a=1");
        assert_eq!(setup.targets(), [("localhost".to_owned(), port)]);
    }

    #[tokio::test]
    async fn requests_without_the_cookie_get_a_403_without_a_stream() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let _forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        let (_, head) = send_to(
            port,
            &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: a=1\r\n\r\n"),
        )
        .await;
        assert!(head.starts_with("http/1.1 403"), "{head}");
        assert_eq!(setup.streams(), 0);
    }

    #[tokio::test]
    async fn cross_origin_requests_from_the_share_need_no_cookie() {
        let (page, api) = (free_port().await, free_port().await);
        let setup = Setup::with_origins_redirected(vec![
            format!("http://localhost:{page}"),
            format!("http://localhost:{api}"),
        ])
        .await;
        let _forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        let from = |origin: &str| {
            format!(
                "GET /api HTTP/1.1\r\nHost: localhost:{api}\r\nOrigin: {origin}\r\n\
                 Connection: close\r\n\r\n"
            )
        };
        let (_, head) = send_to(api, &from(&format!("http://localhost:{page}"))).await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        let (_, head) = send_to(api, &from("http://evil.example")).await;
        assert!(head.starts_with("http/1.1 403"), "{head}");
    }

    #[tokio::test]
    async fn rebinding_keeps_the_ports_served() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        setup.proxy.rebind().await;
        let cookie = forwarder.cookie();
        let (_, head) = send_to(
            port,
            &format!(
                "GET / HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {}={}\r\n\
                 Connection: close\r\n\r\n",
                cookie.name, cookie.value
            ),
        )
        .await;
        assert!(head.starts_with("http/1.1 200"), "{head}");
        // The proxy's own port is served again too.
        let (_, head) = setup
            .send(&format!(
                "CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"
            ))
            .await;
        assert!(head.starts_with("http/1.1 407"), "{head}");
    }

    #[tokio::test]
    async fn the_host_header_picks_among_hosts_on_one_port() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![
            format!("http://localhost:{port}"),
            format!("http://127.0.0.1:{port}"),
        ])
        .await;
        let forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        let cookie = forwarder.cookie();
        let (mut tcp, _) = send_to(
            port,
            &format!(
                "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {}={}\r\n\
                 Connection: close\r\n\r\n",
                cookie.name, cookie.value
            ),
        )
        .await;
        tcp.read_to_end(&mut Vec::new()).await.unwrap();
        assert_eq!(setup.targets(), [("127.0.0.1".to_owned(), port)]);
    }

    #[tokio::test]
    async fn forwarders_are_shared_and_release_their_ports() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let first = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        let second = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        drop(first);
        assert!(TcpListener::bind(("127.0.0.1", port)).await.is_err());
        drop(second);
        // The accept loops are aborted on drop; give them a moment to close.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok());
    }

    #[tokio::test]
    async fn a_taken_port_is_reported() {
        let occupant = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = occupant.local_addr().unwrap().port();
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let taken = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await;
        assert!(
            matches!(taken, Err(SdkError::PortInUse(_))),
            "{:?}",
            taken.err()
        );
    }

    #[tokio::test]
    async fn a_removed_share_answers_410() {
        let port = free_port().await;
        let setup = Setup::with_origins_redirected(vec![format!("http://localhost:{port}")]).await;
        let forwarder = setup
            .proxy
            .forward_loopback_origins(setup.share.clone())
            .await
            .unwrap();
        setup
            .client
            .row(&setup.share)
            .unwrap()
            .write_terminal_state(TerminalState::Removed.as_str())
            .unwrap();
        let cookie = forwarder.cookie();
        let (mut tcp, head) = send_to(
            port,
            &format!(
                "GET / HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {}={}\r\n\r\n",
                cookie.name, cookie.value
            ),
        )
        .await;
        assert!(head.starts_with("http/1.1 410"), "{head}");
        tcp.shutdown().await.ok();
        assert_eq!(setup.streams(), 0);
    }

    #[test]
    fn only_http_loopback_origins_are_forwarded() {
        let app = SharedApp {
            id: "a".into(),
            name: "a".into(),
            kind: wire::AppKind::Web,
            mode: AppProxyMode::ForwardProxy,
            start_url: None,
            proxied_origins: [
                "http://localhost:4200",
                "http://127.0.0.1:4200",
                "http://localhost:4200",
                "https://localhost:8443",
                "http://dev.internal:4200",
                "nonsense",
            ]
            .map(String::from)
            .to_vec(),
        };
        assert_eq!(
            loopback_ports(&[app]),
            BTreeMap::from([(4200, vec!["localhost".into(), "127.0.0.1".into()])])
        );
    }
}
