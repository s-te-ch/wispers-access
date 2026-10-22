//! What the forward proxy's tests and the loopback forwarding's tests share: a
//! client with one forward-proxy share, a host node that serves its
//! FORWARD_PROXY streams, and an app behind it.

use crate::guest_node::GuestNode;
use crate::transports::{BoxFuture, ConnectionCheck, Stream, Transport, TransportError};
use crate::{Client, ClientConfig, CredentialRoutedForwardProxy, NoObserver, ShareId, SharedApp};
use data_encoding::BASE64;
use hyper::server::conn::http1 as http1_server;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OnceCell;
use wire::{AppProxyMode, ForwardProxyError, ForwardProxyReply};
use wispers_access_wire as wire;

/// A client with one share whose forward-proxy app lists some origins, served
/// by a [`FakeHost`], and its proxy.
pub struct Setup {
    data_dir: std::path::PathBuf,
    pub client: Client,
    pub proxy: Arc<CredentialRoutedForwardProxy>,
    pub share: ShareId,
    /// The port of the [`echo_app`] behind the host node.
    pub app_port: u16,
    streams: Arc<AtomicUsize>,
    targets: Arc<Mutex<Vec<(String, u16)>>>,
}

impl Setup {
    /// The app lists the echo app's origin, and the host node connects each
    /// tunnel to where it's meant to go.
    pub async fn new() -> Self {
        Self::build(
            |app_port| vec![format!("http://127.0.0.1:{app_port}")],
            false,
        )
        .await
    }

    /// The app lists `origins`, and the host node connects each tunnel to the
    /// echo app wherever it's meant to go. For loopback origins, whose ports
    /// the test binds on this same machine.
    pub async fn with_origins_redirected(origins: Vec<String>) -> Self {
        Self::build(move |_| origins.clone(), true).await
    }

    async fn build(origins: impl Fn(u16) -> Vec<String>, redirect: bool) -> Self {
        let data_dir = std::env::temp_dir().join(format!(
            "wispers-access-forward-proxy-{}",
            uuid::Uuid::new_v4()
        ));
        let client = Client::new_with_runtime(
            ClientConfig {
                data_dir: data_dir.to_string_lossy().into_owned(),
                secrets: None,
                observer: None,
            },
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        let app_port = echo_app().await;
        let origins = origins(app_port);
        let row = client.inner.db.new_row().unwrap();
        row.write_share_info(&wire::ShareInfo {
            config_hash: wire::ConfigHash(1),
            name: "Dev".into(),
            transport: "iroh".into(),
            apps: vec![SharedApp {
                id: "dev".into(),
                name: "Dev".into(),
                kind: wire::AppKind::Web,
                mode: AppProxyMode::ForwardProxy,
                start_url: Some(format!("{}/", origins[0])),
                proxied_origins: origins,
            }],
            ca_certificates: Vec::new(),
        })
        .unwrap();
        row.write_deduped_hostname("dev").unwrap();
        row.mark_complete().unwrap();
        let share = row.share_id().unwrap();
        let streams = Arc::new(AtomicUsize::new(0));
        let targets = Arc::new(Mutex::new(Vec::new()));
        let node = GuestNode::new(
            "dev".into(),
            row,
            Box::new(FakeHost {
                streams: streams.clone(),
                targets: targets.clone(),
                redirect_to: redirect.then_some(app_port),
            }),
            Arc::new(NoObserver),
        );
        client.inner.guest_nodes.lock().unwrap().insert(
            share.clone(),
            Arc::new(OnceCell::new_with(Some(Arc::new(node)))),
        );
        let proxy = client
            .start_credential_routed_forward_proxy(0)
            .await
            .unwrap();
        Setup {
            data_dir,
            client,
            proxy,
            share,
            app_port,
            streams,
            targets,
        }
    }

    /// The `Proxy-Authorization` value for the share's credential.
    pub fn authorization(&self) -> String {
        let credential = self.proxy.credential(self.share.clone()).unwrap();
        let pair = format!("{}:{}", credential.username, credential.password);
        format!("Basic {}", BASE64.encode(pair.as_bytes()))
    }

    /// Sends `head` (a request without a body) to the proxy and returns
    /// the connection and the response head, lowercased.
    pub async fn send(&self, head: &str) -> (TcpStream, String) {
        send_to(self.proxy.port(), head).await
    }

    /// How many streams the host node got.
    pub fn streams(&self) -> usize {
        self.streams.load(Ordering::SeqCst)
    }

    /// Where the tunnels were meant to go, in order.
    pub fn targets(&self) -> Vec<(String, u16)> {
        self.targets.lock().unwrap().clone()
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.data_dir).ok();
    }
}

