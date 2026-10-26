# Card rendering guide

- Everything a card looks like, for the bot's posts and the admin preview: `mod.rs` (`Card`, `CardKit`), `redesign/` (layouts, V2 budget), `heading/`, `art.rs`, `chart/` + `estimates.rs` (the share chart), `preview.rs`, `render.rs`, `format.rs` (proposal-card wording), `chardata.rs` (character-data board copy, exact words), `text.rs` (v4 date and boss text). Layout rules: `src/bot/AGENTS.md`.
- May import `src/app/` and `src/discord/`; never `src/api/`, `src/bot/`, the HTTP framework or the gateway crate.
