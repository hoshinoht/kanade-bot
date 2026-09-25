# Kanade v5.0 compatibility boundary (inventory draft)

Status: **evidence and proposed decisions only**. This document is the
`contract/freeze-v5-scope` worker artifact; it is not owner ratification and it
does not authorize production cutover. The parent must obtain the four owner
decision groups before product edits proceed.

Inventory snapshot: 2026-09-22, v4.9.0 source tree on `feat/kanade-v5`.
The machine-readable companion is [`inventory.json`](inventory.json), and the
deterministic source comparison is:

```text
uv run python scripts/check_v5_inventory.py
```

The checker intentionally uses only Python's standard library and the local
repository. It parses route decorators, router registration, slash-command
registration, Typer command decorators, `Settings`, `SCHEMA_SQL`, chat-tool
schemas/registries, worker task creation, and portal asset paths. It also
checks every cited source and test path. It does not import the running bot,
open Discord/Ollama, read private data, or call the network.

## Evidence summary

The current source contains the following individually inventoried surfaces:

| Surface | Count | Source of truth |
| --- | ---: | --- |
| Decorated routes | 123 | `bot/api/app.py`, `bot/api/routes_api.py`, `bot/api/routes_web.py` |
| Dynamically registered router/mount operations | 3 | `bot/api/app.py#create_app` |
| Discord slash commands | 35 | `bot/agent/commands.py`, `bot/agent/debug.py` |
| `bossctl` Typer commands/callbacks | 48 | `bot/cli.py` |
| Standalone/runtime entry points | 8 | `pyproject.toml`, `bot/__main__.py`, `bot/cli.py`, `bot/export.py`, `bot/extract/__main__.py`, `bot/health.py`, `bot/infrastructure/bundle/generate_contract.py`, `bot/portal_styles.py` |
| Configured healthcheck entrypoints | 1 | `deploy/Dockerfile#HEALTHCHECK`, `deploy/compose.yaml#services.bot.healthcheck`, `bot/health.py#main` |
| SQLite tables in `SCHEMA_SQL` | 25 | `bot/infrastructure/db.py#SCHEMA_SQL` |
| Environment-backed `Settings` fields | 53 | `bot/infrastructure/config.py#Settings` |
| `.env.example` keys | 52 | `.env.example` |
| Persisted runtime-config keys and markers | 17 | `bot/api/service.py#CONFIG_KEYS`, `bot/agent/client.py` |
| Discovered async task/loop creation sites | 6 sites / 7 task creations | `bot/__main__.py`, `bot/agent/client.py`, `bot/agent/rescan.py`, `bot/api/server.py`, `bot/extract/pipeline.py` |
| Model-visible chat tools | 12 | `bot/chat/tools/schemas.py#TOOLS` |
| Server-rendered/legacy portal assets | 73 | `bot/api/templates/**/*.html`, `bot/api/static/portal*` |

The table count is 25, including four v14 maintenance and delivery-ledger tables;
checker follows the executable v4 `SCHEMA_SQL`, not the proposed count. The
SQLite DDL is not a v5 compatibility contract.

## Disposition vocabulary

- **Retain** — the user-visible capability and its contract are in v5, with
  implementation changes allowed. The owner must freeze vectors/wire details
  before parity acceptance.
- **Import-only** — v4 data is consumed through the versioned portable bundle,
  but the v4 surface is not a v5 runtime/API surface. No raw SQLite import
  (sole exception: the testing import `kanade import v4`, `v4-import.md`).
- **Remove** — deliberately absent in v5. The absence and migration behavior
  must be tested; no placeholder may imply parity.
- **Defer** — not in the v5.0 implementation boundary yet. It remains an
  explicit release blocker or separately authorized integration, not a silent
  fallback.

Every item has one disposition in `inventory.json`. Policy maps in that file
are inheritance only: they provide the role owner and real test references for
each individually named item.

## Decided scope for this pass

These boundaries come from the authorized v5 direction and are treated as
decided scope, not as evidence that implementation or release is complete:

1. Governed durable chatbot memory is removed. There are no v5
   `chat_memory_*` tables, memory routes, memory slash/CLI commands, memory
   portal views, or memory bundle section. A migration preflight reports the
   excluded v4 row count; a memory section is rejected or explicitly reported,
   never silently imported.
