//! The REST API presented to guest nodes, made accessible over CTRL streams.
//!
//! Every stream carries one request and one response. `GET /v1/events` is the
//! exception: after the request, it stays open and streams server-sent events
//! until the guest node goes away.

use crate::config::ShareConfig;
use crate::protocol::{Peer, StreamContext};
use crate::storage::{self, Redemption};
use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tracing::{debug, error as log_error, info, warn};
use wire::{ActivationError, ConfigHash, ShareChanged, ShareInfo, SharedApp};
use wispers_access_wire as wire;

type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// Serves one CTRL stream, for the share ID found in `ctx`.
pub async fn serve<S>(io: S, ctx: StreamContext, peer: Peer) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |req| route(req, ctx.clone(), peer.clone()));
    let served = http1::Builder::new()
        // The guest node may FIN right after the request and wait for the
        // response on the other half.
        .half_close(true)
        .serve_connection(TokioIo::new(io), service)
        .await;
    if let Err(e) = served {
        debug!(error = %e, "guest API stream ended with an error");
    }
    Ok(())
}

/// Route the request to the correct handler.
async fn route(
    req: Request<Incoming>,
    ctx: StreamContext,
    peer: Peer,
) -> Result<Response<BoxedBody>, Infallible> {
    info!(
        method = %req.method(),
        path = req.uri().path(),
        user_id = peer.user_id.as_deref().unwrap_or("-"),
        "guest API request"
    );

    // Handle unauthorised peers first. They're only allowed to activate.
    if !(peer.is_authorized() || req.uri().path() == wire::ACTIVATION_PATH) {
        return Ok(error(StatusCode::FORBIDDEN, "not-activated"));
    }

    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, wire::SHARE_PATH) => get_share(&req, &ctx.config),
        (&Method::GET, wire::EVENTS_PATH) => get_events(&ctx.events),
        (&Method::POST, wire::ACTIVATION_PATH) => {
            post_activation(req, &ctx.db, &peer.peer_id, &ctx.config).await
        }
        (&Method::DELETE, wire::GUEST_PATH) => delete_guest(&ctx.db, &peer.peer_id),
        (_, wire::SHARE_PATH | wire::EVENTS_PATH | wire::ACTIVATION_PATH | wire::GUEST_PATH) => {
            error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
        }
        _ => error(StatusCode::NOT_FOUND, "no such route"),
    };
    Ok(response)
}

/// The share config, stripped down to what a guest node cares about, with the
/// config hash as a strong entity tag so a guest that is up to date gets a 304
/// and no body.
fn get_share(req: &Request<Incoming>, config: &ShareConfig) -> Response<BoxedBody> {
    let hash = ConfigHash(config.config_hash());
    let up_to_date = req
        .headers()
        .get(hyper::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| hash.matches_if_none_match(v));
    let builder = Response::builder()
        .header(hyper::header::ETAG, hash.etag())
        .header(hyper::header::CACHE_CONTROL, "no-cache");
    if up_to_date {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(empty())
            .expect("static response is valid");
    }
    share_response(builder, config)
}

/// A 200 with the share as JSON.
fn share_response(
    builder: hyper::http::response::Builder,
    config: &ShareConfig,
) -> Response<BoxedBody> {
    let share_info = share_info(config);
    let body = serde_json::to_vec(&share_info).expect("ShareInfo serialises");
    builder
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(full(body))
        .expect("static response is valid")
}

/// Strip down a ShareInfo to the part a guest node needs to know,
/// i.e. leave out upstreams.
fn share_info(config: &ShareConfig) -> ShareInfo {
    ShareInfo {
        config_hash: ConfigHash(config.config_hash()),
        name: config.name.clone(),
        transport: config.transport.kind().as_str().to_owned(),
        apps: config
            .apps
            .iter()
            .map(|s| SharedApp {
                id: s.id.clone(),
                name: s.name.clone(),
                kind: s.kind,
            })
            .collect(),
    }
}

const MAX_ACTIVATION_BODY: usize = 1024;

