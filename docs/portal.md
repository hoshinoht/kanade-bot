# Portal, CLI and API

The web portal, `bossctl`, and the JSON API — one HTTP server inside the
bot process, loopback-only, reachable over your tailnet.

One HTTP API, two front ends. It runs **inside the bot process** — FastAPI on the
same asyncio loop as discord.py — so the portal reads live state, writes to the
same SQLite file the bot has open, and can post to Discord, with no second
process to supervise.

### Set the token

```sh
openssl rand -hex 32          # paste into ADMIN_TOKEN= in .env
docker compose up -d --build
open http://127.0.0.1:8080    # sign in with that token
```

Until `ADMIN_TOKEN` is set the API still starts and `/healthz` still answers, but
every other request comes back `503 set ADMIN_TOKEN` — a half-configured
deployment fails loudly rather than quietly serving your schedule. Rotating the
token signs every browser session out.

### The pages

| Page             | What it is for                                                                                                                                     |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Week**         | The default. A seven-column rail for the boss week — starting on the reset day, not Monday — with a pip per run, then the runs grouped by day. Filter by channel, member or boss; move, preview-ping, and a status control (planned · confirmed · own time · done · cancelled) on each row. Past and cancelled runs are hidden until you ask. |
| **Fixed**        | The baseline timings, with a create/edit form. Bosses are picked from a **boss grid** — the in-game list, one row per boss with its real difficulties as pills — or typed as tokens. The party comes from the synced roster. |
| **Bosses**       | The same grid, read-only, with the difficulties the group actually has timings for ticked. A quick "what do we run".                                |
| **Inbox**        | What the extractor proposed and nobody has answered: the change, its confidence, and the exact chat lines it cited. Approve, edit-then-approve, or reject — the same code path a ✅ on the Discord card runs, and the card is edited to say it was applied via the portal. |
| **Extractions**  | Every model call: the prompt as sent, the raw JSON back, the latency, and the changes it produced. This is the prompt-tuning tool.                  |
| **Chat**         | Every chatbot interaction: who asked what, the reply, rounds, tool calls, latency and token counts, with per-model totals up top. On rows raised by a ❌ follow-up the "question" is the scheduler's own prompt (it starts `[Note from the scheduler…]`) — no member typed it. |
| **Members**      | The roster as synced from the bossing role, plus the chat aliases the extractor matches names against.                                              |
| **Reminders**    | Queued and sent reminder rows, with a link straight to each posted message in Discord.                                                              |
| **Config**       | Morning ping time, countdown offsets, pause chat watching, turn the extractor off, post the weekly digest now, **re-read the party channels**, and a **channel access** table showing what the bot may actually do in each one. The `.env`-only values are listed read-only underneath. |

Every time on every page is in the group's timezone, which is named in the header.
The pages are server-rendered; [htmx](https://htmx.org) (pinned, from cdnjs, with
an integrity hash) only upgrades the actions to in-place swaps. Every control is
a real form, so with the CDN blocked or JavaScript off the portal still works.

### Reaching it from your phone: the Caddy front door

The container publishes the port on the host's loopback only
(`ports: ["127.0.0.1:8080:8080"]`), so nothing on your LAN can see it. The
`caddy` service in the same compose file puts it on your tailnet, over HTTPS,
under a name of your own, with a real Let's Encrypt certificate:

- **A name that resolves everywhere but routes nowhere public.** The hostname's
  A record — at your DNS provider, DNS-only, never proxied — points at this
  machine's Tailscale IP, which nothing outside your tailnet can reach.
- **A certificate with no open port.** Caddy passes the ACME DNS-01 challenge
  with a DNS provider API token (in `.env.caddy`), so issuance never needs an
  inbound connection from the internet.
- **443 on the tailnet interface alone.** Compose publishes Caddy's port on
  `CADDY_BIND_IP` (set it to the Tailscale IP in `.env`); the LAN never sees
  it, and the loopback default keeps a fresh clone off the network entirely.

[`caddy/Caddyfile.example`](../caddy/Caddyfile.example) walks through the
setup. Then open **https://your-hostname** from any device signed into your
tailnet; the portal asks for the token once and keeps a seven-day cookie.

This replaced `tailscale serve`, and one thing did not survive the move:
`serve` attached your tailnet login to each request as a `Tailscale-User-Login`
header, which the portal could be told to trust with
`TRUST_TAILSCALE_HEADERS=true`. Caddy attaches no identity, so the flag now
buys nothing — and behind any reverse proxy it must stay **off** (the default).
A proxied header is whatever the client typed, and the "only this machine can
set it" argument the trust rested on went away with `serve`.

> **Keep the DNS record DNS-only.** Proxying the hostname through your DNS
> provider's CDN would publish the portal to the whole internet, exactly what
> this arrangement exists to avoid. The same goes for pointing the A record at
> anything other than the Tailscale IP.

To stop publishing it:

```sh
docker compose stop caddy
```

### `bossctl`

```sh
uv run bossctl schedule                   # from the project directory
uv tool install .                         # or install it on your PATH
docker compose exec bot bossctl schedule  # or from inside the container
```

It reads `ADMIN_TOKEN` from the environment or from the nearest `.env`, and talks
to `BOSSCTL_URL` (default `http://127.0.0.1:8080`). Ids may be any unique prefix.
An API refusal is printed as its message and exits non-zero.

```
bossctl schedule [--week next] [--channel ID] [--user ID] [--boss hstar]
bossctl fixed list | add | edit | rm
bossctl fixed add -b "hstar, hfa" -d mon -t 21:30 -c <channel-id> -m <user-id> -m <user-id>
bossctl pending                          # what the extractor proposed
bossctl approve <id> | reject <id>
bossctl amend <run> --to "wed 21:30"
bossctl cancel <run> | otot <run>
bossctl rsvp <run> yes --user <user-id>
bossctl members | nick <user-id> MY
bossctl reminders [--run <id>]
bossctl rescan [--window week] [--channel ID ...]   # default: every watched channel
bossctl rescan-stop [JOB]                # stop it after the current channel
bossctl rescans                          # the last few
bossctl channels                         # what a rescan would cover
bossctl swap <run> --out ID --in ID      # change the party for one week only
bossctl access                           # per-channel read/post permissions
bossctl status <run> planned|confirmed|otot|done|cancelled
bossctl restore <run>
bossctl digest [--channel <id>] [--week next]
bossctl ping <run> day_of                # posts a 🧪 TEST reminder now
bossctl extractions [-n 25] | extraction <id> [--no-prompt]
bossctl export --channel <id> --since 2026-06-01 --out data/exports/party.jsonl
bossctl config get [key] | config set <key> <value>
```

`config set` takes the four runtime settings the portal edits —
`day_of_ping_time`, `countdown_minutes`, `paused`, `extract_enabled`. Everything
else is `.env` and a redeploy.

### The API itself

Everything under `/api` is JSON, documented at
<http://127.0.0.1:8080/api/docs>. Errors are `{"error": "..."}` with a real
status code.

```sh
TOKEN=$(grep '^ADMIN_TOKEN=' .env | cut -d= -f2-)
curl -s http://127.0.0.1:8080/healthz                                   # -> ok
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/api/schedule
```

`GET /healthz` is the one unauthenticated route and returns nothing but `ok`; the
compose healthcheck uses it alongside the SQLite heartbeat.
