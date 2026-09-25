# v5 runtime bootstrap

`kanade` is the single Rust executable. Cargo package version `1.0.0-beta.1`
corresponds to the planned release label `1.0.0b`; Cargo does not accept the
latter as a semver version.

## Available now

Only the explicit offline development server is runnable:

```sh
KANADE_TIMEZONE=Asia/Kuala_Lumpur kanade serve --offline
KANADE_HEALTHCHECK_URL=http://127.0.0.1:8080/healthz kanade healthcheck
```

The admin listener binds `127.0.0.1:8080` by default; `GET /healthz` reports
offline mode plus unavailable scheduler, storage, and Discord capabilities.
All non-loopback IPv4 and IPv6 binds are rejected (container networking gets an
explicit opt-in in harden-and-package). Runtime configuration comes only from
the process environment; `.env` is not loaded automatically. `healthcheck`
accepts only a loopback `http://HOST:PORT/healthz` URL and has a bounded
timeout.

### HTTP environment

| Variable | Default | Meaning |
|---|---|---|
| `KANADE_ADMIN_BIND` | `127.0.0.1:8080` | Admin listener, loopback only. Replaces `KANADE_BIND`, which is now refused with a rename error. |
| `KANADE_PUBLIC_BIND` | unset | Public listener, loopback only; it exists only when set and must differ from the admin bind. |
| `KANADE_ADMIN_HOST` | unset | Exact `host[:port]` the admin listener serves; unset accepts only `localhost`, `127.0.0.1`, `[::1]` (any port). |
| `KANADE_PUBLIC_HOST` | unset | Exact `host[:port]` of the public listener; required with `KANADE_PUBLIC_BIND`, must differ from the admin host. |
| `KANADE_TRUSTED_PROXY` | unset | IP of the edge peer; only it may supply `X-Forwarded-*`/`Forwarded` and `Tailscale-*` headers to admin. |
| `KANADE_CLOUDFLARED_PEER` | unset | IP of the cloudflared peer; only it may supply `X-Forwarded-*` and `CF-*` headers (client IP from `CF-Connecting-IP`) to public. |
| `KANADE_WEB_DIR` | unset | Web workspace root; serves `apps/admin/dist` and `apps/public/dist` (same layout as `tools/pwa-mock`). Unset serves no shell. |
| `KANADE_BOSS_DIR` | unset | Private boss art root (`portraits/`, `portraits/icon/`, `artwork/entry/`). Unset or missing art is 404. |
| `KANADE_IDENTITY_DIR` | unset | Cached `avatar.*`/`banner.*`; unset serves generated SVG stand-ins. |

Empty values count as unset. Values are never echoed in errors or logs.

### Listeners

Two routers, authorized by mounting: the public router is built without any
admin route, so every `/api/admin/*`, `/__*` and `/healthz` path is a generic
`404` there for every method. Both apply, outermost first: security headers,
Host allow-list (`421 misdirected` for unknown, missing, duplicated or
mismatched absolute-form hosts), proxy trust (forwarding, Cloudflare and
Tailscale headers from any other peer are stripped before routing), then a
64 KiB declared-body limit (`413`) and a 30 s handler timeout (`503
timeout`). Errors are `ApiError` bodies `{error, message}` with generic text.

Headers match `tools/pwa-mock` (CSP, report-only Trusted Types, `nosniff`,
`no-referrer`, COOP/CORP `same-origin`, Permissions-Policy, per-path
Cache-Control) without the mock's dev-only `report-uri`; non-2xx answers are
`no-store`; `Strict-Transport-Security` is sent on the public origin only; no
CORS headers are ever sent.

| Route | Admin | Public |
|---|---|---|
| `GET /healthz` | Direct loopback peers only (any loopback Host name); relayed requests 404 | 404 |
| `GET /api/identity`, `/identity/{avatar,banner}` | yes | yes |
| `GET /api/public/status` | 404 | `{portal: "closed"}` |
| `/api/public/*`, `/art/*` | 404 / art | `503 closed` |
| `GET /art/{portraits,icons,entry}/{key}` | file or 404 | `503 closed` |
| other `GET`/`HEAD` | the app's static files; extensionless paths get `index.html`, missing files 404 | same, public app |

Admin API routes arrive with admin authentication; until then `/api/admin/*`
is `404`. Static paths reach the filesystem only as plain segments (no `..`,
dotfiles, percent-encoding or backslashes) and only inside the canonical app
root, so symlinks cannot escape it.

Known gap: `axum::serve` sets no header-read timeout, so slow-header clients
are bounded by the edge/cloudflared in front of the loopback listeners until
harden-and-package revisits connection limits.

`SIGINT` and `SIGTERM` stop accepting work and drain HTTP requests up to
`KANADE_SHUTDOWN_TIMEOUT_SECONDS` (default 10, range 1–120). Logs are JSON and
emit only safe configuration-error descriptions, not environment values.

## Deliberate boundaries

`serve` without `--offline` fails: production adapters are not implemented.
`ctl`, `import`, and `export` are reserved commands that return a nonzero
not-implemented result. `serve --offline` wires no scheduler, persistence,
Discord, import/export, admin API or mutation route.

The runtime installs Rustls' `ring` provider before command processing. SQLx
and Twilight are intentionally absent until storage and Discord work needs
them, so the production graph has no SQLx MySQL/RSA path from the spike.

## Source layout

The bootstrap is organized by responsibility rather than a flat source
directory:

```text
src/
├── lib.rs
├── main.rs
├── api/
│   ├── mod.rs
│   ├── server.rs        # binding, both listeners, graceful drain
│   ├── listeners.rs     # per-origin Site policy and router assembly
│   ├── admin/mod.rs     # admin-only routes
│   ├── public/mod.rs    # public-only routes (closed portal)
│   ├── guard/           # host, proxy, headers, limits
│   ├── assets.rs        # shell, art, identity
│   └── error.rs         # ApiError
├── cli/
│   ├── mod.rs
│   └── healthcheck.rs
└── runtime/
    ├── mod.rs
    ├── application.rs
    ├── config.rs
    ├── error.rs
    ├── logging.rs
    └── tls.rs

tests/
├── api/                 # listeners, guards, headers, static serving
└── runtime_bootstrap/
    └── main.rs
```

`main.rs` remains the entrypoint: it installs the crypto provider, captures
process inputs, delegates to the runtime application, and maps failures to
exit codes. `lib.rs` only declares the top-level module groups. The nested
module paths are canonical; the former flat paths are not re-exported.

### Feature-boundary roadmap

Future implementation directories will be added with their first real module;
the bootstrap does not create empty placeholders:

- `bot/` — Discord client, commands, cards, and workers.
- `chat/` — conversation tools, prompts, and safety.
- `extract/` — extraction pipeline.
- `domain/` — pure scheduling rules.
- `infrastructure/` — persistence and external integrations.
- `api/` — routes, authentication, and wire types.
- `runtime/` — process start, configuration, and lifecycle.
- `cli/` — operational commands and outbound healthcheck client.