/// Sends `head` (a request without a body) to a port on `127.0.0.1` and
/// returns the connection and the response head, lowercased.
pub async fn send_to(port: u16, head: &str) -> (TcpStream, String) {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tcp.write_all(head.as_bytes()).await.unwrap();
    let mut received = Vec::new();
    while !received.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        tcp.read_exact(&mut byte).await.unwrap();
        received.push(byte[0]);
    }
    (tcp, String::from_utf8(received).unwrap().to_lowercase())
}

/// A host node that serves FORWARD_PROXY streams as waserver does, minus the
/// check of the target. Counts the streams and records their targets.
struct FakeHost {
    streams: Arc<AtomicUsize>,
    targets: Arc<Mutex<Vec<(String, u16)>>>,
    /// Where to connect every tunnel instead of its target, if anywhere.
    redirect_to: Option<u16>,
}

impl Transport for FakeHost {
    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            self.streams.fetch_add(1, Ordering::SeqCst);
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let targets = self.targets.clone();
            let redirect_to = self.redirect_to;
            tokio::spawn(async move {
                let mut first = [0u8; 1];
                far.read_exact(&mut first).await.unwrap();
                assert_eq!(first[0], wire::StreamType::ForwardProxy as u8);
                let preamble: wire::ForwardProxyPreamble =
                    wire::read_message(&mut far).await.unwrap();
                targets
                    .lock()
                    .unwrap()
                    .push((preamble.host.clone(), preamble.port));
                let connected = match redirect_to {
                    Some(port) => TcpStream::connect(("127.0.0.1", port)).await,
                    None => TcpStream::connect((preamble.host.as_str(), preamble.port)).await,
                };
                let Ok(mut tcp) = connected else {
                    let reply = ForwardProxyReply {
                        error: Some(ForwardProxyError::Unreachable),
                    };
                    wire::write_message(&mut far, &reply).await.unwrap();
                    return;
                };
                wire::write_message(&mut far, &ForwardProxyReply::default())
                    .await
                    .unwrap();
                tokio::io::copy_bidirectional(&mut far, &mut tcp).await.ok();
            });
            Ok(Box::new(near) as Stream)
        })
    }

    fn check_connection(
        &self,
        _deadline: std::time::Duration,
    ) -> BoxFuture<'_, Result<ConnectionCheck, TransportError>> {
        Box::pin(async { Ok(ConnectionCheck::NoConnection) })
    }

    fn drop_connection(&self) {}
}

/// An app that answers every request with its request line, whether it saw a
/// `Proxy-Authorization` header, and its `Cookie` header (`-` if none).
pub async fn echo_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(|req: hyper::Request<_>| async move {
                    let seen = format!(
                        "{} {} proxy-authorization={} cookie={}",
                        req.method(),
                        req.uri(),
                        req.headers().contains_key("proxy-authorization"),
                        req.headers()
                            .get("cookie")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-")
                    );
                    Ok::<_, Infallible>(hyper::Response::new(http_body_util::Full::new(
                        bytes::Bytes::from(seen),
                    )))
                });
                let _ = http1_server::Builder::new()
                    .serve_connection(TokioIo::new(tcp), service)
                    .await;
            });
        }
    });
    port
}
