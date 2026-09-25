# Extraction orchestration

Status: `src/extract/pipeline/`, `src/extract/backlog.rs`,
`src/extract/rescan/` (slice E4). v4 reference: `bot/extract/pipeline.py`
(`Pipeline.offer`/`flush`/`extract`/`rescan_window`/`apply_plan`) and
`bot/agent/rescan.py`. The self-service redirect (slice N1) is wired
below; cards and ✅/❌ (slice E5) are `bot::cards` (`docs/v5/discord-adapter.md`
*Proposal cards*), whose `CardOutbox` implements `Outbox`. Serve wiring
(S9) is `src/runtime/serve/extract/` with `bot::extract_feed` (gateway
messages and the rescan `History`); see `runtime-bootstrap.md` *Live
serve*. Not wired yet: the Limits view, self-service links (cards only
while the public portal is closed) and `check_reasoning_effort` at startup.
Serve sets `handled_by_chat` from the chat driver's verdict (answered,
queued, shed or rate-limited) and keeps it for that message's later edits;
`store_message` caches such a message as processed, so neither a live pass
nor the startup rescan reads it (a manual rescan still may, as v4). Only
regular messages and replies are offered or backfilled (system messages
such as a thread's creation carry its name as content; v4 did not filter).
`Extractor::cancel_calls` (shutdown) cuts calls and permit waits in flight,
and any started later: each is logged `failed` with `CALL_CANCELLED`
(`cancelled: serve shut down`) and its messages stay unprocessed; a rescan's
turned-away wait is cut too.

**The off switch** (`Guild::extraction_enabled`) is read before every call
(none is sent while off), before every rescan burst and channel, and before
proposing: a pass whose calls answered after the switch went off logs them
`failed` with `CALL_SWITCHED_OFF` (`cancelled: extraction switched off`),
proposes nothing and posts no card. Serve's settings hook, on on→off, also
calls `Extractor::interrupt_calls` (cuts the calls and permit waits in
flight, same log) and `Rescans::switched_off` (queued and running jobs end
`cancelled` with `error` `switched off`; unlike `close`, new jobs are taken
again once back on).

## Ports

No Twilight types cross into `src/extract`:

- **In**: `MessageEvent::{Posted, Edited, Deleted}` carrying
  `IncomingMessage` (id, parent channel, author and `AuthorKind`, times,
  content, `MessageOrigin::{Live, Replay}`, `handled_by_chat`).
- **`Guild`**: extraction switch/pause, watched channels, members, bossing
  role, boss table, channel names.
- **`Proposer`**: the scheduler's `propose` and `supersede_proposals`. The
  pipeline changes nothing else.
- **`Outbox`**: `card` (one per burst or rescan channel, with the new
  proposals and the ids this pass retired), `answers` (chat RSVPs for the
  reaction path), `redirect` (a link-first self-service link that replaced
  the card: change, author, `SelfServiceTip`) and `backlog_dropped` (audit).
  A card entry carries the staged `ProposedChange` and may carry
  `self_service: Option<SelfServiceTip>` (link, optional lead-in, how the
  lead-in was made, and the weekly tip it `claimed`). `card` and `redirect`
  return `PostResult`: only `NotPosted` (nothing sent and nothing kept for a
  later pass) gives every tip the post carried back
  (`ModelLogStore::release_tip`); `Posted` (including a send whose outcome
  is unknown) and `Pending` (a card saved for the stranded repost, link
  included) keep them spent. A refusal's logged `message` never carries
  store text: a store failure logs "the change could not be staged".
- **`SelfServiceDeps`** (optional in `Deps`; absent means cards only):
  `PortalLinks`, a `Nudger<SharedRewriter>` (production:
  `GovernedRewriter`) and `Personas` (the member's resolved persona).
- **`rescan::History`**: `backfill(channel, since)` from Discord.

## Live bursts

- **Loop guard.** Bot, webhook and the bot's own messages are dropped
  before anything else and never cached. Members' messages in watched
  channels are cached (`ModelLogStore::upsert_message`) whatever happens
  next, as in v4. Messages the chatbot handled, from members without the
  bossing role, or sent while extraction is off are cached but never read.
- **Debounce.** A message the keyword gate hits joins its channel's burst
  and pushes the flush `debounce` (default 90 s, v4
  `EXTRACT_DEBOUNCE_SECONDS`) out. An **edit** with new content clears the
  message's `processed_at` (store), rejoins the burst and pushes the flush
  out again; it never flushes at once. A repeated delivery with unchanged
  content does nothing. A **ping** (`@here`/mention) with a boss or a time
  flushes the channel's burst at once (v4 `urgent`). A delete leaves the
  cache, the burst and the backlog.
- **Flush.** The burst's cached, unprocessed, still-gated messages are read.
  A burst of bare answers in a channel with no strong scheduling message in
  the last 6 h is marked processed with no call (v4 `should_extract`).
  Otherwise the burst is cut to the prompt budget (`split_until`; the
  runner's schema instruction is always budgeted), and each piece is one
  model call. Flushes run as tasks, so events keep flowing during a call;
  the governor serialises model traffic.
- **Shutdown.** Closing the event channel drops buffered bursts (their
  messages stay unprocessed in the cache) and waits for flushes in flight.

## The governed call

Every call opens `ModelClient::open_extraction` (extraction priority,
`permit_wait` 120 s queueing, `call_timeout` 120 s), an identity session via
`identity::open_session` (Passthrough by default; an `external` route
fails closed), and runs the reference loop: `complete`, then on
`Next::Retry` one `answer_retry` in the same session, each request from
`extraction_request` over `ExtractionAttempts::messages()`. There is no
direct provider call. The answer goes through `plan_burst` with the
injected wall clock as `now`.

## Proposals

- **Chat answers** (`rsvp` yes/no on a matched run) go to
  `Outbox::answers`, which applies them through the reaction path (v4
  `_apply_rsvp`); `maybe` is dropped. They are not proposals.
- **Self-service redirect.** Every other kept change is planned with
  `redirect::plan` under `PipelineConfig::self_service.effective_mode()`,
  which is `cards_only` while `public_portal_open` is false (the default),
  so nothing changes before the public launch. Only a change with exactly
  one author is planned; changes sharing an evidence message count as a
  multi-change message. A link-first self-service move goes to
  `Outbox::redirect` with no proposal; otherwise the proposal and card
  proceed and the link rides on the card entry. The author's weekly tip
  (`Nudger::tip`, boss week from the configured reset and the injected
  clock) is claimed only once the link is certain: after the redirect is
  decided, or after the proposal succeeded; cards-only plans never claim
  it. The lead-in is labelled (`LineSource::as_str`) in the call's log
  `guardrail` as `{"nudges": [...]}`; the model's text is never logged.
- **Rewrite.** `GovernedRewriter` guards the `rewrite` route with the
  identity codec (an external route under passthrough is refused), opens
  `ModelClient::open_rewrite` and sends one plain request. Content filter,
  cut-off or empty replies are `RewriteFailure::Refused`, misconfiguration
  (`SessionError::is_misconfiguration`, a missing or refused route) is
  `Misconfigured`, anything else `Unavailable`; every case uses the seed.
- **Supersede (v4 `_record`).** Before anything is proposed, each target
  (a run: `from_channel` = this channel, or a new boss set in this channel)
  is passed to `supersede_proposals`. Then each change is proposed with
  source `extraction` and `source_id` = its call's log id: the first change
  per target with `Supersede::Older` (its key is stored), any sibling with
  `Keep`, so a `sub` proposed beside a `move` for the same run never
  retires it. A target whose every new change is refused has still had its
  older proposals retired, as v4 retired them before writing. A link-first
  redirected move's run is a target too: its link replaces today's card, so
  older cards for that run retire (the `Card` then has no entries, only
  `superseded`).
- **Several calls** (a split burst, a rescan) are consolidated first
  (`plan::consolidate`, latest word per target); each kept change stays
  attributed to the call it came from.
- **Up-front refusals** (`D-PROPOSE-REFUSES`: `Refused`, `NoEffect`,
  `Expired`, or a store failure) create nothing and are logged in the call's
  structured `refusals` (`[{change, code, message}]`: the change kind, a
  stable snake_case code from `pipeline::refusal_code`, v4's words); `error`
  is only for failures. Migration 0011 adds `extractions.refusals`; the
  admin API's `Extraction` schema does not expose it yet (follow-up for the
  API lane).
- Answered calls mark their messages processed (`ModelLogStore::mark_read`),
  each only if its content is still what the call read: a message edited
  while the call was in flight stays unprocessed and its pending burst reads
  the new text. Failed and turned-away calls leave them for a later read.

## Extraction log

Exactly one `ExtractionLog` row per model call (a burst that fits one
prompt is one row), written after proposing: model alias (the session's,
else the route's), `reasoning` (configured effort), prompt text, raw
response (or the error when there is none), latency (tokio time), request
count (`Session::requests_used`, the answer retry included), message ids,
authors, proposal ids. A store failure while loading the prompt's
schedule or channel history is logged as a fixed sentence (`the schedule
could not be read`, `the channel history could not be read`); the store's
own text (it can carry paths) goes only to the server log as
`extraction_store_failed`. `outcome`:

| outcome | when |
|---|---|
| `turned_away` | governor refusal that clears by waiting (breaker open, queue wait, rate ceiling), gateway admission refusal, or backend unavailable: nothing ran upstream; the messages are read again later |
| `content_blocked` | the provider's content filter stopped the answer (`ContentFiltered`); no answer retry, never requeued |
| `failed` | any other failure, including permanent governor refusals (unknown role, ungrouped alias, forbidden route, may-not-wait, retry budget exhausted: never requeued), or a reply still invalid after the answer retry |
| `proposed` | at least one proposal was created |
| `self_service_link` | none created, but a link-first redirect took at least one change |
| `no_change` | answered with nothing to propose (dropped, refused, or chat answers only) |

## Backlog

Late messages (`MessageOrigin::Replay`) and the messages of turned-away
bursts wait in the backlog. Live serve never offers `Replay` messages
(parent decision: stale history makes no card unless rescanned); it caches
them only, so in serve the backlog holds turned-away bursts. The backlog: deduplicated
by message id (a replay of an already-cached, unchanged message is not
queued at all), bounded by `backlog_capacity` (default 1000); past it the
oldest are dropped and reported through `Outbox::backlog_dropped`. One
burst leaves per `drain_interval` (default 15 s): the oldest entry's
channel, its queued messages in time order up to the first 3 h gap and at
most 12. A turned-away drain goes back to the front and the next drain
waits for `max(drain_interval, retry_at)` (the breaker's probe time or the
rate wait), so an open breaker is waited out, not spun on. A turned-away
live burst joins the backlog and delays the next drain the same way; a
drain finishing meanwhile keeps the later hold. A message belongs to either
a pending live burst or the backlog, never both: live activity (post or
edit) takes it out of the backlog, a replay of a message in a pending burst
stays with the burst, and a turned-away message edited meanwhile is left to
its new burst.

## Rescan jobs

- **Submit** returns at once with a `queued` row in `rescan_jobs`. A
  request whose channels a queued or running job covers attaches to it; a
  queued job with another window is cancelled (`error` `replaced by a newer
  request`: request fields never change in the store) and a new job is
  queued. `NoChannels`, an unknown window and `Closed` are refused.
  Submits (and `close`) are serialised, so concurrent identical requests
  queue one job; a queued job the worker starts meanwhile is attached to.
- **Worker**: one job at a time, in order; `running` with `started_at`,
  then channels one after another, `results` written after each channel,
  and a final `done` | `failed` (every channel failed) | `cancelled`.
  Before it starts, `Rescans::recover` ends jobs a previous process left
  `queued` or `running` (a crash, an abort) as `cancelled` / `interrupted`.
- **Cancel**: a queued job is cancelled at once; a running one stops before
  its next burst or channel (a call in flight finishes; what was read is
  still proposed and logged). `close()` cancels queued jobs (`shut down`)
  and stops the running one. The final status is latched under the lock
  `cancel` reads: a stop accepted while the job ran always ends
  `cancelled`, and a job already final refuses it (`cancel` is `false`).
- **Windows** (`docs/v5/admin-api.md`): `week` = v4 `week` (current boss
  week; an empty week widens once to the week before), `since_reset` = the
  same start without widening, `two_weeks` = v4 `2weeks`; v4 spellings
  `2weeks`/`48h`/`24h` also work; automated jobs are capped at `48h` and
  never widen. The stored `window` is the requested spelling.
- **Per channel** (v4 `rescan_window`): backfill through `History` (new or
  changed messages are cached), read every cached role-holder message the
  gate hits since the window start — processed or not, unless the request
  says `unprocessed_only` (the startup rescan: a restart never re-reads,
  re-proposes or reposts what a pass already read) — cut into
  conversations (`group_for_rescan`), one call per conversation spaced by
  `drain_interval`; only the turned-away pieces of a conversation are read
  again, at most 3 attempts in all, each after the governor's wait; what is
  still turned away is counted in `unread` with an error, never reported as
  read. Then one consolidated proposal pass and one card.
  Re-reading handled messages does not duplicate proposals: a change
  already applied is dropped as `already scheduled`, and one still pending
  replaces the older proposal (v4 supersede).
- **Result** per channel (JSON in `results`): `channel_id`, `name`,
  `window`, `since`, `widened`, `backfilled`, `stored`, `gated`, `bursts`,
  `calls`, `extracted`, `proposals`, `refused`, `dropped`, `stale`,
  `cancelled`, `unread`, `errors`. `errors` hold failed calls and what
  could not be written; turned-away calls are not failures (read again, or
  counted in `unread`). A thread whose history cannot be read is skipped
  (`thread_history_skipped` {thread_id, kind}) and named in `errors`.

## Startup check

`check_reasoning_effort(alias, effort, capabilities)` refuses an extraction
effort the model does not publish (the runner would refuse every call,
`D-SHAPING`). The config slice must call it at startup and on every
extraction alias or effort change (`TODO(config slice)` in
`pipeline/config.rs`).

## Differences from v4

- Proposals are refused up front instead of carded and refused at ✅
  (`D-PROPOSE-REFUSES`); the log row is written once, after proposing
  (v4 logged, then attached amendment ids).
- Edits re-debounce (v4 ignored edits); late messages go through the
  backlog at a fixed rate; rescans are paced by the same interval.
- A turned-away burst is kept for a later read instead of being lost until
  the next rescan.
- A rescan dry run (`post=False`, `/debug extract`) is not part of this
  slice.
