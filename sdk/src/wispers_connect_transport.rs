//! The Wispers Connect transport implementation.

use crate::guest_node;
use crate::secrets::{SecretStore, SecretStoreError};
use crate::storage::{self, ShareId};
use crate::transports::{BoxFuture, Stream, TerminalState, Transport, TransportError};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;
use tracing::{info, warn};
use wispers_access_wire as wire;
use wispers_connect as wc;

/// The secret store keys of a share's node state: the root key the node
/// minted, and its registration with the hub.
const ROOT_KEY: &str = "root_key";
const REGISTRATION: &str = "registration";

/// Registers this device as a node of the share's connectivity group and
/// activates it. A failure after registration logs the node out again, so
/// no registration is orphaned on the hub.
pub async fn join(
    row: &storage::Row,
    secrets: &Arc<dyn SecretStore>,
    registration_token: &str,
    activation_code: &str,
    backend: Option<&str>,
) -> Result<wire::ShareInfo> {
    // Register the Wispers node. If the invite named a self-hosted backend,
    // use override_hub_addr().
    let ns = wc::NodeStorage::new(NodeSecrets::new(row, secrets)?);
    if let Some(backend) = backend {
        info!(backend, "using a self-hosted Wispers Connect backend");
        ns.override_hub_addr(backend);
    }
    let mut node = ns.restore_or_init_node().await?;
    info!("registering the Wispers node");
    node.register(registration_token).await?;

    // From here on the hub holds a registration that consumes quota, so
    // a failed join must log the node out again (revoke + deregister) rather
    // than orphan the registration. This is best-effort.
    match activate_and_fetch(&mut node, row, activation_code, backend).await {
        Ok(info) => Ok(info),
        Err(e) => {
            match node.logout().await {
                Ok(()) => info!("join failed; deregistered from the hub again"),
                Err(le) => {
                    warn!(error = %le, "join failed; could not deregister from the hub either")
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
    info!("activating the Wispers node");
    node.activate(activation_code).await?;

    info!("fetching the share from its host node");
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
    row.write_wispers_connect_backend(backend)?;
    Ok(info)
}

/// Deregisters from the hub, best effort: for a removed share the hub
/// already rejects us, and for a revoked one logout cleanly retires the
/// zombie registration.
pub async fn leave(row: &storage::Row, secrets: &Arc<dyn SecretStore>) {
    let attempt = async {
        let backend = row.read_wispers_connect_backend()?;
        let store = NodeSecrets::new(row, secrets)?;
        let ns = wc::NodeStorage::new(store);
        if let Some(backend) = backend.as_deref() {
            ns.override_hub_addr(backend);
        }
        let mut node = ns.restore_or_init_node().await?;
        node.logout().await?;
        Ok::<(), anyhow::Error>(())
    };
    match attempt.await {
        Ok(()) => info!("deregistered from the hub"),
        Err(e) => warn!(
            error = format!("{e:#}"),
            "could not deregister from the hub; removing locally anyway"
        ),
    }
    // Logout deletes the node state on success; make sure of it either way.
    match NodeSecrets::new(row, secrets) {
        Ok(store) => {
            if let Err(e) = store.delete_all() {
                warn!(error = %e, "could not delete the share's node state");
            }
        }
        Err(e) => warn!(
            error = format!("{e:#}"),
            "could not delete the share's node state"
        ),
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
    pub async fn restore(
        row: storage::Row,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, TransportError> {
        let backend = row
            .read_wispers_connect_backend()
            .map_err(TransportError::Transient)?;
        let store = NodeSecrets::new(&row, &secrets).map_err(TransportError::Transient)?;
        let ns = wc::NodeStorage::new(store);
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
                info!("establishing the QUIC connection");
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
                warn!(
                    error = format!("{e:#}"),
                    "opening a stream failed, evicting the connection"
                );
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

/// The node's persisted state, in the secret store under the share's id.
/// What wispers-connect reads and writes through `NodeStateStore`.
struct NodeSecrets {
    secrets: Arc<dyn SecretStore>,
    share: ShareId,
}

impl NodeSecrets {
    fn new(row: &storage::Row, secrets: &Arc<dyn SecretStore>) -> Result<Self> {
        Ok(Self {
            secrets: secrets.clone(),
            share: row.share_id()?,
        })
    }

    fn delete_all(&self) -> Result<(), SecretStoreError> {
        self.secrets
            .delete(self.share.clone(), ROOT_KEY.to_owned())?;
        self.secrets
            .delete(self.share.clone(), REGISTRATION.to_owned())
    }
}

impl wc::NodeStateStore for NodeSecrets {
    fn load(&self) -> Result<Option<wc::PersistedNodeState>, wc::StorageError> {
        let Some(root_key) = self
            .secrets
            .load(self.share.clone(), ROOT_KEY.to_owned())
            .map_err(to_wc_error)?
        else {
            // No root key: nothing has been saved yet.
            return Ok(None);
        };
        let key: [u8; wc::ROOT_KEY_LEN] = root_key
            .try_into()
            .map_err(|_| wc::StorageError::InvalidRootKey)?;
        let registration = self
            .secrets
            .load(self.share.clone(), REGISTRATION.to_owned())
            .map_err(to_wc_error)?
            .and_then(|b| wc::deserialize_registration(&b).ok());
        Ok(Some(wc::PersistedNodeState::from_stored(key, registration)))
    }

    fn save(&self, state: &wc::PersistedNodeState) -> Result<(), wc::StorageError> {
        self.secrets
            .save(
                self.share.clone(),
                ROOT_KEY.to_owned(),
                state.root_key_bytes().to_vec(),
            )
            .map_err(to_wc_error)?;
        match state.registration() {
            Some(registration) => self
                .secrets
                .save(
                    self.share.clone(),
                    REGISTRATION.to_owned(),
                    wc::serialize_registration(registration),
                )
                .map_err(to_wc_error),
            None => self
                .secrets
                .delete(self.share.clone(), REGISTRATION.to_owned())
                .map_err(to_wc_error),
        }
    }

    fn delete(&self) -> Result<(), wc::StorageError> {
        self.delete_all().map_err(to_wc_error)
    }
}

fn to_wc_error(e: SecretStoreError) -> wc::StorageError {
    wc::StorageError::Io(std::io::Error::other(e))
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
