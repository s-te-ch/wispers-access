//! Connectivity to waservers.

use crate::storage;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::OnceCell;
use wispers_connect as wc;

/// A bidirectional stream to the server, ready for the wire protocol.
pub type Stream = Box<dyn Bidirectional>;

/// `dyn` allows one non-auto trait, so `AsyncRead + AsyncWrite` cannot be
/// a trait object by itself. This bundles the two; the blanket impl below
/// makes every async read+write stream a `Bidirectional`.
pub trait Bidirectional: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Bidirectional for T {}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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

//-- Wispers Connect -----------------------------------------------------------

/// The Wispers Connect transport: a QUIC connection to the circle's server
/// node (always node 1), brokered by the hub.
pub struct WispersConnect {
    node: wc::Node,
    /// The live connection, established on first use. Replaced with a fresh
    /// cell when a stream fails to open on it, so the next caller redials.
    conn: Mutex<Arc<OnceCell<Arc<wc::QuicConnection>>>>,
}

impl WispersConnect {
    /// Restores the node from its stored state. A terminal error here means
    /// the hub has rejected this node for good.
    pub async fn restore(row: storage::Row, backend: Option<&str>) -> Result<Self, TransportError> {
        let ns = wc::NodeStorage::new(row);
        if let Some(backend) = backend {
            ns.override_hub_addr(backend);
        }
        let node = match ns.restore_or_init_node().await {
            Ok(node) => node,
            Err(e) => {
                return Err(match terminal_from_node_err(&e) {
                    Some(state) => TransportError::Terminal(state),
                    None => TransportError::Transient(e.into()),
                });
            }
        };
        if matches!(node.state(), wc::NodeState::Revoked) {
            return Err(TransportError::Terminal(TerminalState::Revoked));
        }
        Ok(Self::from_node(node))
    }

    /// For a node that was just activated, as in `join`.
    pub fn from_node(node: wc::Node) -> Self {
        Self {
            node,
            conn: Mutex::new(Arc::new(OnceCell::new())),
        }
    }

    async fn try_open_stream(&self) -> Result<Stream, TransportError> {
        let cell = self.conn.lock().expect("unpoisoned").clone();
        let conn = match cell
            .get_or_try_init(|| async {
                eprintln!("establishing QUIC connection");
                self.node.connect_quic(1).await.map(Arc::new)
            })
            .await
        {
            Ok(conn) => conn.clone(),
            Err(e) => {
                return Err(match terminal_from_p2p_err(&e) {
                    Some(state) => TransportError::Terminal(state),
                    None => TransportError::Transient(e.into()),
                });
            }
        };
        match conn.open_stream().await {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => {
                // The connection has broken: evict it so the next attempt
                // redials. Several tasks may race here, so only replace the
                // cell that failed.
                eprintln!("conn.open_stream failed, evicting connection: {:#}", e);
                let mut current = self.conn.lock().expect("unpoisoned");
                if Arc::ptr_eq(&*current, &cell) {
                    *current = Arc::new(OnceCell::new());
                }
                Err(TransportError::Transient(e.into()))
            }
        }
    }
}

impl Transport for WispersConnect {
    fn describe(&self) -> String {
        format!(
            "Wispers Connect group {}, node {}",
            self.node
                .connectivity_group_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "?".to_owned()),
            self.node
                .node_number()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".to_owned())
        )
    }

    fn open_stream(&self) -> BoxFuture<'_, Result<Stream, TransportError>> {
        Box::pin(async {
            // One retry covers a connection that died and had to be
            // re-established. A terminal rejection is not retried: it can
            // only repeat.
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

fn terminal_from_node_err(e: &wc::NodeStateError) -> Option<TerminalState> {
    if e.is_unauthenticated() || e.is_not_found() {
        return Some(TerminalState::Removed);
    }
    if e.is_revoked() {
        return Some(TerminalState::Revoked);
    }
    None
}

fn terminal_from_p2p_err(e: &wc::P2pError) -> Option<TerminalState> {
    match e {
        wc::P2pError::Revoked => Some(TerminalState::Revoked),
        wc::P2pError::Hub(h) if h.is_unauthenticated() || h.is_not_found() => {
            Some(TerminalState::Removed)
        }
        _ => None,
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

    #[test]
    fn p2p_revocation_is_terminal() {
        assert_eq!(
            terminal_from_p2p_err(&wc::P2pError::Revoked),
            Some(TerminalState::Revoked)
        );
        assert_eq!(terminal_from_p2p_err(&wc::P2pError::NotActivated), None);
    }
}