2. The server-rendered Jinja/HTMX portal and SSE stream are not ported. An
   installable PWA replaces them and uses authenticated JSON fetch with bounded
   polling. The JSON service/application write path remains shared by PWA,
   CLI, and Discord.
3. v5 has one writer, one Rust process, and one portable bundle contract. The
   database owner must evaluate both SQLite and PostgreSQL on the actual
   platform before selecting either backend; neither is an owner-approved v5
   winner or default in this artifact. v4 DDL/schema version is not a
   migration interface.
4. The read-only schedule origin is public through an outbound-only Cloudflare
   Tunnel at `kanade.hoshinoht.dev/schedule`, gated by Discord OAuth, guild
   membership, and the bossing role. The admin origin remains tailnet-only;
   public routing never mounts admin, log, config, memory, or mutation paths.
5. The process is single-writer and the portable bundle is the forward and
   reverse migration contract. A live v4 SQLite file is never copied into or
   manipulated by Rust; online backup remains the v4 snapshot mechanism.
6. The actual kanata HTTP adapter is last: the provider trait, fake/offline
   provider, schemas, budgets, timeout/quarantine behavior, and contract tests
   land first. The final kanata package is integrated after non-kanata code and
   offline checks, before provider-dependent release gates/rehearsal. Direct
   Ollama coupling is not carried into Rust.
7. Retained original history and operational IDs are preserved privately through
   the portable migration contract, with memory excluded. Committed fixtures and
   vectors are synthetic/anonymized only; private exports, live IDs, and PII are
   never committed.

The retained bounded per-turn/channel conversation context in `ChatPilot` is
not governed durable memory. It is a runtime behavior seam and must not be
confused with the removed `bot/chat/memory.py` feature or its tables.

## Route boundary

### JSON API

The 55 `/api` handlers are individually listed. All non-memory scheduler,
roster, RSVP, reminder, digest, extraction, chat-history, audit, limits,
rescan, debug, message-export, catalog, config, and posting operations are
**Retain**. The eight `/api/memory...` handlers are **Remove**. The API route
prefix comes from `router = APIRouter(prefix="/api")`; the application mounts
that router in `create_app`.

`GET /api/messages` is NDJSON history export, not SSE; it remains a CLI/history
compatibility surface. `GET /api/limits` is the PWA polling source replacing
the HTML/SSE limits panel.

`/api/openapi.json`, `/api/docs`, and `/healthz` are app-level routes. The
stylesheet fallback is a generated/server-rendered portal artifact and is
**Remove** with the old portal. The static mount itself remains as the PWA
asset mount, with its contents replaced by versioned assets.

### HTML portal and stream

The 64 `routes_web.py` handlers are individually listed, including the dynamic
portrait and entry-art routes. HTML page, fragment, and form routes are
**Remove** as routes because templates/HTMX are not a v5 contract; their
capabilities map to retained JSON routes and PWA workflows. The unauthenticated
cached identity/artwork paths are **Retain** where they remain useful to the
PWA/public schedule. `/limits/events` is explicitly **Remove**: SSE is replaced
by bounded polling. Login/session safety is retained as behavior, but the old
HTML login route is not promised as a v5 route; the new PWA/OAuth/session
contract must be specified by the API/security owner.

The PWA must preserve semantic labels/status/errors, failed-input retention,
keyboard-visible focus, responsive narrow/wide operation, offline/error/retry
states, safe redirects/session handling, and destructive recovery. It must not
create a second write path.

### Public route allow-list (proposed implementation contract)

The public origin exposes only the PWA shell, Discord OAuth login/callback, and
read-only schedule/digest JSON. Everything else is absent/404 there by
construction. OAuth is ratified (2026-09-25): scope `identify` only, a fixed
callback per origin and a separate Discord application for the public origin;
see `oauth-security.md`. The exposure, guild gate, bossing-role gate, and
admin-origin separation are decided scope.

## Discord and CLI boundary

All 35 slash commands are listed individually. Fixed scheduling, bot pause/
resume, schedule/run mutation, RSVP, rescan, pings, style, limits, nick,
pingtime, admin `say`, and the debug/test operations are **Retain** subject to
the provider/PWA/security changes. The five `/memory` subcommands are
**Remove**. Group registration in `register_commands` is checked separately so
decorated-but-unregistered commands cannot disappear from the matrix.

