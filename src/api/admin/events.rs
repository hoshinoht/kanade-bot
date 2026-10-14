//! `GET /api/admin/events`: server-sent change hints for open admin pages
//! (`text/event-stream`). The stream opens with a `ready` event, then sends one
//! `{topic, seq}` message per committed change and a comment each heartbeat.
//! It ends when the session ends, after the hub's lifetime, on shutdown, or
//! when the client goes away.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::request::Parts,
    response::{IntoResponse, Response},
    routing::get,
};
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
    sse::{self, data},
};

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
    sse::response(body)
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
    let ready = EventReady {
        seq: ready,
        boot: boot.clone(),
    };
    if frames.send(sse::opening(&ready)).await.is_err() {
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
