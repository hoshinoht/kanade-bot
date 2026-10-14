//! `text/event-stream` framing shared by the admin and member event streams.

use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::{Body, Bytes, HttpBody},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use hyper::body::Frame;
use serde::Serialize;
use tokio::sync::mpsc;

/// How long the browser waits before reconnecting a stream that ended.
const RETRY_MS: u32 = 3_000;

pub(crate) fn data<T: Serialize>(event: Option<&str>, value: &T) -> Bytes {
    let json = serde_json::to_string(value).unwrap_or_default();
    match event {
        Some(name) => format!("event: {name}\ndata: {json}\n\n"),
        None => format!("data: {json}\n\n"),
    }
    .into()
}

/// A stream's first frame: the reconnect delay, then the `ready` event.
pub(crate) fn opening<T: Serialize>(ready: &T) -> Bytes {
    let mut first = format!("retry: {RETRY_MS}\n").into_bytes();
    first.extend_from_slice(&data(Some("ready"), ready));
    first.into()
}

/// The response streaming `frames` as the pump writes them. Cache-Control
/// (`no-store, no-transform`) comes from the header layer, which sets it on
/// every event stream.
pub(crate) fn response(frames: mpsc::Receiver<Bytes>) -> Response {
    let mut response = Body::new(Stream(frames)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    // Proxies that buffer by default (nginx-style) pass this through unbuffered.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
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
