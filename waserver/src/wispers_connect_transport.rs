//! The Wispers Connect transport implementation.

use crate::initialization::Rollback;
use crate::ipc;
use crate::protocol::{self, Peer};
use crate::serving::{self, BoxFuture, Closer, ExitReason, ServingHandle, shutdown_signal};
use crate::status::{GuestStatus, InviteStatus, TransportReport, TransportStatus, invite_status};
use crate::storage;
use crate::wcbe;
use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::{RwLock, watch};
use tracing::{error, info, warn};
use wispers_access_wire as wire;
use wispers_connect as wc;

//-- Share initialisation ------------------------------------------------------

/// `init` sets up a new share with Wispers Connect as the transport.
pub async fn init(
    rollback: &mut Rollback,
    dir: &storage::ShareDir,
    config_text: &str,
    api_key: Option<&str>,
    display_name: &str,
    backend: Option<&str>,
) -> Result<()> {
    let Some(api_key) = api_key else {
        anyhow::bail!("--api-key (or WC_API_KEY) is required for wispers-connect");
    };
    let wcbe_client = wcbe::Client::new(api_key, &wcbe::api_base(backend));

    let cg_id = wcbe_client
        .add_connectivity_group(display_name)
        .await
        .map_err(explain_group_quota)?;
    rollback.push("connectivity group", {
        let (client, cg_id) = (wcbe_client.clone(), cg_id.clone());
        async move { client.remove_connectivity_group(&cg_id).await }
    });

    let state = dir.create(config_text, storage::TransportKind::WispersConnect)?;
    rollback.push("share directory", {
        let dir = dir.clone();
        async move { dir.delete().map_err(Into::into) }
    });
    state.set_wispers_connect_state(&storage::WispersConnectState {
        api_key: api_key.to_owned(),
        connectivity_group_id: cg_id.clone(),
        backend: backend.map(str::to_owned),
    })?;

    // Create the serving Wispers node and register it with the backend. The
    // registration goes away with the group, so it needs no undo of its own.
    let node_storage = wc::NodeStorage::new(state);
    if let Some(backend) = backend {
        node_storage.override_hub_addr(backend);
    }
    let mut node = node_storage.restore_or_init_node().await?;
    let token = wcbe_client
        .get_registration_token(&cg_id, Some("Host"), None /* metadata */)
        .await?;
    node.register(&token).await.context("registration failed")?;
    Ok(())
}

/// Group creation is where the plan's connectivity-group quota bites.
/// Make the error actionable.
fn explain_group_quota(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<wcbe::QuotaExceeded>() {
        Some(q) if q.quota == "groups_per_domain" => anyhow::anyhow!(
            "cannot create a new share: your plan's connectivity-group quota \
             is used up ({} of {}). Delete an unused share with `waserver \
             deinit <share>` or upgrade your plan.",
            q.current,
            q.limit
        ),
        _ => e,
    }
}

/// Removes the connectivity group, which deregisters every node. A share
/// whose `init` never got that far has nothing to remove.
pub async fn deinit(wcs: Option<storage::WispersConnectState>) -> Result<()> {
    let Some(wcs) = wcs else {
        return Ok(());
    };
    wcbe::Client::new(&wcs.api_key, &wcbe::api_base(wcs.backend.as_deref()))
        .remove_connectivity_group(&wcs.connectivity_group_id)
        .await
}

//-- HostNode implementation ---------------------------------------------------

/// The host node is always the first node of its connectivity group: `init`
/// registers it before any invite exists. Guests dial it by this number.
pub const HOST_NODE_NUMBER: i32 = 1;

/// Host node implementation for Wispers Connect.
pub struct HostNode {
    node: Arc<wc::Node>,
    wcbe_client: wcbe::Client,
    connectivity_group_id: String,
    /// The self-hosted backend.
    backend: Option<String>,
    /// wispers-connect's handle to the live hub session (its own
    /// `ServingHandle`, unrelated to ours). `None` while `run` is still
    /// connecting but IPC is already up.
    hub_session: RwLock<Option<wc::ServingHandle>>,
}

