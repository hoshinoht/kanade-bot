# Goal

Selecting a major persona must change its identity, baseline behaviour, and baseline staging copy as one coherent unit for the next chatbot answer. Yuuki Sakuna should resolve to the content currently deployed at `identities/persona.md`, `behaviours/default.md`, and `behaviours/staging.yaml`; Nazupi should resolve to `identities/nazupi.md`, `behaviours/profiles/nazupi.md`, and the matching Nazupi profile-staging YAML after deployment owners copy/normalize those private files into the new layout.

## Scope and non-goals

In scope:

- A manifest-backed bundle-by-directory convention with fixed component filenames.
- A loader/runtime split that validates and swaps complete immutable bundles.
- A per-generation snapshot for both normal answers (`_answer`) and rejection follow-ups (`_ask_instead`) so work spanning `await` points cannot mix old and new persona components.
- Safe manifest IDs and legacy aliases, complete-component failure semantics, and private-content-safe diagnostics.
- Portal/API/CLI, SQLite config-row, and `PERSONA_PATH` compatibility without a database schema migration.
- Existing behaviour profiles as orthogonal per-member/per-role overlays, including explicit treatment of the existing `nazupi` profile.
- Tracked example/template and documentation reorganization; operator-owned migration instructions for ignored live files.

Non-goals:

- Do not change the code-owned prompt policies under `bot/chat/prompts/`.
- Do not modify live ignored persona files in implementation; deployment owners perform the documented copy/merge.
- Do not migrate or clear saved member reply-style rows.
- Do not add portal editing for major-persona files or manifests.
- Do not remove legacy-layout support in this release.

## Authorization, decisions and assumptions

- **Authorization:** PLAN ONLY. Readiness does not authorize implementation. This child planner must return control to the parent.
- **Decision — use YAML:** use `config/personas/personas.yaml` because a safe human label, one deployment default, and explicit legacy alias mapping cannot be reliably derived from private prompt text or directory names. YAML already has a pinned runtime dependency. The YAML points only to a **default persona ID** and alias IDs; it does **not** point to identity/behaviour/staging files.
- **Decision — fixed files:** every manifest persona uses `personas/<id>/{identity.md,default.md,staging.yaml}`. Reject configurable component paths because they reintroduce cross-bundle coupling, typo-prone pointers, and path traversal/containment work.
- **Decision — no SQLite migration:** retain config key `persona`. Old filename values are accepted aliases at read time. New successful selections persist canonical IDs. Do not increment `SCHEMA_VERSION` or touch migration code.
- **Decision — full-bundle fallback:** never combine an identity from one bundle with default behaviour or staging from another. Missing/invalid selected bundles fall back as a whole to the manifest default; if the manifest/default is unusable, use the complete tracked example bundle and mark the runtime at risk.
- **Decision — symlinks:** reject every symlink bundle directory and every symlink required component file, even when it resolves inside the persona root or into another bundle. Uniform rejection keeps bundle ownership literal and auditable.
- **Decision — profile precedence:** active reply-profile Markdown follows the selected persona's default behaviour in the prompt and may override its examples/voice exactly as today. A profile staging YAML is a partial override merged onto the selected persona's baseline `StagingLines`.
- **Decision — existing `nazupi` profile users:** copy, do not move, `behaviours/profiles/nazupi.md` and its staging override when creating the Nazupi major bundle. Keep the original profile and saved rows untouched. Thus a user/role selecting profile `nazupi` continues to get that overlay on Yuuki Sakuna or any other persona; on the Nazupi major persona the same-named profile still applies by normal profile precedence. This can be textually redundant, but it is deterministic and avoids hidden name-based exceptions or user-data migration. Operators may later unpublish/rename the overlay as a separate product decision.
- **Assumption:** persona IDs use stable lowercase slugs (`^[a-z0-9][a-z0-9-]{0,49}$`); labels are non-empty display text; aliases are exact legacy config tokens, not paths.
- **Assumption:** deployment migration can include a restart after an atomic manifest/tree update. Portal selection among already valid bundles remains hot and affects the next answer.
- **Constraint:** `AGENTS.md`, `bot/chat/agent.py`, and `tests/test_chat_agent.py` contain unrelated user work. An executor must inspect the live diff before editing, make narrow changes, and never reset/overwrite those edits. New atomicity tests should live outside `tests/test_chat_agent.py` unless integrating there is unavoidable.

## Approach and verified references

### Recommended filesystem

