//! The iroh transport implementation.

use crate::guest_node;
use crate::secrets::SecretStore;
use crate::storage;
use crate::transports::{BoxFuture, Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{info, warn};
use wispers_access_wire as wire;

/// The secret store key of this device's Ed25519 key for a share; its
/// public key is what the host node bound to the invite.
const SECRET_KEY: &str = "iroh_secret";

/// Joins the share through iroh - creates an iroh endpoint, connects to the
/// share's host node, activates (i.e. redeems the invite code).
pub async fn join(
    row: &storage::Row,
    secrets: &Arc<dyn SecretStore>,
    endpoint_id: wire::EndpointId,
    secret: &wire::InviteSecret,
) -> Result<wire::ShareInfo> {
    let host = iroh::EndpointId::from_bytes(&endpoint_id.0).context("invalid endpoint ID")?;
    let key = iroh::SecretKey::generate();
    let transport = Iroh::bind(key.to_bytes(), host).await?;
    info!("connecting to the share's host node");
    let stream = transport.open_stream().await.map_err(|e| match e {
        TransportError::Terminal(state) => anyhow::anyhow!("{}", state.describe()),
        TransportError::Transient(e) => e.context("connecting to the Wispers Access host"),
    })?;
    info!("activating");
    let info = guest_node::activate(stream, secret).await?;
    // The first use of the share restores a transport of its own; this
    // one has done its job. Close it properly, iroh complains otherwise.
    transport.endpoint.close().await;
    row.write_iroh_endpoint_id(&host.to_string())?;
    secrets.save(
        row.share_id()?,
        SECRET_KEY.to_owned(),
        key.to_bytes().to_vec(),
    )?;
    Ok(info)
}

/// How long to try telling the host node we are leaving before giving up.
const LEAVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Tells the host node this device is leaving, so it stops listing us and
/// never serves this key again. Best effort: the host may be unreachable,
/// and the share is removed locally either way.
pub async fn leave(row: &storage::Row, secrets: &Arc<dyn SecretStore>) {
    let attempt = async {
        let transport = Iroh::restore(row, secrets).await.map_err(describe)?;
        let told = async {
            let stream = transport.open_stream().await.map_err(describe)?;
            guest_node::leave(stream).await
        }
        .await;
        // Close properly either way, so the host node sees us go rather than
        // time out, and iroh has nothing to complain about.
        transport.endpoint.close().await;
        told
    };
    match tokio::time::timeout(LEAVE_TIMEOUT, attempt).await {
        Ok(Ok(())) => info!("told the host node we are leaving"),
        Ok(Err(e)) => {
            warn!(
                error = format!("{e:#}"),
                "could not tell the host node we are leaving; removing locally anyway"
            )
        }
        Err(_) => warn!("the host node did not answer in time; removing locally anyway"),
    }
    match row.share_id() {
        Ok(share) => {
            if let Err(e) = secrets.delete(share, SECRET_KEY.to_owned()) {
                warn!(error = %e, "could not delete the share's iroh key");
            }
        }
        Err(e) => warn!(
            error = format!("{e:#}"),
            "could not delete the share's iroh key"
        ),
    }
}

/// A transport error as a plain error, for paths where a terminal state is
/// just another reason the host node could not be told.
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
    host: iroh::EndpointId,
    /// The live connection, established on first use. Replaced with a fresh
    /// cell when a stream fails to open on it, so the next caller redials.
    conn: Mutex<Arc<OnceCell<iroh::endpoint::Connection>>>,
}

impl Iroh {
    /// Binds an endpoint with the share's stored key.
    pub async fn restore(
        row: &storage::Row,
        secrets: &Arc<dyn SecretStore>,
    ) -> Result<Self, TransportError> {
        let restored = async {
            let host: iroh::EndpointId = row
                .read_iroh_endpoint_id()?
                .context("share has no iroh host")?
                .parse()
                .context("stored host endpoint ID is invalid")?;
            let secret: [u8; 32] = secrets
                .load(row.share_id()?, SECRET_KEY.to_owned())?
                .context("share has no iroh key")?
                .try_into()
                .map_err(|_| anyhow::anyhow!("stored iroh key has the wrong length"))?;
            Self::bind(secret, host).await
        };
        restored.await.map_err(TransportError::Transient)
    }

    /// Binds an endpoint with `secret`, to reach `host`. `join` uses this
    /// with a freshly minted key before there is a row.
    pub async fn bind(secret: [u8; 32], host: iroh::EndpointId) -> Result<Self> {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::from_bytes(&secret))
            .bind()
            .await
            .context("binding the iroh endpoint")?;
        Ok(Self {
            endpoint,
            host,
            conn: Mutex::new(Arc::new(OnceCell::new())),
        })
    }

    async fn try_open_stream(&self) -> Result<Stream, TransportError> {
        let cell = self.conn.lock().expect("unpoisoned").clone();
        let conn = match cell
            .get_or_try_init(|| async {
                info!("connecting to the host node over iroh");
                self.endpoint.connect(self.host, wire::ALPN).await
            })
            .await
        {
            Ok(conn) => conn.clone(),
            Err(e) => return Err(TransportError::Transient(e.into())),
        };
        match conn.open_bi().await {
            Ok((send, recv)) => {
                // The host node may have closed us right after the handshake;
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
    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            match self.try_open_stream().await {
                Err(TransportError::Transient(e)) => {
                    warn!(
                        error = format!("{e:#}"),
                        "opening a stream failed, retrying once"
                    );
                    self.try_open_stream().await
                }
                other => other,
            }
        })
    }
}

/// The contract's close codes, as the host node they were dialled from sends
/// them: only those may end a share for good.
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