pub async fn bind(share: &str, state: storage::StateDb) -> Result<HostNode> {
    let Some(wcs) = state.wispers_connect_state()? else {
        anyhow::bail!("Share {} has no Wispers Connect credentials", share);
    };
    let node_storage = wc::NodeStorage::new(state);
    if let Some(backend) = wcs.backend.as_deref() {
        node_storage.override_hub_addr(backend);
    }
    let node = Arc::new(node_storage.restore_or_init_node().await?);
    if !node.is_registered() {
        anyhow::bail!("Wispers Connect node is not registered");
    }
    let Some(cg_id) = node.connectivity_group_id() else {
        anyhow::bail!("host node not registered");
    };
    Ok(HostNode {
        connectivity_group_id: cg_id.to_string(),
        wcbe_client: wcbe::Client::new(&wcs.api_key, &wcbe::api_base(wcs.backend.as_deref())),
        backend: wcs.backend,
        node,
        hub_session: RwLock::new(None),
    })
}

impl serving::HostNode for HostNode {
    fn run(&self, serving_handle: ServingHandle) -> BoxFuture<'_, Result<ExitReason>> {
        Box::pin(self.serve(serving_handle))
    }

    fn invite<'a>(
        &'a self,
        node_name: &'a str,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<wire::Invite>> {
        Box::pin(self.mint_invite(node_name, user_id))
    }

    fn revoke_guest(&self, number: i64) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let node_number =
                i32::try_from(number).map_err(|_| anyhow::anyhow!("no node {number}"))?;
            revoke_on_hub(&self.node, &self.wcbe_client, node_number).await
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.stop_hub_session())
    }
}

impl HostNode {
    /// Connects to the hub and serves until the session ends or a signal.
    async fn serve(&self, serving_handle: ServingHandle) -> Result<ExitReason> {
        let (session_handle, session, mut incoming) = self
            .node
            .start_serving()
            .await
            .context("starting Wispers node serving loop")?;
        *self.hub_session.write().await = Some(session_handle);
        serving_handle.set_reachable().await;
        info!("Connected to hub");

        // Run the Wispers serving session.
        let mut session_task = tokio::spawn(async move { session.run().await });

        // Resolves on SIGTERM/SIGINT so we tear the hub session down
        // cleanly instead of being killed mid-flight (e.g. by a container
        // supervisor).
        let shutdown = shutdown_signal();
        tokio::pin!(shutdown);

        // Main serving loop. Accept connections from the peer nodes or
        // from IPC.
        loop {
            tokio::select! {
                // Incoming QUIC connection.
                Some(result) = incoming.quic.recv() => {
                    tokio::spawn(handle_quic_conn(
                        result,
                        self.node.clone(),
                        serving_handle.clone(),
                    ));
                },
                // Session end.
                result = &mut session_task => {
                    break handle_session_end(result).map(|()| ExitReason::Stopped)
                }
                // Shutdown signal: stop the session the same way
                // `waserver stop` does, drain it, then exit cleanly. This
                // stop was intended, so we report success (exit 0)
                // regardless of how the session terminates.
                _ = &mut shutdown => {
                    info!("shutdown signal received; shutting down gracefully");
                    if let Err(e) = serving_handle.shutdown().await {
                        warn!(error = format!("{:#}", e), "error during graceful shutdown");
                    }
                    let _ = (&mut session_task).await;
                    break Ok(ExitReason::Signal);
                }
            }
        }
    }

