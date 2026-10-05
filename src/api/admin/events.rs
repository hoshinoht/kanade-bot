//! `GET /api/admin/events`: server-sent change hints for open admin pages
//! (`text/event-stream`). The stream opens with a `ready` event, then sends one
//! `{topic, seq}` message per committed change and a comment each heartbeat.
//! It ends when the session ends, after the hub's lifetime, on shutdown, or
//! when the client goes away.

use std::{
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::State,
    http::{HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use hyper::body::Frame;
use serde::Serialize;
use tokio::{
    sync::{broadcast::error::RecvError, mpsc},
    time::{Instant, interval_at, sleep},
};

use super::context::state;
use crate::api::{
    auth::AdminAuth,
    dto::events::EventReady,
    error::ApiError,
    events::{EventsConfig, Subscription},
    listeners::Site,
};

/// How long the browser waits before reconnecting a stream that ended.
const RETRY_MS: u32 = 3_000;

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/events", get(events))
}

async fn events(State(site): State<Arc<Site>>, parts: Parts) -> Response {
    let Some(auth) = site.auth.as_ref().map(Arc::clone) else {
        return ApiError::AUTH_UNAVAILABLE.into_response();
    };
    // Holding a stream open is not activity: it never extends the idle window.
    let session = match auth.authenticate_quietly(&parts).await {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    let state = match state(&site) {
        Ok(state) => state,
        Err(error) => return error.into_response(),
    };
    if state.events.is_closed() {
        return ApiError::UNAVAILABLE.into_response();
    }
    let Some(subscription) = state.events.subscribe() else {
        return ApiError::TOO_MANY_STREAMS.into_response();
    };
    let (frames, body) = mpsc::channel(8);
    tokio::spawn(pump(
        frames,
        subscription,
        auth,
        session.session_id().map(str::to_owned),
        state.events.config(),
    ));
    let mut response = Body::new(Stream(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    // Proxies that buffer by default (nginx-style) pass this through unbuffered.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn data<T: Serialize>(event: Option<&str>, value: &T) -> Bytes {
    let json = serde_json::to_string(value).unwrap_or_default();
    match event {
        Some(name) => format!("event: {name}\ndata: {json}\n\n"),
        None => format!("data: {json}\n\n"),
    }
    .into()
}

/// Writes one stream until something ends it; dropping `frames` ends the body.
async fn pump(
    frames: mpsc::Sender<Bytes>,
    subscription: Subscription,
    auth: Arc<AdminAuth>,
    session: Option<String>,
    config: EventsConfig,
) {
    let Subscription {
        permit: _permit,
        mut hints,
        ready,
        boot,
        mut closed,
    } = subscription;
    let opening = format!("retry: {RETRY_MS}\n");
    let mut first = Vec::from(opening.as_bytes());
    first.extend_from_slice(&data(
        Some("ready"),
        &EventReady {
            seq: ready,
            boot: boot.clone(),
        },
    ));
    if frames.send(first.into()).await.is_err() {
        return;
    }
    let lifetime = sleep(config.max_lifetime);
    tokio::pin!(lifetime);
    let mut heartbeat = interval_at(Instant::now() + config.heartbeat, config.heartbeat);
    let closing = async move {
        let _ = closed.wait_for(|closed| *closed).await;
    };
    tokio::pin!(closing);
    loop {
        let frame = tokio::select! {
            () = frames.closed() => return,
            () = &mut closing => return,
            () = &mut lifetime => return,
            _ = heartbeat.tick() => {
                // Bearer (CLI) streams have no session row; the lifetime bounds them.
                if let Some(id) = &session
                    && !auth.session_live(id).await
                {
                    return;
                }
                Bytes::from_static(b": keep-alive\n\n")
            }
            hint = hints.recv() => match hint {
                Ok(hint) => data(None, &hint),
                // Skipped hints show as a seq gap; the client re-reads everything.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
        };
        // A reader that stops reading must not hold the shutdown drain.
        tokio::select! {
            sent = frames.send(frame) => if sent.is_err() { return },
            () = &mut closing => return,
            () = &mut lifetime => return,
        }
    }
}

/// The response body: frames as the pump writes them, ending when it stops.
struct Stream(mpsc::Receiver<Bytes>);

impl HttpBody for Stream {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(cx)
            .map(|bytes| bytes.map(|bytes| Ok(Frame::data(bytes))))
    }
}