/// Redeems an invite secret (in the body), binding the peer to the metadata
/// associated with the invite, namely the user ID. On success, returns the
/// share config.
async fn post_activation(
    req: Request<Incoming>,
    db: &storage::StateDb,
    peer_id: &str,
    config: &ShareConfig,
) -> Response<BoxedBody> {
    let body = match Limited::new(req.into_body(), MAX_ACTIVATION_BODY)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return refuse_activation(peer_id, ActivationError::Malformed),
    };
    let Ok(request) = serde_json::from_slice::<wire::Activation>(&body) else {
        return refuse_activation(peer_id, ActivationError::Malformed);
    };
    let now = chrono::Utc::now();
    match db.redeem_invite(&request.secret, peer_id, now) {
        Ok(Redemption::Activated(guest)) => {
            info!(
                peer_id,
                guest = guest.number,
                user_id = %guest.user_id,
                "guest activated"
            );
            let hash = ConfigHash(config.config_hash());
            share_response(
                Response::builder()
                    .header(hyper::header::ETAG, hash.etag())
                    .header(hyper::header::CACHE_CONTROL, "no-cache"),
                config,
            )
        }
        Ok(Redemption::Refused(why)) => refuse_activation(peer_id, why),
        Err(e) => {
            log_error!(error = %e, "activation failed on the state database");
            error(StatusCode::INTERNAL_SERVER_ERROR, "state database error")
        }
    }
}

fn refuse_activation(peer_id: &str, why: ActivationError) -> Response<BoxedBody> {
    warn!(peer_id, why = why.as_str(), "activation refused");
    let status = StatusCode::from_u16(why.status()).expect("contract statuses are valid");
    error(status, why.as_str())
}

/// Remove the guest node from the DB. The guest closes the connection itself.
fn delete_guest(db: &storage::StateDb, peer_id: &str) -> Response<BoxedBody> {
    let guest = match db.guest_by_peer(peer_id) {
        Ok(Some(guest)) => guest,
        // Wispers Connect guests have no row here; they leave via the hub.
        Ok(None) => return error(StatusCode::NOT_FOUND, "no such guest"),
        Err(e) => {
            log_error!(error = %e, "leave failed on the state database");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "state database error");
        }
    };
    if let Err(e) = db.remove_guest(guest.number) {
        log_error!(error = %e, "leave failed on the state database");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "state database error");
    }
    info!(peer_id, guest = guest.number, user_id = %guest.user_id, "guest node left");
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(empty())
        .expect("static response is valid")
}

/// How often an idle events stream sends a comment. Heartbeats keep middleboxes
/// from timing the stream out and makes a stream whose guest silently went away
/// fail on the next write instead of lingering until the next reload.
const EVENTS_HEARTBEAT: Duration = Duration::from_secs(30);

/// Returns a stream of server-side events, concretely share config updates.
fn get_events(events: &broadcast::Sender<u64>) -> Response<BoxedBody> {
    let events = BroadcastStream::new(events.subscribe()).filter_map(|item| {
        let hash = item.ok()?;
        Some(Ok(Frame::data(sse_event(hash))))
    });
    let mut interval = tokio::time::interval(EVENTS_HEARTBEAT);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let heartbeats = IntervalStream::new(interval)
        .skip(1) // the first tick is immediate
        .map(|_| Ok(Frame::data(Bytes::from_static(b": ping\n\n"))));
    // A comment first, so the headers reach the guest before any event.
    let opening = tokio_stream::once(Ok(Frame::data(Bytes::from_static(b": connected\n\n"))));
    let body = StreamBody::new(opening.chain(events.merge(heartbeats)));
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "text/event-stream")
        .header(hyper::header::CACHE_CONTROL, "no-cache")
        .body(BoxBody::new(body))
        .expect("static response is valid")
}

fn sse_event(config_hash: u64) -> Bytes {
    let data = serde_json::to_string(&ShareChanged {
        config_hash: ConfigHash(config_hash),
    })
    .expect("ShareChanged serialises");
    Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        wire::EVENT_SHARE_CHANGED,
        data
    ))
}

fn error(status: StatusCode, msg: &str) -> Response<BoxedBody> {
    let body = serde_json::json!({ "error": msg }).to_string();
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(full(body.into_bytes()))
        .expect("static response is valid")
}

fn full(bytes: Vec<u8>) -> BoxedBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn empty() -> BoxedBody {
    full(Vec::new())
}