    /// A registration token from the hub plus an activation code from the
    /// live session.
    async fn mint_invite(&self, node_name: &str, user_id: &str) -> Result<wire::Invite> {
        let metadata = wcbe::NodeMetadata {
            user_id: user_id.to_owned(),
        };
        let registration_token = self
            .wcbe_client
            .get_registration_token(
                &self.connectivity_group_id,
                Some(node_name),
                Some(&metadata),
            )
            .await?;
        let Some(session) = self.hub_session.read().await.clone() else {
            anyhow::bail!("Not connected to hub");
        };
        // Invites are delivered out-of-band (chat, email, QR), so use
        // the long-lived profile; the interactive one expires in two
        // minutes.
        let activation_code = session
            .generate_activation_code_with_ttl(wc::TtlProfile::Asynchronous)
            .await?
            .format();
        Ok(wire::Invite::WispersConnect {
            registration_token,
            activation_code,
            backend: self.backend.clone(),
        })
    }

    async fn stop_hub_session(&self) -> Result<()> {
        match self.hub_session.read().await.clone() {
            Some(session) => session.shutdown().await.context("shutdown failed"),
            None => Ok(()),
        }
    }
}

async fn handle_quic_conn(
    r: Result<wc::QuicConnection, wc::P2pError>,
    node: Arc<wc::Node>,
    serving_handle: ServingHandle,
) {
    let conn = match r {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to accept QUIC connection: {}", e);
            return;
        }
    };
    let peer = conn.peer_node_number;
    info!(peer, "QUIC connection accepted");

    // Resolve the peer's identity once per connection. Without one the
    // guest does not get served: the app relies on the identity header.
    let Some(user_id) = resolve_identity(&node, peer).await else {
        warn!(peer, "no identity resolved; closing the connection");
        return;
    };
    info!(peer, user_id = %user_id, "resolved peer identity");

    // Track the connection for `waserver status`, and hand out a closer for
    // `revoke` and shutdown. The connection can only be closed by value, so
    // the closer does not close: it sends the code here, and the loop below
    // closes on this task, which owns the connection.
    let (close_requests, mut close_request) = watch::channel(None);
    let closer: Closer = Box::new(move |code| {
        let _ = close_requests.send(Some(code));
    });
    let registration = serving_handle
        .register_connection(peer, user_id.clone(), Some(closer))
        .await;
    let close_with = loop {
        let stream = tokio::select! {
            r = conn.accept_stream() => r,
            _ = close_request.changed() => break *close_request.borrow(),
        };
        match stream {
            Ok(stream) => {
                let peer = Peer {
                    // The node number, authenticated by the library against
                    // the group's cryptographic roster. `POST /v1/activation`
                    // is unused here: the library's own activation (node 1
                    // endorsing the guest) makes the node a guest, so every
                    // secret is refused.
                    peer_id: peer.to_string(),
                    user_id: Some(user_id.clone()),
                };
                // Each stream is served against the config as of now; a
                // reload applies from the next stream on.
                let ctx = serving_handle.stream_context().await;
                tokio::spawn(async move {
                    if let Err(e) = protocol::handle(stream, ctx, peer).await {
                        error!(error = format!("{:#}", e), "QUIC stream handler error");
                    }
                });
            }
            Err(e) => {
                warn!(peer, error = %e, "QUIC connection closed");
                break None;
            }
        }
    };
    if let Some(code) = close_with {
        info!(peer, code = code.code(), "closing the connection");
        let reason = String::from_utf8_lossy(code.reason()).into_owned();
        if let Err(e) = conn.close_with_error(u64::from(code.code()), &reason).await {
            warn!(peer, error = %e, "closing the connection failed");
        }
    }
    serving_handle.unregister_connection(registration).await;
}

/// The peer's `user_id` from its node metadata, fetched from the hub. `None`
/// (no metadata, no user ID, or the hub unreachable) means the guest is not
/// served. The metadata is what `invite` attached to the registration token,
/// so identity is trusted to the hub here while membership is not; binding it
/// at endorsement instead needs the library to report which node used which
/// code.
async fn resolve_identity(node: &wc::Node, peer: i32) -> Option<String> {
    let group = match node.group_info().await {
        Ok(g) => g,
        Err(e) => {
            warn!(peer, error = %e, "could not fetch group info for identity");
            return None;
        }
    };
    let info = group.nodes.iter().find(|n| n.node_number == peer)?;
    let metadata: wcbe::NodeMetadata = serde_json::from_str(&info.metadata).ok()?;
    Some(metadata.user_id).filter(|s| !s.is_empty())
}

