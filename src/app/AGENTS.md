# Shared services guide

- What the bot (`src/bot/`) and the web server (`src/api/`) both use: `write.rs` (the one scheduler `Writer`), `read.rs` (object-safe `ReadStore`, `GuideBook`, `CharacterRoster`), `guild.rs` (`ChannelList`, `GuildAccess`, `AvatarRef`), `access.rs` (staff rule and command gates), `config.rs` (`ConfigDesk`, `SettingsChanged`), `rescan.rs`, `ownership.rs`, `roster.rs` (roster persistence; sessions end through `RosterHooks`, which the API implements), `ports.rs` (what the bot hands the web side), `clock.rs`, `crypto.rs`, `ids.rs`.
- Never import from `src/api/`, `src/bot/`, `src/discord/` or `src/cards/`, nor the HTTP framework or the gateway crate; a wire shape (DTO, view) stays in `src/api/`.
