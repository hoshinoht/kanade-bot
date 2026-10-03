# Changelog

Notable changes to the Boss Scheduler Bot, newest first.

## 1.0.0-beta.1 (in development)

**Added**

- v5 admin Bosses knowledge pages show each boss's strategies (when to use it,
  risk, damage needed, payoff and steps) when its document has them.
- v5 `kanade backup [--name FILE]` snapshots the stopped store (`VACUUM INTO`,
  0600) into `KANADE_BACKUP_DIR` with its manifest, which now records
  `created_at`; it refuses while the bot owns the store and never overwrites.
  `GET /api/admin/history/checkpoints` lists those manifests newest first,
  re-checking each head against the chain on every request (`anchor`:
  `matches`, `older_schema` or `mismatch`) and reporting
  `backup_dir_configured`. Compose mounts the backups directory read-only into
  the bot and adds a `backup` tool service; the deploy runbook takes the
  snapshot after stopping the bot, beside the volume tarball. Mirrored in the
  dev mock and API types. The admin History → Checkpoints tab (B_HistoryCk)
  shows a verification card ("Chain verified · n records · head · checked
  just now", or "Chain check failed" in the risk wash) with **Verify again**
  (a read-only re-check, announced politely) over a backups table whose
  Anchored column reads ✓ matches, ○ older schema or ✗ mismatch in words;
  with no backups it says none were taken yet, or that this server has no
  backup directory.

- v5 admin Config API adds three read-only fields for the Config page:
  `models.capacity_check[].group` names the capacity group each check is about
  (null for cross-group checks), `env[].copy` gives the raw value to paste into
  the deployment env (null when unset), and `last_digest` reports the newest
  active weekly digest (guild-offset `posted_at`, boss-week start, whether it
  is this week, channel name and message link). Mirrored in the dev mock.
  The admin PWA shows them: Models → Capacity puts each group's verdict in a
  Startup check column (cross-group checks stay listed under the table),
  Weekly digest gains a "Last posted" card (guild time, week, channel, a
  Discord link when known; omitted without a digest), and each env row with a
  raw value gets a "Copy KANADE_…" button. Persona's reply profiles lead with a
  fixed "Default voice" row (the active persona as written; never selected or
  saved). The dev mock's env rows now use the server's keys, labels and order.

- v5 admin Inbox items carry an optional one-line `consequence` read off the
  merge preview (e.g. "Party unchanged · 2 reminders will move", "Adds Finn",
  "2 reminders will be dropped"): party joins and leaves of the changed runs
  and the upcoming reminders that move, drop or appear; `null` with
  conflicts, expiry, no effect or nothing to say. Served by the pwa-mock and
  typed in `@kanade/api-types`; the decision card shows it before Approve
  (inside the change card on phones).

- v5 admin API reads and changes a weekly timing's owner: `FixedRow` adds
  `owner_id`, and `POST`/`PATCH /api/admin/fixed` accept an optional `owner_id`
  (a rostered, non-bot member, not necessarily in the party; else `422 invalid`).
  An owner change is blamed as `owner`, conflicts like other timing fields and
  survives idempotent replay after the owner loses the bossing role. The admin
  PWA Fixed editor shows an Owner select beside Day and Time (stacked on
  phones), preselects the saved owner, defaults a new timing to the signed-in
  Discord member or else the first party member picked, and reads an owner
  refusal out on the field.

- v5 admin phone navigation drawer shows Members and Reminders counts beside
  Week: `GET /api/admin/summary` adds `members` (bossing roster) and
  `reminders` (upcoming cards), derived as the Members and Reminders pages count.
- v5 admin API removes a chat alias: `DELETE /api/admin/members/{id}/aliases/{alias}`
  drops it from the member (others keep their order) and frees it for anyone;
  removing an alias the member does not hold returns the unchanged row. The admin
  PWA Members sheet gives each alias chip a × that removes it in place.


- v5 admin Bosses now uses the M3E catalog and checked-in knowledge workspace:
  level-ordered boss rows keep weekly difficulty ticks visible beside the selected
  strategy, facts, provenance and seasonal availability, with this week's linked
  timings alongside. Phone navigation moves from the catalog into a backable detail.

- v5 replays offline ✅/❌ on pending proposal cards after startup and fresh
  gateway READY, using current approver roles; conflicting answers stay pending
  with a visible note, and stale chat rejection follow-ups are not sent.

- v5 model logs retain Kanata reasoning text (64 KiB, visibly truncated) and
  reported reasoning tokens; admin Chat/Extractions show collapsed reasoning
  and token counts, and copied chat transcripts include both. Oversized counts
  stay unknown, and retries keep a single truncation marker.

- v5 admin Inbox items read closer to their boards: the header and thread
  panel form one column beside the decision pane, a proposal shows Kanade's
  one-line italic summary under its header on wide screens, the facts line
  is smaller with the read time in body type, the phone header is tighter,
  and the "Can't approve" line appears only when Approve is held back. Wide
  Extractor items follow VarRail2: the header (larger art, summary, a
  confidence burst) runs across the top, the thread fills the height beside
  a 300 px decision card that holds the change (the new time large, under
  its date), with the thread's channel and time span in its bar, the facts
  and "Open in Discord" at its foot, used messages marked "used", and
  "See the card" beside Reject; Extractor list rows lead with the boss art
  and say how much of the thread was used.

- v5 admin Config sections now follow their M3E boards: each section has a
  heading and lead, settings cards that scroll, and a save bar that stays put
  (dirty dot, "field old → new", Discard, Save <section>; disabled when clean,
  and the contents list marks a section with unsaved changes). On/off settings
  are switch cards that apply at once, ask before turning off (before opening
  the public portal), and offer Undo for 10 s. Persona splits into "Active
  persona & profiles" / "Role overrides" pill tabs and Models into "Roles" /
  "Context windows" / "Capacity"; "Use this persona" and "Post it now…" confirm
  first. The Context windows "In effect now" table has its own card heading
  above the table, and Re-read spaces its channel chips and Window row like
  B_CfgReread, with a "Channels · n of m" count, Select all / Clear and a
  one-at-a-time note.

- v5 admin Inbox items now carry `thread`: the stored channel messages around
  a proposal's evidence (oldest first, at most 37, the extractor's context plus
  a burst), each marked `used` when the proposal cites it. It is read from the
  watched-message cache, so deleted or pruned messages never reappear; member
  requests and proposals without a card have `thread: null`.
- v5 Discord chat now answers boss strategy questions from the checked-in boss
  knowledge (`KANADE_KNOWLEDGE_DIR`), without sources, as v4 did; without a
  knowledge directory the strategy tools are not offered at all.
- Boss guides refreshed for MapleSEA (researched 2026-10-02): SEA names first,
  full per-difficulty facts for every boss, and the KMS HP values both before
  and after OVERDRIVE. Guides gain named strategy options, each with its risk,
  damage requirement and payoff (e.g. four Radiant Malefic Star altar routes).
  Chat can now answer event bosses by name or alias (new Meilin prep guide;
  Bellona is kept as a prep guide until she reaches MapleSEA).
- v5 admin Bosses shows event bosses (e.g. Kai, Meilin) with their portrait
  or icon like catalog rows and a "Seasonal boss · Challengers World Season n"
  chip, also on their knowledge page with the portrait. `/art` now serves art
  for a key an event knowledge document declares (exact case), and the events
  read carries `portrait`, `portrait_sm` and `art`.
- v5 admin Config now uses the M3E Settings window: a grouped, deep-linkable
  contents list with current-value hints, a "find a setting" search, settings
  cards in the section panel, and a page-line risk chip (Config only) in place
  of the Manage Messages banner, linking to Channel access.