All 48 `bossctl` command/callback surfaces are listed individually. The eight
`bossctl memory ...` commands are **Remove**. The remaining commands are
**Retain** through the same Rust binary/application service; preserving the
`bossctl` executable name as an alias/symlink is recommended for compatibility
but remains an owner decision. A second scheduling implementation or direct
SQLite access is not allowed.

The complete discovered module/script set is `python -m bot`, `bossctl`,
`python -m bot.cli`, `python -m bot.export`, `python -m bot.extract`,
`python -m bot.health`, `python -m bot.infrastructure.bundle.generate_contract`,
and `python -m bot.portal_styles`. The service runtime and portable migration
behavior are retained conceptually; raw JSONL export
and direct Ollama extraction are import-only/deferred or replaced by the v5
bundle/provider contracts as recorded in the inventory. `python -m bot.health`
is the retained heartbeat plus `/healthz` contract and is the configured
Docker/Compose healthcheck. No Python runtime is required in the v5 image, and
Python rollback code remains available during the v5.0 rollback window. The
checker has configuration-negative coverage for removal or command changes in
each healthcheck source; there is currently no dedicated runtime test of
`bot.health.check/main`, which remains an explicit validation gap rather than
an invented coverage claim.

## Data and migration boundary

The 25 current tables are individually listed. The 17 non-memory families are
retained as v5 behavior/state concepts: roster, fixed timings, runs, proposals,
RSVPs, reminders, weekly digest bindings, message/extraction history, debug
card bindings, decline notices, runtime config, rescan history, chat history,
audit, and rate-limit overrides. The v5 schema is newly designed; table names,
columns, and `SCHEMA_VERSION=13` are not wire compatibility promises.

The four governed-memory families are **Remove**:
`chat_memory_enrollments`, `chat_memories`, `chat_memory_events`, and
`chat_memory_retrievals`. Before migration, v4 counts these rows and reports the
excluded count. Bundle import rejects/reports a memory section before mutating
the target. Secrets, private files, artwork bytes, and a live SQLite file are
never bundle inputs.

The migration owner still needs the upstream bundle-v1 producer contract with
required history sections (`messages`, `extractions`, `chat_interactions`,
`audit`, `rescan_jobs`), checksums, active delivery bindings, and durable
maintenance prepare/resume. Pending proposals and active cards cannot be
silently regenerated: delivered bindings, reaction/card state, digest state,
decline state, and deduplication markers must survive or block cutover.
Retained history keeps its original IDs privately in the bundle/import path;
synthetic committed fixtures must not become a substitute for private migration
fidelity. Delivery bindings are retained only for the current boss week
(Thursday through Wednesday). Out-of-week active bindings must be explicitly
accounted for and retired without replay; the history retention policy is not
pruned by this binding cutoff. Pending extraction/chat proposals block final
export until resolved or explicitly retired.

## Configuration boundary

`Settings` fields and `.env.example` keys are separately checked because the
current example does not document every field and also contains deployment/CLI
keys not owned by `Settings`. Every example key has an explicit disposition,
including `BOSSCTL_URL` and `CADDY_BIND_IP`. Retain guild, channel, role, timezone/reset,
scheduling, storage, auth, tailnet, API, logging, extraction, backfill, chat
gating, rate, and deployment-hardening behavior. Rename/map fields only through
the ratified v5 config contract.

Remove direct `OLLAMA_*` configuration from Rust; the provider owner supplies a
versioned endpoint/model/auth/budget contract without guessing kanata's final
shape. Remove `CHAT_MEMORY_ENABLED` and legacy persona/staging path fields from
the v5 runtime configuration; the prompt-layout owner supplies the compiled
persona/profile contract. `BOSSCTL_URL` and the CLI alias remain optional
compatibility choices, not silently assumed requirements.

## Retained domain, extractor, chat, and portal surfaces

The `surfaces` section identifies the behavioral seams individually rather than
claiming that a whole package is parity. The important constraints are:

- Domain uses aware UTC values, configured IANA timezone, distinct
  calendar-week versus reset-based boss-week semantics, canonical boss tokens,
  unique-prefix IDs, and source-backed boss knowledge.
