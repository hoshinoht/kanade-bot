# e2e_server

The dev and browser e2e server for the web apps: the real `kanade` admin and public routers, composed as `serve` composes them, over a seeded temporary SQLite store, with fake Discord, NEXON, scouter and model gateway. Built only with `--features test-support`; never shipped.

```sh
cd web && bun run build && bun run serve:dev   # admin 127.0.0.1:4173, public :4174, live clock
cargo run --release --example e2e_server --features test-support -- \
  --admin-port A --public-port P --web-dir web [--now RFC3339] [--boss-dir PATH]
```

Without `--now` the clock is live and the seed is relative to the current boss week; `bun run e2e` pins it at `2026-09-29T04:00:00Z` (Tue 12:00, Kuala Lumpur). Sign in to the admin with `break_glass.token` from `/__e2e/whoami`. Everything lives under a 0700 temp directory, removed on exit.

## Controls

Both origins unless noted; source in `controls.rs`, `controls_p5.rs`, `controls_p6.rs`.

- `GET /__e2e/whoami`: `{server, now, origin, admin_cookie, break_glass: {cookie, token}, tailscale: {login, edge_secret}}`.
- `POST /__e2e/reset`: restart over a fresh template copy, clock back at its start, modules and fakes at their defaults; `{now}`.
- `POST /__e2e/clock` `{at}` or `{advance_s}`: set or move the server clock; `{now}`.
- `POST /__e2e/modules` `{roster?, character_data?}`: restart over the live store with those modules.
- `POST /__e2e/limits` `{groups: "three" | "default"}`: the canned governor groups Limits shows.
- `POST /__e2e/chat-allowance` `{member, answers}`: spend answers from a member's live chat window.
- `POST /__e2e/public/end`: end every member-portal session; `{ended}`.
- `POST /__e2e/estimates` `{scenario, ign?}`: what later scouter lookups do (`ok`, `paced`, `assumed`, `hold`, `busy`, `critical_rate`, `manual_check`, `failed`, `timeout`, `queued`, `off`).
- `POST /__e2e/character-data` `{health}`: Config's character-data health (`ok`, `alert`, `refused`, `setup`, `budget`).
- `POST /__e2e/public/characters` `{roster: "board" | "empty"}`: Asahi's own characters as the member boards draw them, or none.
- `POST /__e2e/arrive` `{kind}`: a change made outside the portal (`reaction`, `move`, `run`, `proposal`, `chat`, `extraction`, `member`).
- `POST /__e2e/public/hint` `{topic, member?}`: a member-stream hint (`schedule`, `mine`, `allowance`).
- `POST /__e2e/public/remove` `{run}`: Ren takes Asahi off `run`.
- `POST /__e2e/inbox/kalos-v4`: `p-kalos-fixed` changes to Sunday under an admin editing it; `{version}`.
- `POST /__e2e/drafts/board`, `POST /__e2e/graph/board`: the drafts and History graph boards' state.
- `POST`/`DELETE /__e2e/presence`, `POST /__e2e/presence/board`: another admin's presence, nobody else, or the presence boards' admins.
- `GET`/`POST`/`DELETE /__e2e/reports`: the CSP report sink every enforced policy names.
- `GET /__e2e/discord/oauth2/authorize`: the fake Discord page; the `kanade_fake_discord` cookie picks the user (`eligible`, `partner`, `ineligible`, `bot`, a seeded snowflake) or `deny`.
- `/__showcase/` (admin only): the core primitives showcase (`bun run showcase` in `web/`).
