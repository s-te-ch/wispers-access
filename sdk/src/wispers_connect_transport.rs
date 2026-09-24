//! The Wispers Connect transport implementation.

use crate::guest_node;
use crate::storage;
use crate::transports::{BoxFuture, Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;
use wispers_access_wire as wire;
use wispers_connect as wc;

/// Registers this device as a node of the share's connectivity group and
/// activates it. A failure after registration logs the node out again, so
/// no registration is orphaned on the hub.
pub async fn join(
    row: &storage::Row,
    registration_token: &str,
    activation_code: &str,
    backend: Option<&str>,
) -> Result<wire::ShareInfo> {
    // Register the Wispers node. If the invite named a self-hosted backend,
    // use override_hub_addr().
    let ns = wc::NodeStorage::new(row.clone());
    if let Some(backend) = backend {
        println!("Using Wispers Connect backend: {}", backend);
        ns.override_hub_addr(backend);
    }
    let mut node = ns.restore_or_init_node().await?;
    println!("Registering Wispers node...");
    node.register(registration_token).await?;

    // From here on the hub holds a registration that consumes quota, so
    // a failed join must log the node out again (revoke + deregister) rather
    // than orphan the registration. This is best-effort.
    match activate_and_fetch(&mut node, row, activation_code, backend).await {
        Ok(info) => Ok(info),
        Err(e) => {
            match node.logout().await {
                Ok(()) => eprintln!("Join failed; deregistered from the hub again."),
                Err(le) => {
                    eprintln!("Join failed; could not deregister from the hub either ({le}).")
                }
            }
            Err(e)
        }
    }
}

/// The steps after registration: activation and asking the host node what the
/// share is. Any failure here makes `join` roll the registration back.
async fn activate_and_fetch(
    node: &mut wc::Node,
    row: &storage::Row,
    activation_code: &str,
    backend: Option<&str>,
) -> Result<wire::ShareInfo> {
    println!("Activating Wispers node...");
    node.activate(activation_code).await?;

    println!("Fetching the share from its host node...");
    // Straight on the node rather than through a transport: keeps the node
    // for a rollback.
    let conn = node
        .connect_quic(1)
        .await
        .context("connecting to the host node")?;
    let stream = conn.open_stream().await.context("opening a stream")?;
    let info = guest_node::fetch_info(Box::new(stream), None)
        .await?
        .context("the host node answered 304 to an unconditional request")?;
    row.write_backend(backend)?;
    Ok(info)
}

/// Deregisters from the hub, best effort: for a removed share the hub
/// already rejects us, and for a revoked one logout cleanly retires the
/// zombie registration.
pub async fn leave(row: &storage::Row) {
    let backend = match row.read_backend() {
        Ok(backend) => backend,
        Err(e) => {
            println!("Could not read the share ({e}); removing locally anyway.");
            return;
        }
    };
    let ns = wc::NodeStorage::new(row.clone());
    if let Some(backend) = backend.as_deref() {
        ns.override_hub_addr(backend);
    }
    match ns.restore_or_init_node().await {
        Ok(mut node) => match node.logout().await {
            Ok(()) => println!("Deregistered from the hub."),
            Err(e) => println!("Could not deregister from the hub ({e}); removing locally anyway."),
        },
        Err(e) => println!("Could not restore the node ({e}); removing locally anyway."),
    }
}

/// The Wispers Connect transport: a QUIC connection to the share's host node
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
    pub async fn restore(row: storage::Row) -> Result<Self, TransportError> {
        let backend = row.read_backend().map_err(TransportError::Transient)?;
        let ns = wc::NodeStorage::new(row);
        if let Some(backend) = backend.as_deref() {
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
    fn p2p_revocation_is_terminal() {
        assert_eq!(
            terminal_from_p2p_err(&wc::P2pError::Revoked),
            Some(TerminalState::Revoked)
        );
        assert_eq!(terminal_from_p2p_err(&wc::P2pError::NotActivated), None);
    }
}