- v5 admin Limits page and "Post digest" now work against the real server:
  limits are read live, a usage window can be cleared, and a manual digest for
  this or next week posts through normal delivery to a channel the bot knows
  (replacing that week's digest, as in v4). Config-change audits now name the
  acting admin.
- v5 admin History now uses the M3E audit timeline list-detail layout, with a
  side-pane change review on wide screens and a full-screen detail sheet on phones.
- v5 posts v4-exact decline notices again: a ❌ reaction replies to the card,
  `/rsvp no` and portal declines go to the run's home channel, and extraction
  declines post in the source channel, with v4's cooldown. Changing the answer
  back deletes the notice, even if it was still being sent. Chat proposal cards
  and v4 import post none. Store schema v20: rollback needs the previous image
  plus a pre-v20 backup.
- v5 admin Fixed now uses the M3E weekly-timing list-detail layout: the editor
  is a side pane beside the timings on wide screens and a full-screen sheet on phones.
- v5 admin Members now uses the M3E roster list-detail layout: the member
  editor is a side pane beside the roster on wide screens and a full-screen
  sheet on phones.
- v5 admin Members matches its M3E board: one framed roster with bare run
  counts and capitalised @mention levels, a detail header with the member
  overline, handle and an icon close button, connected @mention pills, alias
  chips, and a This week list of the member's runs; "Sort: runs" orders the
  roster by runs this week (A–Z between equals) and can switch to A–Z; phones
  drop the reply-style column so the roster never scrolls sideways.
- v5 now writes v4-exact weekly-timing added and removed notices with their
  deciding commits (never during v4 import), and refreshes posted proposal
  cards after committed portal inbox approvals or rejections.
- v5 admin Inbox now uses the M3E contained list, thread and decision-pane primitives across Extractor and Self-service, including the approved phone detail action bar.

- v5 chat now follows up a qualifying ❌ on its own proposal card with one
  read-only, silent reply to the card. It verifies the original chat asker,
  uses a per-channel cooldown, spends no chat allowance and retains only the
  visible assistant answer in channel context.
- v5 admin planner cards can swap directly by dropping on another live card,
  or with `S` during a keyboard lift; the run sheet also provides an accessible
  picker with a two-run preview and one-step undo.
- v5 admin planners can atomically swap two live runs' slots in one boss week,
  with one history record, reversible as one change and per-run move notices.
- v5 run lengths are a live, saved setting (`v5.run_lengths`, seeded from
  `[settings.run_lengths]`/`KANADE_RUN_LENGTHS` until saved): a default per
  boss (30 minutes) plus per boss/difficulty overrides (Hard Black Mage 60);
  invalid seeds refuse startup. Admin week runs carry their length in minutes
  (the sum over their bosses), the groundwork for planner drops that set the
  time.
- v5 logs token usage: schema v19 adds nullable provider-reported
  prompt/completion tokens and the local prompt estimate to every extraction
  call (summed over the attempts that reported a pair) and every chat round
  (`null` = not reported; older rows and v4 imports stay `null`). The admin
  API shows them on chat rows, turn rounds and extraction rows/details (with
  the call's context window), and per-model chat and extraction summaries add
  reported sums and the median reported/estimate ratio. Rolling back to a
  v18 image requires the paired pre-upgrade volume backup.
- v5 chat shows the persona's staging line as a silent reply with typing
  while it answers, then edits that message into the answer; longer answers
  continue as silent follow-ups. A deleted question's placeholder is
  withdrawn, an answer that stops early ends with `*(reply incomplete)*`,
  shutdown turns a pending placeholder into the failure line, and chat-log
  rows record what was delivered (`guardrail.delivery`, `incomplete:
  delivered k of n parts`).
- v5 model context windows are live, saved settings (`v5.model_context`,
  seeded from `[models.context]`): per-alias override, then the catalog's
  published window, then a cloud (65,536) or local (8,192) default, capped at
  131,072 and by optional per-role caps. Per-role reply reserves (chat 1024,
  extraction 2500, rewrite 96) are sent as `max_tokens`, clamped to Kanata's
  published `max_output_tokens`. Config API, `kanade models check`, startup
  logs and model-log rows show the effective window and its source, and warn
  when a local model's window exceeds 16,384 or a reserve fills its window.
  Changes apply to the next chat question, extraction pass or rewrite without
  a restart; the fixed 65,536 serve window is gone. Config → Models has a
  Context windows panel with paired sliders and number fields, per-model
  override rows showing published maxima, and the local 16k warning.
- v5 chat trims oldest history, then older tool results, before failing a
  too-long question with a typed context-budget error and a member reply;
  a Kanata size 400 above the published output maximum no longer downgrades
  the model's sampling controls.
- v5 gives countdown and weekly digest headers a bounded persona-flavored
  interjection, preserving code-owned schedule facts and stable stored text
  across retries and edits; schema v18 stores phrases before delivery claims.
  Rolling back to a v17 image requires the paired pre-upgrade volume backup.
- v5 stores ordered Reply-profile assignments by Discord role and applies
  role-first styling live, with current-role validation and conflict-safe admin
  updates; Config now has a named guild-role picker and add, change, remove,
  reorder and conflict-recovery controls.
- An opt-in, governed synthetic privacy re-identification probe that keeps
  network calls out of tests and reports only aggregate recovery counts.
- Source-checked v5 compatibility inventory covering routes, commands, runtime
  entrypoints, configuration, storage, workers, chat tools, and portal assets,
  with regression checks for missing entrypoints and environment dispositions.
- Relocated the runnable v4 rollback implementation to `legacy/python/` without
  moving private deployment state.
- Added initial portable migration schema modules, synthetic history/delivery
  fixtures and independently validated JSON Schema artifacts; runtime export,
  import and maintenance operations are not implemented yet.
- Added deterministic, bounded migration archives, pure guild/catalog/reference
  preflight checks, and private no-overwrite publication with Linux/macOS path-race
  protection. These helpers do not yet export or import a running database.
- Added the modular Rust runtime foundation with explicit offline serving,
  loopback health checks, strict configuration, redacted errors and bounded
  shutdown. Scheduler, storage and Discord adapters remain under development.
- Added synthetic Python-derived domain and initial stateful scheduler vectors,
  with replay/drift checks and full input-schema validation before opening the
  oracle store. Broader scheduling and migration parity remain in development.
- Added an offline Rust LLM-provider interface, deterministic fake provider,
  structured output/tool-call validation, bounded payloads and retries, and
  redacted diagnostics. Kanata networking and chat integration remain deferred.
- v5: capability-aware model requests (Kanata `/v1/models` metadata, operator-declared
  capabilities or a minimal profile) and a generic OpenAI-compatible HTTP adapter for
  Kanata or a plain Ollama `/v1` endpoint: https with verified certificates, plain http
  only for local hosts, optional bearer key, a single capability lookup per call and one
  reshaped retry when a model rejects a field. Env/config wiring is still pending.
- v5: Rust domain core (reset and calendar weeks, fixed slots, time parsing, IDs, boss
  catalog validation) matching the frozen v4 domain vectors, including DST handling.
- v5: strict persona layout under `config/personas/` (catalog, bundles, profiles) with
  a Rust loader, trusted Kanade fallback, atomic reloads and per-turn snapshots; the
  Kanade bundle adds a header-rewrite prompt for small models. Public v4 persona prompt
  vectors, plus reminder and run-mutation scheduler vectors, are frozen for parity.
- v5: Rust scheduler core for week materialisation, fixed-run adoption, reminder rows,
  RSVP reactions and run lifecycle, behind a store interface with an in-memory store and
  a shared conformance suite. A day-of ping can no longer fire after its run starts on
  DST spring-forward days. Dispatch, mention and digest scheduler vectors are frozen.
- v5: run mutations (status, amend, swap, fixed-run edits) plus new fixed-edit choices
  per manually moved run ("update to fixed" or "keep for this week") and a "reset to
  fixed" action that refuses slots already passed. Removing someone from a fixed run
  drops their RSVPs and recomputes status. A pure notification core plans mentions,
  reminder dispatch and weekly digests; reminders and notices whose home channel is
  unavailable now fall back to the post channel with an admin warning instead of being
  silently dropped.
- v5: Discord adapter groundwork on Twilight: sends report delivered, definitely not
  sent or ambiguous (never retried), mentions limited to explicit member lists, v4
  gateway intents with clear fatal-close reasons, reaction RSVPs and roster events,
  and a slash-command framework with v4 permission gates and 3-second-safe replies.
- v5: SQLite store (SQLx, no TLS in the store) with checksummed migrations, atomic
  commits guarded by a store revision (conflicting saves are re-planned), single-writer
  ownership, and private (0600) online backups; restore validates the copy first and
  refuses to run beside leftover SQLite journal files.
- v5: store ownership hardening: an operator lock directory (`owner_lock_dir`, 0700)
  with a lock named after the database file's identity, private canonical paths with no
  symlinks, writes and backups refused if the database file is replaced, and
  cancellation-safe open and restore.
- v5: tamper-evident schedule history: every change records who, where and the
  before/after rows in a hash chain the database refuses to edit; admins can revert a
  change, restore a boss week to a point or undo a member's changes (always forward,
  never resending pings); repeated requests apply once; backups carry a history anchor.
- v5: history blame (who last changed each run or fixed-run detail) and immutable
  checkpoints (automatic at every boss-week start plus admin-named) with a previewed,
  week-scoped restore.
- v5: schedule drafts (git-style branches): admins stage changes that ping nobody,
  preview the resulting week, rebase and merge them as one recorded change with
  per-detail three-way conflict checks (no force); merges post one summary per affected
  channel, and drafts for a boss week that has passed expire automatically.
- v5: edits made from an out-of-date screen no longer silently overwrite someone
  else's change: each edit can state the last change it saw per detail, a clash on the
  same detail is refused with who changed it and when, different details still merge,
  and only admins can apply theirs anyway (recorded as an explicit override).
- v5: member requests (git-style pull requests): members ask for a new weekly run, a
  weekly-run change, or to join, leave or swap; admins preview, edit, approve (as one
  conflict-checked merge) or reject with a reason. Permission is re-checked at
  approval, party changes are stored as add/remove so concurrent requests merge,
  members have pending and daily limits, frozen members can't submit, and public
  channels only ever show a bot-written summary of the request.
- v5: cherry-pick: admins re-apply a past change (time, party, status or answers) to
  the same weekly run in another boss week, previewed first and re-checked against
  the schedule rules; conflicts refuse by default, and force applies exactly the
  conflicts that were previewed and is recorded as an override.
- v5: attendance model (off by default, v4 behaviour unchanged until enabled): members
  can set a standing "always in" answer per weekly run and admins can mark a weekly run
  "assume everyone's coming"; answers read confirmed / assumed / unknown / declined with
  tallies like "4/4 (2 assumed)", runs turn at risk only on a ❌ or on unknown answers
  close to start, morning pings mention only unknown members, and marking a run done
  records who came (members confirm only their own), with private attendance patterns
  that suggest standing answers. With attendance on, an admin-set planned/confirmed
  status sticks (shown "set by admin") until a reaction, party change, move or new
  status ends it, and a run's status no longer changes once it has started (after the
  start only a new status ends the pin).
- v5: model traffic governor: per-backend permit groups with priority queues (admin
  first, extraction and follow-ups last), a request-rate ceiling, a capped retry
  budget and a circuit breaker that sheds new work during an outage and ramps back
  up with a single probe. Model calls go through it: a chat question holds one slot for
  all its tool rounds with a request cap of rounds + 1, gateway admission refusals are
  requeued once after Retry-After, a 504 ends the question, and retries need budget.
  Nudge rewrites get their own session that never waits and sends one request, and a
  content-filter stop is reported apart from a length cut-off (logged as
  `content_blocked` by extraction).
- v5: model requests match the real Kanata gateway contract: its published per-model
  admission limits are read and checked against the limiter's permits, request values
  are validated locally (so a bad value never marks a model unsupported), Kanata's
  breaker wait is honoured, "off" reasoning sends `none` where supported, empty tool
  results get a placeholder, and every request carries an `x-request-id`.
- v5: persona prompt compiler matching v4's assembled prompts byte for byte, with strict
  staging lines (literal boss-name substitution), optional self-service nudge pools and a
  nudge rewrite prompt per bundle/profile (Kanade ships its own lines). Persona YAML no longer coerces unquoted
  numbers or booleans into text.
- v5: storage for extraction and chat logs (per-round chat detail, filterable by model,
  date, outcome, channel, member, tool and text with keyset paging), rescan jobs,
  allowance overrides and one self-service tip per member per week, plus extractor/chat
  proposals stored as system drafts that expire and supersede older ones. Logs are
  pruned after 90 days in small batches.
- v5: HTTP server with separate admin and public listeners (admin routes never exist on
  the public one), Host and proxy-header guards, the PWAs' security headers, body and
  time limits, and static serving of both apps and boss art; the public portal answers
  closed. `KANADE_BIND` is renamed `KANADE_ADMIN_BIND`.
- v5: models follow Kanata: each role's model and reasoning level come from settings and
  are checked against what Kanata publishes, a role counts as cloud when Kanata's trust
  zone says so (or is unknown), `KANADE_ALLOW_EXTERNAL_UNMASKED=1` lets such calls through
  for testing with loud warnings and flagged log rows, and `kanade models check [--probe]`
  lists Kanata's models and sends one tiny test request per role.
- v5: `kanade import v4` brings v4's weekly fixed runs and recent chat and extraction logs
  (with the messages they reference) into v5 from a read-only v4 snapshot, previewing by
  default and adding nothing when run again; the image now carries the boss catalogue.
- v5: `kanade serve` now runs live without Discord: it owns v5's own database, loads the
  catalogue, knowledge, personas and settings (env seeds under saved values), and serves
  the admin app against real data; sessions report how the admin signed in. Kanade chats
  in every channel of the configured chat categories (no per-channel chat list).
- v5: the model client is built at startup from the configured Kanata endpoint, key and
  per-role models; an unreachable model list leaves extraction and chat degraded rather
  than stopping the bot, and unpublished reasoning levels are reported.
- v5: schedule notices use v4's exact wording (moves, cancels, own time, done, restores,
  swaps, weekly-timing changes, quiet mode, "via portal"); approving Kanade's card only
  announces moves, as in v4; notices older than 6 h are dropped instead of posted late,
  an unreadable one no longer stalls reminders, and delivered notices are purged after
  90 days.
- v5: notices from schedule changes (admin edits, inbox and ✅ approvals, requests,
  rollbacks, expiry) are saved with the change itself and posted once by the reminder
  tick, surviving crashes without being lost or sent twice.
- v5: container packaging in `deploy/`: a hardened ~50 MB distroless image (non-root,
  read-only, no toolchains) and a Compose stack that serves the admin portal on the
  tailnet through the existing edge site, with secrets from host files and a runbook
  for switching between v4 and v5.
- v5: the Discord adapter now reads messages, channels and threads (a thread counts as its
  parent channel), keeps a guild cache for channel names, reachability and the watched
  categories, drops every event from other guilds or DMs, and can reply, page members,
  page channel history and list channels. Commands are only ever registered per guild.
- v5: runtime settings stored in the database (v4's `config` keys where they exist, new
  `v5.` keys for channels, categories, reset and modes), seeded from env until saved, and
  saved one section at a time.
- v5: serve configuration for the live bot: Discord token from a file only (plain env
  refused), a required "v4 stopped" confirmation, guild/role ids, store paths, model
  endpoint and settings seeds, all validated at startup with secrets never logged.
- v5: startup loaders for the boss catalogue (tracked `boss/bosses.yaml`, parsed strictly),
  the boss knowledge directory (schema-checked) and the persona directory.
- v5: admin logs and rescan API: chat and extraction logs with filters, totals and facets
  (withheld questions stay hidden and unsearchable; extraction details list refused
  changes), and rescans of watched channels that start at once, report unread messages
  and can be cancelled safely. Failed extraction calls never show store error text, a
  cancel racing a finishing rescan always reports how it really ended, and log lists no
  longer load prompts.
- v5: admin inbox API: Kanade's extraction and chat proposals and members' requests in
  one list with a preview of what approving would change; approve or reject each
  (safe to retry), approve a proposed move, add or split at a corrected time in one step,
  and proposal decisions require a Discord sign-in. Discord card refreshes and notices
  follow once the bot is wired up.
- v5: admin history API: paged change history with a total and week/person filters,
  single change records, who-changed-what per run, a history integrity check, and
  revert, restore-week and revert-a-person's-changes with a preview that never writes
  and retry-safe apply. Cherry-pick and the history graph follow once their contracts
  are settled. Out-of-range change numbers get "not found"/"invalid" rather than a
  server error, a refused rollback lists every requested change, and an applied
  rollback reports the rows it actually saved.
- v5: admin edit API: move runs, change status, answers and parties, reset to the
  weekly timing, preview pings, create, edit and retire weekly timings, and edit
  members and aliases. Edits made from an out-of-date screen are refused (409) instead
  of overwriting someone else's change, a retried request is applied only once, and
  Discord member updates no longer overwrite portal member edits. Weekly-timing edits
  must send the week `version` they were loaded at (the admin app change follows),
  moves stay inside the run's boss week, and portal answers are recorded like v4 chat
  answers (any answer can be cleared; a hand-set status is kept). Not yet wired into
  `serve`.