```text
config/personas/
├── README.md
├── personas.example.yaml                 # tracked manifest template
├── personas.yaml                         # live, deployment-private manifest
├── personas/
│   ├── example/                          # tracked complete fallback/template
│   │   ├── identity.md
│   │   ├── default.md
│   │   └── staging.yaml
│   ├── yuuki-sakuna/                     # live, ignored
│   │   ├── identity.md
│   │   ├── default.md
│   │   └── staging.yaml
│   └── nazupi/                           # live, ignored
│       ├── identity.md
│       ├── default.md
│       └── staging.yaml
└── behaviours/
    └── profiles/                         # orthogonal overlays, unchanged concept
        ├── example.md                    # tracked
        ├── <profile>.md                  # live, ignored
        └── staging/
            ├── example.yaml              # tracked partial-override template
            └── <profile>.yaml            # live, ignored partial override
```

Remove the superseded tracked major-persona templates (`persona.example.md`, `identities/example.md`, `behaviours/default.example.md`, and `behaviours/staging.example.yaml`) after the new complete tracked example bundle is in place. The loader still recognizes ignored live legacy files when no manifest exists.

`.dockerignore` must mirror the new tracked/private boundary: admit `personas.example.yaml`, `personas/example/identity.md`, `personas/example/default.md`, `personas/example/staging.yaml`, and the existing tracked profile examples while excluding `personas.yaml`, every non-example bundle, and live profile content. `Dockerfile` must replace its explicit old-template copies with explicit copies of all three new fallback files (or an equivalently narrow copy proven by the image test). This is required because the existing Dockerfile names the old identity/default templates and does not explicitly package a fallback staging file.

### Manifest schema

`personas.example.yaml` and the private `personas.yaml` use:

```yaml
schema_version: 1
default: yuuki-sakuna
personas:
  - id: yuuki-sakuna
    label: Yuuki Sakuna
    aliases:
      - persona.md
  - id: nazupi
    label: Nazupi
    aliases:
      - nazupi.md
```

Rules:

- Require exactly top-level `schema_version`, `default`, and `personas`; reject unknown keys and unsupported versions.
- Require a non-empty list with unique IDs, aliases that collide with neither another alias nor any canonical ID, a known `default`, and non-empty labels. Reject unknown descriptor keys. Bound IDs/labels/list size to prevent accidental unbounded portal/config data.
- IDs are stable machine values stored on future selection; labels are the human-facing portal choices. Labels need not be unique, so the UI may show a secondary slug when labels collide.
- Aliases exist only for migration. Reject empty aliases, aliases containing `/`, `\\`, NUL, `.`/`..`, or absolute/path-shaped values. Permit legacy filename tokens such as `persona.md` and `nazupi.md`.
- Do not accept `identity`, `behaviour`, `staging`, `directory`, or other path fields.

Each bundle `staging.yaml` is a complete flat mapping, not the old combined default/profiles document:

```yaml
schedule: "Checking your runs…"
guide: "Reading checked-in notes…"
guide_named: "Reading checked-in notes for {boss}…"
write: "Drafting the proposal card…"
generic: "Thinking…"
```

All five known keys are required; unknown keys and non-string/empty values are errors. Per-profile staging files remain partial flat mappings and inherit absent keys from the currently active persona bundle.

### Runtime and ownership model

Add `bot/chat/persona_catalog.py` as the deployment-config boundary:

- `PersonaDescriptor(id, label, aliases)` — manifest metadata only.
- `PersonaCatalog(default_id, descriptors, mode)` — validates/resolves a canonical ID from a canonical ID or legacy alias and reports portal-safe options. Selection options contain only descriptors whose three fixed components pass validation; invalid descriptors retain path-only diagnostics but are not selectable.
- `PersonaBundle(id, label, identity, default_behaviour, staging, source, fell_back, issue)` — one fully loaded immutable base bundle. Source metadata contains paths/IDs only, never prompt text in API/errors/logs.
- `PersonaRuntime(bundle, profile_staging)` — one immutable reference containing the selected base and profile staging overrides merged against that base.
- `load_catalog(root)`, strict `prepare_runtime(catalog, selection)`, fallback-capable `load_configured_runtime(...)`, and `load_example_bundle()` own manifest/legacy discovery, containment checks, reads, validation, fallback, and diagnostics. Strict preparation is for requested mutations and never falls back; configured startup/reload loading may fall back as specified below. `load_example_bundle()` is the single direct fallback/image-verification entry point and validates all three tracked components.

Keep `bot/chat/persona.py` focused on prompt parsing/assembly (`PromptComponents`, voice/example extraction, system prompt policy order). During the compatibility window it may expose thin wrappers for existing callers/tests, but directory enumeration and deployment fallback logic move to `persona_catalog.py`. Keep staging value parsing in `bot/chat/progress.py`; expose separate strict helpers for a complete persona baseline and a partial profile override rather than making the catalog duplicate staging validation.

`ChatPilot` holds one `_persona_runtime` reference. `reload_persona()` becomes a compatibility wrapper that builds a complete runtime first and replaces the reference once. Add a prepare/activate split used by `service.set_config`:

1. Resolve the submitted ID against the current catalog.
2. Load identity, default behaviour, baseline staging, and all profile-staging merges into a candidate runtime without mutating DB/runtime.
3. If validation fails, return a private-safe `400` and retain both the DB row and active runtime.
4. Persist the candidate's canonical ID to config key `persona`.
5. Assign `_persona_runtime = candidate` synchronously, with no `await` between persistence and assignment.

The database and in-memory assignment cannot be one ACID transaction, but preloading makes the only post-write operation an in-process reference assignment that cannot fail under normal operation. If `Repo.set_config` fails, no swap occurs. Audit only canonical old/new IDs or labels, never loaded text.

At the beginning of each generation entry point—`_answer` for member messages and `_ask_instead` for rejection follow-ups—capture `runtime = self.persona_runtime()`. In each path, thread that same runtime through reply-overlay resolution, staging selection, prompt/conversation assembly, generation and every model/tool/rewrite round, budget rebuilding, and trailing voice reminders. `_ask_instead` must capture before it resolves its member overlay, builds its prompt, or posts its generic staging placeholder. This prevents a portal update during any later `await` from mixing old identity/default text with new staging/voice. The next invocation of either entry point captures the new snapshot.

### Safe paths and invalid/missing behavior

- Never join a user-supplied selection directly. Resolve it by exact membership in parsed descriptor IDs/aliases, then join the already validated canonical ID with fixed filenames.
- Use `lstat`/`Path.is_symlink()` before reads and reject every symlink at the bundle-directory and required-component levels. This includes links to targets inside the same bundle, links to another bundle under the same root, and links outside the root. Then resolve root/candidates, require containment beneath the literal `config/personas/personas/<id>` directory, and accept regular files only.
- Read UTF-8, strip, require non-empty identity/default Markdown, and validate complete staging through `progress.py`. Do not introduce an arbitrary prompt-file size policy as part of this redesign; existing prompt budgeting remains unchanged.
- A portal/API request for an unknown or invalid bundle fails before persistence/swap and names only safe IDs/component filenames.
- If a stored selection is an alias, resolve it to the canonical bundle but leave the row untouched until a successful operator selection. This preserves existing DB state and audit meaning.
- If a stored selection is unknown/unloadable at startup/reload, load the manifest default as a whole and expose `configured`, `effective`, `fell_back`, and a safe issue in Config. If the default or manifest is invalid, load the complete tracked `personas/example` bundle as a whole, offer no invalid choices, and show an at-risk recovery message. Never silently use code staging with deployment identity/behaviour.
- A malformed/missing profile staging override is orthogonal: quarantine that override with a path-only warning and let that profile inherit the active persona baseline. It must not invalidate or block selection of an otherwise valid major persona.
- When `personas.yaml` is absent, use a legacy catalog adapter: current identity filename selection and `PERSONA_PATH` behavior plus global `behaviours/default.md` and `behaviours/staging.yaml`. This is compatibility mode, clearly warned/documented, and can be removed only in a later release.
- Cached runtimes survive files disappearing mid-turn. A later explicit reload/switch either produces a complete new runtime or keeps the old runtime; it never clears one cache at a time.

### Portal, DB, environment, and profile behavior

- Preserve config key/API input name `persona`; its value becomes a canonical persona ID in manifest mode and remains a filename in legacy mode.
- Keep the API additive and preserve these existing fields and types:
  - `persona_file: str` — basename of the active identity source: `identity.md` in manifest mode/new tracked fallback; the selected or legacy fallback identity basename in legacy mode.
  - `persona_fallback: bool` — `true` only when the configured token could not be used and a complete manifest default or tracked example runtime is effective. A valid legacy alias such as `persona.md` resolving to Yuuki is not itself a fallback.
  - `persona: str` — the raw persisted config token, preserving an old alias until the next successful operator selection stores a canonical ID.
  - `persona_choices: list[str]` — fully loadable canonical IDs in manifest mode; selectable filenames in legacy mode; empty when only the tracked fallback is usable.
- Add exactly these fields and types:
  - `persona_labels: dict[str, str] = {}` — labels keyed by every value in `persona_choices`; manifest labels in manifest mode and filename-as-label in legacy mode. A mapping avoids adding another object list to the generic CLI surface.
  - `persona_effective: str = ""` — active canonical ID in manifest mode, active filename in legacy mode, or `example` for the tracked fallback.
  - `persona_effective_label: str = ""` — active manifest label, legacy filename, or `Example persona` for the tracked fallback.
  - `persona_catalog_mode: Literal["manifest", "legacy", "fallback"] = "legacy"` — `manifest` when a valid manifest catalog is driving selection (including fallback to its valid default), `legacy` when `personas.yaml` is absent and old layout rules are active, and `fallback` when no usable manifest/legacy runtime exists and the tracked example bundle is active.
  - `persona_issue: str | None = None` — path/ID-only recovery summary when fallback/degraded state exists; never prompt text.
