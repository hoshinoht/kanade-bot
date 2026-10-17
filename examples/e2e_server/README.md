# e2e_server

The dev and browser e2e server for the web apps: the real `kanade` admin and public routers, composed as `serve` composes them, over a seeded temporary SQLite store, with fake Discord, NEXON, scouter and model gateway. Built only with `--features test-support`; never shipped.

```sh
cd web && bun run build && bun run serve:dev   # admin 127.0.0.1:4173, public :4174, live clock
cargo run --release --example e2e_server --features test-support -- \
  --admin-port A --public-port P --web-dir web [--now RFC3339] [--boss-dir PATH]
```

Without `--now` the clock is live and the seed is relative to the current boss week; `bun run e2e` pins it at `2026-09-29T04:00:00Z` (Tue 12:00, Kuala Lumpur). Sign in to the admin with `break_glass.token` from `/__e2e/whoami`. Everything lives under a 0700 temp directory, removed on exit.

Module `guides` runs serve's real publisher (`bot::guides::Publisher`) over the generation's fake Discord, ready at once (the fake gateway never sends `READY`, which serve's own publisher waits for). The seed adds two invented channels, `#guide-limbo` and `#guide-raids`, and two invented guides: "Limbo party roles", published to `#guide-limbo` (five messages) through the worker itself, and "Weekly bossing etiquette", a draft with no channel. Each generation's fake Discord starts holding every message the store says is posted.

## Browser suite

`bun run e2e` (in `web/`, after `bun run build`: it serves the built `dist`) starts one server per Playwright worker: worker i serves admin on base + 2i and public on base + 2i + 1.

- `KANADE_E2E_WORKERS`: workers, 1–5 (default 5, 4 on CI).
- `KANADE_E2E_PORT_BASE`: the base port (default 4373, so 4373–4382); parallel worktrees use bases 100 apart.
- `KANADE_E2E_SERVER_BIN`: a prebuilt server binary (repo-root relative, e.g. `target/release/examples/e2e_server`) instead of `cargo run`.
- `KANADE_CAPTURES=1`: also run the review-capture specs, which only write PNGs.

CI runs the browser suite only from a manual "Run workflow" with `e2e` ticked: the `web` job builds `dist` and this server once, then the main project runs in 4 shards and motion on its own runner (about 17 minutes); pushes and pull requests skip it.

## Controls

Both origins unless noted; source in `controls.rs`, `controls_p5.rs`, `controls_p6.rs`, `controls_guides.rs`.

- `GET /__e2e/whoami`: `{server, now, origin, admin_cookie, break_glass: {cookie, token}, tailscale: {login, edge_secret}}`.
- `POST /__e2e/reset`: restart over a fresh template copy, clock back at its start, modules and fakes at their defaults; `{now}`.
- `POST /__e2e/clock` `{at}` or `{advance_s}`: set or move the server clock; `{now}`.
- `POST /__e2e/modules` `{roster?, character_data?, guides?}`: restart over the live store with those modules.
- `POST /__e2e/limits` `{groups: "three" | "default"}`: the canned governor groups Limits shows.
- `POST /__e2e/chat-allowance` `{member, answers}`: spend answers from a member's live chat window.
- `POST /__e2e/public/end`: end every member-portal session; `{ended}`.
- `POST /__e2e/estimates` `{scenario, ign?}`: what later scouter lookups do (`ok`, `paced`, `assumed`, `hold`, `busy`, `critical_rate`, `assumption_failed`, `manual_check`, `failed`, `timeout`, `queued`, `off`); `{scenario: "aged", days, run | character}`: a finished lookup of a run's party or one character fetched `days` (1–30) before the server clock, to show "older than n days".
- `POST /__e2e/character-data` `{health}`: Config's character-data health (`ok`, `alert`, `refused`, `setup`, `budget`).
- `POST /__e2e/public/characters` `{roster: "board" | "empty"}`: Asahi's own characters as the member boards draw them, or none.
- `POST /__e2e/arrive` `{kind}`: a change made outside the portal (`reaction`, `move`, `run`, `proposal`, `chat`, `extraction`, `member`).
- `POST /__e2e/public/hint` `{topic, member?}`: a member-stream hint (`schedule`, `mine`, `allowance`).
- `POST /__e2e/public/remove` `{run}`: Ren takes Asahi off `run`.
- `POST /__e2e/inbox/kalos-v4`: `p-kalos-fixed` changes to Sunday under an admin editing it; `{version}`.
- `POST /__e2e/drafts/board`, `POST /__e2e/graph/board`: the drafts and History graph boards' state.
- `POST`/`DELETE /__e2e/presence`, `POST /__e2e/presence/board`: another admin's presence, nobody else, or the presence boards' admins.
- `GET`/`POST`/`DELETE /__e2e/reports`: the CSP report sink every enforced policy names.
- `POST /__e2e/discord/guide` `{script}`: what the fake Discord does next for the guide publisher (module `guides`, on by default): `ambiguous_create` (the next post times out unposted, so its message is left unconfirmed), `edit_404` (the next edit finds its message deleted by hand), `offline` / `online` (the gateway disconnects or comes back; a reset brings it back), `hold` / `release` (the next post waits until released). 404 while the module is off.
- `GET /__e2e/discord/calls`: every call the generation's fake Discord answered, in order (`op`, `channel`, `message`, `outcome`; posts also `silent`, `embeds`, `files`).
- `POST /__e2e/guides/board`: the guides boards' list (`G_List`) over the seed's two guides, through the store and the live publisher; the default seed stays as it is. It adds `#guide-kalos`, `#guide-star`, `#guide-carling` and `#guide-bm` (the generation's channel list; `#guide-bm` without Attach Files, a reset brings the seed's list back) and four guides: Kalos basics published (5 messages), Radiant Malefic Star published with art since replaced (Out of date), Black Mage prep refused for the missing grant (Blocked), Carling + Star night with message 4 unconfirmed (Needs attention); Limbo party roles is left publishing its seven-message version with its next post held (`{script: "release"}` lets it finish). 204; 404 while the module is off.
- `GET /__e2e/discord/oauth2/authorize`: the fake Discord page; the `kanade_fake_discord` cookie picks the user (`eligible`, `partner`, `ineligible`, `bot`, a seeded snowflake) or `deny`.
- `/__showcase/` (admin only): the core primitives showcase (`bun run showcase` in `web/`).