- v5: admin read API: week, stats, summary, weekly timings, reminders, members,
  channels, personas, bosses, boss events and knowledge, each matching the published
  JSON schemas; members gain stored aliases, reply style and roles (migration 0010) so
  the staff check survives a restart. A correct break-glass token can no longer be
  locked out by guesses from many addresses, and an edge request without a usable
  `X-Forwarded-For` is refused. Discord member updates now carry roles and a computed
  Administrator permission, so admin sign-ins end as soon as someone loses staff status.
  Not yet wired into `serve`.
- v5: admin sign-in: Discord login (identify only, PKCE, one-time state, token revoked
  after use), Tailscale identity from the edge for allow-listed logins, and a break-glass
  token read from a file; server-side sessions with idle/absolute timeouts, a staff
  re-check every 5 minutes and CSRF checks on every change. Not yet wired into
  `serve` (it answers "sign-in unavailable" until the store and bot member cache are).
  Hardened after review: rate limits on sign-in, only `identify` scope and no bot
  accounts accepted, the client secret sent with HTTP Basic, an edge secret required
  before Tailscale identity is trusted (listeners may bind a private address by
  opt-in), and a same-origin landing page after Discord sign-in.
- v5: extractor and chat proposals: each amendment becomes a proposal only if it can
  apply now (otherwise it's refused with v4's reason and no card), is approved with ✅ by
  a run participant, an admin or the timing's owner, merges through the shared schedule
  write path, replaces older proposals for the same target, and expires silently after
  24 hours. Proposed answers recount the run's status like v4; approved cancels win over
  status changes since the proposal, and an approved move revives a cancelled run.
- v5: extraction proposal cards on Discord, formatted as in v4: posted through the
  delivery journal with no pings, ✅ to apply and ❌ to reject (by a participant, an
  admin or the owner), and refreshed when applied, rejected or superseded. Card details
  are stored with each proposal (migration 0011) so cards re-render after a restart;
  extraction logs record refused changes in a structured field. A post that never went
  out gives the weekly nudge tip back. A ✅ on a run edited by hand after the card went
  up is refused in plain words and the card is marked out of date; reactions from
  people who may not answer change nothing, and internal errors go to admins, never
  the channel.
- v5: self-service redirect decisions (move it yourself, request it, or keep the card;
  cards only while the public portal is closed) and persona nudges: rotating seed lines
  from the profile, bundle or built-in pools, at most one tip per member per boss week,
  with an optional 2-second rewrite on a small model that falls back to the seed line.
  Rewrites are refused (seed line used) for markdown, hidden format characters, invite
  links, changed placeholders or a word on a built-in safe-for-work deny-list (including
  sound-alike spellings such as "dih"/"bih" and common Malay, Indonesian, Singlish,
  Tagalog, Thai and Vietnamese swears), and the
  filled line is checked again. Kanade has her own rewrite voice and request lines.
  Extraction now plans the redirect for each kept change (still cards only while the
  public portal is closed) and uses up the weekly tip only when a link is posted.
- v5: chat traffic and safety: per-member allowance overrides from the admin store,
  refunds when a question fails through no fault of the asker, a short per-channel
  queue instead of dropping questions, a guard that pauses clean retries during a storm,
  a fixed reply when the content filter blocks an answer (personas may set their own
  `failures.content_blocked` line), and blocked questions kept out of everyone's later
  context. Not yet wired to Discord.
- v5: chat question loop and context: history, reply chains and anchors trimmed to a
  token budget, one limiter slot per question with a round cap plus one clean retry
  (also used when the content filter stops a reply), a failed card post told to the
  model, and one chat log row per question with a round per request. Like v4, the last
  round is sent without tools, and a call to an unknown or unoffered tool or with bad
  arguments gets a steering note instead of failing the reply. Not yet wired to Discord.
- v5: chat gate, authority, read and proposal tools (proposals only through the
  scheduler, refused up front when unworkable), reply sanitising and injection guards,
  matching the frozen v4 chat vectors; tools load in bundles as needed instead of all
  12 every round, and every tool argument and result goes through the identity codec.
  Compact times after a day (`wed 930`) read as evening like v4, and a bare hour after a
  weekday (`sat 10`) keeps the weekday instead of v4's day-of-month misreading.
- v5: extraction pipeline: watched messages are debounced into bursts, read through the
  model limiter (one answer retry), logged once per call with their outcome, and turned
  into proposals only through the scheduler; a bounded backlog drains slowly and waits
  out an open breaker, and rescans run one at a time and can be cancelled. Not yet wired
  to Discord (cards and reactions come next). An edit made while a message is being read
  is read again rather than lost, a misconfigured model route fails instead of retrying
  forever, and rescans retry only the turned-away pieces and report what stayed unread.
- v5: extraction rules ported (keyword gate, message bursts and rescan windows, day/time
  resolution, run matching, per-run merge), matching the frozen v4 vectors exactly,
  plus the extractor's answer schema, prompt, one-retry answer handling and burst
  planning. Member identities in extraction prompts go through the identity codec; the
  extractor checks model answers itself (v4 coercions) with one answer retry per call.
- v5: delivery journal for Discord sends (claim, bind to the channel actually used,
  mark ambiguous, retire rejected or replaced cards, recover in-flight sends after a
  restart without resending), a message-to-run card index for reactions, and a guard
  that never reopens a reminder whose delivery may already have happened.
- v5: scheduler tick and delivery executor: reminders and weekly digests go out
  through the journal in v4's order, never twice; sends that never left are retried,
  refused ones retire with a throttled admin alert, a digest is replaced only after
  the old card is confirmed deleted, and a clock that moved backwards posts nothing.
- v5: the admin app's history follows the Rust history API: plain labels for who changed
  each run field, reminder changes described (a move's re-placed reminders fold into one
  line), guild-local week headings, and a revert dialog that lists conflicts and previews
  what forcing would change. Reverts no longer send a conflicting request id.
- v5 admin app: no more flash on background refresh (the Answers chart is updated in place);
  tool-trace arguments and results open in a dialog; Copy transcript (Markdown or JSON) on
  chat turns; Reminders and Reply profiles gain paging, filters and one-line rows; Maple Mono
  is the code font.
- v5 extraction stops at once when switched off or paused (in-flight calls cut, rescans
  cancelled), restarts only read unprocessed messages (no reposted cards), chat questions are
  never extracted automatically, and only regular messages and replies are read.
- v5 admin app brand shows the live Discord bot: its name (server nickname, global name, then
  user name) and its avatar and banner, cached from Discord's CDN into `/data/identity` after
  each READY or profile change, with versioned URLs so browsers pick up new art.
- v5 container logs explain persona and chat behaviour: `persona_selected` at startup (fallbacks
  and profile load problems warn), `settings_changed` with keys and before/after values,
  `persona_switched`/`personas_reloaded`, and chat lifecycle events (`chat_admitted`,
  `chat_ignored` for summons only, `chat_answered`/`chat_failed` with persona, profile, model
  and route, `chat_cancelled`, `chat_setup_changed`), never with message text or member ids.
- v5 serve runs extraction live (S9): watched messages feed the pipeline, proposal cards post
  and resolve with ✅/❌ (one card desk for extraction and chat cards), `/rescan` and the admin
  Rescan panel page Discord history, a 24 h startup rescan runs when extraction is on, chat
  questions are never extracted, health reports `extraction`, and shutdown cuts in-flight
  extraction calls so it stays inside the container's stop grace.
- v5 admin app: role and bot mentions resolve to names, message authors are copyable names,
  Config → Models shows read-only capacity groups with one line for Kanata's limits, the
  masthead is regrouped (status chip, account menu, compact Commands), state pills share
  semantic colour profiles, the chatbot rate fields sit on a grid, and the model picker lists
  base models only.
- v5 chat answers mentions of the bot's managed role, drops queued questions when chat is
  switched off, never calls the model for a question deleted before its answer starts, keeps
  unposted answers out of history, survives a panicking answer, and bounds chat shutdown.
- v5 chat allowance may be 0 per member ("staff only"): members without an override are ignored
  silently, staff are exempt, and `/limits` says chat is staff only.
- v5 chat logs keep each tool call's result (up to 8 KiB, marked when cut) and its time; the
  turn view also shows imported v4 outputs and timings. `kanade import v4 --refresh-logs`
  re-imports only `v4-` log rows, and v4 rounds with no requested tools take the call names.
- v5 serve runs the Kanade chatbot in every channel and thread of the chat categories: pilot-role
  gate, queue-position reactions, allowances with refunds, replies that ping nobody, chat log
  rows, proposal cards approved with ✅, live on/off from settings, graceful shutdown, and a
  `chat` state in health.
- v5 config API reports the capacity groups the bot actually runs (default `gateway` group or
  `kanade.toml` groups, with `groups_source`), checks each group against Kanata's limits, and
  no longer presents the bot's permits as the key's limit (`key_limits.max_in_flight` is null).
- v5 admin app shows display names instead of raw Discord ids (click to copy the id),
  reply profiles as a table, and a tidier Chat log: the implicit bot mention is dropped from
  question previews, times are guild-local ("Mon 28 Sep · 00:00"), and model/duration/name
  cells stay on one line.
- The private v4 rollback env moved from the root `.env` to `legacy/python/.env`; the v4
  Compose file and guides point there.
- v5 reads non-secret settings from `kanade.toml` (`KANADE_CONFIG`; template
  `kanade.example.toml`), including model capacity groups; `KANADE_*` env vars still work
  and override the file, and unknown keys stop startup naming the key only.
- v5: slash-command replies longer than Discord's limits are split at line boundaries
  into follow-up messages (`/fixed list` no longer hangs on "thinking…"); a refused
  completion gets one short fallback edit, and the interaction log names the failure kind.
- v5: the admin API sends name data so the app never needs raw ids: a roles list
  (`/api/admin/roles`), the bot's own user id in the identity, author ids on evidence and
  extraction messages, and full member ids on chat rows.
- v5: serve logs its model setup at startup (listing, each role's model, effective
  reasoning level and its source, refused or unmasked routes), and
  `KANADE_{EXTRACT,CHAT,REWRITE}_REASONING` seed the reasoning levels so
  `kanade models check` matches what serve would use.
- v5: Kanata's `model:level` aliases (e.g. `gpt-6-luna:high`) are recognised as reasoning
  variants of their base model: the config API marks them, a role saved on one uses its
  fixed level, and `kanade models check` lists them under the base.
- v5: reasoning "off" is only used where Kanata allows it (no published list, or one
  that includes `none`); models that require reasoning get their lowest published level,
  the Config API refuses "off" for them, and `kanade models check` shows the level each
  role actually uses.
- v5: live Discord wiring in `serve`: connects with the bot token only when
  `KANADE_EXPECT_V4_STOPPED=1`, registers the retained commands for the guild only,
  syncs the roster (so admins can sign in with Discord), runs the reminder and notice
  tick, handles card and RSVP reactions, reports Discord and scheduler health, and shuts
  down in order; `KANADE_DISCORD_GATEWAY=0` runs the admin API alone. Commands from
  Discord (including `/swap`) no longer carry "(via portal)", and a Discord `/fixed edit`
  posts nothing, as in v4. Interrupted sends are recovered before Discord starts, and a
  refused login (bad token or intents) leaves v5 up but degraded instead of restarting
  and logging in again with the shared token.
- v5: admin config API: the Config page reads every runtime setting, Kanata's live model
  list and the server-only facts, saves one section at a time (reasoning and capacity
  rules checked, safe to retry), and reloads persona profiles live.
- v5: slash commands ported from v4 (guild-scoped): `/fixed add|edit|remove|list`,
  `/schedule`, `/amend`, `/status` (now also covers cancel, own time, done and restore),
  `/swap`, `/rsvp`, `/pings`, `/style`, `/limits`, `/rescan`, `/say`, staff-only `/nick`,
  `/debug ping|clear_test`, with v4's permissions and wording; `/bot`, `/pingtime`,
  `/debug status` and `/debug extract` are dropped in favour of the admin app.
- v5: boss names and difficulty pills no longer run together: Reminders lists one boss per
  line and inline boss tags keep a gap between bosses.
- v5: design experiments (on by default; `?experiments=off` or the command palette turns
  them off): a small morphing loading indicator on buttons while they save, sign in,
  reload or decide, and a gently waving progress bar on rescans; both stay still under
  reduced motion.
- v5: the week planner's cards are wide again (v4's 230px day columns) and the drag grip
  has its own column, so it never covers a boss name or difficulty badge; the Limits page
  says it isn't available yet instead of loading forever.
