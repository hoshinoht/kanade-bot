//! The v5 HTTP skeleton: two listeners whose authorization is by mounting,
//! Host and proxy guards, security headers, request bounds and static serving.
//! Every test serves synthetic files from its own temp directory over loopback.

mod account;
mod assets;
mod auth;
mod avatars;
mod config;
mod etag;
mod events;
mod header_rewrite;
mod headers;
mod history;
mod inbox;
mod limits;
mod logs;
mod origins;
mod outbox;
mod proxy;
mod reads;
mod reminders;
mod replays;
mod schemas;
mod sign_ins;
mod support;
mod tonight;
mod writes;
