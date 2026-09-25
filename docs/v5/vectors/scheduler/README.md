# Scheduler vectors

This producer-side slice records stateful v4 scheduler behavior through a real
in-memory `Repo`: materialisation, matching one-off adoption, fixed-run edit and
retirement, reminder rows, and RSVP reactions. Run from `legacy/python/`:

```sh
uv run python -m scripts.v5_vectors.scheduler
uv run python -m scripts.v5_vectors.scheduler --check
```

Each case serializes its aware clock, timezone/reset configuration, deterministic
UUID source sequence, owner/member/channel IDs, and operations. The generator
JSON-round-trips documents before invoking the replayer; the replayer accepts one
case, validates its input before opening a store, patches the `db.new_id`,
`db.utcnow`, and imported `materialise.utcnow` seams only for that invocation,
and opens one clean in-memory store. Tests JSON-round-trip cases and replay each
twice to prove clean-store byte-identical output.
IDs are retained as meaningful state rather than normalized away.

Snapshots preserve ordered fixed runs, runs, reminder rows, RSVPs, and recorded
side effects (none in this persistence-only slice). They intentionally omit
SQLite `created_at` bookkeeping only; it does retain RSVP `source` and aware
`at`, because they are portable scheduler facts. Reminder rows are included
because they are a materialisation consequence. Delivery, mention policy,
digest, and Discord transport effects are separate future slices and are not
faked here.

The checked-in Draft 2020-12 schema discriminates every operation, constrains
status/source/state domains, types final rows, and rejects unknown keys. Its
`replayCase` definition requires only `case_id` and `input`, while `case` adds
the completed `expected` result. Runtime replay applies that same input schema
with `Draft202012Validator` and a `FormatChecker` before opening its store;
`validation.validate_document` applies the complete document schema before its
semantic gate. Static validation accepts a materialized key only after its
fixed timing has been declared and materialization requested; replay then
accepts it only if the v4 oracle actually produced that run. It checks
reference ordering, exact fixed-edit field sets, expected-step cardinality,
and operation/result pairing that JSON Schema cannot express across arrays.
Each step has exactly one success value or the declared exact error type;
expected outputs come only from replaying the v4 oracle.

## Reminder and mutation families

`index.json` lists every family with its own schema: `reminders.json`
(`reminders.schema.json`) and `mutations.json` (`mutations.schema.json`) sit
beside the original `scheduler.json`, and the same generator/`--check`
command writes all three. Their producers live in
`scripts/v5_vectors/scheduler/{reminders,mutations}/`, sharing `oracle.py`
(clock/UUID seams, exact-error capture, snapshot) and `contract.py`
(schema-first gate).

The design matches `scheduler.json`: JSON-round-tripped inputs, a schema and
static-reference gate before any `Repo` opens, one clean in-memory store per
replay, a pinned aware clock (`set_clock` moves it explicitly), a deterministic
UUID sequence whose exhaustion fails the contract even where v4 swallows the
error (audit writes), and only the step's declared exact error class captured.
Every argument a v4 call receives is in the step (`ping_time`, `countdowns`,
`rebuild`, `announce`, `mark`) or the case input; `now` is always the pinned
clock. Each operation's success value is typed by `$defs/result_<op>` and the
document gate pairs it with its operation.

Snapshots add reminder `message_id` and order reminders by run, then
`(fire_at, kind)`. Instants are UTC as stored; `reminder_specs` results keep
the offsets v4 returns (guild zone for `day_of`, the input's offset for
countdowns).

**reminders** — `reminder_specs` status policy (otot keeps only `day_of`,
cancelled/done nothing, countdowns deduplicated and descending, `countdown_0`,
late-night runs and a run exactly at ping time pinged the previous morning),
`ensure_reminders` writing past and exactly-now slots as sent, pruning unsent
kinds without rebuild, rebuild deleting sent/posted rows, duplicate
`add_reminder` returning `null`, `reconcile_day_of` (reopen skipped, move
queued, retire into the past, keep posted, skip cancelled, idempotent),
`reschedule_unposted_reminder`, `mark_done` at exactly 2 h versus +1 s, and
`is_stale` at exactly 12 h / 30 min versus +1 s (strict `>`; unknown kinds use
the countdown grace). DST cases in `America/New_York` cover ping times and
fixed slots in the spring-forward gap and fall-back overlap.

**Recorded DST quirk (escalated, not fixed).** `reminder_specs` builds the
day-of instant with `datetime.combine(...).replace(tzinfo=tz)` and compares
wall clocks. On 2026-03-08 a 02:30 ping time does not exist; zoneinfo resolves
it with the pre-transition offset (07:30Z = 03:30 EDT). A run at 03:00 EDT
(07:00Z) therefore gets its `day_of` 30 minutes *after* it starts, and a
02:30 fixed slot gets it exactly at the start. In the fall-back overlap a
01:45 ping (first occurrence) before a 01:30 second-occurrence run is moved to
the previous day. The vectors freeze v4 as it behaves.

