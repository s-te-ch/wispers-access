//! This device's node in one share, and the guest API it speaks.

use crate::Observer;
use crate::storage;
use crate::transports::{Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::StatusCode;
use hyper::client::conn::http1 as http1_client;
use hyper_util::rt::TokioIo;
use std::sync::{Arc, Mutex};
use tracing::warn;
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
        }
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

    /// A stream from the transport, unless the share is known to be dead.
    /// A terminal failure is recorded here, once, for this run and the next.
    async fn open_stream(&self) -> Result<Stream, TransportError> {
        if let Some(state) = *self.dead.lock().expect("unpoisoned") {
            return Err(TransportError::Terminal(state));
        }
        match self.transport.open_stream().await {
            Err(TransportError::Terminal(state)) => {
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
                Err(TransportError::Terminal(state))
            }
            other => other,
        }
    }
}

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
