//! The iroh transport implementation.

use crate::initialization::Rollback;
use crate::ipc;
use crate::protocol::{self, Peer, StreamOutcome};
use crate::serving::{self, BoxFuture, Closer, ExitReason, ServingHandle, shutdown_signal};
use crate::status::{GuestStatus, InviteStatus, TransportReport, TransportStatus, invite_status};
use crate::storage::{self, GuestNode};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use iroh::endpoint::{Connection, Incoming, RecvStream, SendStream, VarInt};
use std::time::Duration;
use tracing::{error, info, warn};
use wire::CloseCode;
use wispers_access_wire as wire;

//-- Share initialisation ------------------------------------------------------

/// `init` sets up a new share with iroh as the transport.
pub fn init(rollback: &mut Rollback, dir: &storage::ShareDir, config_text: &str) -> Result<()> {
    let state = dir.create(config_text)?;
    rollback.push("share directory", {
        let dir = dir.clone();
        async move { dir.delete().map_err(Into::into) }
    });
    let secret = iroh::SecretKey::generate();
    state.set_iroh_secret(&secret.to_bytes())?;
    println!("Endpoint ID: {}", secret.public());
    Ok(())
}

//-- HostNode implementation ---------------------------------------------------

/// Host node implementation for iroh.
pub struct HostNode {
    endpoint: iroh::Endpoint,
    db: storage::StateDb,
}

pub async fn bind(share: &str, state: storage::StateDb) -> Result<HostNode> {
    let Some(secret) = state.iroh_secret()? else {
        anyhow::bail!("Share {} has no iroh key", share);
    };
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(iroh::SecretKey::from_bytes(&secret))
        .alpns(vec![wire::ALPN.to_vec()])
        .bind()
        .await
        .context("binding the iroh endpoint")?;
    info!(endpoint_id = %endpoint.id(), "iroh endpoint bound");
    Ok(HostNode {
        endpoint,
        db: state,
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
        Box::pin(async move { self.mint_invite(node_name, user_id) })
    }

    fn revoke_guest(&self, number: i64) -> Result<GuestNode> {
        Ok(self
            .db
            .revoke_guest(number, chrono::Utc::now().timestamp())?)
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.close_endpoint())
    }
}

impl HostNode {
    /// Accepts connections until the endpoint is closed or a signal.
    async fn serve(&self, serving_handle: ServingHandle) -> Result<ExitReason> {
        // Reachable once a home relay is up and the address record is out.
        tokio::spawn({
            let (endpoint, handle) = (self.endpoint.clone(), serving_handle.clone());
            async move {
                endpoint.online().await;
                info!("iroh endpoint online");
                handle.set_reachable().await;
            }
        });

        let shutdown = shutdown_signal();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        // `None`: the endpoint was closed, by `waserver stop`.
                        break Ok(ExitReason::Stopped);
                    };
                    tokio::spawn(
                        handle_incoming(
                            incoming,
                            serving_handle.clone(),
                            self.db.clone(),
                        ),
                    );
                }
                _ = &mut shutdown => {
                    info!("shutdown signal received; shutting down gracefully");
                    if let Err(e) = serving_handle.shutdown().await {
                        warn!(error = format!("{:#}", e), "error during graceful shutdown");
                    }
                    break Ok(ExitReason::Signal);
                }
            }
        }
    }

    /// Records a one-time invite and composes its code.
    fn mint_invite(&self, node_name: &str, user_id: &str) -> Result<wire::Invite> {
        let secret = wire::InviteSecret(rand::random());
        let now = chrono::Utc::now();
        self.db.create_invite(
            storage::NewInvite {
                secret: &secret,
                user_id,
                node_name,
                expires_at: (now + INVITE_VALIDITY).timestamp(),
            },
            now.timestamp(),
        )?;
        Ok(wire::Invite::Iroh {
            endpoint_id: wire::EndpointId(*self.endpoint.id().as_bytes()),
            secret,
        })
    }

    /// Shutdown, but wait for guests to acknowledge the close, so they see an
    /// orderly close.
    async fn close_endpoint(&self) -> Result<()> {
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.endpoint.close()).await;
        Ok(())
    }
}

/// How long an iroh invite can be redeemed.
const INVITE_VALIDITY: chrono::Duration = chrono::Duration::hours(24);