fn handle_session_end(
    result: Result<Result<(), wc::ServingError>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(Ok(())) => {
            info!("Session ended normally");
        }
        Ok(Err(e)) => {
            return Err(anyhow::anyhow!("Session error: {}", e));
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Session task panicked: {}", e));
        }
    }
    Ok(())
}

//-- `waserver revoke` implementation ------------------------------------------

/// Revokes the node. Through the daemon when it runs, so the serving host
/// node signs the revocation (its roster stays current) and the guest's live
/// connection gets the `revoked` close. Otherwise with a restored copy of the
/// host node, and the guest learns on its next dial.
pub async fn revoke(share: &str, dir: storage::ShareDir, node_number: i32) -> Result<()> {
    if node_number == HOST_NODE_NUMBER {
        anyhow::bail!("Node {node_number} is the host and cannot be revoked");
    }
    match ipc::Client::connect(share).await {
        Ok(mut client) => {
            let number = i64::from(node_number);
            match client.request(&ipc::Request::RevokeGuest { number }).await {
                Ok(ipc::Response::Success { .. }) => {}
                Ok(ipc::Response::Error { error, .. }) => anyhow::bail!("{error}"),
                Err(e) => anyhow::bail!("error sending command to server: {e}"),
            }
        }
        Err(_) => revoke_offline(share, dir, node_number).await?,
    }
    println!("Node {node_number} is now revoked and deregistered");
    Ok(())
}

/// `revoke` without a daemon: restores the host node and revokes with that.
async fn revoke_offline(share: &str, dir: storage::ShareDir, node_number: i32) -> Result<()> {
    let state = dir.open_state()?;
    let Some(wcs) = state.wispers_connect_state()? else {
        anyhow::bail!("Share {} has no Wispers Connect credentials", share);
    };
    let node_storage = wc::NodeStorage::new(state);
    if let Some(backend) = wcs.backend.as_deref() {
        node_storage.override_hub_addr(backend);
    }
    let node = node_storage.restore_or_init_node().await?;
    let client = wcbe::Client::new(&wcs.api_key, &wcbe::api_base(wcs.backend.as_deref()));
    revoke_on_hub(&node, &client, node_number).await
}

/// Revokes the node in the group's roster, then deregisters it on the hub,
/// which frees its quota slot.
async fn revoke_on_hub(node: &wc::Node, client: &wcbe::Client, node_number: i32) -> Result<()> {
    // Check group info for the node first, revoke if activated, and deal with
    // other states accordingly.
    let info = node.group_info().await?;
    match info.nodes.iter().find(|n| n.node_number == node_number) {
        Some(n) => match n.state {
            wc::NodeState::Activated => {
                // The standard case. The node is both activated and registered.
                node.revoke_node(node_number).await?;
            }
            wc::NodeState::Revoked => {
                // The node was already revoked but not yet deregistered. Try again below.
            }
            // TODO: Registered-but-never-activated could technically happen, so
            // we should have a cleaner solution than the one below.
            _ => anyhow::bail!("node {node_number} has never been activated."),
        },
        None => anyhow::bail!("Node {} is unknown", node_number),
    }

    // At this point we can be sure the node is revoked, so we proceed to
    // deregistering it.
    let Some(group_id) = node.connectivity_group_id() else {
        anyhow::bail!("host node not registered");
    };
    client
        .delete_node(&group_id.to_string(), node_number)
        .await?;
    Ok(())
}

//-- Status --------------------------------------------------------------------

