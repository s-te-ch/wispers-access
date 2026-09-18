//! Circle management logic.

use crate::storage;
use crate::transports::{Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Empty};
use hyper::StatusCode;
use hyper::client::conn::http1 as http1_client;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use wire::{CircleInfo, ConfigHash, Share};
use wispers_access_wire as wire;

/// A circle this client has joined. Manages circle metadata (e.g. the list of
/// shares) and the transport used to communicate to the circle's server, makes
/// guest-API calls to the server as needed.
pub struct Circle {
    /// The `<circle>` label in `<share>.<circle>.localhost`.
    label: String,
    id: String,
    display_name: String,
    row: storage::Row,
    transport: Box<dyn Transport>,
    /// Set once the transport reports a terminal failure mid-session. From
    /// then on the circle answers without dialing.
    dead: Mutex<Option<TerminalState>>,
}

impl Circle {
    pub fn new(
        label: String,
        id: String,
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

    /// The shares as last fetched from the server.
    pub fn shares(&self) -> Result<Vec<Share>> {
        self.row.read_shares()
    }

    /// Opens a DATA stream for one share, ready for the HTTP request.
    pub async fn open_data_stream(&self, share_id: &str) -> Result<Stream, TransportError> {
        let mut stream = self.open_stream().await?;
        wire::open_data_stream(&mut stream, share_id)
            .await
            .map_err(|e| TransportError::Transient(e.into()))?;
        Ok(stream)
    }

    /// Asks the server for the share list if it changed since the stored
    /// one, and stores the answer. `None` = unchanged.
    pub async fn refresh(&self) -> Result<Option<CircleInfo>> {
        let stream = self.open_stream().await.map_err(|e| match e {
            TransportError::Terminal(state) => anyhow::anyhow!("{}", state.describe()),
            TransportError::Transient(e) => e,
        })?;
        let known = self.row.read_circle_config_hash()?;
        let info = fetch_info(stream, known).await?;
        if let Some(info) = &info {
            self.row.write_circle_info(info)?;
        }
        Ok(info)
    }

    /// A stream from the transport, unless the circle is known to be dead.
    /// A terminal failure is recorded here, once, for this run and the next.
    async fn open_stream(&self) -> Result<Stream, TransportError> {
        if let Some(state) = *self.dead.lock().expect("unpoisoned") {
            return Err(TransportError::Terminal(state));
        }
        match self.transport.open_stream().await {
            Err(TransportError::Terminal(state)) => {
                eprintln!(
                    "[{}] circle is no longer available — {}",
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

/// `GET /v1/circle` over a fresh stream. `None` when the server says the
/// circle is unchanged since `known`. Also what `join` uses, before there is
/// a circle to hang it on.
pub async fn fetch_info(
    mut stream: Stream,
    known: Option<ConfigHash>,
) -> Result<Option<CircleInfo>> {
    wire::open_ctrl_stream(&mut stream).await?;
    let (mut sender, conn) = http1_client::handshake(TokioIo::new(stream))
        .await
        .context("guest API handshake (is waserver up to date?)")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut req = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(wire::CIRCLE_PATH)
        .header(hyper::header::HOST, "waserver")
        .header(hyper::header::CONNECTION, "close");
    if let Some(known) = known {
        req = req.header(hyper::header::IF_NONE_MATCH, known.etag());
    }
    let req = req
        .body(Empty::<bytes::Bytes>::new())
        .expect("static request is valid");
    let resp = sender
        .send_request(req)
        .await
        .context("GET /v1/circle (is waserver up to date?)")?;
    match resp.status() {
        StatusCode::NOT_MODIFIED => Ok(None),
        StatusCode::OK => {
            let body = resp
                .into_body()
                .collect()
                .await
                .context("reading the circle")?
                .to_bytes();
            let info: CircleInfo = serde_json::from_slice(&body).context("parsing the circle")?;
            Ok(Some(info))
        }
        status => anyhow::bail!("server answered {} to GET /v1/circle", status),
    }
}

//-- Registry ------------------------------------------------------------------

/// CircleRegistry manages all circles this client has joined.
#[derive(Default)]
pub struct CircleRegistry {
    live: HashMap<String, Arc<Circle>>,
    by_id: HashMap<String, String>,
    dead: HashMap<String, TerminalState>,
}

pub enum Lookup {
    Live(Arc<Circle>),
    Dead(TerminalState),
    Unknown,
}

impl CircleRegistry {
    pub fn insert(&mut self, circle: Circle) {
        self.by_id.insert(circle.id.clone(), circle.label.clone());
        self.live.insert(circle.label.clone(), Arc::new(circle));
    }

    pub fn insert_dead(&mut self, label: String, id: String, state: TerminalState) {
        self.by_id.insert(id, label.clone());
        self.dead.insert(label, state);
    }

    /// By label, or by connectivity group id.
    pub fn get(&self, key: &str) -> Lookup {
        let label = self.by_id.get(key).map(String::as_str).unwrap_or(key);
        if let Some(circle) = self.live.get(label) {
            return Lookup::Live(circle.clone());
        }
        if let Some(state) = self.dead.get(label) {
            return Lookup::Dead(*state);
        }
        Lookup::Unknown
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Circle>> {
        self.live.values()
    }
}
