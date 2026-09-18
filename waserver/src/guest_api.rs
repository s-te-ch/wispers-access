//! The REST API presented to guest nodes, made accessible over CTRL streams.
//!
//! Every stream carries one request. `GET /v1/events` is the long-lived
//! one: it stays open and streams server-sent events until the guest goes
//! away.

use crate::config::CircleConfig;
use anyhow::Result;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tracing::{debug, info};
use wire::{CircleChanged, CircleInfo, ConfigHash, Share};
use wispers_access_wire as wire;

type BoxedBody = BoxBody<Bytes, std::io::Error>;

/// Serves one CTRL stream against `config`, with `events` as the source of
/// the events stream.
pub async fn serve<S>(
    io: S,
    config: Arc<CircleConfig>,
    events: broadcast::Sender<u64>,
    user_id: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |req| {
        route(req, config.clone(), events.clone(), user_id.clone())
    });
    let served = http1::Builder::new()
        // One request per stream: the guest may FIN its side right after
        // the request and wait for the response on the other half.
        .half_close(true)
        .serve_connection(TokioIo::new(io), service)
        .await;
    // Whatever went wrong here, the guest caused it or went away (an events
    // stream ends with a failed write once the guest is gone). Just log it.
    if let Err(e) = served {
        debug!(error = %e, "guest API stream ended with an error");
    }
    Ok(())
}

async fn route(
    req: Request<Incoming>,
    config: Arc<CircleConfig>,
    events: broadcast::Sender<u64>,
    user_id: Option<String>,
) -> Result<Response<BoxedBody>, Infallible> {
    info!(
        method = %req.method(),
        path = req.uri().path(),
        user_id = user_id.as_deref().unwrap_or("-"),
        "guest API request"
    );
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, wire::CIRCLE_PATH) => get_circle(&req, &config),
        (&Method::GET, wire::EVENTS_PATH) => get_events(&events),
        (_, wire::CIRCLE_PATH | wire::EVENTS_PATH) => {
            error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
        }
        _ => error(StatusCode::NOT_FOUND, "no such route"),
    };
    Ok(response)
}

/// The circle as the guest may know it, with the config hash as a strong
/// entity tag so a guest that is up to date gets a 304 and no body.
fn get_circle(req: &Request<Incoming>, config: &CircleConfig) -> Response<BoxedBody> {
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
    let body = serde_json::to_vec(&circle_info(config)).expect("CircleInfo serialises");
    builder
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(full(body))
        .expect("static response is valid")
}

/// Everything a guest may know about the circle. Upstreams stay out.
fn circle_info(config: &CircleConfig) -> CircleInfo {
    CircleInfo {
        config_hash: ConfigHash(config.config_hash()),
        name: config.name.clone(),
        transport: config.transport.kind().as_str().to_owned(),
        shares: config
            .shares
            .iter()
            .map(|s| Share {
                id: s.id.clone(),
                name: s.name.clone(),
                kind: s.kind,
            })
            .collect(),
    }
}

/// How often an idle events stream sends a comment. Keeps middleboxes from
/// timing the stream out and makes a stream whose guest silently went away fail
/// on the next write instead of lingering until the next reload.
const EVENTS_HEARTBEAT: Duration = Duration::from_secs(30);

/// A server-sent event stream that stays open. A guest that falls behind
/// the channel simply misses the older hashes, which is fine: every event
/// says "fetch", and the fetch is conditional.
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
    let data = serde_json::to_string(&CircleChanged {
        config_hash: ConfigHash(config_hash),
    })
    .expect("CircleChanged serialises");
    Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        wire::EVENT_CIRCLE_CHANGED,
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
