//! The v5 HTTP skeleton: two listeners whose authorization is by mounting,
//! Host and proxy guards, security headers, request bounds and static serving.
//! Every test serves synthetic files from its own temp directory over loopback.

mod account;
mod assets;
mod auth;
mod avatars;
mod boss_picks;
mod character_data;
mod characters;
mod config;
mod drafts;
mod estimates;
mod etag;
mod events;
mod fixed_characters;
mod guides;
mod header_rewrite;
mod headers;
mod history;
mod inbox;
mod limits;
mod logs;
mod origins;
mod outbox;
mod presence;
mod proxy;
mod readiness;
mod reads;
mod reminders;
mod replays;
mod run_characters;
mod schemas;
mod sign_ins;
mod support;
mod tonight;
mod writes;