- v5: the admin app signs in for real: it offers only the methods the server enables
  (Discord, admin token), explains each Discord refusal, returns you to the page you
  were on after a sign-out, signs out properly, hides proposal decisions for non-Discord
  sessions, and says "not available on this server yet" for unbuilt routes instead of
  "Can't reach Kanade".
- v5: the admin inbox follows the Rust inbox: member request types and chat proposals are
  labelled, conflicts can only be rejected, "Move & approve" works for moves, new runs and
  splits, reasons are asked only for member requests, and token/Tailscale sessions are
  told to sign in with Discord to decide Kanade's proposals.
- v5: weekly-timing edits can no longer pair a fresh week version with an old row (which
  could silently undo another admin's change); only a real out-of-date refusal asks to
  reopen the form, and history names admins by who they are.
- v5: the admin app now sends the CSRF token and a retry key with every save, and the
  week version with weekly-timing edits (an out-of-date form shows the reload message
  and keeps what was typed); the dev mock enforces the same rules.
- v5: Svelte 5 web workspace (`web/`) with separate admin and public installable apps
  that keep the portal's look (with WCAG AA contrast fixes), run under a strict CSP with
  no inline code, work offline, and include a keyboard-accessible drag-and-drop planner.
- v5 admin app: v4-style masthead and grouped navigation with boss portraits and entry
  art, plus Week, run sheet (incl. Reset to fixed), Fixed (with per-run update/keep
  choice), Bosses with knowledge, Members and Reminders screens; a `web` CI job runs
  lint, type checks, unit and browser tests against the dev mock (`tools/pwa-mock`).
- v5 admin app: Inbox, Extractions (with re-read jobs), Chat, Limits (model backend
  groups, queue, breaker, admission refusals), History (timeline, diffs, previewed revert,
  restore-week and revert-by-member, checkpoints, "who changed this"), and Bosses pages
  on the new knowledge files; pages load on demand and each app ships its own styles.
- v5 admin app: full Config settings (12 sections incl. three-role model picker,
  per-alias capacity check against Kanata's published limits, reasoning levels
  limited to what each model publishes, a fail-closed cloud/PII warning, the
  self-service redirect mode, public portal toggle, reply-style profiles shown
  read-only with reload, publish and role order), a Channel access section, per-run
  re-read buttons, and a lazily-loaded planner (admin initial JS 71.8 → 43.6 KB);
  settings saves refuse read-only or unknown fields; proposed PWA contracts in
  `docs/v5/admin-api.md` and `docs/v5/limits-contract.md` for the backend to confirm.
- v5 planner cards: the drag-handle column is gone; a small grip sits in the corner,
  the whole card drags (after a short move, or a long press on touch), `M` picks a
  card up from the keyboard, and a press made before the drag code loads still drags.
- v5 admin app: Inbox works by keyboard on phones (arrows browse, Enter opens, Back
  returns to the item), empty tabs explain themselves, the week header fits one row
  at laptop widths, and Chat/Extractions filters show only valid chips and no stale
  rows after an error.
- v5 public app: while the portal is closed only the app shell, status and bot
  identity are served, and the page keeps checking so a reopened portal appears.
- v5: `boss/knowledge/` schema v2 with per-difficulty facts (levels, force, HP, spec)
  and credited iSIingGunz guide imports (Jupiter, Radiant Malefic Star, First Adversary,
  Baldrix, Limbo, seasonal Kai), plus `scripts/boss_knowledge` fetch/validate tooling
  with an anti-copy guard.
- v4 tooling: `scripts/convert_personas_v5.py` converts a v4 persona directory
  (`personas.yaml`, persona folders, behaviours) into the v5 catalog/bundle/profile
  layout using the v4 loaders, with a names-only dry run, no overwrite without
  `--force` and private file permissions.
- v4 rollback Compose now mounts the private v4 persona files from
  `config/personas-v4/`, leaving root `config/personas/` for the v5 layout.
- Added the unreleased Python maintenance foundation with v14 state/lease tables,
  exclusive store ownership, pre-upgrade snapshots and task-bound retirement.
  Startup recovery authority is revoked before repository access is returned.
  Operational maintenance, transfer and production upgrades remain disabled or
  unapproved pending delivery journaling, ingress and reconciliation integration.
- Added v15 per-source adoption-resolution storage with protected control-plane
  writes and snapshot-first synthetic v13/v14 upgrades. Existing upgrades stay
  BLOCKED/pending; runtime classification and private/live database upgrades remain
  out of scope.
- Added offline v13/v14 first-adoption source classification with content-free,
  domain-separated evidence and a fixed-purpose task-bound BLOCKED seed lease.
  Adoption remains pending; no Discord fetch, fake send attempt, or memory-family
  classification is introduced.
- Added private synthetic whole-message adoption binding and reasoned source/
  attempt retirement under a separate persisted BLOCKED task authority. Synthetic
  observations are not proof of remote authorship; no fetch, release, or memory
  handling is included. Only reminder and card message groups may bind or retire
  multiple targets; multi-target decline or digest groups are rejected unchanged.
- Added the J0 delivery-journal kernel: immutable send plans, request/observable
  fingerprints, pre-send intent and target claims, one-call transport handling,
  and atomic native bindings. The kernel alone was not a release or
  live-operation authorization.
- Added J1 v4 Discord effect wiring for reminders, digests, grouped extractor
  cards, and slash/portal debug cards. Bound effects precede reactions; J1 card
  edits, cleanup deletions, and RSVP reactions fail closed under maintenance
  leases. Ambiguous sends are not retried, and confirmed digest/debug deletions
  retire exact bound targets with durable reasons. No production use or release
  is authorized.
- Added J2 v4 chatbot source/semantic-slot journaling for final, refusal, staging,
  and rejection-follow-up replies. Chat placeholder edits, deletion, and reactions
  are lease-gated; an uncertain edit does not trigger a second final reply.
- Added J3 journaling for non-memory mutation notices, repeatable `/say` and guide
  posts, and decline notice binding/retraction. Guide fingerprints cover validated
  decoded files and ordered embeds; uncertain sends are not reported as refusals.
  Personal-memory effects remain excluded, and no release is authorized.
- Added G1 closed-state runtime attempt recovery: a content-free blocker report,
  operator bind of intent/indeterminate attempts to verified bot-authored message
  evidence, and reasoned no-replay retirement (reminder skipped shape, decline
  cooldown, digest week marker, retained v15 memory as `feature_removed`), all under
  a persisted task-bound BLOCKED/FROZEN-only lease. A retired card leaves its
  amendment proposed and reported; extraction never reposts it, so it is resolved
  by portal approve/reject. A retired reminder is never re-armed by a ping-time
  change; moving its run still schedules fresh reminders for the new time. A
  retired decline permanently suppresses further decline notices for that run and
  member. Binding a current-week digest records the week so the reset tick does not
  replace it; a future-week digest retires without a marker, so that week's reset
  still posts (an earlier preview that did land may need manual deletion). Bounded
  Discord history listing is advisory only. No sends, endpoints, or BLOCKED→OPEN
  transition.
- v4: Config → Models picks the extraction and chat Kanata aliases and reasoning
  levels at runtime from the live `/v1/models` list (also `GET /api/config/models`
  and `bossctl config set`), with capability badges, reasoning levels limited to
  each alias's published list, and a privacy warning for `external` or `-cloud`
  aliases. Selections are audited, stored as runtime config rows seeded once from
  `EXTRACT_MODEL`/`CHAT_PILOT_MODEL`/`EXTRACT_REASONING`/`CHAT_PILOT_THINK`, carried
  in bundle `runtime_config`, and honored by startup checks.
- v5: production pseudonym codec (`PseudonymCodec`) that replaces member names,
  aliases, mentions and ids with per-session random fictional given names and
  decodes model output back; not yet wired into chat or extraction (off by default).
- v5: provider-boundary leak scanner that refuses a masked model request still
  carrying a member name, id or Discord snowflake, before anything is sent.
- v5: reminders and the weekly digest post as v4-style embed cards with boss
  portrait and day-of entry artwork uploaded as attachments; the day-of heading is
  rewritten in the persona's voice by the small rewrite model (v4 text as fallback),
  and cards refresh after ✅/❌ reactions.
- v5: pseudonymization switch `models.pseudonymize` (`KANADE_PSEUDONYMIZE`, off by
  default) masks member identities in chat, extraction and rewrite prompts, fails
  closed without a roster, logs `external_masked` routes and `identity_leak_blocked`
  refusals, and stores each masked chat turn's model view and name mapping for admins.
- v5: chat turn details record the persona, reply profile, per-round model, effort
  and route as actually sent, errors and tool rounds, and show admins a "Model view
  (masked)" section with the masked rounds and fake-name table; extractions can be
  filtered by `identity_leak`.
- v5: model and reasoning changes saved in Config apply from the next chat question,
  extraction call and rewrite without a restart (new aliases stay fail-closed until
  Kanata lists them; ungrouped aliases are refused); rewrites now send the rewrite
  role's reasoning level.
- v5: temporary 64k context window for chat and extraction.
- v5: `/debug ping` posts a `🧪 TEST — ` copy of a run's day-of, countdown, amend
  or decline message (embed cards with art) whose ✅/❌ drive real RSVPs, and
  `/debug clear_test` deletes the channel's recent test cards.
- v5: masking now also covers single words of multi-word member names (e.g. a
  first name inside party channel names); words shared by two members get a
  neutral fake name that decodes to nobody.
- v5: chat schedule answers show only the runs the model named (named difference
  D-GROUND-FILTERED) and keep the rest of the answer, including code; replies too
  long for one message continue in follow-up messages instead of being cut.

**Changed**

- v5 admin selected list rows smoothly grow to reveal their full content across
  Fixed, Week, Inbox, History, Members, Bosses and Config; compact multi-boss
  timings use overlapping portraits, and planner days always keep one card column.

- v5 chat reuses another channel's live proposal for the same run change,
  pointing to its existing card (or saying it is still being posted) instead
  of creating a duplicate, while retaining normal retirement and card
  refreshes; cardless chat proposals qualify only during a two-minute grace.

- v5 admin shared M3E furniture now follows the mockups: an unboxed page line
  (display title, bold mono count, serif context, no ⓘ), a "Live HH:MM" chip
  and "Ctrl K" pill, 48 px window title bars with pill search and pill
  controls, panes flush to their window with list items as card surfaces and
  a tonal selection, mono overlines, pill and connected buttons, tonal status
  chips, an icon Approve key with Reject taking the risk fill when Approve is
  blocked, no visible "open" on selected rows, and the phone drawer's Week
  count and Ctrl K hint. A hovered list row now gets its own state layer
  (`--row-hover`) instead of turning the pane's colour.
- v5 admin Fixed, History and Inbox follow their mockups more closely: a
  Fixed timing's whole row is the selectable card (one button per row) at
  the mockup's density (weekday over time, 28 px portraits, flags inline, the
  party as one line of names), its flags read in lower case and Add carries
  a plus; History's filters read
  "Week: every week" and the change pane drops its extra header row; Inbox
  section labels are mono overlines with tonal header chips, and an item
  opened on a phone has "‹ Inbox" in the top bar, a compact header and a
  bottom action bar (pencil, Reject…, Approve).
- v5 admin Fixed's boss picker shows each boss's difficulties on one line in
  compact rows (an existing timing lists its own bosses until "All n
  bosses…"), with 40 px fields and name-chip party picks; the Inbox item
  shows the boss art, the member's words as a speech bubble, participant
  changes as chips, the thread as rows with initials and used messages
  lifted (ready for a per-message `used` mark and a Used/All toggle), the
  message time opening Discord instead of an "open" link, and a decision
  pane that explains a blocked Approve and what Reject does.
- v5 admin Inbox threads come from the item's `thread` (the channel messages
  around the evidence, each marked used) with a Used/All toggle starting on
  Used; Fixed rows open from anywhere on the row without a positioned overlay
  (WebKit-safe) and keep their fill when hovered; selected Fixed and History
  rows also go bold; the phone's back step returns focus after the drawer.
- v5 admin History's Checkpoints tab says "No backups recorded yet" (no
  backup directory configured) instead of an empty table and hides the
  Timeline's filters; tabbed title bars centre their window dots on the tabs.
- v5 admin History rows carry the mockup's dot, a facts line with the row
  count and a one-line summary; a change's fields read one per line, and on
  wide screens "Revert a member's changes…" sits at the foot of the open
  change. Fixed timings lay out as grid rows (still one table, one button per
  row), with Home channel on its own line in the editor.