/// Fills in the TransportReport, for status reporting.
pub async fn report(wcs: Option<&storage::WispersConnectState>) -> TransportReport {
    let transport = |connectivity_group_id, group: Option<&wcbe::GroupDetail>| {
        TransportStatus::WispersConnect {
            backend: wcs.and_then(|w| w.backend.clone()),
            connectivity_group_id,
            group_created_at: group.map(|g| g.created_at.clone()),
            node_quota: group.and_then(|g| g.node_quota),
        }
    };
    let Some(wcs) = wcs else {
        return TransportReport {
            guests: Err(NOT_INITIALISED.to_owned()),
            invites: Err(NOT_INITIALISED.to_owned()),
            transport: Some(transport(None, None)),
        };
    };
    let client = wcbe::Client::new(&wcs.api_key, &wcbe::api_base(wcs.backend.as_deref()));
    let (group, tokens) = tokio::join!(
        client.get_connectivity_group(&wcs.connectivity_group_id),
        client.list_registration_tokens(&wcs.connectivity_group_id)
    );
    let group = group.map_err(|e| format!("{:#}", e));
    TransportReport {
        transport: Some(transport(
            Some(wcs.connectivity_group_id.clone()),
            group.as_ref().ok(),
        )),
        guests: group.as_ref().map(to_guests).map_err(Clone::clone),
        invites: tokens
            .map(|tokens| tokens.iter().map(to_invite).collect())
            .map_err(|e| format!("{:#}", e)),
    }
}

/// The share has a config but no usable state: an `init` that did not
/// complete, or a broken config file (reported separately as `configError`).
const NOT_INITIALISED: &str = "share has no Wispers Connect credentials (init incomplete?)";

/// The group's nodes minus the host node itself, which the hub lists as one of
/// them.
fn to_guests(group: &wcbe::GroupDetail) -> Vec<GuestStatus> {
    let mut guests: Vec<GuestStatus> = group
        .nodes
        .iter()
        .filter(|n| n.node_number != HOST_NODE_NUMBER)
        .map(|n| GuestStatus {
            node_number: n.node_number,
            name: n.name.clone(),
            user_id: n.metadata.as_deref().and_then(parse_user_id),
            created_at: n.created_at.clone(),
            last_seen_at: n.last_seen_at.clone(),
            connected_to_host: None,
            connected_since: None,
            // waserver revokes *and* removes nodes, so a revoked node never
            // shows up in the roster.
            revoked: false,
            revoked_at: None,
        })
        .collect();
    guests.sort_by_key(|g| g.node_number);
    guests
}

fn parse_user_id(metadata: &str) -> Option<String> {
    let meta: wcbe::NodeMetadata = serde_json::from_str(metadata).ok()?;
    Some(meta.user_id).filter(|s| !s.is_empty())
}

fn to_invite(token: &wcbe::RegistrationToken) -> InviteStatus {
    InviteStatus {
        node_name: token.node_name.clone(),
        user_id: token.node_metadata.as_deref().and_then(parse_user_id),
        created_at: token.created_at.clone(),
        expires_at: token.expires_at.clone(),
        used_at: token.used_at.clone(),
        status: invite_status(token.used_at.as_deref(), &token.expires_at, Utc::now()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_comes_from_node_metadata() {
        assert_eq!(
            parse_user_id(r#"{"userId": "lara@example.com"}"#),
            Some("lara@example.com".to_owned())
        );
        assert_eq!(parse_user_id(r#"{"userId": ""}"#), None);
        assert_eq!(parse_user_id("not json"), None);
    }

    #[test]
    fn group_quota_error_names_the_way_out() {
        let quota = anyhow::Error::new(wcbe::QuotaExceeded {
            quota: "groups_per_domain".to_owned(),
            limit: 3,
            current: 3,
        });
        let msg = format!("{}", explain_group_quota(quota));
        assert!(msg.contains("3 of 3"), "{msg}");
        assert!(msg.contains("waserver deinit"), "{msg}");

        // Other errors pass through untouched.
        let other = anyhow::Error::new(wcbe::QuotaExceeded {
            quota: "nodes_per_group".to_owned(),
            limit: 12,
            current: 12,
        });
        let msg = format!("{}", explain_group_quota(other));
        assert!(!msg.contains("waserver deinit"), "{msg}");
    }
}