/// How long shutdown waits for guests to acknowledge the close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a non-activated guest node may sit on a connection without
/// activating. This isn't security-relevant, just hygiene.
const ACTIVATION_WINDOW: Duration = Duration::from_secs(10);

/// How long to let a refused guest read its error before the connection is
/// closed under it; it normally closes first.
const REFUSAL_GRACE: Duration = Duration::from_secs(2);

/// Accept, authenticate, and handle an incoming connection.
async fn handle_incoming(incoming: Incoming, handle: ServingHandle, state: storage::StateDb) {
    let conn = match incoming.accept() {
        Ok(accepting) => match accepting.await {
            Ok(conn) => conn,
            Err(e) => {
                warn!(error = %e, "iroh handshake failed");
                return;
            }
        },
        Err(e) => {
            warn!(error = %e, "iroh connection refused");
            return;
        }
    };
    let peer_id = conn.remote_id().to_string();
    info!(peer_id, "iroh connection accepted");
    let guest = match state.guest_by_peer(&peer_id) {
        Ok(Some(guest)) if guest.revoked_at.is_some() => {
            info!(peer_id, guest = guest.number, "revoked key; closing");
            close(&conn, CloseCode::Revoked);
            return;
        }
        Ok(Some(guest)) => guest,
        Ok(None) => match activate(&conn, &peer_id, &handle, &state).await {
            Some(guest) => guest,
            None => return,
        },
        Err(e) => {
            error!(peer_id, error = %e, "state database lookup failed; closing");
            close(&conn, CloseCode::Closing);
            return;
        }
    };
    serve_guest(conn, guest, handle, state).await;
}

/// Handles an incoming connection of an unauthorised peer, limiting it to the
/// only valid action it can take at this point: activation.
async fn activate(
    conn: &Connection,
    peer_id: &str,
    handle: &ServingHandle,
    state: &storage::StateDb,
) -> Option<GuestNode> {
    let (send, recv) = match tokio::time::timeout(ACTIVATION_WINDOW, conn.accept_bi()).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            info!(peer_id, error = %e, "unknown key went away before activating");
            return None;
        }
        Err(_) => {
            info!(peer_id, "unknown key did not activate in time; closing");
            close(conn, CloseCode::Unknown);
            return None;
        }
    };
    let ctx = handle.stream_context().await;
    let peer = Peer {
        peer_id: peer_id.to_owned(),
        user_id: None,
    };
    match protocol::handle(join(send, recv), ctx, peer).await {
        Ok(StreamOutcome::Served) => {}
        Ok(StreamOutcome::Refused) => {
            info!(
                peer_id,
                "unknown key opened something other than a CTRL stream; closing"
            );
            close(conn, CloseCode::Unknown);
            return None;
        }
        Err(e) => {
            warn!(
                peer_id,
                error = format!("{:#}", e),
                "activation stream failed"
            );
        }
    }
    // Whatever the stream carried, the state database says whether this
    // key is a guest now. If not, the response (if any) had the reason;
    // let the guest read it, then close: the key is still unknown.
    match state.guest_by_peer(peer_id) {
        Ok(Some(guest)) if guest.revoked_at.is_none() => Some(guest),
        Ok(_) => {
            let _ = tokio::time::timeout(REFUSAL_GRACE, conn.closed()).await;
            close(conn, CloseCode::Unknown);
            None
        }
        Err(e) => {
            error!(peer_id, error = %e, "state database lookup failed; closing");
            close(conn, CloseCode::Closing);
            None
        }
    }
}