- v5 admin Inbox threads keep cited messages that were deleted (merged back
  in as used, "no longer stored") and start on All when none is used; the
  phone action bar is focused pencil → Reject… → Approve as it is seen; Fixed
  rows show the party as plain text (full list in the tooltip) and ring a
  focused row inside its card.
- v5 admin Fixed's editor follows B_Fixed's spacing: sections 12 px apart,
  "All n bosses…" and the typed bosses on one line under the rows, the note
  after the party, and the party showing its picks and the first few others
  with a "+n" chip for the rest; the list head lines up with the rows, and on
  phones the party sits under the bosses with no sideways scroll.
- v5 admin planner drops set the time as well as the day: a run dropped
  between runs starts right after the one above (its start + run length), at
  the top of a day it ends right before the run below, an empty day keeps its
  time and own-time runs stay untimed, held within 00:00–23:59. The drag shows
  and announces the resulting time ("→ 23:00"); an overlap that double-books
  a member shows a clash warning (icon and words) on the indicator and both
  cards but still saves. Keyboard moves step by the default run length;
  Shift+Up jumps to just after the previous run, Shift+Down to just before the
  next. A drop made while someone else changed the week is refused as a
  conflict rather than overwriting it. Config gains a Run lengths section
  (default minutes with a slider, boss + difficulty overrides checked against
  the boss list).
- v5 admin app shell (M3E slice 1, gates G1, G2, G6, G7): a navigation rail
  replaces the masthead's grouped nav (96 px, expanded to 240 px from 1440 px
  wide and collapsible, remembered per browser; Inbox badge; account at the
  foot), a 36 px page line replaces the page-head cards (title, count, page
  controls, an ⓘ for one-time help, the Live chip and Commands), windows get
  a 20 px frame and the 1180 px page cap is gone. Phones and phone landscape
  get a 48 px top bar (menu, title, Live, Inbox) and a navigation drawer that
  traps focus and returns it to the menu, instead of the pinned links and
  "More". New M3E tokens (`--select*`, `--pane`, `--board`, `--row`,
  `--chip-fill`, `--seg-fill`) with contrast overrides and a per-face check.
- v5 admin page line: the title group (title, count, context) sits in an
  outlined surface shape with 12 px corners, in every colourway and face (ink
  read washed out on the bare ground, e.g. blossom); the controls stay their
  own chips, History's filters become field chips, and the line stays 36 px.
  Chat's per-model stats no longer stack one row per model: compact chips for
  the two busiest models ("model 37 ✓ · p50 2.2 s") and a "+n models · e
  errors" button opening the full per-model table keep the line one row.
- v4: Kanata request bodies follow per-alias capability metadata from `/v1/models`
  (cached, minimal when absent): `response_format`, `temperature`/`seed` and
  `reasoning_effort` are sent only to aliases that accept them, so extraction and
  chat work on cloud and codex aliases. A 400 naming one of those fields drops it
  for that alias (for the cache TTL) and retries once. The extraction prompt budget
  counts the in-prompt JSON schema, and the client trims a trailing `/v1`.
- Changed the new Rust release and supporting spike code to GPL-3.0-only;
  the independently licensed Python rollback tree retains MIT.
- Organized the Rust bootstrap into responsibility-based runtime, API and CLI
  subdirectories, with a dedicated integration-test directory.
- Refined the unpublished migration contract to preserve grouped Discord sends,
  require matching proposal-card bindings and canonical Discord IDs, and reject
  excluded memory/generic delivery records.
- Restored SQLite foreign-key enforcement when reopening existing v14 repositories,
  including a FROZEN maintenance restart.
- Schema v16: opening a v9–v15 store takes the pre-upgrade snapshot, then drops
  every `chat_memor*` table; retained delivery-journal rows are kept, and a v15
  store keeps its maintenance state unchanged. The snapshot still contains the
  old memory rows. Leftover `CHAT_MEMORY_*` settings are ignored with one startup
  warning.
- v4 extraction and chat now call the Kanata gateway's OpenAI-compatible API over
  verified HTTPS with a bearer key read from `KANATA_API_KEY_FILE` (a Compose secret
  in the container), replacing the direct Ollama client and the `ollama` package.
  New settings: `KANATA_BASE_URL`, `KANATA_TIMEOUT`, `EXTRACT_MODEL`,
  `EXTRACT_REASONING`, `MODEL_CONTEXT_TOKENS`; `CHAT_PILOT_MODEL` and
  `EXTRACT_MODEL` have no default and are required at startup when their feature is
  enabled; a database extractor switch that outranks `EXTRACT_ENABLED=false` gets
  one startup ERROR instead of per-burst warnings. Extraction uses a strict JSON
  schema (closed objects, all fields required). A trailing `/v1` on
  `KANATA_BASE_URL` is trimmed. `OLLAMA_*` settings are ignored with one startup
  warning naming each replacement; the `ollama` pytest marker is now `live_model`.
- v4 `.env.example` now lists only the settings an operator must fill in; every
  other setting, with its default, is in the `docs/setup.md` settings reference.
  Fresh databases now seed a 01:00 day-of ping and a single 60-minute countdown
  (was 09:00 and 60,15); existing portal-stored values are unchanged. Portal docs
  now describe the shared edge front door instead of the kanade Caddy service.
- v4 container files moved to `legacy/python/deploy/` (`Dockerfile`,
  `Dockerfile.dockerignore`, `compose.yaml`); build with
  `docker build -f deploy/Dockerfile .` from `legacy/python` and run Compose with
  `-f deploy/compose.yaml`. v5 container files will live in the root `deploy/`.
  The obsolete kanade Caddy image and Caddyfile example were removed; TLS ingress
  is the shared edge.

**Removed**

- Removed model-request pseudonymization and its external-unmasked opt-in for
  chat, extraction and rewrite. Configured external models receive raw member
  data with a warning; retired privacy keys now refuse startup. Historical
  masked chat records remain readable to admins under their existing retention.
- Removed the v4 personal-memory feature: typed-preference requests and prompt
  injection, `/memory` commands, enrollment notice DMs, proposal cards, the
  cleanup worker, the portal Memory pages, `/api/memory` routes,
  `bossctl memory`, and the `CHAT_MEMORY_ENABLED` setting.

**Fixed**

- v5 chat "when is my next run" lists every upcoming run from now across boss
  and calendar weeks: `get_schedule` with `week:"auto"` and no day no longer
  stops at Sunday, so runs early next calendar week (still this boss week) are
  found instead of "No upcoming runs for you in this week".
- v5 chat keeps a persona sentence that mentions a run and puts the run's
  record under it, instead of replacing the whole sentence with the record.
