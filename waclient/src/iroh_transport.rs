//! The iroh transport implementation.

use crate::circles;
use crate::storage;
use crate::transports::{BoxFuture, Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use wispers_access_wire as wire;

/// Joins the circle through iroh - creates an iroh endpoing, connects to the
/// circle's waserver, activates (i.e. redeems the invite code).
pub async fn join(
    row: &storage::Row,
    endpoint_id: wire::EndpointId,
    secret: &wire::InviteSecret,
) -> Result<wire::CircleInfo> {
    let server = iroh::EndpointId::from_bytes(&endpoint_id.0).context("invalid endpoint ID")?;
    let key = iroh::SecretKey::generate();
    let transport = Iroh::bind(key.to_bytes(), server).await?;
    println!("Connecting to the circle's server...");
    let stream = transport.open_stream().await.map_err(|e| match e {
        TransportError::Terminal(state) => anyhow::anyhow!("{}", state.describe()),
        TransportError::Transient(e) => e.context("connecting to the Wispers Access server"),
    })?;
    println!("Activating...");
    let info = circles::activate(stream, secret).await?;
    row.write_iroh_state(&storage::IrohState {
        secret_key: key.to_bytes(),
        server_endpoint_id: server.to_string(),
    })?;
    Ok(info)
}

/// How long to try telling the server we are leaving before giving up.
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Tells the server this device is leaving, so it stops listing us and
/// never serves this key again. Best effort: the server may be unreachable,
/// and the circle is removed locally either way.
pub async fn leave(row: &storage::Row) {
    let attempt = async {
        let transport = Iroh::restore(row).await.map_err(describe)?;
        let stream = transport.open_stream().await.map_err(describe)?;
        circles::leave(stream).await?;
        // Close properly, so the server sees us go rather than time out.
        transport.endpoint.close().await;
        Ok::<(), anyhow::Error>(())
    };
    match tokio::time::timeout(LEAVE_TIMEOUT, attempt).await {
        Ok(Ok(())) => println!("Told the server we are leaving."),
        Ok(Err(e)) => {
            println!("Could not tell the server we are leaving ({e:#}); removing locally anyway.")
        }
        Err(_) => println!("The server did not answer in time; removing locally anyway."),
    }
}

/// A transport error as a plain error, for paths where a terminal state is
/// just another reason the server could not be told.
fn describe(e: TransportError) -> anyhow::Error {
    match e {
        TransportError::Terminal(state) => anyhow::anyhow!("{}", state.describe()),
        TransportError::Transient(e) => e,
    }
}

/// The iroh transport: a QUIC connection to the server's endpoint, dialled
/// by its key with this device's own key, no control plane in between.
pub struct Iroh {
    endpoint: iroh::Endpoint,
    server: iroh::EndpointId,
    /// The live connection, established on first use. Replaced with a fresh
    /// cell when a stream fails to open on it, so the next caller redials.
    conn: Mutex<Arc<OnceCell<iroh::endpoint::Connection>>>,
}

impl Iroh {
    /// Binds an endpoint with the circle's stored key.
    pub async fn restore(row: &storage::Row) -> Result<Self, TransportError> {
        let state = row
            .read_iroh_state()
            .map_err(TransportError::Transient)?
            .context("circle has no iroh key")
            .map_err(TransportError::Transient)?;
        let server = state
            .server_endpoint_id
            .parse()
            .context("stored server endpoint ID is invalid")
            .map_err(TransportError::Transient)?;
        Self::bind(state.secret_key, server)
            .await
            .map_err(TransportError::Transient)
    }

    /// Binds an endpoint with `secret`, to reach `server`. `join` uses this
    /// with a freshly minted key before there is a row.
    pub async fn bind(secret: [u8; 32], server: iroh::EndpointId) -> Result<Self> {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::from_bytes(&secret))
            .bind()
            .await
            .context("binding the iroh endpoint")?;
        Ok(Self {
            endpoint,
            server,
            conn: Mutex::new(Arc::new(OnceCell::new())),
        })
    }

    async fn try_open_stream(&self) -> Result<Stream, TransportError> {
        let cell = self.conn.lock().expect("unpoisoned").clone();
        let conn = match cell
            .get_or_try_init(|| async {
                eprintln!("connecting to the server over iroh");
                self.endpoint.connect(self.server, wire::ALPN).await
            })
            .await
        {
            Ok(conn) => conn.clone(),
            Err(e) => return Err(TransportError::Transient(e.into())),
        };
        match conn.open_bi().await {
            Ok((send, recv)) => {
                // The server may have closed us right after the handshake;
                // a stream opens locally before that is noticed.
                if let Some(state) = conn.close_reason().as_ref().and_then(terminal_from_close) {
                    return Err(TransportError::Terminal(state));
                }
                Ok(Box::new(tokio::io::join(recv, send)))
            }
            Err(e) => {
                // The connection has broken: evict it so the next attempt
                // redials. Several tasks may race here, so only replace the
                // cell that failed.
                let mut current = self.conn.lock().expect("unpoisoned");
                if Arc::ptr_eq(&*current, &cell) {
                    *current = Arc::new(OnceCell::new());
                }
                Err(match terminal_from_close(&e) {
                    Some(state) => TransportError::Terminal(state),
                    None => TransportError::Transient(e.into()),
                })
            }
        }
    }
}

impl Transport for Iroh {
    fn describe(&self) -> String {
        format!("iroh endpoint {}", self.server)
    }

    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            match self.try_open_stream().await {
                Err(TransportError::Transient(e)) => {
                    eprintln!("open_stream attempt 1 failed, retrying once: {:#}", e);
                    self.try_open_stream().await
                }
                other => other,
            }
        })
    }
}

/// The contract's close codes, as the server they were dialled from sends
/// them: only those may end a circle for good.
fn terminal_from_close(e: &iroh::endpoint::ConnectionError) -> Option<TerminalState> {
    let iroh::endpoint::ConnectionError::ApplicationClosed(close) = e else {
        return None;
    };
    match wire::CloseCode::from_code(close.error_code.into_inner()) {
        Some(wire::CloseCode::Unknown) => Some(TerminalState::Removed),
        Some(wire::CloseCode::Revoked) => Some(TerminalState::Revoked),
        _ => None,
    }
}