/// Serves a bound guest's streams until the connection ends. Identity is
/// the guest's, resolved once here and never looked up again.
async fn serve_guest(
    conn: Connection,
    guest: GuestNode,
    handle: ServingHandle,
    state: storage::StateDb,
) {
    let peer_id = guest.peer_id.clone();
    info!(peer_id, guest = guest.number, user_id = %guest.user_id, "serving guest");
    if let Err(e) = state.touch_guest(&peer_id, chrono::Utc::now().timestamp()) {
        warn!(peer_id, error = %e, "could not record last seen");
    }
    let closer: Closer = {
        let conn = conn.clone();
        Box::new(move |code| close(&conn, code))
    };
    let registration = handle
        .register_connection(guest.number as i32, guest.user_id.clone(), Some(closer))
        .await;
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let peer = Peer {
                    // A bound guest posting an activation again gets the
                    // idempotent answer.
                    peer_id: peer_id.clone(),
                    user_id: Some(guest.user_id.clone()),
                };
                let ctx = handle.stream_context().await;
                tokio::spawn(async move {
                    if let Err(e) = protocol::handle(join(send, recv), ctx, peer).await {
                        error!(error = format!("{:#}", e), "iroh stream handler error");
                    }
                });
            }
            Err(e) => {
                info!(peer_id, error = %e, "iroh connection closed");
                break;
            }
        }
    }
    handle.unregister_connection(registration).await;
}

/// Closes a connection with a contract close code; the reason phrase is
/// the code's name. Returns at once; the frame goes out in the background.
fn close(conn: &Connection, code: CloseCode) {
    conn.close(VarInt::from_u32(code.code()), code.reason());
}

/// The stream pair as the one `AsyncRead + AsyncWrite` the handlers take.
fn join(send: SendStream, recv: RecvStream) -> tokio::io::Join<RecvStream, SendStream> {
    tokio::io::join(recv, send)
}

//-- `waserver revoke` implementation ------------------------------------------

/// Revoke the given guest node from the share. Use the daemon if it runs, so
/// the guest's live connections get the `revoked` close. Otherwise, go straight
/// to the state database, and the guest node learns about it on its next dial.
pub async fn revoke(share: &str, dir: storage::ShareDir, number: i64) -> Result<()> {
    match ipc::Client::connect(share).await {
        Ok(mut client) => match client.request(&ipc::Request::RevokeGuest { number }).await {
            Ok(ipc::Response::Success { .. }) => {}
            Ok(ipc::Response::Error { error, .. }) => anyhow::bail!("{error}"),
            Err(e) => anyhow::bail!("error sending command to server: {e}"),
        },
        Err(_) => {
            dir.open_state()?
                .revoke_guest(number, chrono::Utc::now().timestamp())?;
        }
    }
    println!("Guest {number} is now revoked");
    Ok(())
}

//-- Status --------------------------------------------------------------------

/// Fills in the TransportReport, for status reporting.
pub fn report(state: Option<&storage::StateDb>) -> TransportReport {
    let Some(state) = state else {
        let err = "share has no state database (init incomplete?)";
        return TransportReport {
            guests: Err(err.to_owned()),
            invites: Err(err.to_owned()),
            transport: Some(TransportStatus::Iroh { endpoint_id: None }),
        };
    };
    let now = Utc::now();
    TransportReport {
        guests: state
            .guests()
            .map(|guests| guests.iter().map(guest_status).collect())
            .map_err(|e| format!("{:#}", e)),
        invites: state
            .invites()
            .map(|invites| invites.iter().map(|i| invite_row_status(i, now)).collect())
            .map_err(|e| format!("{:#}", e)),
        transport: Some(TransportStatus::Iroh {
            endpoint_id: state
                .iroh_secret()
                .ok()
                .flatten()
                .map(|key| iroh::SecretKey::from_bytes(&key).public().to_string()),
        }),
    }
}

fn guest_status(g: &storage::GuestNode) -> GuestStatus {
    GuestStatus {
        node_number: g.number as i32,
        name: Some(g.display_name.clone()),
        user_id: Some(g.user_id.clone()),
        created_at: fmt_unix(g.activated_at),
        last_seen_at: g.last_seen_at.map(fmt_unix),
        connected_to_host: None,
        connected_since: None,
        revoked: g.revoked_at.is_some(),
        revoked_at: g.revoked_at.map(fmt_unix),
    }
}

fn invite_row_status(i: &storage::InviteRow, now: DateTime<Utc>) -> InviteStatus {
    let expires_at = fmt_unix(i.expires_at);
    let used_at = i.consumed_at.map(fmt_unix);
    InviteStatus {
        status: invite_status(used_at.as_deref(), &expires_at, now),
        node_name: Some(i.node_name.clone()),
        user_id: Some(i.user_id.clone()),
        created_at: fmt_unix(i.created_at),
        expires_at,
        used_at,
    }
}

fn fmt_unix(secs: i64) -> String {
    DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
