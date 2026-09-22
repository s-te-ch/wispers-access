//! Share management logic.

use crate::storage::{self, ShareId};
use crate::transports::{Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::StatusCode;
use hyper::client::conn::http1 as http1_client;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wire::{App, ConfigHash, ShareInfo};
use wispers_access_wire as wire;

/// A share this client has joined. Manages share metadata (e.g. the list of
/// apps) and the transport used to communicate with the share's host node,
/// makes guest-API calls to the host node as needed.
pub struct Share {
    /// The `<share>` label in `<app>.<share>.localhost`.
    label: String,
    id: ShareId,
    display_name: String,
    row: storage::Row,
    transport: Box<dyn Transport>,
    /// Set once the transport reports a terminal failure mid-session. From
    /// then on the share answers without dialing.
    dead: Mutex<Option<TerminalState>>,
}

impl Share {
    pub fn new(
        label: String,
        id: ShareId,
        display_name: String,
        row: storage::Row,
        transport: Box<dyn Transport>,
    ) -> Self {
        Self {
            label,
            id,
            display_name,
            row,
            transport,
            dead: Mutex::new(None),
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The host node as the transport identifies it, for humans.
    pub fn describe_transport(&self) -> String {
        self.transport.describe()
    }

    /// The apps as last fetched from the host node.
    pub fn apps(&self) -> Result<Vec<App>> {
        self.row.read_apps()
    }

    /// Opens a DATA stream for one app, ready for the HTTP request.
    pub async fn open_data_stream(&self, app_id: &str) -> Result<Stream, TransportError> {
        let mut stream = self.open_stream().await?;
        wire::open_data_stream(&mut stream, app_id)
            .await
            .map_err(|e| TransportError::Transient(e.into()))?;
        Ok(stream)
    }

    /// Asks the host node for the app list if it changed since the stored
    /// one, and stores the answer. `None` = unchanged.
    pub async fn refresh(&self) -> Result<Option<ShareInfo>> {
        let stream = self.open_stream().await.map_err(|e| match e {
            TransportError::Terminal(state) => anyhow::anyhow!("{}", state.describe()),
            TransportError::Transient(e) => e,
        })?;
        let known = self.row.read_share_config_hash()?;
        let info = fetch_info(stream, known).await?;
        if let Some(info) = &info {
            self.row.write_share_info(info)?;
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
                eprintln!(
                    "[{}] share is no longer available — {}",
                    self.label,
                    state.describe()
                );
                let _ = self.row.write_terminal_state(state.as_str());
                *self.dead.lock().expect("unpoisoned") = Some(state);
                Err(TransportError::Terminal(state))
            }
            other => other,
        }
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

//-- Registry ------------------------------------------------------------------

/// ShareRegistry manages all shares this client has joined.
#[derive(Default)]
pub struct ShareRegistry {
    live: HashMap<String, Arc<Share>>,
    by_id: HashMap<String, String>,
    dead: HashMap<String, TerminalState>,
}

pub enum Lookup {
    Live(Arc<Share>),
    Dead(TerminalState),
    Unknown,
}

impl ShareRegistry {
    pub fn insert(&mut self, share: Share) {
        self.by_id.insert(share.id.to_string(), share.label.clone());
        self.live.insert(share.label.clone(), Arc::new(share));
    }

    pub fn insert_dead(&mut self, label: String, id: ShareId, state: TerminalState) {
        self.by_id.insert(id.to_string(), label.clone());
        self.dead.insert(label, state);
    }

    /// By label, or by share id.
    pub fn get(&self, key: &str) -> Lookup {
        let label = self.by_id.get(key).map(String::as_str).unwrap_or(key);
        if let Some(share) = self.live.get(label) {
            return Lookup::Live(share.clone());
        }
        if let Some(state) = self.dead.get(label) {
            return Lookup::Dead(*state);
        }
        Lookup::Unknown
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Share>> {
        self.live.values()
    }
}
