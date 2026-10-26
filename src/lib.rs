#[cfg(not(any(feature = "bot", feature = "web")))]
compile_error!("kanade needs the `bot` feature, the `web` feature or both");

#[cfg(feature = "web")]
pub mod api;
pub mod app;
#[cfg(feature = "bot")]
pub mod bot;
pub mod cards;
pub mod chat;
pub mod cli;
pub mod discord;
pub mod extract;
pub mod runtime;
