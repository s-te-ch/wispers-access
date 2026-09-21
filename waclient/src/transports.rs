//! Generic connectivity to the waserver. See *_transport for concrete
//! implementations for particular transports.

use crate::iroh_transport;
use crate::storage;
use crate::wispers_connect_transport;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
use wispers_access_wire as wire;

/// A bidirectional stream to the server, ready for the wire protocol.
pub type Stream = Box<dyn Bidirectional>;

/// `dyn` allows one non-auto trait, so `AsyncRead + AsyncWrite` cannot be
/// a trait object by itself. This bundles the two; the blanket impl below
/// makes every async read+write stream a `Bidirectional`.
pub trait Bidirectional: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Bidirectional for T {}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Joins the circle the invite is for, selecting the appropriate transport.
pub async fn join(invite: wire::Invite, row: &storage::Row) -> Result<wire::CircleInfo> {
    match invite {
        wire::Invite::WispersConnect {
            registration_token,
            activation_code,
            backend,
        } => {
            wispers_connect_transport::join(
                row,
                &registration_token,
                &activation_code,
                backend.as_deref(),
            )
            .await
        }
        wire::Invite::Iroh {
            endpoint_id,
            secret,
        } => iroh_transport::join(row, endpoint_id, &secret).await,
    }
}

/// Restores the transport for a circle's database row.
pub async fn restore(row: storage::Row) -> Result<Box<dyn Transport>, TransportError> {
    match row
        .read_transport_kind()
        .map_err(TransportError::Transient)?
    {
        wire::Transport::Iroh => Ok(Box::new(iroh_transport::Iroh::restore(&row).await?)),
        wire::Transport::WispersConnect => Ok(Box::new(
            wispers_connect_transport::WispersConnect::restore(row).await?,
        )),
        wire::Transport::Tailscale => Err(TransportError::Transient(anyhow::anyhow!(
            "tailscale circles are not supported by this waclient"
        ))),
    }
}

/// Releases whatever the transport holds beyond this device before the
/// circle is removed. Best effort: the row goes either way.
pub async fn leave(row: &storage::Row) -> Result<()> {
    match row.read_transport_kind()? {
        wire::Transport::Iroh => iroh_transport::leave(row).await,
        wire::Transport::WispersConnect => wispers_connect_transport::leave(row).await,
        wire::Transport::Tailscale => {}
    }
    Ok(())
}

/// One implementation per transport. Object-safe, so a registry can hold
/// circles on different transports; hence the boxed futures.
pub trait Transport: Send + Sync {
    /// Opens a fresh stream, connecting or reconnecting as needed. A passing
    /// failure is retried once; a final one is reported as such.
    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>>;

    /// How this transport identifies the server, for humans.
    fn describe(&self) -> String;
}

pub enum TransportError {
    /// The server side is gone for good; dialing again cannot help.
    Terminal(TerminalState),
    /// An outage or a broken connection. Worth another try later.
    Transient(anyhow::Error),
}

/// Why a circle is permanently unusable. `Removed` = the hub rejected our
/// credentials outright (circle deleted server-side); `Revoked` = this device
/// was revoked from the circle's roster.
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
            Self::Removed => "the circle was removed on the server side",
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