- Update `ConfigOut` with those exact defaults/types and keep `ConfigIn.persona: str | None` unchanged. The portal consumes `persona_choices` plus `persona_labels`; no existing field changes element type.
- Make `bossctl config get` generic rendering total for scalars, string lists, mappings, and lists containing objects: retain comma-joining only for all-string lists and JSON-encode object collections/mappings deterministically. This also protects existing object-list config fields such as behaviour plugins.
- The portal label becomes **Persona**, each option leads with its human label, and the action becomes **Use this persona**. Show the slug secondarily only for duplicate labels or troubleshooting. The fallback state must say what is configured, what complete bundle is effective, and how to recover; do not expose prompt content. This follows `HPK-4 — Leading identity`, `COPY-1 — UI copy`, `RECOVER-1 — Prevention + recovery`, and `STATE-1 — Derive states`.
- `PERSONA_PATH` remains accepted as a deprecated seed input. In manifest mode resolve `Path(PERSONA_PATH).name` as a canonical ID or alias; if it does not resolve, seed the manifest `default`. In legacy mode retain current basename semantics and external-path fallback. `seed_config` remains insert-only, so an existing row is never overwritten on restart.
- Keep `staging_path` only for legacy mode and document its deprecation. Keep `staging_profiles_dir` because reply-profile staging remains a separate supported extension point. Do not add a new environment variable or dependency.
- Existing `nazupi` profile users retain their saved/role-selected overlay. Profile precedence stays role assignment, then selectable member choice, then persona default. A missing/unpublished saved profile continues to fall back to the selected persona's default as today.

### Deployment-private migration recipe (document, do not execute)

1. Back up the private persona tree and SQLite DB.
2. Create both bundle directories without deleting legacy files.
3. Copy `identities/persona.md` to `personas/yuuki-sakuna/identity.md`; copy `behaviours/default.md` to its `default.md`; extract/normalize the complete `default` staging mapping from `behaviours/staging.yaml` into its flat `staging.yaml`.
4. Copy `identities/nazupi.md` to `personas/nazupi/identity.md`; copy (do not move) `behaviours/profiles/nazupi.md` to its `default.md`; create a complete Nazupi `staging.yaml` by merging the matching profile override onto the old global default. Retain the original Nazupi profile Markdown/YAML for existing profile users.
5. Write `personas.yaml` last via temporary-file rename, with aliases for existing DB/env tokens. Restart, verify Config reports the expected configured/effective persona, then hot-switch both directions and verify identity/default/staging together.
6. Retain legacy private files for rollback during the compatibility release; remove them only after rollback is no longer needed.

### Verified current references

- `bot/__main__.py:87-117` seeds config key `persona` from `Path(settings.persona_path).name` and does not overwrite existing rows.
- `bot/agent/client.py:223-241` returns the stored filename and live directory-derived choices.
- `bot/chat/persona.py:15-29,295-415,491-523` currently mixes deployment path/loading concerns with prompt component assembly.
- `bot/chat/agent.py:541-686` caches identity/default/staging separately and `reload_persona()` does not reload staging.
- `bot/chat/agent.py:885-938,1080-1128,1312-1343,1584-1591` shows both `_answer` and `_ask_instead` resolving overlay/staging/prompt state before async generation while later trailing reminders can re-read persona state.
- `bot/chat/progress.py:68-171,183-193` already validates staging keys and merges profile overrides over one global default.
- `bot/behaviour_plugins.py:305-347` defines role/member/default profile precedence independently of major persona selection.
- `bot/api/service.py:2172-2217,2246-2281,2459-2477` exposes filename choices and currently persists before clearing only identity/default caches.
- `bot/api/models.py:472-525` and `bot/api/templates/config.html:100-117,302-313` expose/render the current filename/fallback contract.
- `bot/infrastructure/config.py:123-129` owns legacy persona/staging environment paths.
- `.gitignore:37-55`, `config/personas/README.md`, `.env.example:171-172`, and `docs/chatbot.md:82-125,152-167` document and track the old split layout.
- `Dockerfile:28-39` copies config and then explicitly names old identity/default fallback files but no staging fallback; `.dockerignore:1-10` admits only the old tracked persona paths.
- `bot/cli.py:1320-1364` comma-joins every list in generic config output, while config already contains object-list values; `tests/test_cli.py:545-573,1265-1277` covers basic config get/set but not persona IDs or object-list output.
- Relevant coverage exists in `tests/test_chat_persona.py`, `tests/test_chat_progress.py`, `tests/test_chat_followup.py`, `tests/test_portal_config.py`, `tests/test_chat_config.py`, `tests/test_behaviour_plugins.py`, and `tests/test_cli.py`.

## Work packages

### catalog-loader/bundle-schema-loader
<!-- workplan-phase-id: catalog-loader -->
<!-- workplan-step-id: bundle-schema-loader -->

