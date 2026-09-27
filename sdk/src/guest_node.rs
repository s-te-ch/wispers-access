//! This device's node in one share, and the guest API it speaks.

use crate::Observer;
use crate::storage;
use crate::transports::{ConnectionCheck, Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::StatusCode;
use hyper::client::conn::http1 as http1_client;
use hyper_util::rt::TokioIo;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;
use tracing::{debug, warn};
use wire::{ConfigHash, ShareInfo};
use wispers_access_wire as wire;

/// This device's guest node in one share: its transport to the host node,
/// its row in the store, and the guest API calls it makes. Restored on the
/// share's first use, by `Client::guest_node`.
pub struct GuestNode {
    /// The `<share>` label in `<app>.<share>.localhost`.
    label: String,
    row: storage::Row,
    transport: Box<dyn Transport>,
    observer: Arc<dyn Observer>,
    /// Set once the transport reports a terminal failure mid-session. From
    /// then on the share answers without dialing.
    dead: Mutex<Option<TerminalState>>,
    /// Held while `check_connection` probes the cached connection, so a
    /// stream asked for meanwhile waits for the verdict rather than going
    /// out on a connection that may be dead.
    probe_gate: tokio::sync::Mutex<()>,
    /// Time when the last QUIC stream was opened, used to determine whether or
    /// not to reestablish the connection if it drops.
    last_used: Mutex<Option<Instant>>,
}

impl GuestNode {
    pub fn new(
        label: String,
        row: storage::Row,
        transport: Box<dyn Transport>,
        observer: Arc<dyn Observer>,
    ) -> Self {
        Self {
            label,
            row,
            transport,
            observer,
            dead: Mutex::new(None),
            probe_gate: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(None),
        }
    }

    /// Check connection health and drop/redial the connection if necessary.
    /// Dead connections get redialed if they had been used recently (within
    /// `RECENT_USE`). Otherwise, we just drop them. The probe delay is capped
    /// by `PROBE_DEADLINE`.
    pub async fn check_connection(&self) {
        let _gate = self.probe_gate.lock().await;
        match self.transport.check_connection(PROBE_DEADLINE).await {
            Ok(ConnectionCheck::Alive) => {
                debug!(share = self.label, "cached connection answered");
                return;
            }
            Ok(ConnectionCheck::NoConnection) => {}
            Ok(ConnectionCheck::Dead) => {
                warn!(share = self.label, "cached connection is dead; dropped");
            }
            Err(TransportError::Terminal(state)) => {
                self.record_terminal_error(state);
                return;
            }
            Err(TransportError::Transient(e)) => {
                warn!(
                    share = self.label,
                    error = format!("{e:#}"),
                    "could not check the connection"
                );
            }
        }
        if self.recently_used() {
            self.reconnect().await;
        }
    }

    fn recently_used(&self) -> bool {
        self.last_used
            .lock()
            .expect("unpoisoned")
            .is_some_and(|at| at.elapsed() < RECENT_USE)
    }

    /// Dials the host node now, in anticipation of future requests, with the
    /// aim to cut down on user-visible latency. This ignores errors — followup
    /// requests will deal with those.
    async fn reconnect(&self) {
        match self.open_stream_ungated().await {
            Ok(_) => debug!(share = self.label, "reconnected ahead of the next request"),
            Err(TransportError::Terminal(_)) => {}
            Err(TransportError::Transient(e)) => warn!(
                share = self.label,
                error = format!("{e:#}"),
                "could not reconnect ahead of the next request"
            ),
        }
    }

    /// Forget the cached connection.
    pub fn drop_connection(&self) {
        self.transport.drop_connection();
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Opens a DATA stream for one app, ready for the HTTP request.
    pub async fn open_data_stream(&self, app_id: &str) -> Result<Stream, TransportError> {
        let mut stream = self.open_stream().await?;
        wire::open_data_stream(&mut stream, app_id)
            .await
            .map_err(|e| TransportError::Transient(e.into()))?;
        Ok(stream)
    }

    /// Asks the host node for the share if it changed since the stored copy,
    /// and stores the answer. `None` = unchanged.
    pub async fn refresh(&self) -> Result<Option<ShareInfo>, RefreshError> {
        let stream = self.open_stream().await.map_err(|e| match e {
            TransportError::Terminal(_) => RefreshError::Terminal,
            TransportError::Transient(e) => RefreshError::Transient(e),
        })?;
        let known = self.row.read_share_config_hash()?;
        let info = fetch_info(stream, known).await?;
        if let Some(info) = &info {
            self.row.write_share_info(info)?;
            self.observer.on_share_changed(self.row.read_share()?);
        }
        Ok(info)
    }

    /// Open a QUIC stream to the host node on the share's transport, opening a
    /// new QUIC connection if necessary. This will wait for in-flight probes to
    /// finish.
    async fn open_stream(&self) -> Result<Stream, TransportError> {
        drop(self.probe_gate.lock().await);
        self.open_stream_ungated().await
    }

    /// Open a QUIC stream to the host node like `open_stream`, except without
    /// blocking on in-flight probes. Any terminal errors (e.g. if this guest
    /// node has been revoked) get recorded, to prevent futile retries in the
    /// future.
    async fn open_stream_ungated(&self) -> Result<Stream, TransportError> {
        if let Some(state) = *self.dead.lock().expect("unpoisoned") {
            return Err(TransportError::Terminal(state));
        }
        match self.transport.open_stream().await {
            Err(TransportError::Terminal(state)) => {
                self.record_terminal_error(state);
                Err(TransportError::Terminal(state))
            }
            Ok(stream) => {
                *self.last_used.lock().expect("unpoisoned") = Some(Instant::now());
                Ok(stream)
            }
            other => other,
        }
    }

    /// Record that we've received a terminal error from the host node,
    /// preventing retries in the future.
    fn record_terminal_error(&self, state: TerminalState) {
        warn!(
            share = self.label,
            "share is no longer available: {}",
            state.describe()
        );
        *self.dead.lock().expect("unpoisoned") = Some(state);
        match self
            .row
            .write_terminal_state(state.as_str())
            .and_then(|()| self.row.read_share())
        {
            Ok(share) => self.observer.on_share_changed(share),
            Err(e) => warn!(
                share = self.label,
                error = format!("{e:#}"),
                "could not record the share's state"
            ),
        }
    }
}

/// How recently a share must have been used for `check_connection` to
/// redial it unasked: long enough for a trip to another app and back, short
/// enough that this morning's share is not dialled at noon.
const RECENT_USE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// How long a checked connection gets to answer. A live one answers in one
/// round trip, well under a second even on cellular; this only ever waits
/// on a dead one whose death the transport has not noticed yet.
const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

pub enum RefreshError {
    /// The host node has turned this device away for good. The store has
    /// the state.
    Terminal,
    Transient(anyhow::Error),
}

impl From<anyhow::Error> for RefreshError {
    fn from(e: anyhow::Error) -> Self {
        RefreshError::Transient(e)
    }
}

/// `GET /v1/share` over a fresh stream. `None` when the host node says the
/// share is unchanged since `known`. Also what `join` uses, before there is a
/// share to hang it on.
pub async fn fetch_info(stream: Stream, known: Option<ConfigHash>) -> Result<Option<ShareInfo>> {
    let mut req = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(wire::SHARE_PATH);
    if let Some(known) = known {
        req = req.header(hyper::header::IF_NONE_MATCH, known.etag());
    }
    let req = req
        .body(Full::new(bytes::Bytes::new()))
        .expect("static request is valid");
    let resp = guest_api_request(stream, req)
        .await
        .context("GET /v1/share (is waserver up to date?)")?;
    match resp.status() {
        StatusCode::NOT_MODIFIED => Ok(None),
        StatusCode::OK => Ok(Some(read_share(resp).await?)),
        status => anyhow::bail!("the host node answered {} to GET /v1/share", status),
    }
}

/// `POST /v1/activation` over a fresh stream on a connection whose key the host
/// node does not know yet: binds that key to the invite and returns the share.
/// A refusal carries the contract's reason.
pub async fn activate(stream: Stream, secret: &wire::InviteSecret) -> Result<ShareInfo> {
    let body = serde_json::to_vec(&wire::Activation { secret: *secret })?;
    let req = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri(wire::ACTIVATION_PATH)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(bytes::Bytes::from(body)))
        .expect("static request is valid");
    let resp = guest_api_request(stream, req)
        .await
        .context("POST /v1/activation")?;
    if resp.status() == StatusCode::OK {
        return read_share(resp).await;
    }
    let status = resp.status();
    let body = resp.into_body().collect().await?.to_bytes();
    match serde_json::from_slice::<wire::ApiError>(&body) {
        Ok(err) => anyhow::bail!("activation refused: {}", err.error),
        Err(_) => anyhow::bail!("the host node answered {} to the activation", status),
    }
}

/// `DELETE /v1/guest` over a fresh stream: this device leaves the share.
pub async fn leave(stream: Stream) -> Result<()> {
    let req = hyper::Request::builder()
        .method(hyper::Method::DELETE)
        .uri(wire::GUEST_PATH)
        .body(Full::new(bytes::Bytes::new()))
        .expect("static request is valid");
    let resp = guest_api_request(stream, req)
        .await
        .context("DELETE /v1/guest")?;
    match resp.status() {
        StatusCode::NO_CONTENT => Ok(()),
        status => anyhow::bail!("the host node answered {} to DELETE /v1/guest", status),
    }
}

/// One request to the guest API, on a CTRL stream that carries nothing else.
async fn guest_api_request(
    mut stream: Stream,
    mut req: hyper::Request<Full<bytes::Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>> {
    wire::open_ctrl_stream(&mut stream).await?;
    let (mut sender, conn) = http1_client::handshake(TokioIo::new(stream))
        .await
        .context("guest API handshake (is waserver up to date?)")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let headers = req.headers_mut();
    headers.insert(hyper::header::HOST, "waserver".parse().expect("valid"));
    headers.insert(hyper::header::CONNECTION, "close".parse().expect("valid"));
    Ok(sender.send_request(req).await?)
}

async fn read_share(resp: hyper::Response<hyper::body::Incoming>) -> Result<ShareInfo> {
    let body = resp
        .into_body()
        .collect()
        .await
        .context("reading the share")?
        .to_bytes();
    serde_json::from_slice(&body).context("parsing the share")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoObserver;
    use crate::transports::BoxFuture;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A transport whose host node never answers: streams open, then hang,
    /// and a check finds the connection dead.
    struct Silent {
        connected: Arc<AtomicBool>,
        dials: Arc<AtomicUsize>,
        /// The far ends, kept so the streams stay open rather than see EOF.
        far_ends: Mutex<Vec<tokio::io::DuplexStream>>,
    }

    impl Transport for Silent {
        fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
            Box::pin(async {
                if !self.connected.swap(true, Ordering::SeqCst) {
                    self.dials.fetch_add(1, Ordering::SeqCst);
                }
                let (near, far) = tokio::io::duplex(4096);
                self.far_ends.lock().expect("unpoisoned").push(far);
                Ok(Box::new(near) as Stream)
            })
        }

        fn check_connection(
            &self,
            _deadline: std::time::Duration,
        ) -> BoxFuture<'_, Result<ConnectionCheck, TransportError>> {
            Box::pin(async {
                // Nobody ever answers: a cached connection is found dead.
                if self.connected.swap(false, Ordering::SeqCst) {
                    Ok(ConnectionCheck::Dead)
                } else {
                    Ok(ConnectionCheck::NoConnection)
                }
            })
        }

        fn drop_connection(&self) {
            self.connected.store(false, Ordering::SeqCst);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_that_stays_silent_is_dropped_by_the_check() {
        let db = storage::DB::in_memory();
        let row = db.new_row().unwrap();
        row.write_deduped_hostname("rt").unwrap();
        row.mark_complete().unwrap();
        let connected = Arc::new(AtomicBool::new(false));
        let dials = Arc::new(AtomicUsize::new(0));
        let node = GuestNode::new(
            "rt".into(),
            row,
            Box::new(Silent {
                connected: connected.clone(),
                dials: dials.clone(),
                far_ends: Mutex::new(Vec::new()),
            }),
            Arc::new(NoObserver),
        );

        // Never in use: nothing to probe, no dial.
        node.check_connection().await;
        assert_eq!(dials.load(Ordering::SeqCst), 0);

        // A cached connection that never answers is dropped by the check
        // and, since the share was in use, redialled at once.
        drop(node.open_stream().await.ok().expect("a stream"));
        assert_eq!(dials.load(Ordering::SeqCst), 1);
        node.check_connection().await;
        assert_eq!(dials.load(Ordering::SeqCst), 2);
        assert!(connected.load(Ordering::SeqCst));

        // Dropped by the app (a network change) and checked on resume:
        // redialled without a probe.
        node.drop_connection();
        node.check_connection().await;
        assert_eq!(dials.load(Ordering::SeqCst), 3);

        // Long unused: left for the next request to dial.
        node.drop_connection();
        tokio::time::advance(RECENT_USE + std::time::Duration::from_secs(1)).await;
        node.check_connection().await;
        assert_eq!(dials.load(Ordering::SeqCst), 3);
    }
}