**mutations** — real `bot.api.service.set_status`, `amend_run`,
`swap_participants`, and `update_fixed` against a duck-typed host
(`mutations/host.py`) with a synthetic catalog, seeded members and ping
levels, and the real `BossBot.materialise_weeks`. Only Discord transport is
stood in for: any channel ID resolves, and `post_plain` records `channel_id`,
the resolved `mentions`, `effect_kind`, and `effect_context` as
`side_effects`. Notice text, the `(via portal)` suffix, audit rows, delivery
journal and channel fallback are out of scope. Step values keep only the
portable scheduler facts of `run_view`/`fixed_view`; portrait, label and
card-rendering fields are dropped. Covered: restore from
otot/cancelled/done clearing participants' RSVPs (non-participants keep
theirs), no-op status posting nothing, `announce=false`, portal versus
command marking, `at_risk`/invented status refusals, amend resetting
confirmed/at_risk to planned (otot stays), occupied-week `run_move_conflict`,
unreadable dates, swap refusals and status recompute, and fixed edits of
bosses/participants/channel/note/day+time that skip cancelled runs and re-snap
an amended run only when the slot changes.

## Dispatch, mention and digest families

`dispatch.json`, `mentions.json` and `digest.json` (each with its own
`*.schema.json`, all listed in `index.json`) follow the same discipline and are
written by the same generator/`--check`. Producers live in
`scripts/v5_vectors/scheduler/{dispatch,mentions,digest}/`.

Dispatch and digest bind the real `BossBot` methods (`dispatch_reminders`,
`_send_day_of`, `_send_countdown`, `day_of_card_for`, `countdown_card_for`,
`find_channel`, `post_week_digest`, `_post_digest`, `materialise_weeks`, …) to
a host (`discord_host.py`, replay loop in `host_replay.py`). Only Discord is
stood in for: `get_channel` yields a stub text channel for
`available_channel_ids`, `fetch_channel` raises `NotFound` otherwise, sends
succeed with a deterministic message-ID counter (from `700000000000000001`) or
raise a scripted timeout (`set_transport`). Sends go through the real
`effects.send_card` and delivery journal on the in-memory `Repo`.
`delivery.py` records each journalled `SendPlan` as an **intent** in
`side_effects`: `effect_kind`, destination `channel_id`, `dedupe_scope`,
targets in plan order (day-of targets are time-sorted there), the mention
allow-list (`mentions`, `role_mentions`, `mention_everyone`) and the journal
`outcome` (`bound`, `suppressed`, or the exception it raised). Card text is not
recorded. The journal and maintenance coordinator's own `datetime.now`/`uuid4`
aliases are patched too, so bound `sent_at`/`posted_at` equal the pinned
clock; their internal attempt/operation IDs never appear in outputs. v4's
warning logs are muted during replay since intent outcomes record their
effect.

**dispatch** — suppression of cancelled runs, stale rows (day-of 12 h and
countdown 30 min, each at the boundary and +1 s) and unknown kinds, all
retired as sent without a message; day-of grouped per home channel into one
intent whose targets are time-sorted and whose mentions come from
`everyone_on`; countdowns to the party minus decliners; a second dispatch
sending nothing; `mark_done` before dispatch (v4 tick order) removing a past
run's unsent pings; an ambiguous send recorded once and suppressed thereafter
until the row goes stale; and a missing channel with no `POST_CHANNEL_ID`
leaving rows queued.

**Recorded dispatch quirk (escalated, not fixed).** When a run's home channel
is unavailable, or the run has none, `find_channel` falls back to
`POST_CHANNEL_ID`, but binding validation requires the reminder's run to
belong to the destination channel. The journal refuses the intent
(`raised:DeliveryBindingError`) before any send, the row stays queued, every
tick retries, and once stale it is retired as sent: the ping is silently lost.

**mentions** — `resolve_mentions` for every essential and informational kind
and an unknown kind (treated as informational), `wants_mention` per level,
order preservation and de-duplication (including an integer candidate),
unrostered IDs (pinged by essential posts only), `audience` names/mentions,
countdown candidates via `not_declined`, day-of people via `everyone_on`,
the swap audience (remaining participants followed by those leaving, as
`swap_participants` builds it), and `BossBot._prepared`: an explicit
`mention_users=[]` (what the digest passes) overrides a card's own list, and
quiet mode clears everything. Level normalisation and `set_ping_level`
refusals keep exact `ValueError`/`KeyError` messages. No UUID is consumed.

**digest** — the `test_digest.py` schedule: the first tick on a new database
only records the week; ticks after the reset post exactly once; a host asleep
through the reset posts exactly one digest; a restart that rewrote
`last_materialised_week` still posts; an uncertain send is recorded once and
then suppressed, never retried; no `POST_CHANNEL_ID` still records each week
and setting it later does not back-post; the previous week's digest is retired
but kept; and `retire_weekly_digests_before`. Snapshots add `weekly_digests`
and the `last_digest_week`/`last_materialised_week` config. Each digest intent
carries `digest`: which runs v4's own `digest_card` included, read back as the
short run IDs per day field plus the summary's cleared/live/unsettled/at-risk
counts through a strict pattern (drift fails generation). Cancelled runs are
excluded; at-risk runs count as unsettled and separately.

`BossBot.tick` itself is not replayed (it also backs up to disk and expires
proposals); the dispatch and digest cases compose its steps in v4 order.