- **Objective:** provide one strict, path-safe catalog/loader that returns only complete immutable persona runtimes and adapts the old layout when no manifest exists.
- **Owned files:** new `bot/chat/persona_catalog.py`, loader-related sections of `bot/chat/persona.py` and `bot/chat/progress.py`, new `tests/test_persona_catalog.py`, and focused additions to `tests/test_chat_persona.py`/`tests/test_chat_progress.py`.
- **Blocked/shared files:** do not edit private live files. Coordinate signatures exported from `persona.py` with the runtime package before integration.
- **Dependencies and integration order:** first package. Its dataclasses and prepare/load API are prerequisites for runtime/API work.
- **Approach:** parse the exact schema above; validate IDs/aliases/default; resolve fixed contained paths; parse all three components; construct full manifest-default or tracked-example fallback; expose legacy mode without allowing partial manifest bundles.
- **Acceptance criteria:**
  - Yuuki/Nazupi descriptors load the fixed three files and return canonical IDs/labels.
  - `persona.md` and `nazupi.md` aliases resolve without interpreting them as paths.
  - Traversal, absolute/path-shaped IDs or aliases, duplicate IDs/aliases, unknown keys/version/default, every symlink bundle directory/component (including within-root and cross-bundle links), empty Markdown, and incomplete/invalid staging are rejected.
  - An invalid selected bundle yields one complete default/fallback bundle, never mixed components.
  - Profile staging partials merge against whichever persona baseline is supplied.
  - Legacy layout behavior remains available only when the manifest is absent.
- **Validation:** `uv run pytest -q tests/test_persona_catalog.py tests/test_chat_persona.py tests/test_chat_progress.py`; expected: all tests pass with explicit assertions for source IDs and all five staging values.
- **Escalation trigger:** a need for arbitrary component paths, inheritance between major personas, a second config format, or a new dependency requires parent/product review.

### runtime-surfaces/atomic-runtime-swap
<!-- workplan-phase-id: runtime-surfaces -->
<!-- workplan-step-id: atomic-runtime-swap -->

- **Objective:** make a selected complete runtime the sole source for identity/default/staging and pin it for an entire normal-answer or rejection-follow-up generation.
- **Owned files:** narrow integration changes in `bot/chat/agent.py`, `bot/agent/client.py`, `bot/api/service.py`, `bot/api/models.py`, `bot/api/templates/config.html`, `tests/test_chat_followup.py`, `tests/test_portal_config.py`, and `tests/fake_bot.py`.
- **Blocked/shared files:** `bot/chat/agent.py` has unrelated user changes; re-read and preserve its diff. Do not assign or overwrite `tests/test_chat_agent.py`. Put normal-answer snapshot coverage in `tests/test_chat_persona.py` or `tests/test_persona_catalog.py`, and rejection-follow-up coverage in the owned `tests/test_chat_followup.py`.
- **Dependencies and integration order:** depends on `catalog-loader/bundle-schema-loader`. Integrate before compatibility/docs assertions.
- **Approach:** replace three mutable caches with one immutable runtime reference; preload candidate, persist canonical ID, then assign. Capture runtime independently at both `_answer` and `_ask_instead` entry and pass it through overlay resolution, staging, prompt assembly, all generation/tool/rewrite rounds, budget rebuilding, and trailing voice paths. Resolve one profile against the captured persona default and use its name for that runtime's merged staging map.
- **Acceptance criteria:**
  - Switching Yuuki → Nazupi changes prompt identity, default behaviour sentinel, and all baseline staging sentinels on the next answer without restart.
  - A simulated portal switch during a normal-answer model/tool `await` does not change that turn's trailing voice or staging; the following `_answer` uses only the new bundle.
  - `tests/test_chat_followup.py` induces a switch while `_ask_instead` is awaiting follow-up generation and proves its posted staging, system prompt/overlay, tool/model rounds, and trailing reminder all use the captured old runtime; a subsequent follow-up uses only the new runtime.
  - Invalid candidate selection changes neither repo config nor `_persona_runtime`; no partial cache clear occurs.
  - Repo failure changes no runtime; a successfully persisted preloaded runtime swaps without an intervening `await`.
  - Profile Markdown and partial staging still win over persona defaults for matching role/member selections; no profile uses another persona's baseline by accident.
  - Audit/API/portal output contains IDs, labels, paths, and safe issue summaries only—never private text.
