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

It binds `127.0.0.1:8080` by default, serves `GET /healthz`, and reports its
offline mode plus unavailable scheduler, storage, and Discord capabilities.
All non-loopback IPv4 and IPv6 binds are rejected in this bootstrap. Runtime
configuration comes only from the process environment; `.env` is not loaded automatically.
`healthcheck` accepts only a loopback `http://HOST:PORT/healthz` URL and has a
bounded timeout.

`SIGINT` and `SIGTERM` stop accepting work and drain HTTP requests up to
`KANADE_SHUTDOWN_TIMEOUT_SECONDS` (default 10, range 1–120). Logs are JSON and
emit only safe configuration-error descriptions, not environment values.

## Deliberate boundaries

`serve` without `--offline` fails: production adapters are not implemented.
`ctl`, `import`, and `export` are reserved commands that return a nonzero
not-implemented result. No scheduler, SQLite schema, persistence, Discord,
PWA, import/export, or mutation route exists in this bootstrap.

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
│   └── server.rs
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
