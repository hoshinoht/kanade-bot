//! The v5 HTTP skeleton: two listeners whose authorization is by mounting,
//! Host and proxy guards, security headers, request bounds and static serving.
//! Every test serves synthetic files from its own temp directory over loopback.

mod assets;
mod auth;
mod headers;
mod history;
mod inbox;
mod logs;
mod origins;
mod outbox;
mod proxy;
mod reads;
mod schemas;
mod support;
mod writes;