- **Validation:** `uv run pytest -q tests/test_chat_followup.py tests/test_portal_config.py tests/test_chat_persona.py tests/test_chat_progress.py tests/test_behaviour_plugins.py`; expected: all focused tests pass, including both generation-entry snapshot tests, full-bundle switching, and failed-swap assertions.
- **Frontend evidence required during implementation:** render `/config` at wide and narrow widths for normal, duplicate-label, no-choice, legacy-mode, and fallback/error states; verify label/action clarity, keyboard-select operation, focus visibility, and recovery copy. Store captures in the implementation evidence location chosen by the parent; no render was available during planning.
- **Escalation trigger:** true DB-memory transactional rollback, interruption of already-running answers, or portal editing of bundle files is out of scope and requires architecture/product approval.

### runtime-surfaces/compatibility-contract
<!-- workplan-step-id: compatibility-contract -->

- **Objective:** migrate values and surfaces compatibly without schema churn.
- **Owned files:** `bot/__main__.py`, comments/default handling in `bot/infrastructure/config.py`, compatibility projections in `bot/api/service.py`/`bot/api/models.py`, `bot/cli.py`, `tests/test_chat_config.py`, compatibility cases in `tests/test_portal_config.py`, and `tests/test_cli.py`.
- **Dependencies and integration order:** depends on the catalog resolution API; land with or after atomic runtime integration.
- **Approach:** preserve `persona` as the config/API key; resolve old values at read time; save canonical IDs on successful selection only; implement the exact additive API schema above; retain absent-manifest legacy behavior and insert-only `PERSONA_PATH` seeding; make generic CLI output JSON-safe for object values without changing `persona_choices` into an object list.
- **Acceptance criteria:**
  - Existing `persona.md` selects Yuuki and existing `nazupi.md` selects Nazupi in manifest mode.
  - Existing DB rows are not rewritten at startup; restart does not override a portal choice.
  - Fresh manifest-mode DB resolves the `PERSONA_PATH` basename, or uses manifest default if unmatched; fresh legacy mode retains current basename behavior.
  - No `SCHEMA_VERSION`, creation SQL, or migration-test changes are present.
  - Portal/API selection uses canonical IDs, the CLI help says persona ID rather than filename, and `persona_choices` remains a list of strings.
  - Manifest, legacy, selected-default fallback, and tracked-example fallback tests assert every preserved/new field's exact type and meaning.
  - `bossctl config set persona nazupi` sends `{"persona":"nazupi"}` and reports the returned canonical value.
  - Generic `bossctl config get` renders all-string lists as before and safely JSON-renders mappings and lists of objects without `TypeError`.
  - Legacy mode is visible as a warning/deprecation state but remains operational.
- **Validation:** `uv run pytest -q tests/test_chat_config.py tests/test_portal_config.py tests/test_cli.py`; expected: seed/alias/canonicalization/legacy cases, exact additive response schema, object rendering, and persona CLI mutation pass.
- **Escalation trigger:** automatic rewriting of existing rows, a new environment key, or immediate removal of legacy mode requires parent approval.

### deployment-migration/templates-docs-migration
<!-- workplan-phase-id: deployment-migration -->
<!-- workplan-step-id: templates-docs-migration -->

- **Objective:** make the clean layout the documented/default deployment path while keeping private content out of source control.
- **Owned files:** `.gitignore`, `.dockerignore`, `Dockerfile`, `.env.example`, `config/personas/README.md`, new `config/personas/personas.example.yaml`, new tracked `config/personas/personas/example/{identity.md,default.md,staging.yaml}`, superseded tracked major-persona examples, and `docs/chatbot.md`.
- **Blocked/shared files:** never read, copy, edit, stage, or quote ignored live prompt content. Document source/destination paths and structural transformations only.
- **Dependencies and integration order:** schema text depends on package 1; documentation may proceed in parallel with runtime work after that contract is fixed, then receive a final path/schema consistency review.
- **Approach:** adjust Git and Docker ignore exceptions so only the manifest example, complete three-file example bundle, and profile examples are tracked/packaged; update Dockerfile paths after old template removal; document copy/merge/write-manifest-last migration; mark old environment paths as compatibility-only; retain behaviour-profile templates and directories.
- **Acceptance criteria:**
  - The tracked example is a complete loadable bundle and manifest schema example.
  - Live `personas.yaml` and `personas/<non-example>/` remain ignored; profile live files remain ignored.
  - Docker build context and final image contain non-empty `config/personas/personas/example/identity.md`, `default.md`, and `staging.yaml`; no removed old-template COPY can fail the build and no private manifest/persona bundle is admitted.
  - The packaged loader can load the three-file example as one complete fallback runtime inside the built image.
  - Documentation explicitly maps Yuuki and Nazupi source paths without exposing contents, explains complete staging normalization, and says to retain the Nazupi profile for existing users.
  - Documentation distinguishes major persona from reply profile and states profile/staging precedence.
  - Rollback and restart/hot-switch steps are clear.
