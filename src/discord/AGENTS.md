# Discord REST guide

- Discord without the gateway, shared by the bot and the web server: `transport/` (outcome-classifying `DiscordTransport`, `FakeDiscord` behind test-support), `mentions.rs`, `guild_cache/`, `identity/` (the bot's avatar and banner), `avatars.rs` (member portrait cache), `roster/` (members as `RosterUpdate`s, paged reconciliation). Transport and mention rules: `src/bot/AGENTS.md`.
- May import `src/app/`, `src/chat/` and infrastructure; never `src/api/`, `src/bot/`, the HTTP framework or the gateway crate.