- Scheduler keeps fixed-run materialization/adoption, one-off/manual run
  changes, status/RSVP transitions, persisted reminders, stale suppression,
  day-of/countdown policy, digest replacement, decline notices, card refresh,
  explicit mention allow-lists, roster/channel authorization, audit best effort,
  reconnect, heartbeat, and graceful shutdown.
- Extraction keeps gate → window → resolve → merge → match → commit and
  rescan/backfill orchestration. Model output is untrusted; cards/proposals are
  the only schedule write path. Kind-specific commit behavior is listed
  separately in the machine inventory.
- Chat keeps role/channel/mention gating, prompt/persona assembly, source-backed
  strategy, read/write tool separation, proposal authorship, rate limits,
  follow-ups, injection/note-spoofing defenses, and the exactly 12 model-visible
  tools. All completions go through the provider seam. Durable governed memory
  and memory overlays are removed.
- Server-rendered templates, partials, HTMX/SSE JavaScript, and generated CSS
  are individually inventoried as removed portal assets. The PWA owner must
  port workflow semantics, not DOM identity.

## Workers and delivery bindings

The current background sites are individually checked: the 30-second reminder/
maintenance tick, sequential rescan worker, in-process API task, extractor
debounce timers, stale-card refresh drain, and process lifecycle tasks. v5 keeps
one event/runtime and one writer, persists delivery state, and uses injected
aware clocks/fakes in tests. Memory cleanup is not retained as a feature; any
retention work in v5 must be explicitly scoped to non-memory data.

The delivery ledger must preserve pending/delivered reminder state and external
message/channel bindings, active digest bindings, decline cooldown/message
state, proposal/card bindings, and deduplication markers. Only future unsent
derived reminders may be materialized. A visible reaction or role change during
maintenance must be reconciled authoritatively before handoff/commit.

## Open decisions and conservative recommendations

These are intentionally unresolved; no sign-off is invented here.

| Decision | Current status | Conservative recommendation pending sign-off |
| --- | --- | --- |
| Release owner | Unresolved; role not named in plan state; user retains release approval | Assign one release/operations owner before stack/release gates; the role owns go/no-go evidence, while the user retains release approval. |
| Rollback owner | Unresolved; role not named in plan state; user retains rollback approval | Assign one migration/rollback operator and one witness; only that role prepares reverse bundle evidence, while the user retains rollback approval. |
| Emergency RPO | Normal rollback policy decided; emergency exception unresolved | Normal rollback has zero accepted-write loss through the portable bundle. No lossy checkpoint restart is pre-approved; any emergency exception requires explicit user approval and quantified reconciliation. |
| Active-card horizon | Decided for cutover | Preserve bindings only for the current boss week (Thursday-Wednesday). Explicitly account for and retire out-of-week active bindings without replay; do not prune retained history at this cutoff. |
| Pending proposals | Decided migration gate | Block final export until every pending extraction/chat proposal is resolved or explicitly retired. Never silently drop or regenerate it. |
| `bossctl` executable alias | Optional | Preserve `bossctl` as an alias to the one Rust binary unless the CLI owner explicitly accepts a breaking rename. |
| Persona-styled reminders | Optional | Default to deterministic plain reminders. Add a bounded provider rewrite only after provider availability, failure fallback, notification policy, and budget are approved. |
| PostgreSQL | Selected evaluation before backend choice | Evaluate SQLite and PostgreSQL on the actual platform, including provisioning, secrets, transactions, backup/restore, tests, image/runtime cost, and rollback. Record the winner only after owner review; this artifact does not assume SQLite. |
| Kanata transport/auth/schema | External and unverified | Freeze the provider trait/fake first; integrate the actual HTTP adapter last, immediately before provider-dependent release checks, with no direct Ollama fallback in Rust. |

## Acceptance handoff

The evidence slice is ready for review when `check_v5_inventory.py` passes and
the parent has reviewed every `Remove`/`Defer` row, the public route allow-list,
the migration/history treatment, and the four owner decision groups. This
artifact does **not** claim that `freeze-v5-scope` is ratified or that the
workplan step is complete. Parent/user ratification is required before any
product-code relocation or Rust implementation wave.