- **Validation:** run `uv run ruff check .`, `uv run ruff format --check .`, `uv run python -m bot.portal_styles --output /tmp/portal.css`, and `uv run pytest -q -m "not ollama"`; then `docker build -t kanade-bot-persona-test .`; then `docker run --rm --entrypoint python kanade-bot-persona-test -c "from bot.chat.persona_catalog import load_example_bundle; b = load_example_bundle(); assert b.identity and b.default_behaviour and b.staging"`. Expected: all commands exit 0 and the in-image loader proves the complete packaged fallback.
- **Escalation trigger:** tracking real prompt content, removing private legacy files automatically, or changing deployment bind mounts requires explicit operator approval.

## Validation and review strategy

Focused validation in dependency order:

1. `uv run pytest -q tests/test_persona_catalog.py tests/test_chat_persona.py tests/test_chat_progress.py`
2. `uv run pytest -q tests/test_chat_followup.py tests/test_portal_config.py tests/test_chat_config.py tests/test_behaviour_plugins.py tests/test_cli.py`
3. `uv run ruff check .`
4. `uv run ruff format --check .`
5. `uv run python -m bot.portal_styles --output /tmp/portal.css`
6. `uv run pytest -q -m "not ollama"`
7. `docker build -t kanade-bot-persona-test .`
8. `docker run --rm --entrypoint python kanade-bot-persona-test -c "from bot.chat.persona_catalog import load_example_bundle; b = load_example_bundle(); assert b.identity and b.default_behaviour and b.staging"`

Behavioral evidence must show:

- full Yuuki and Nazupi sentinels (identity/default/all staging keys) switch together;
- per-generation pinning across an induced async switch in both `_answer` and `_ask_instead`, including the follow-up placeholder and trailing reminder;
- profile precedence and partial staging inheritance against both baselines;
- old DB/env filename aliases, canonical future saves, and absent-manifest legacy mode;
- unknown/invalid/missing configurations preserve the previous runtime or use one complete marked fallback;
- exact additive API types/meanings and safe generic CLI rendering/mutation;
- errors, audit rows, portal, and API never contain private prompt text;
- all three tracked fallback components are present and loadable in the built image.

Independent implementation review should inspect path containment/all-symlink rejection, manifest strictness, fallback coherence, service mutation order, snapshot propagation through both generation entry points and trailing reminders, same-name profile behavior, exact additive API/CLI compatibility, Docker packaging, and preservation of the existing user diff. Parent structural validation already returned `valid=true` for:

```sh
bun ~/.config/opencode/scripts/check-workplan.ts /Users/cantabile/projects/personal/maplestory/kanade-bot atomic-persona-bundles
```

Structural validity is separate from readiness: the parent helper returned `valid=true`, proving the artifact shape and links but not executability or authorization. Independent checker session `ses_f8aa70dfaffefcUnzlNjoQU8B0` identified four material gaps; each is resolved in this revision and recorded below. Manual re-read after revision found no open blocker/critical/major finding.

## Risks and open questions

- **Manifest appears before files are complete:** mitigate by writing all bundle files first and atomically renaming `personas.yaml` last; runtime falls back as a whole.
- **Same-name Nazupi profile duplicates instructions:** accepted compatibility tradeoff. Do not add hidden suppression by name. A later explicit profile cleanup can address it without coupling namespaces now.
- **Profile partial staging changes meaning across personas:** intentional inheritance. Tests must prove only specified keys override each selected baseline.
- **Long-running generation sees a portal switch:** pin immutable runtime at `_answer`/`_ask_instead` entry; the next invocation uses the new runtime rather than mutating work already in progress.
- **Legacy mode becomes permanent:** log/show a deprecation issue and document a future removal boundary, but do not set a removal release without parent/product decision.
- **API label evolution:** preserve string `persona_choices`; add fields rather than changing its element type.
- **Unrelated edits in `agent.py`:** implementation must reconcile against current diff and use narrow edits; no checkout/reset or wholesale rewrite.
- **Container privacy regression:** explicit `.dockerignore` allow-list and in-image fallback loading are required acceptance, not optional CI follow-up.
- **Decision — bundle filenames:** revised during implementation at operator request. Descriptors accept optional bare `identity`/`behaviour`/`staging` filenames (defaults `identity.md`/`default.md`/`staging.yaml`); directory paths are still rejected. The operator's bundles now use the fixed defaults after renaming.
- **Decision — behaviours flattened:** reply profiles moved from `behaviours/profiles/` to `behaviours/` (staging `behaviours/staging/`). Code reads the new locations first with the old ones as legacy fallbacks.
- **Decision — identities/ removed:** the empty `config/personas/identities/` directory was stale (live identities live in `personas/<id>/`) and was removed.
- **Open decisions:** none.

## Plan review ledger

