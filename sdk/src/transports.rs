//! Generic connectivity to the waserver. See *_transport for concrete
//! implementations for particular transports.

use crate::iroh_transport;
use crate::secrets::SecretStore;
use crate::storage;
use crate::wispers_connect_transport;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use wispers_access_wire as wire;

/// A bidirectional stream to the host node, ready for the wire protocol.
pub type Stream = Box<dyn Bidirectional>;

/// `dyn` allows one non-auto trait, so `AsyncRead + AsyncWrite` cannot be
/// a trait object by itself. This bundles the two; the blanket impl below
/// makes every async read+write stream a `Bidirectional`.
pub trait Bidirectional: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Bidirectional for T {}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Joins the share the invite is for, selecting the appropriate transport.
pub async fn join(
    invite: wire::Invite,
    row: &storage::Row,
    secrets: &Arc<dyn SecretStore>,
) -> Result<wire::ShareInfo> {
    match invite {
        wire::Invite::WispersConnect {
            registration_token,
            activation_code,
            backend,
        } => {
            wispers_connect_transport::join(
                row,
                secrets,
                &registration_token,
                &activation_code,
                backend.as_deref(),
            )
            .await
        }
        wire::Invite::Iroh {
            endpoint_id,
            secret,
        } => iroh_transport::join(row, secrets, endpoint_id, &secret).await,
    }
}

/// Restores the transport for a share's database row.
pub async fn restore(
    row: storage::Row,
    secrets: Arc<dyn SecretStore>,
) -> Result<Box<dyn Transport>, TransportError> {
    match row
        .read_transport_kind()
        .map_err(TransportError::Transient)?
    {
        wire::Transport::Iroh => Ok(Box::new(
            iroh_transport::Iroh::restore(&row, &secrets).await?,
        )),
        wire::Transport::WispersConnect => Ok(Box::new(
            wispers_connect_transport::WispersConnect::restore(row, secrets).await?,
        )),
        wire::Transport::Tailscale => Err(TransportError::Transient(anyhow::anyhow!(
            "tailscale shares are not supported by this waclient"
        ))),
    }
}

/// Releases whatever the transport holds beyond this device before the
/// share is removed, and deletes the share's secrets. Best effort on the
/// far side: the row goes either way.
pub async fn leave(row: &storage::Row, secrets: &Arc<dyn SecretStore>) -> Result<()> {
    match row.read_transport_kind()? {
        wire::Transport::Iroh => iroh_transport::leave(row, secrets).await,
        wire::Transport::WispersConnect => wispers_connect_transport::leave(row, secrets).await,
        wire::Transport::Tailscale => {}
    }
    Ok(())
}

/// One implementation per transport. Object-safe, so a registry can hold
/// shares on different transports; hence the boxed futures.
pub trait Transport: Send + Sync {
    /// Opens a stream to the host node, opening the connection or reusing a
    /// cached one as needed. Fails if retrying the connection fails.
    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;

    /// Run a transport-level ping to check the connection's health, with the
    /// given deadline. Returns
    /// - `Alive` if the connection is alive and well,
    /// - `Dead` if an existing connection died (in which case it's also dropped),
    /// - `NoConnection` if there was no cached connection to check.
    ///
    /// A terminal close code by the host is reported as such, as it is from
    /// `open_stream`.
    fn check_connection(
        &self,
        deadline: Duration,
    ) -> BoxFuture<'_, Result<ConnectionCheck, TransportError>>;

    /// Drops the cached connection, if any.
    fn drop_connection(&self);
}

/// What `Transport::check_connection` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionCheck {
    /// Nothing cached; nothing was dialled.
    NoConnection,
    /// The cached connection answered.
    Alive,
    /// The cached connection did not answer, or was closed; it is dropped.
    Dead,
}

pub enum TransportError {
    /// The host node is gone for good; dialing again cannot help.
    Terminal(TerminalState),
    /// An outage or a broken connection. Worth another try later.
    Transient(anyhow::Error),
}

/// Why a share is permanently unusable. `Removed` = the hub rejected our
/// credentials outright (share deleted on the host node); `Revoked` = this
/// device was revoked from the share's roster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalState {
    Removed,
    Revoked,
}

impl TerminalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "removed" => Some(Self::Removed),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Removed => "the share was removed by its host node",
            Self::Revoked => "this device's access was revoked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_round_trip_through_storage_form() {
        for state in [TerminalState::Removed, TerminalState::Revoked] {
            assert_eq!(TerminalState::parse(state.as_str()), Some(state));
        }
        assert_eq!(TerminalState::parse("gone"), None);
    }
}