- v5 chat "when is my next run?" answers with just the soonest run, and
  `get_schedule` `week:"auto"` without a day stops at the end of this boss
  week instead of listing every later week; plural or qualified asks ("my next
  runs", "next run for hard lucid") still list this boss week's remaining runs.
- v5 chat also keeps a persona sentence that names a run by its date and time
  (no id) only when every fact in it was read and matches the run (fail-closed),
  with the run's record under it; another catalog boss or difficulty, a wrong
  or unread number, date, tally, status or channel (also in words), a
  negation, am/pm, a relative day such as "tonight", or more than one time
  still gets the listing. Not caught: a plain name of a boss outside the
  catalog, a lowercase everyday-word alias ("star"), member names, and facts
  phrased in words outside the checked lists.
- v5 avatar initials (Members sheet, inbox transcript, rail, drawer, account
  menu, sign-in and public masthead) show the first readable letter of a name,
  skipping emoji and symbols: "🥔猫铃薯🥔" shows "猫" instead of a broken half
  of the emoji.
- v5 admin phones keep selected list rows on their compact line across Fixed,
  Week, Inbox, History, Members, Bosses and Config, including landscape; the
  sideways settings strip never grows, while desktop rows still expand; the
  Inbox heading wraps each boss name with its pill as one unit, and a Members
  name cell keeps the name whole, ellipsises aliases and drops the "chat only"
  chip below the name rather than past the column.

- v5 admin Bosses: a whole catalog or event row selects its boss, weekly timings
  read as one outlined list and open that timing's editor in Fixed
  (`/fixed?open=<id>`), difficulty notes sit in their own accented callouts, and
  seasonal chips use a short `Seasonal boss · CW3` tag (full season name on hover)
  so the event list no longer scrolls sideways.
- v5 admin Inbox shows a proposed status change as status chips under its
  field name (old struck, new toned, e.g. At risk → Unconfirmed) instead of
  raw `at_risk`/`planned` text pushed into a side column.

- v5 extraction refuses hints naming only done or cancelled runs for kinds
  that act on existing runs; Add/Fix keep v4 behaviour. Dayless RSVP/Sub
  answers stay in their evidence message's boss week without escaping a
  channel that has live runs; bare clocks keep their implicit resolved day
  ("Refuse + anchor", including on rescans).
- v5 Discord chat: asking which seasonal bosses or guides exist now names the seasonal event bosses (Kai, Meilin) with their event and availability.
- v5 Discord chat: after a schedule lookup, a reply announcing a posted card
  (e.g. a move proposal waiting for ✅) is no longer replaced by the lookup's
  run listing; the card reply posts as the model wrote it.
- v5 admin History: the window no longer scrolls away when a row opens, only
  the clicked row of a multi-week change is marked open, tabs and Week/Who
  filters match the other M3E title bars, a change shows compact field diffs
  with the raw JSON in a viewer, and repeated summary lines collapse into one
  with a count. History, Fixed and Members no longer draw two sets of window dots.
- v5 re-reading party channels while watching is paused or the extractor is
  off now says so and points to Config → Watching (API `409 extraction_off`,
  same wording from `/rescan`), instead of "The service is unavailable".
- v5 admin Week help ("How to move runs") now opens as a contained card and
  mentions swapping; the run sheet keeps its action buttons on their own row so
  a moved run's extra actions no longer squeeze the run details.
- v5 long slash-command replies now post their follow-up parts in serve;
  the production transport refused every follow-up (and would have refused
  any other default-bodied transport call) as invalid without sending it.
- v5 chat queue fixes: waiting questions' keycap positions are renumbered
  when a question leaves the queue; a queued question whose channel left the
  chat category is refunded instead of answered; a panic while a question's
  context is built refunds its allowance and logs a failed row. A lookup
  that panics again while the question is being cleaned up no longer aborts
  the process: cleanup is deferred off the unwinding stack and, if it still
  fails, only settles and refunds.
- v5 extraction reads each message version once across live bursts, backlog
  drains and startup/manual rescans (in-memory claims; the other reader
  defers and gets the row back if the owner fails). Rows are marked read
  before proposals or cards are posted, so a crash never repeats effects; an
  edit during a running call drops the stale answer and the edit is proposed
  instead (v4 proposed the old text). Gateway receipt times use the injected
  clock.
- v5 self-schedule lookup now recognizes an exact bot name copied into a
  model's participant argument without treating other members as self.
- v5 day-of reminder fields now label only members still waiting for an answer
  beneath the unconfirmed tally in either attendance mode; the top line retains
  the full party. V4_COMPAT scheduling, tallies and pings are unchanged.
- v5 resolves an explicitly first-person schedule question to its asker when
  a model omits the participant or invents an unrecognized mention;
  third-person requests remain distinct.
- v5 pauses new delivery admissions during gateway outages and until a fresh
  roster reconciliation succeeds, while allowing admitted work to settle.
  Cancelling an admitted send releases its in-memory owner without retrying a
  possibly committed journal claim or an ambiguous Discord operation.
- v5 masks complete links—including unknown handles in URLs—before model calls
  and refuses any URL left at the provider boundary; member-facing links are
  restored locally without storing the token-to-link mapping.
- v5 Reply profiles default to private until explicitly published; admins get
  per-profile and selected-batch Publish/Make private controls without stale
  saves republishing someone else's private choice, and member `/style`
  choices update without a restart.
- v5 admin Chat log Name copy buttons meet minimum pointer target sizes on
  desktop and phones without widening the dense table; the full-page axe
  check now includes the Who column.
- v5 waits for the initial roster reconciliation before its first delivery
  tick, and bounds Discord command-task draining on shutdown so stalled
  interactions cannot hold the store past the grace period.
- v5 Discord sign-in consumes a one-time callback state even when that callback
  is rate-limited, preventing it from being replayed after the quota resets.
- v5 chat replies keep code-block indentation and persona ellipses (`Mou...`
  no longer becomes `Mou..`).
- v5 admin: tool-trace buttons announce a short preview, selecting text in a
  viewer no longer closes it, pagers stay in range after a reload, and copied
  Markdown transcripts fence the question and reply.
- v5 admin Chat log: long questions are clamped to two lines (full text in the
  tooltip and on the turn page) instead of stretching the row.
- v5 admin Config → Channel access now shows the bot's permissions per watched
  and digest channel instead of "not available on this server yet".
- v5 public app: boss entry art on the week board rendered as full-size images
  covering the cards (the run-card styles were missing from the public stylesheet);
  a browser test now checks the art stays inside its card on both apps.
- A failed or maintenance-denied move/problem notice no longer aborts applying
  the remaining amendments on a ✅ card; digest and test-ping failures now say
  delivery was not confirmed instead of claiming Discord rejected them.
- v4 now starts on a BLOCKED, FROZEN or adoption-pending store instead of dying on
  a denied config seed: seeding runs only under an admitted OPEN operation, the API
  starts after Discord login, and the tick, heartbeat, on-ready
  roster/week/identity/backfill work and rescan worker are skipped with no database
  writes after bootstrap normalization (an adoption-pending store is still
  normalized to BLOCKED on open; reconciliation is flagged in-process). The healthcheck reads the mode and accepts a live API
  without a heartbeat while closed.

## Unreleased

**Changed**

- Upgraded  with automated scoring, multi-repetition capabilities, and markdown reporting.

## Unreleased

**Changed**

- Upgraded `scripts/bench_headers.py` with automated scoring, multi-repetition capabilities, and markdown reporting.

## 4.9.0

**Added**

- Weekly digest cards are now tracked per boss week, update after clears and
  RSVPs, replace earlier portal posts, and retire into a persistent weekly log.
- Governed Discord memory uses schema v12 persistence with notification-first,
  case-by-case enrollment, typed review cards, and subject-controlled approval.
- Deterministic typed proposals and scoped retrieval keep memory limited to
  presentation preferences; no chat transcript is imported or backfilled.
- Member `/memory` controls plus authenticated portal, API, and `bossctl` admin
  operations support enrollment, correction, revocation, opt-out, and deletion.
- Boss strategy responses carry bounded source attribution, with full provenance
  and source URLs on authenticated per-boss knowledge pages.

**Changed**

- Weekly digests now lead with each boss and an explicit cleared/planned status,
  with scheduling metadata on a separate line and cleared progress in the summary.
- The chat tool-schema growth guard now allows up to 4,096 estimated tokens.
- Retention cleanup runs on the first tick and hourly thereafter regardless of the
  memory switch; failures are isolated, logged, and retried on the next hourly window.
- Governed memory now expires proposals after 7 days, active preferences after
  180 days, inactive content and retrieval diagnostics after 30 days (diagnostics
  also keep only the newest 500), and lifecycle events after 365 days.
- The rollout remains disabled by default; enabling the capability never enrolls
  members or enables production collection by itself.

**Fixed**

- Whole-next-week chatbot lookups now recover when the model redundantly sends
  `day="next"`, avoiding unnecessary weekday-by-weekday tool loops.
- Chat scheduling now passes Discord user mentions directly to proposal tools
  instead of reasking for names, and routes requests to set up recurring runs
  through new-weekly proposals rather than existing-weekly changes.
- Reworked member review into a single tabbed Memory governance window, keeping enrollment,
  preference setting, records, and activity within the fixed viewport review surface.

## 4.8.2

**Changed**

- Split repository guidance into subsystem-specific `AGENTS.md` files covering
  boss data, runtime packages, chatbot tools, portal assets, and tests.

## 4.8.1

**Changed**

- Removed unused helpers and logger scaffolding, stale generated comments, and
  repeated documentation introductions without changing runtime behavior.
- Pruned redundant implementation-detail tests and made shared chatbot fixtures
  deterministic across boss-week reset boundaries.

## 4.8.0

**Added**

- Atomic major persona bundles: one portal selection now swaps identity,
  default behaviour, and staging copy together from
  `config/personas/personas/<id>/`, described by the private
  `config/personas/personas.yaml` manifest (IDs, labels, default, legacy
  aliases, optional per-bundle filenames). Invalid selections change neither
  the stored choice nor the active answer; in-flight answers stay pinned to
  one bundle.
- Config page **Persona** control with human labels, configured-vs-effective
  display, and safe recovery guidance that never shows prompt text. The API
  keeps existing persona fields and adds `persona_labels`,
  `persona_effective`, `persona_effective_label`, `persona_catalog_mode`,
  and `persona_issue`; `bossctl config` renders the new mappings safely.
- The tracked fallback bundle is Kanade-flavoured (`personas/kanade/`): with
  no configured persona the bot answers in the default Kanade voice instead
  of a placeholder. Keep bundles compact and run with an `OLLAMA_NUM_CTX`
  that fits them plus the tool schemas.

**Changed**

- Chat schedule lookups now treat unqualified weeks as guild-local calendar
  weeks, while explicit boss-week lookups retain reset-to-reset semantics.
- Chat schedule replies now use bounded two-line records; trusted requests for
  runs left, remaining, upcoming, or next exclude completed runs without giving
  the model a filtering option.
- Reply-profile overlays moved up to `config/personas/behaviours/` with
  staging overrides in `config/personas/behaviours/staging/`; the old
  `behaviours/profiles/` and `behaviour-plugins/` paths remain readable as
  legacy fallbacks. The stale `config/personas/identities/` directory is
  gone; live identities live in the bundles.
- `PERSONA_PATH` is now a deprecated fresh-database seed only.

## 4.6.0

**Added**

- Repository-specific `AGENTS.md` guidance for setup, focused checks,
  architecture boundaries, generated assets, migrations, and private files.
- Spaced full-name boss aliases such as `Hard Black Mage`,
  `Normal Radiant Malefic Star`, and `Normal The First Adversary` now resolve in
  commands, extraction, and chatbot run lookups.

**Changed**

- Renamed the internal `Star` boss key and tokens to `MaleficStar`, including
  catalog, knowledge, assets, examples, and a v10-to-v11 SQLite migration.
  Existing `star`, `nstar`, and `hstar` aliases remain accepted.
- Reminder cards now use the lead boss's catalog colour, with the previous card
  colours retained as fallbacks when no boss colour is available.

**Fixed**

- Moving a recurring run across the boss-week boundary into a week that already
  contains that recurring run now returns a clear conflict instead of leaking a
  SQLite uniqueness error through the portal, slash command, or proposal card.
- Same-weekday future times such as `sat 22:30` now resolve to later today rather
  than the same weekday one week later; past times still roll forward.

## 4.5.0

**Added**

- Reply-profile search on the Config page, filtering by name and
  instructions, plus a public/private visibility filter (client-side,
  like the existing pagination).
- Reply-profile editors open as modals instead of inline disclosures,
  matching the member sheet.
- Bulk publish / make-private for reply profiles, with a two-stage select
  toggle (page, then all) that prioritises the active filter.
- `.githooks` mirroring the CI gates, documented in Development.

**Fixed**

- Updated seven stale tool-schema tests to the deliberately trimmed 4.3.0
  copy instead of re-adding bulk to the schemas.

## 4.4.0

**Added**

- Persona-aware chatbot staging lines loaded from YAML: defaults in
  `config/personas/behaviours/staging.yaml` with per-profile overrides in
  `behaviours/profiles/staging/<profile>.yaml` (partial files inherit the
  rest from `default`; unknown profiles use `default`; `{boss}` still
  interpolates the resolved boss).
- Silent processing indicator for chat answers: a no-ping staging
  placeholder plus typing indicator while the model works. Success deletes
  the placeholder and posts the final answer with its single ping; failures
  edit the placeholder in place. Rejection follow-ups use the active
  profile's generic line.
- `silent` option on `post_plain` (`AllowedMentions.none()`) for staging
  placeholders, plus `edit_plain` / `delete_placeholder` helpers.

**Changed**

- Staging copy is validated at startup: unknown keys and non-string values
  are reported (falling back to defaults) instead of surfacing mid-request.
- `bot.__version__` brought back in line with the package version.

## 4.3.0

**Added**

- Separate `CHAT_PILOT_THINK` reasoning for speech, falling back to
  `OLLAMA_THINK`, with per-call model+think logging.
- `list_fixed` read-only chat tool for recurring weekly timings.
- Portal Fixed edit can shift a timing's home channel to another watched
  channel (live runs follow; unwatched channels rejected).
- Strategy narrow/clarify replies are rewritten in voice in one no-tools
  round, falling back to the static meaning on failure.

**Changed**

- Strategy answers rewrite the guide in voice (opener, bullets, closer),
  copy boss names exactly, and omit source URLs / Sources (no Discord
  embeds; the tool strips them, the domain keeps them for audit).
- Scheduler four-sentence limit applies to scheduler replies only;
  strategy/guides use compact bullets and are exempt.
- Chat tool schemas trimmed for context budget, with a schema-token guard.

**Fixed**

- `get_schedule` treats blank day/participant/difficulty as omitted and
  reports bad `scope` with its valid values.
- Role mentions (`<@&id>`) no longer split multi-boss strategy targets.
- Proposal cards say NOT DONE until ✅ (no more "move's done").

## 4.2.0

**Added**

- A validated, source-backed boss strategy knowledge base and read-only chat
  tool. Explicit strategy questions retrieve checked-in mechanics before the
  bot answers in its configured persona.

**Changed**

- Boss metadata, strategy documents, portraits, icons and entry artwork now
  live together under `boss/`.
- `bossctl guide` derives boss entries from the canonical catalog and uploads
  thumbnails as file data instead of sharing filesystem paths with the bot.

**Fixed**

- Chat no longer leaks tool refusals into Discord. A clarification that
  contains a `?` anywhere (not just trailing) is kept as-is on `REFUSED`,
  unless it falsely claims a card went up. Overwritten refusals are passed
  through the member-facing filter, which now strips model-only
  instructions (`short form` / `back to the tool` / `for the tool only` /
  `never show them`, `propose_*` names).

## 4.1.1

**Added**

- `bossctl guide` — posts the full server guide to a Discord channel.
  Reads message templates from `config/guide.yaml` and boss entries from
  `config/guide_bosses.yaml`, attaching portrait thumbnails as embed
  images with per-boss accent colours. API endpoint `POST /api/guide`
  added for messages with embeds and file uploads.
- Guide config files (`guide.yaml`, `guide_bosses.yaml`) are
  git-ignored with committed examples and a `config/README.md` for
  setup instructions.
- Role IDs in guide messages now render as clickable Discord mentions.

**Changed**

- **Personas directory moved** from `personas/` to `config/personas/`. All
  bind mounts, config defaults, gitignore rules, and documentation updated.
  Legacy root-level identity files and `behaviour-plugins/` profiles remain
  readable during migration.
- Guide "Talk to me" section updated: speech-pilot works anywhere under
  the bossing category, not a single channel.
- Guide messages now have vertical spacing between sections.
- Footer is a standalone message (no `---` divider).

## 4.0.1

**Added**

- `bossctl post-message` — post a Discord message from the CLI with full
  markdown support. Accepts `--channel`, `--file`, or `--stdin` for longer
  content. API endpoint `POST /api/say` added alongside it.
- Members page redesigned as a compact card list with modal detail sheets,
  replacing the overflowed 8-column table.

**Changed**

- Docker volumes renamed from `kanade-bot_*` to `kanade_*`; old volumes
  migrated and removed.
- `get_schedule` now correctly resolves the entire boss week when `day` is
  omitted or empty, instead of refusing with a validation error.
- Member list rows show a chevron affordance and tighter spacing.

**Fixed**

- Member list rows now visually indicate clickability.
- Verbose comments condensed across `get_schedule.py`, `_modal.scss` and
  `_runsheet.scss`.

## 4.0.0

**Added**

- **Member-selectable reply styles** (`/style` command): members choose from a
  public catalog of 13 profiles — chuunibyou, concise, gacha-addict, imouto,
  kouhai, kuudere, mesugaki, ojou-sama, onee-san, raid-leader, sleep-deprived,
  tsundere, vip-butler — with Discord autocomplete, ephemeral feedback, and
  `none` to reset to default. Choices are saved per-member in SQLite.
- **Component prompt system**: the chatbot prompt is assembled from discrete
  components in explicit precedence — identity, default behaviour, active
  profile, assistant scope, scheduler policy, grounding policy, runtime
  context, and voice cue — instead of a single monolithic template.
- **Dynamic assistant name** extracted from `# Persona: ...` in the identity
  file; never hard-coded.
- **Role-based style precedence**: first configured readable matching role
  silently supersedes a member's saved choice. The member sees the same
  response style; the mechanism is never disclosed.
- **Portal style visibility**: Members page shows both the saved style and the
  style that would apply to the next reply. Config page includes profile
  publication toggle, role-priority move controls, and broken-entry
  diagnostics.
- **Profile deletion guard**: profiles assigned to a role or published cannot
  be deleted.
- Code-owned prompt assets: `assistant-scope.md`, `scheduler-policy.md`,
  `grounding-policy.md` in `bot/chat/prompts/`.

**Changed**

- **Schema v9 → v10**: `members` table gains nullable `reply_style`.
  Unversioned databases and schemas older than v9 are rejected at startup.
- Compose project renamed from `kanade-bot` to `kanade`; `bot` and `caddy`
  services retained, Valkey deferred until multi-process scaling.
- Persona directory restructured into `identities/`, `behaviours/`, and
  `behaviours/profiles/`; legacy filenames remain valid with new paths taking
  precedence.
- Behaviour-plugins renamed to reply profiles under `behaviours/profiles/`;
  portal labels updated accordingly.
- Style resolution uses first-configured-readable-match instead of composing
  all matching role assignments.
- `/style` reports a choice was "saved", never "active".
- Identity template deliberately excludes a `**Voice:**` slot; default
  behaviour carries the trailing voice cue.
- `CHAT_PILOT_HISTORY_TTL_S` default reduced to 2700 seconds.
- Verbose comments condensed across all touched files.
- Dockerfile copies only tracked fallback templates; live files come from the
  host bind mount.

**Fixed**

- Profile deletion no longer silently orphans role assignments or public
  publication flags.
- Missing persona files fall back to tracked templates with a log warning
  instead of failing silently.
- Tool-schema diagnostic logging added for easier troubleshooting.

## 3.3.0

**Added**

- The web portal now persists and displays each model round, including thinking
  traces, raw responses, tool arguments, complete tool results and posted-card
  outcomes.
- `get_schedule` now supports `today`, `tonight`, `tomorrow` and weekday filters,
  composed with participant and channel scopes.
- Added `Lotus` boss into list of available bosses.

**Changed**

- Reorganised the Python package into `agent`, `domain`, and `infrastructure`
  namespaces, and split the chatbot's monolithic tool module into individual
  tools behind the existing `bot.chat.tools` interface.
- Reminder reconciliation now preserves mappings for already-posted reminders,
  reopens eligible skipped reminders, retires newly past reminders and rebuilds
  countdowns only for live runs.

**Fixed**

- Run lookup now resolves weekdays to one concrete date across current and next
  boss weeks and refuses conflicting day references instead of choosing one.
- First-person schedule requests no longer mistake the bot's user or managed-role
  trigger mention for a roster participant.
- Bare date questions now default to the whole group's schedule across all
  channels instead of silently applying person and channel filters.
- Member-facing replies no longer expose scheduler function/option syntax or
  emit the `<none>` placeholder; known runs in other channels remain explicit.
- Failed Discord proposal posts can no longer be described as successfully
  posted cards merely because their database rows were created.
- Reminder reconciliation no longer deletes live morning-message mappings,
  creates no-op audit entries or leaves stale reminder states behind.
- Wide Limits tables now scroll on narrow screens instead of clipping the mobile
  portal.
- `get_schedule` tool call description tightened

## 3.2.0

**Added**

- `get_schedule` can filter by `participant="me"` or one roster name while
  retaining whole-group and channel-only schedule scopes.

**Changed**

- Chatbot factual replies now use compact Discord Markdown: bold boss names and
  actions, italic dates and times, and code-formatted ids, statuses and RSVP
  tallies.
- Visible cross-channel schedule references are now clickable Discord channel
  links.
- Multi-block chatbot replies preserve one blank line around headings and
  remarks while keeping consecutive schedule rows compact.
- Package, runtime and API version metadata now report `3.2.0`.

**Fixed**

- Named participant filters are resolved against the roster instead of silently
  returning the unfiltered schedule. Invalid, unknown and ambiguous participant
  values are refused with a clarification prompt.

## 3.1.2

**Fixed**

- `parse_when` now resolves `next <weekday> HH:MM` (e.g. `next tuesday 22:30`).
  `dateparser` returns `None` for this form when `PREFER_DATES_FROM=future` is
  set; the fix falls back to the extractor's own day/time resolver, which already
  handles `next` via `_NEXT_RE` and `_WEEKDAY_ALIASES`.
- Failed proposal cards now include the proposal summary in the pipeline error
  log, making it possible to identify which `propose_add` triggered a post
  failure without enabling `DEBUG` logging.
- Tool response text is now logged at `DEBUG` on the success path in addition to
  the existing argument trace, completing the picture for `LOG_LEVEL=DEBUG`.
- The Config section pills no longer overlap on mobile, and wide settings content
  can no longer force the portal beyond the viewport.

## 3.1.1

**Changed**

- The portal stylesheet now has an SCSS source layout: `bot/api/static/portal.scss`
  is the ordered entrypoint and `bot/api/static/portal/*.scss` holds the split
  partials, broken out from the former monolithic `portal.css`.
- `bot/api/static/portal.css` is now generated and git-ignored. Compose builds it
  into the image from the SCSS sources; local and packaged runs serve the same
  bundle from memory when the artifact is absent.
- The package and API version metadata now report `3.1.1`.

## 3.1.0

**Added**

- **Behaviour plugins** layer reusable Markdown instructions on top of the active
  chatbot persona without replacing its voice or operating rules. Discord roles
  can be assigned different plugins, and a member holding several configured
  roles receives every matching plugin in assignment order while the main chat
  role remains required for access.
- **Live plugin management in Config → Chatbot**: create, edit and delete plugin
  files, then add, update or remove role assignments without restarting the bot.
  Plugin and assignment editors are collapsible and independently paginated for
  larger lists, preserving the current page across form submissions.
- Deployments can seed initial role assignments with `CHAT_ROLE_PLUGINS`.
  Portal-managed assignments persist in SQLite, plugin instructions persist in
  the writable, git-ignored `personas/behaviour-plugins/` directory, and the
  tracked `example.md` documents how to write additional plugins.

**Changed**

- Matching behaviour-plugin instructions are reinforced on every model round,
  including direct answers and automatic clarification follow-ups. The base
  persona's factual and safety rules continue to override style instructions.
- The compose persona mount is writable so portal-created plugin files survive
  container rebuilds while the rest of the container root remains read-only.

## 3.0.2
**Added**
- Minor UI tweaks to the portal

## 3.0.1
**Fixed**
- The chat model could combine a canonical boss token with a second difficulty, 
  generating `XBM Hard` for “Extreme BM.” Validation correctly rejected `Hard` 
  as an unknown second boss, so no proposal card was created. Updated the 
  `propose_add` tool description to distinguish canonical tokens from spoken 
  difficulty-first names, prohibit combining both forms, and explicitly map 
  “Extreme BM” to `XBM`.

## 3.0.0

**Added**

- **The portal redesigned as "Kanade's Desktop"**: cream windows with chrome
  title bars on a coloured ground, five selectable colourways — marigold (the
  default), blossom, periwinkle, coral, twilight — each with an after-hours
  dark face, and a System/Light/Dark control. The choice lives in the browser
  and is stamped before first paint, so nothing flashes. The bot's own Discord
  avatar and banner are the portal's identity: the masthead, the favicon and
  the login window's hero.
- **The Week page is a day board**: seven columns starting at the boss-week
  reset, where an empty day collapses to a spine and the days with runs take
  the room. Compact cards open a run sheet — the full card in a dialog, with
  a plain fragment link when JavaScript is off — and a now-strip answers the
  page's four questions (next run, answers owed, inbox, model) before any of
  it is read.
- **No page scrolls on desktop**: the table pages became searchable, paginated
  windows that scroll inside a fixed frame — server-side search and paging on
  Audit, Extractions, Chat and Reminders, search on Members and Fixed — and
  Config became one Settings window, a table of contents on the left and one
  section at a time on the right, switched by fragment alone.
- **The bosses bring their own artwork**: `config/portraits` now has two sizes
  (the full art goes out on Discord's embed thumbnails; the portal's small
  renders keep the crisper 64px icons), and `config/artwork/entry` holds each
  boss's entry splash, laid behind the week's run cards as a veil that costs
  no height — one boss takes the side vignette, two take a corner each and
  meet in a seam. Both directories are git-ignored beside tracked READMEs;
  everything renders fine without them.
- **The portal draws its own icons** — inline Feather strokes in
  `currentColor`, so every colourway and dark face tints every icon. Discord
  keeps its emoji vocabulary untouched: over there a reaction *is* an emoji.
- **The morning ping carries the boss's entry splash**: the day-of message
  wears the lead boss's entry art as its embed image, on top of the portrait
  thumbnail it already had. Countdowns stay text-lean.
- **Limits, a chat's detail and an extraction's detail became tabbed browser
  windows** — fragment-switched tabs on the window chrome, the same no-script
  `:target` machinery as Settings, with live counts on the Limits tabs and the
  allowance form kept outside the polled region so a refresh never eats what
  you were typing.
- **Personas moved into `personas/`**, bind-mounted read-only into the
  container — tracked README and template, everything a deployment actually
  writes git-ignored — and the Config page's Chatbot panel says which file the
  voice is coming from, marked when it fell back to the template.
- **Voices swap live**: every `.md` in `personas/` (bar the README) is a
  dropdown on the Chatbot panel, the choice is runtime config seeded from
  `PERSONA_PATH`'s basename, and the next answer is in the new voice — no
  restart. Submissions are validated by membership in the real directory
  listing, audits carry filenames only, and a chosen file that goes missing
  falls back to the template and says so on the panel.
- **The README shows the portal**: six screenshots in `docs/images/`, with the
  week board in a `<picture>` tag so GitHub serves the light face to light
  readers and the Twilight one after dark.
- **A caddy front door** (`caddy/` service in compose): the portal is served
  over HTTPS at a personal domain with a real Let's Encrypt certificate,
  reachable only from the tailnet. The public A record points at the host's
  Tailscale IP — a CGNAT address that resolves everywhere and routes nowhere
  outside the tailnet — and the DNS-01 challenge means no port ever opens to
  the internet. Docker publishes 443 on that IP alone (`CADDY_BIND_IP` in
  `.env`), so the socket never exists on the LAN. Personal pieces follow the
  `.env.example` pattern: `caddy/Caddyfile.example` is the tracked template;
  the real Caddyfile and the Cloudflare token (`.env.caddy`) stay untracked.

**Changed**

- **`tailscale serve` is retired** — it only speaks its machine's ts.net name
  and rejects any other hostname at the TLS handshake, so the old ts.net URL
  is gone. The loopback `127.0.0.1:8080` mapping stays for host-local CLI and
  dev use.
- With the serve proxy gone, the `Tailscale-User-Login` header no longer
  arrives: the portal asks for `ADMIN_TOKEN` login on every device, and
  `TRUST_TAILSCALE_HEADERS` / `ALLOWED_TAILSCALE_LOGINS` are effectively
  idle until some future front door re-authenticates tailnet identity.
- The board's compact cards speak the party's own shorthand — `NCarling`,
  `HStar` — so a boss and its difficulty pill always hold one line; the full
  names stay on the sheet, the tooltips and the screen-reader labels.
- The Config page's `.env` panel names both models — **Data model** for
  extraction and rescans, **Speech model** for the chatbot's conversations —
  where one "Model" row used to stand for two different machines.
- The compose project follows the repo's name: project and container are
  `kanade-bot`, and `docker compose up -d --build` is the whole deploy.

**Fixed**

- Type reads at an honest size everywhere — a seven-step ladder with body text
  at a true 16px — and a difficulty pill can no longer be clipped at a narrow
  column or orphaned on a line away from its boss.
- `pytest -q` no longer doubles into silence: the verbosity flag is out of
  `addopts`, which keeps only the marker filter.
- The Settings sidebar's raised ground meets the window's title bar instead of
  leaving a strip of card surface between the two.

## 2.1.0

**Added**

- **Capacity controls** for the one 13 GB model the host has. A shared model
  lock (`bot/modellock.py`) serialises the chatbot, the extractor and rescans;
  staff questions queue for the model while everybody else is turned away with
  💬 after a short wait (`CHAT_PILOT_LOCK_WAIT_S`). A guild-wide answer budget
  (`CHAT_PILOT_GLOBAL_RATE_*`) sits on top of the per-person window, so handing
  out the pilot role more widely cannot monopolise the machine.
- Rate-limit refusals now say when to come back — ⏳ plus one canned sentence
  per episode with the wait in it, never a model call. Per-member windows can
  be cleared from the portal, `DELETE /api/limits/windows/{id}` and
  `bossctl limits reset`, all audited.
- **Custom rate limits**: the four capacity numbers are runtime config like
  `chat_mode` — seeded from `.env`, edited from the portal and `bossctl`
  without a restart — and members can be granted their own allowance
  (schema v8), applied live and quoted in their own refusal notice.
- **A Limits page** in the portal and `GET /api/limits`: who has the model and
  for how long, both budgets as used-of-total, open per-member windows with
  reset and override controls, everyone holding the pilot role (staff marked
  exempt), and the rescan queue — updated by server-sent events
  (`bot/events.py`, `GET /limits/events`) the moment something changes, with a
  slow visibility-aware poll and a plain Refresh link as fallbacks.
  `bossctl limits` prints the same view.
- **`/limits`** slash command: your own allowance as a progress bar, ephemeral,
  with when a spent answer comes back; staff get one line and no numbers.
  Reading it never spends anything.
- **`propose_change_fixed`**: the chatbot can change an existing weekly timing
  in place — its night, its party, or both — through the usual ✅/❌ card.
  Same row, same run ids, RSVPs kept; several matching weeklies refuse with a
  candidate list rather than guessing, and `propose_move`/`propose_add` steer
  the recurring case here instead of minting duplicates.
- **Chat memory**: remembered turns age out per turn
  (`CHAT_PILOT_HISTORY_TTL_S`, 45 min default) so a stale topic cannot claim
  "move it to 22:00" an hour later; the prompt names the last card posted in
  the channel, party included; and replying to an old bot answer re-anchors
  that exchange into context past the TTL.

**Changed**

- Creating a weekly timing whose week already holds the matching one-off run
  now **adopts** it — same id, answers and reminders kept, retimed to the
  weekly slot — instead of materialising a duplicate beside it. Through every
  door: the card, `/fixed add` and the portal.
- Tool steering closes three live failures: "this is fixed" on a new run maps
  to the weekly flag, "for me" puts the asker on the run, and asking to change
  a weekly that does not exist explains the conversion instead of offering
  other bosses' timings.
- Portal cards for all `fix` variants finally read alike — "change weekly ·
  every Wed 23:30" instead of "new weekly · TBD" — in the inbox and the chat
  interaction trace.

**Fixed**

- Chat generations in two channels could overlap each other and an extraction
  inside Ollama, timing everything out at once while the host did all the
  work; everything now queues for the same lock.
- `resolve_fixed` no longer matches a query's weekday against other bosses'
  weeklies when the boss it names has none.
- The Limits page no longer rebuilds its poll timer on every refresh or wipes
  a half-typed form; forms live outside the refreshed region.

## 2.0.0

**Added**

- **The chatbot** (`bot/chat/`): mention-gated, role-gated, rate-limited, with a
  persona loaded from the data volume. Read tools answer scheduling questions
  directly; write tools draft the same ✅/❌ proposal cards everything else
  uses — the model can never touch the schedule itself. Understands the group's
  own language: "tonight 23:00", "tmr 2300", bare clock times, "Hard Baldrix"
  and "Extreme Kalos" spelled out, weekly versus one-time runs.
- Rejection follow-up: ❌ a card the chatbot drafted for you and it asks — in
  voice, once per card, cooldown-guarded — what you would like instead.
- Chat analytics: every interaction logged with its tool trace, rounds, latency
  and token counts; a Chat page in the portal, `GET /api/chat`, `bossctl chat`.
- The chat model is its own setting (`CHAT_PILOT_MODEL`), so conversation can
  run on a larger model — Ollama's hosted ones included — while extraction
  stays local.
- **Audit trail** (schema v7): every schedule mutation records surface, actor,
  action, subject and detail. Portal actions name the tailnet login when
  `TRUST_TAILSCALE_HEADERS` vouches for it, `bossctl` names the OS user, cards
  name the reacting member, chat-drafted cards name the asker, slash commands
  name the invoker. An Audit page in the portal, `GET /api/audit`,
  `bossctl audit`.
- Container hardening: read-only root filesystem, all capabilities dropped,
  no-new-privileges, memory and pid caps. Dependabot version bumps, security
  alerts and secret-scanning push protection on the repository.

**Changed**

- Chat write tools are scoped server-side: proposing a change to an existing
  run requires being on it (or owning the weekly timing behind it) and asking
  from its home channel; admins are exempt. Retiring superseded cards is
  channel-scoped the same way, so a draft raised elsewhere can no longer bury
  a party's pending card.
- Schedule answers mark finished runs as already happened and say plainly when
  nothing upcoming is left, instead of leaving the arithmetic to the model.
- The system prompt states the configured chat model and the developer
  attribution, so "what model are you on" and "who made you" get facts, not
  inventions.
- Documentation split: setup, commands, extractor, chatbot, portal and
  development each have their own guide under `docs/`; the README is a pitch
  and an index. Licensed under MIT.

**Fixed**

- Member text can no longer impersonate the scheduler's own bracketed notes to
  the model; the note shapes are defused where member text enters the prompt,
  and guild tags like `[SAKU]` pass untouched.
- The persona voice reminder now actually arrives last: gpt-oss's template
  hoists trailing system messages into the top instructions header, so it is
  sent as a user-role scheduler note instead — which is also why card
  confirmations kept coming out flat.
- A blank `API_PORT=` line in `.env` no longer silently fails the container
  healthcheck.

## 1.9.0

**Added**

- `/say` — admins post as the bot, verbatim. The mention allow-list is built from
  the `@mentions` in the text, so the message reaches exactly who it names and
  nobody else. `@everyone`/`@here` is always blocked, and quiet mode silences it
  like everything else.
- `ADMIN_ROLE_ID` is now the "who runs the bot" role: it grants `/say`, `/debug`
  and the right to change any run, not just your own. Discord's own Administrator
  permission and the server owner qualify too, so leaving the setting empty locks
  nobody out.
- The portal records who answered a run and when, alongside the reaction tally.
- Continuous integration: lint, format check and the full offline test suite.

**Changed**

- `/say` and `/debug` no longer appear in a non-admin's command picker, and the
  permission is checked again when the command runs — a server can hand the
  picker entry back out, so hiding it is not the gate.
- Countdown pings now go to everyone on the run except those who have declined.
  An hour out, the people who are coming want the reminder whether or not they
  have ticked; somebody who reacted ❌ has already answered and is named on the
  card without being pinged again.

**Fixed**

- Posted reminder cards no longer freeze at the tally they had when they were
  sent. Every write that changes what a card shows — a reaction, `/rsvp`, the
  portal, an RSVP extracted from chat — queues a re-render, so a card that still
  read "confirmed · 2/4 ✅" hours after everyone had answered now keeps up. Card
  edits carry the same mention allow-list as the original send, so refreshing can
  never become a second way to ping.

## 1.8.0

**Added**

- The weekly digest posts automatically at boss-week reset, idempotent across
  restarts and slept-through resets.
- Nightly database backup: one SQLite online-backup snapshot per local day,
  written to `data/backups` on the host, converted out of WAL so each file is
  self-contained, and pruned to the newest fourteen.

**Changed**

- The digest distinguishes runs that are at risk because somebody declined from
  those that are merely unconfirmed, and counts them separately.

**Fixed**

- Removing a ❌ reaction now retracts the reschedule notice, matching what
  `/rsvp` and the portal already did.
- A fully answered run with someone out no longer reads as all-confirmed.

## 1.7.0

**Added**

- Quiet mode: a runtime toggle that posts everything with an empty mention
  allow-list and a bell marker, for working against a live guild without
  notifying it.
- Post resilience — bounded retry on DNS and timeout failures, stranded proposals
  re-posted, and one channel's failing rescan no longer affects the others.
- Portal: dialog editors on the Fixed page, rendered mentions and move arrows in
  the inbox, and a quiet-mode toggle on Config.

**Changed**

- Prompts are token-budgeted against a calibrated estimator. Oversized message
  bursts are read in chunks but still consolidated onto a single card.
- Card arbitration: decisions beat questions for the same run, the latest
  evidence wins ties, and ambiguous matches are split or dropped rather than
  guessed.
- Compose: the database moved to a named volume with a 60-second stop grace
  period, after a hard kill mid-write corrupted it.

**Fixed**

- A partial move inherits the matched run's own day and time instead of
  resolving to TBD.
- Evidence can no longer match a run in a later boss week, so next week's runs
  cannot be dragged backwards.
- A `sub` with no named replacement proposes a plain weekly removal rather than
  claiming a stand-in is needed.
- An incomplete or retracted tally no longer demotes an already-confirmed run;
  only a decline or a line-up change does.

## 1.6.0

**Added**

- Per-member ping levels — `/pings essential|all|off`, also settable from the
  portal, the API and `bossctl`.
- A single mention resolver: only day-of cards, unanswered countdowns, proposal
  cards and decline notices notify anyone. Every other post names people in
  plain text.
- A live per-channel Manage Messages check, surfaced on the portal's Config
  page, in `/debug status` and in `bossctl access`.
- `scripts/bench_extract.py`, the benchmark behind the current model choice.

**Fixed**

- One timing change per run per card, chosen by precedence, instead of a card
  carrying two contradictory amendments for the same run.
- Run hints and matches now require a shared boss, so an amendment can no longer
  land on an unrelated run.
- Moves, `otot`s and cancels that would change nothing are dropped rather than
  proposed.
- Deleting a card marks its proposals withdrawn, so they leave the inbox.

## 1.5.0

**Added**

- Rescans run on a queue instead of blocking the bot. A request returns a job id
  with progress, cancellation, and a list of recent jobs.
- Per-run member swap for a single week — `/swap`, `bossctl swap`, the API, and a
  chip UI in the portal.

**Changed**

- Rescan bursts are grouped by local calendar day, carry the 25 messages before
  them as context, and produce one consolidated card per channel per rescan.
- Automated rescans are capped at 48 hours and never widen into the previous week.

**Fixed**

- Day-only amendments whose day has already passed are dropped as stale.
- A day's single stated time carries onto same-day moves that lack one.

## 1.4.0

**Added**

- Week-wide rescan that pulls Discord history first, plus a startup backfill for
  each watched channel.
- Explicit run status control — planned, confirmed, otot, done, cancelled — from
  `/status`, the portal, the API and `bossctl`.
- Portal: in-game difficulty pills, a boss-grid picker, a bosses page, and boss
  portraits with a monogram fallback.

**Changed**

- A run is marked done once its slot has passed, and drops out of `/schedule`,
  the portal and `bossctl` by default.
- Every change made through the portal or CLI is announced in the run's home
  channel, marked *(via portal)*.

**Fixed**

- Open redirect on the `next=` parameter of the login route.

## 1.3.0

**Added**

- An HTTP API served by uvicorn inside the bot's own asyncio loop, bound to
  loopback. Bearer-token auth, opt-in Tailscale identity, and a signed session
  cookie; the health endpoint stays unauthenticated.
- The web portal — week view, fixed-timing editor, proposal inbox, extraction
  log, members, reminders and config. Light and dark, and every form works
  without JavaScript.
- `bossctl`, covering the same operations from a terminal.
- The weekly digest card.

## 1.2.0

**Added**

- Chat extraction: a keyword gate with fuzzy boss aliases, a structured-output
  schema, and a deterministic merge, resolve and match pipeline that proposes
  `move`, `add`, `cancel`, `otot`, `sub`, `split` and `fix` amendments as cards.
- A proposal applies only when a participant with the bossing role reacts ✅, and
  expires after 24 hours. An RSVP stated in chat is the one exception and is
  recorded straight away.
- `/rescan`, `/debug extract`, and `python -m bot.extract` for offline dry runs
  over an exported channel.
- A fixture suite runnable against the live model with `pytest -m ollama`.

## 1.1.0

**Added**

- Day-of and countdown reminders are embeds carrying full boss names and
  difficulty.
- A stale-reminder guard, so a host that was asleep does not replay old pings.

**Changed**

- ✅ and ❌ are mutually exclusive. Decline notices are deduplicated, and
  retracted when the decline is withdrawn.
- `/fixed add` no longer adds its creator automatically — only the participants
  named are pinged.
- Times may be typed as `2359`, `930`, `9pm` or `9:30pm`.

## 1.0.0

First working release.

**Added**

- Roster synced from the bossing role, with no manual upkeep.
- `/fixed` baseline timings, materialised into concrete runs at each boss-week
  reset, with the current and next week always populated.
- Day-of and countdown reminders, stored as rows in SQLite so a restart never
  loses or replays a ping.
- ✅/❌ reactions driving run status.
- Deployment with Docker Compose.