- **Resolved major — generation entry coverage:** checker session `ses_f8aa70dfaffefcUnzlNjoQU8B0` found `_ask_instead` missing from the atomic snapshot contract. The runtime package now explicitly covers `_answer` and `_ask_instead`, and owns a switch-during-follow-up-await test in `tests/test_chat_followup.py`.
- **Resolved major — Docker fallback:** the same review found that removing old templates would conflict with current `Dockerfile`/`.dockerignore` and omit packaged staging fallback. Both files are now owned, all three fallback files are required in context/image, and build plus in-image loader checks are acceptance criteria.
- **Resolved major — API/CLI contract:** the same review found the additive schema vague and generic CLI object-list rendering unsafe. The preserved and new fields/types/mode meanings are now exact; `tests/test_cli.py` covers safe objects and `config set persona nazupi`.
- **Resolved major — symlinks:** the same review found “reject or contain” ambiguous. The policy now rejects every symlink bundle directory/component, including within-root and cross-bundle links, with explicit tests.
- **Resolved note — structural validation:** parent workplan helper reported `valid=true`. This confirms structure only; readiness remains the evidence-backed verdict here.

## Execution receipts

### catalog-loader/bundle-schema-loader — attempt 1

- **Worker:** `code-writer`, session `ses_f8a9bf152ffeM4pYL1TJLO7KWj`.
- **Code state:** added `bot/chat/persona_catalog.py` and `tests/test_persona_catalog.py`; updated staging parsing and focused staging tests. Existing unrelated edits in `AGENTS.md`, `bot/chat/agent.py`, and `tests/test_chat_agent.py` remain present.
- **Acceptance evidence:** strict fixed-file manifest bundles, exact ID/alias resolution, whole-bundle fallback, all-symlink rejection, legacy mode, and profile staging inheritance are covered by focused tests.
- **Checks:** `uv run ruff check bot/chat/persona_catalog.py bot/chat/persona.py bot/chat/progress.py tests/test_persona_catalog.py tests/test_chat_persona.py tests/test_chat_progress.py` — exit 0. `uv run pytest -q tests/test_persona_catalog.py tests/test_chat_persona.py tests/test_chat_progress.py` — exit 0, 72 passed, 1 deprecation warning.
- **Result:** package completed; runtime/API integration is next.

### runtime-surfaces + deployment-migration — implementation (parent session)

- **Code state:** single immutable `PersonaRuntime` in `ChatPilot` with prepare/persist/activate swap; per-generation snapshots in `_answer` and `_ask_instead`; exact additive API fields; `Persona` portal control with configured/effective/recovery states; complete tracked example bundle; Docker packages only templates.
- **Review corrections applied:** malformed/non-UTF-8 manifest falls back to the example bundle; legacy identity/default/staging treated as one candidate with sanitized issues; custom legacy staging/profile dirs threaded through; manifest and all bundle/component symlinks rejected; profile-staging failures log path-only warnings; `PERSONA_PATH`/seed root unified on `persona.PERSONA_DIR`.
- **Operator layout adopted:** per-bundle filename overrides (`identity`/`behaviour`/`staging`); profiles flattened to `behaviours/` + `behaviours/staging/` with legacy fallbacks; stale `identities/` removed; portal tests migrated from legacy filenames to manifest bundles; no test reads live ignored bundle contents (synthetic `tmp_path` fixtures only).
- **Checks:** `uv run ruff check .` — exit 0. `uv run ruff format --check .` — exit 0. `uv run python -m bot.portal_styles --output /tmp/portal.css` — exit 0. `uv run pytest -q -m "not ollama"` — exit 0, 3113 passed, 32 deselected. `docker build .` — exit 0, in-image example bundle loads and no private manifest/bundles present.
- **Live verification:** manifest mode with both `yuuki-sakuna` and `nazupi` bundles loading, no issues.
- **Result:** all packages completed.

## Resume point

- **Next executable step:** none; work is complete and verified. Remaining operator step is writing `personas.yaml` last on deploy (already live here) and hot-switching both directions in **Config → Chatbot**.
- **Unmet dependencies:** none.
- **Unresolved findings / decision required:** none.

## Frontend design evidence

| Rule followed | Linked screen | Feature / implementation evidence |
|---|---|---|
| `HPK-4 — Leading identity` | `/config — Chatbot` (Not captured) | Plan requires human persona labels to lead options, with stable slugs secondary only when needed; rendered verification remains for implementation. |
| `COPY-1 — UI copy` | `/config — Chatbot` (Not captured) | Plan changes “Identity”/“Use this identity” to “Persona”/“Use this persona” so the control names the complete effect; not yet rendered. |
| `RECOVER-1 — Prevention + recovery` | `/config — Chatbot` (Not captured) | Plan requires invalid bundles to be unselectable and fallback state to name configured/effective IDs plus recovery without private text; implementation not yet verified. |
| `STATE-1 — Derive states` | `/config — Chatbot` (Not captured) | Normal, duplicate-label, empty, legacy, and fallback/error states are explicit validation targets; visual/keyboard checks remain pending. |
