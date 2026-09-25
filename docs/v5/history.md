# Schedule change history (format `kanade.change.v1`)

Status: implemented in `src/domain/history/` (types, encoding, verification,
revert planning, blame, checkpoints), `src/domain/scheduler/service.rs` (attribution, rollbacks)
and both stores (`src/infrastructure/store/{memory,sqlite}`). The admin API
(A5, `admin-api.md`) serves the record list, records, blame, the chain check
and the three rollbacks; cherry-pick, checkpoints by name and the graph are
not exposed yet.

Every schedule mutation appends one immutable, hash-chained record in the
same transaction as its commit. Records are never edited or deleted; a
rollback is a new record that undoes earlier ones, like `git revert`.

## Attribution

A mutation is only reachable through `SchedulerService::as_origin(origin)`,
a handle that attributes exactly one operation, so no attribution can
outlive the call it was given for:

- `Actor`: `member{id}` (Discord user id), `admin{id}` (admin id or operator
  label) or `system{component}` (e.g. `delivery`). Always derived
  server-side, never taken from a client.
- `Surface`: `discord`, `admin_portal`, `public_portal`, `cli`,
  `chat_approval`, `extraction_approval`, `delivery_tick`, `rollback`,
  `import`.
- `request_id`: an optional caller request/idempotency id.

Rollbacks are always `admin{id}` through `rollback`.

## Record fields

| Field | Meaning |
| --- | --- |
| `seq` | Position in the chain; 0 is the genesis record. |
| `id` | Random UUID (`"genesis"` for seq 0). |
| `revision` | The store revision right after this commit. |
| `at` | The operation's single clock reading (genesis: the Unix epoch). |
| `actor` | `{kind, id}`. |
| `surface`, `request_id` | As above. |
| `weeks` | Boss weeks of every run the change touched, or whose RSVPs/reminders it touched. |
| `rows` | `[{key, before, after}]` for every row whose value changed, in key order; `null` = absent. Keys are `{table, id}` or, for RSVPs, `{table, run_id, user_id}`. Row values are the full domain rows. |
| `notices` | Effect kinds of the notices the operation emitted. |
| `refs` | `[{seq, hash}]` of records this change refers to: the records a rollback undid (reusable for cherry-picks/merges). |
| `prev_hash` | The previous record's `hash`; 64 zeros for genesis. |
| `hash` | Lowercase hex SHA-256 of the canonical body. |

The store reads each touched row's before/after value itself inside the
commit transaction, so a partially-scoped snapshot cannot misrecord.
Revisions are store revisions: delivery-journal writes (binding a sent
reminder, retiring a send) advance the revision without a record, so record
revisions can skip numbers.

## Canonical encoding

The body is the record without `hash` as one JSON object: UTF-8, no
insignificant whitespace, object keys in ascending byte order at every
level, strings escaped as `serde_json` writes them (non-ASCII verbatim;
`"`, `\` and control characters escaped, U+007F verbatim), integers in
decimal, instants as UTC ISO text via `domain::time::to_iso`
(`2026-08-29T01:02:03.456789+00:00`, microseconds only when non-zero), and
absent optionals as `null`. Fields: `actor {id, kind}`, `at`, `format`,
`id`, `notices`, `prev_hash`, `refs [{hash, seq}]`, `request_id`,
`revision`, `rows [{after, before, key}]`, `seq`, `surface`, `weeks`.

Instants outside years 1..=9999 cannot be encoded; sealing such a record
fails instead of hashing an empty string.

The encoding depends on `serde_json` maps being sorted (`preserve_order`
off); a unit test pins it. `docs/v5/vectors/history/golden.json` pins two
full records and hashes (non-ASCII, control characters, nulls, sub-second
instants, `refs`), cross-checked with Python's `json.dumps(...,
ensure_ascii=False, separators=(",", ":"), sort_keys=True)`. **Any change
to the encoder's output is a new format version**, never an edit of v1.

## Storage (SQLite)

Migration `0003_change_log.sql`: `change_log` stores the canonical `body`
bytes and `hash`, plus indexed copies of `seq`, `id`, `revision`, `at`,
actor, surface and `request_id`; `change_log_weeks` indexes records by boss
week. Triggers refuse every `UPDATE` and `DELETE` on both tables. The
genesis record is written when the table is empty at open, with the store's
revision at that time. The in-memory store keeps the same records.

## Verification

`ChangeHistory::verify_history` walks the whole chain and reports the first
broken link. For every record:

1. `seq` continues from 0 without gaps;
2. `prev_hash` equals the previous record's hash;
3. the **stored bytes** hash to the stored `hash`;
4. the stored bytes are exactly the canonical encoding of the parsed record;
5. (SQLite) the indexed columns and week index agree with the body.

Loading a single record (`load_change`, used by rollbacks) applies checks 3
to 5 too. Dropping the triggers and editing a column, a body, the week index
or removing a record is detected.

## Anchoring

A chain rewritten or truncated from some point on is internally consistent,
so it is anchored outside the database:

- every backup gets `<backup>.manifest.json` (`format`
  `kanade.backup.v1`, `history_head {seq, hash}`, `revision`,
  `schema_version`) describing the snapshot;
- `ChangeHistory::history_head` exposes the current head for a periodic
  structured log line (wiring pending);
- `SqliteStore::open_with_anchor(config, anchor)` (and
  `ChangeHistory::contains_anchor` on demand) refuses a history that no
  longer contains the anchored record with that hash
  (`SqliteStoreError::AnchorMissing`). Truncation behind the latest anchor
  is detected; changes after it are only as trustworthy as the database.

## Idempotency

A request id is per actor and names one request. Beside it the store keeps a
**request digest** (column `request_digest`, not part of the hashed body):
SHA-256 over the operation name and the Rust `Debug` form of its normalised
arguments (policies and directories, being configuration, are excluded; a
rollback's digest covers its selection, scope and mode). A partial unique
index `(actor_kind, actor_id, request_id) WHERE request_id IS NOT NULL`
backs the rule.

- Before anything is loaded or planned, the service looks the request id up
  (`ScheduleStore::recorded_request`): the same digest returns
  `SchedulerError::AlreadyApplied { seq, revision }`; another digest returns
  `SchedulerError::IdempotencyMismatch { seq }`. Nothing is applied either
  way, so exact retries of strict or forced reverts and of mutations that
  would not re-plan cleanly (a remove-only swap) all answer
  `AlreadyApplied`, also after a restart.
- The commit transaction checks again for races: the same digest returns the
  earlier record as `Committed { replayed: true }` whatever the revision;
  another digest fails with `StoreError::IdempotencyMismatch`.
- Because the digest uses `Debug` forms, a retry across a build that changes
  an argument type is refused as a mismatch rather than applied twice.

## Rollbacks

All rollbacks first honour the request id (above), then, inside the commit
retry loop, load the named records with their stored bytes checked (unknown
or genesis → `UnknownChange`; stored bytes, hash or indexed columns wrong →
`Tampered`, on both stores), re-read the reminders the delivery journal
holds, sort the records newest first, and apply their inverse as one new
record by `admin` through `rollback`, with `refs` listing every undone
record. The rollback emits one summary notice (`notice.rollback.reverted`)
per affected home channel, listing that channel's runs, the reverted seqs
and cancelled runs; runs (or weekly timings) without a home channel share
one notice that the notice planner sends to the post channel. A rollback
that would change nothing returns `RevertOutcome::Unchanged` and writes no
record and no notice.

- `revert_changes(seqs)`: whole records.
- `restore_week_to(week, revision)`: every later change touching the week,
  **week-scoped**: only runs that were in the week before or after a change,
  and their RSVPs, are reverted. Weekly timings and other weeks' runs are
  left as they are and reported as skipped (`outside week`).
- `revert_by_actor(actor, since)`: everything one actor changed since an
  instant (spam cleanup).
- `preview_revert_changes`, `preview_restore_week`,
  `preview_revert_by_actor` (and `preview_checkpoint_restore`) plan the same
  rollback now and write nothing; they take no request id.

`RevertOutcome::Reverted` carries `rows` (every row the rollback changes,
reminders included, in key order: exactly the record's rows once applied)
and `seq` (the rollback record, `None` for a preview). Reminders are
re-planned on every attempt, so a preview's reminder ids differ from the
apply's.

Rules:

- Forward only: history is never rewritten; reverting a revert re-applies.
- Strict mode refuses with a conflict report (record, row, expected `after`,
  found) when any row differs from what the change left, writing nothing;
  force mode puts the recorded `before` back regardless and reports the
  overridden conflicts.
- Reminders are never restored from history. Only runs whose slot or status
  the rollback changed have their reminders reconciled, as
  `ensure_reminders(rebuild = false)`: sent or skipped reminders, reminders
  an unresolved delivery attempt holds, and unsent reminders whose kind and
  time still match the slot are kept (a due unsent ping stays due); the rest
  are replaced from the reminder policy. Nothing already sent is sent again.
- Runs are never deleted: reverting a run's creation cancels it, and the
  cancelled run keeps its weekly timing's slot for that boss week.

## Blame

`domain::history::blame(store, target)` answers, for a run or a weekly
timing, which change last set each field:

- runs: `slot` (instant, boss week, source and weekly-timing link),
  `bosses`, `participants`, `channel`, `status`, and one `rsvp:<user>` per
  member with a recorded or current answer; v5 adds `status_pin` (while
  held or once set) and one `attended:<user>` per recorded entry;
- weekly timings: `day`, `time`, `bosses`, `participants`, `channel`,
  `note`, `owner`; v5 adds `attendance_default` (once set) and one
  `standing:<user>` per held or recorded standing answer (see *Attendance
  fields*).

Each line carries the record's `seq`, actor, surface and instant, and `via`:
`Direct`; `Rollback { undid, restored_to }` for a rollback, where `undid`
lists the records it undid and `restored_to` is the checkpoint head a
checkpoint restore went back to (never itself "undone"; a restore record is
recognised by its `notice.rollback.restored` notice kind and carries the
checkpoint head as its last ref); `Override { picked, overridden }` for a
change applied over newer ones (marker notice kind `notice.edit.override`):
an administrator's "apply mine anyway" edit (`picked: None`, `overridden`
its refs, see *Edit preconditions*) or a forced cherry-pick (`picked` the
re-applied change, `overridden` the rest of its refs, possibly empty); or
`Referenced(refs)` (another change carrying references, such as a future
cherry-pick). Every line checks that
its attributed record really set that field. A field no record has set (rows that predate the
history, e.g. imported) has no attribution: *unknown (before history)*.
Reminders are not blamed.

Storage: a field index (`change_fields`, migration `0004`: target, field,
`seq`) is written in the same transaction as each record from the
record's rows (`changed_fields`), so a blame is one grouped index lookup
plus one checked record load per distinct change; an on-demand scan would
read the whole history for every run sheet. The index is append-only
(triggers) and derived, and history verification recomputes it for every
record and reports a mismatch as a broken link. Migration `0004` backfills
the index from every record already stored, in the same transaction.

## Checkpoints (tags)

A checkpoint names the history head at a moment: `name`, `kind`
(`auto` | `admin`), `head {seq, hash}`, the head record's `revision`, the
boss `week` it belongs to, `created_at` and `created_by`.

- **Automatic**: the delivery tick, after materialising a boss week, creates
  `week <YYYY-MM-DD> start` (the guild-local reset date) at the head, by
  `system{delivery}`; one per week (a partial unique index), so later ticks
  and restarts return the existing one, as does an automatic name that
  already exists (the same local week under another reset instant). Its head
  is the head when it is written, so it may include writes committed between
  materialising and the checkpoint; a process started mid-week creates its
  week's checkpoint then, mid-week. A checkpoint failure never aborts the
  tick: it raises the `CheckpointFailed` admin alert and the next tick
  retries.
- **Admin**: `SchedulerService::create_checkpoint(admin, name, at, policy)`,
  for the boss week containing `at` (normalised to the week's start). Names
  are 1..=100 characters without outer spaces or control characters, unique
  across all checkpoints, and may not take the automatic `week … start`
  form.
- **Immutable**: never renamed, moved or deleted (triggers refuse `UPDATE`
  and `DELETE`). Retiring was rejected: a checkpoint is only a pointer into
  an immutable chain, so hiding old ones is a UI filter, and a mutable
  retirement flag would add state without protecting anything.
- Verification checks every checkpoint's hash and revision against its
  record; restores re-check both and refuse a mismatch as `Tampered`.
- Each checkpoint is also an external-anchor candidate (`head`).

Restoring: `preview_checkpoint_restore(name, mode)` plans the restore of the
checkpoint's boss week to it and returns the outcome (reverted records,
conflicts, skipped rows, notices) without writing anything;
`restore_to_checkpoint(admin, request_id, name, mode)` applies it through
the rollback path (`restore_week_to` at the checkpoint's revision, week
scoped), and the rollback record's `refs` also include the checkpoint's
head (last) and its notices are `notice.rollback.restored`, naming the
checkpoint. When nothing after the checkpoint touched its week, both preview
and restore return `RevertOutcome::Unchanged` with no records (consistent
with a no-op rollback) rather than an error. An unknown name is
`UnknownCheckpoint`; a checkpoint whose head no longer matches the history
is `Tampered`.

Read APIs: `Checkpoints::list_checkpoints(week)` (creation order, all or one
boss week), `Checkpoints::load_checkpoint(name)`, and `blame` for a run or
weekly timing. `ChangeHistory::list_changes` pages by `ChangeFilter` (`All`,
`Week`, `Actor`, `ActorInWeek`, `Revisions`) and `count_changes` counts the
same filter without genesis (one constant statement per filter on SQLite).

## Attendance fields

The attendance model (`attendance.md`) adds blamed fields on a weekly
timing: `standing:<user>` (a member's "always in"; blamed while held or
once recorded) and `attendance_default` (listed once it has been set). They
are part of the timing's row, so they follow the same history, blame,
precondition and `read_versioned` rules as every other field. The record
encodes them only when set (not `opt_in`, not empty), so v4-shaped rows and
the golden vector are unchanged. Recorded attendance is on the run the same
way: `Run.attendance` (key `attendance` in the run's row, only when
non-empty) and blame field `attended:<user>` on the run (listed while
recorded or once changed), accepted as a precondition; `read_versioned`
returns the run row with its entries. The value of a stale `attended:<user>`
is the run row. Entries follow the run: a participant change (including a
swap on a done run) drops the leavers' entries and reviving a done run
clears them, in the same commit, so blame shows the drop. Changes made by
the delivery tick (`mark_done`, the attendance recount) are derived
statuses and cannot be cherry-picked (`UnsupportedPick`).

A v5 hand-set status is pinned on the run (`Run.status_pin`, key
`status_pin` in the run's row only when set, so v4-shaped rows and the
golden vector are unchanged) and blamed as the run field `status_pin`
(listed while held or once set; its attribution names who pinned or
ended it), accepted as a precondition; its stale value is the run row.

## Role checks belong to the API

The scheduler service records WHO acted (the `Origin` actor) but, apart
from the few checks it states itself, it does not authorise. The
service-level checks are:

- overrides and request decisions are administrator-only; requesters
  withdraw only their own requests;
- `set_standing_answer`: a member sets or clears only their own (an
  administrator anyone's);
- `set_attendance_default`: administrators only;
- `record_attendance`: a member changes only their own entry; a system
  actor is refused;
- `member_attendance`: readable by an administrator or the member
  themselves only.

All of these answer `SchedulerError::Forbidden`. EVERY attributed
operation, including `change_fixed_party`, the draft methods and
`cherry_pick`, still relies on the API layer to check the caller's role
and object-level permissions before calling it.

## Edit preconditions

Direct edits from a screen must not silently overwrite a change made since
that screen was read (a revision race alone re-plans on the current state).
Code: `domain::history::precondition`, `Attributed::expecting`, both stores'
commits.

- **Per-field version.** A field's version is the `seq` of the last change
  that set it: the blame index (`change_fields`), the same value `blame`
  shows (`last.seq`), or `null` when no change set it (unknown before
  history; also an RSVP never given). Fields are exactly blame's: run
  `slot`, `bosses`, `participants`, `channel`, `status`, `rsvp:<user>`,
  `status_pin`, `attended:<user>`; weekly timing `day`, `time`, `bosses`,
  `participants`, `channel`, `note`, `owner`, `standing:<user>`,
  `attendance_default` (see *Attendance fields*).
- **Reading versions.** A screen takes each row and its versions from ONE
  read: `BlameIndex::read_versioned(targets)` returns every target's row (a
  run's RSVP rows too) with its field versions, all in one read
  transaction. Never read the value and then the versions separately: a
  commit landing between the two would pair the old value with the new
  version, and the next edit would overwrite that commit silently.
- **Declaring.** `service.as_origin(origin).expecting(Expect { fields,
  overrides }).<op>(..)` works for every attributed direct mutation.
  `fields` are `Precondition { target, field, seen }` for the fields the
  screen showed and the edit changes. With no expectations nothing changes:
  same behaviour, byte-identical request digests. Draft operations carry
  none; the three-way merge covers them.
- **Check.** The store compares every declared `(target, field)` with the
  index inside the commit transaction (SQLite `BEGIN IMMEDIATE`; memory
  under its lock), after the request-id replay check and before the revision
  check, so no commit can slip between the check and the write. A field is
  stale when its last change is not `seen`, or when its target row no longer
  exists (a retired timing). Only declared pairs are checked: side effects
  (status recompute, reminders) and other fields of the same row are not.
  So a different field changed since does not conflict and the edit is
  re-planned on the current state (field-level merge); the same
  expectations are checked again on every re-plan after a revision
  conflict. An edit that turns out to change nothing succeeds even when a
  declared field is stale (the result is the same), except that a deleted
  declared target is still `StaleEdit` (empty-commit check) and an
  override is refused (`OverrideUnchanged`). A plan the rules refuse (for
  example the timing is gone) is re-checked with an empty commit, and
  reported as `StaleEdit` when a declared field is stale. A target that
  never existed (no row and no change ever recorded for it) is
  `Precondition(UnknownTarget)`, not a stale edit.
- **Errors.** An unknown or duplicated field is
  `SchedulerError::Precondition(UnknownField | DuplicateField)` (the store
  refuses it too, `StoreError::Precondition`). A stale field is
  `SchedulerError::StaleEdit { conflicts }` and nothing is written; each
  conflict has `target`, `field`, `expected` (the declared `seen`),
  `current` (`seq`, `hash`, actor, instant of the last change, or none),
  `target_deleted`, and `current_value` (the row holding the field now; the
  RSVP row for `rsvp:<user>`).
- **Override ("apply mine anyway").** Administrators only. The caller
  resubmits with `seen` set to each conflict's current `seq` and `overrides`
  set to those changes' `{seq, hash}`. Every override must be the `seen` of
  a declared field (`OverrideNotSeen`) that this edit's change set actually
  changes (`OverrideUnchanged`, checked by the store on the sealed record),
  and a recorded change with that hash, checked by the store
  (`UnknownOverride`). A member or system actor is refused with
  `OverrideNotAdmin`, by the service and again by the store from the
  commit's actor. The change is recorded in format v1 with
  `refs` = the overridden changes and the marker notice kind
  `notice.edit.override`; blame shows
  `via: Override { picked: None, overridden: refs }`.
- **Idempotency.** With expectations, the request digest folds them in,
  sorted canonically (fields and overrides), so a retry declaring them in
  another order is still `AlreadyApplied`; the same request id with other
  (or no) expectations is `IdempotencyMismatch`.
- **JSON API contract** (for `admin-api.md` / the public API):
  - every run and weekly-timing read returns `versions: {field: seq|null}`
    taken from the same `read_versioned` read as the row it describes;
  - every direct write accepts `expect: [{field, seen}]` for its target
    (and `rsvp:<user>` for answers), plus, for admins only,
    `override: [{seq, hash}]`;
  - a stale write answers **409** `stale_edit` with `conflicts: [{target:
    {kind, id}, field, expected, current: {seq, hash, actor, at} | null,
    target_deleted, current_value}]`;
  - the client offers "keep theirs", "apply mine anyway" (admins: resend
    with `seen` = `current.seq` and `override` = the listed `current`s), or
    "save as draft / request";
  - malformed expectations (`unknown_field`, `duplicate_field`,
    `override_not_seen`, `override_unchanged`, `unknown_override`) are
    **422**; `unknown_target` is **404** (or 422 when the target was only
    named inside `expect`); an override by a non-admin is **403**.


## Drafts: operations, replay and merge (engine)

Status: pure engine in `src/domain/drafts/` and `src/domain/history/rewind.rs`.
Storage and the administrator draft service are described in the next
section; there is no UI yet.

- **One mutation path.** `schedule::apply_op(draft, ids, &Op, now)` is the
  single implementation of every attributed mutation; the `Attributed`
  service methods build an `Op` and call it. Request digests are unchanged
  byte for byte (pinned in `scheduler/service.rs` `digest_pins`). Week
  starts travel as `WeekStart` (the UTC instant plus the guild-zone view),
  so a week that cannot be viewed in the zone fails only where a planner
  reads it, as before (`edit_fixed_run` with nothing to push, or for an
  absent timing, still returns 0).
- **Draft operations** (`DraftOp`) mirror the member-facing mutations:
  `add_fixed_run`, `apply_fixed_edit`, `fixed_participants`,
  `retire_fixed_run`, `create_run`, `amend_run`, `set_status`,
  `swap_participants`, `set_rsvp`, `reset_to_fixed`, and four used by
  proposals (see *Proposals*): `set_run_bosses { run, bosses }` (replace a
  run's bosses and rebuild its reminders), `ensure_reminders { run }` (add
  the reminders a run lacks, `rebuild = false`), `recount_run { run }`
  (re-derive its status from its answers) and `revive_run { run }` (a
  cancelled or otot run back to `planned`, answers kept; anything else
  unchanged). They were appended to the `v1`
  codec additively; every earlier encoding is unchanged. These four are
  proposal-only (`DraftOp::is_proposal_only`): stored proposals, the codec
  and replay accept them, but `add_draft_op`, `edit_draft_op` and
  `edit_request` refuse them (`DraftError::ProposalOnlyOp(kind)`).
  `fixed_participants { fixed, add, remove }` is a party DELTA for a weekly
  timing, replayed as `schedule::apply_party_delta` (`Op::FixedParticipants`):
  it applies `remove` then `add` (no duplicates, order kept) to the timing's
  party as it is then, and applies the SAME delta to each live run of the
  materialised weeks. So a one-off substitution on a run, and that
  substitute's RSVPs, survive. Removed members' RSVPs on those runs are
  dropped (the swap rule); nobody else's are. A run the delta would leave
  with nobody is skipped, left unchanged and reported (the merge outcome's
  `warnings`: `PartyDeltaSkipped { run_id, reason: Emptied }`, for
  administrators; the approval still succeeds). A request's preview lists
  the same `warnings` in advance, and the `merged` event's detail records
  them after the seq (`… skipped=<run>,…`), so a retry after a lost
  response can still show them. Missing weeks are then
  materialised. Administrators can apply the same delta directly with
  `change_fixed_party`. A normal admin party edit (`apply_fixed_edit`) still pushes
  the timing's whole party onto its runs. The three-way merge sees the
  effect as a set delta, so two drafts adding different members both
  apply. It was
  added to the unreleased `v1` codec in place (keys `add`, `fixed`,
  `remove`), with a golden sample. Rows are named `Target::Existing(id)` or
  `Target::Created(ord)` (the row created by the draft's operation `ord`).
  Policies and the member directory are supplied at replay, never stored.
  A retirement stores its boss weeks explicitly (see `StaleWeeks`).
- **Codec** `kanade.draft_op.v1`: one JSON object per operation, keys
  sorted, instants as UTC ISO text, weekly times `HH:MM:SS`, weekdays
  0 = Monday, enums by their stored spelling, absent optionals `null`,
  plus `format` and `op`. Pinned by `docs/v5/vectors/drafts/draft_ops.json`;
  any encoding change is a new format version. Decoding is strict: each
  object must hold exactly its keys (optionals as `null`), times exactly
  `HH:MM:SS`, instants exactly the encoder's spelling, targets exactly one
  of `existing`/`created`. Key order relies on `serde_json` without
  `preserve_order` (guarded by the history encoder's test and the vector).
- **Replay** applies the operations in order to a snapshot with
  deterministic preview ids (`preview-<n>`), writing nothing and listing
  notices. The first failing operation is `Rejected { ord, error }`. A
  staged `create_run` naming a weekly timing that does not exist is
  rejected (`UnknownFixedRun`); the plain service mutation keeps v4's
  unchecked insert.
- **Rewind**: `history::rewind(current, base, head, records)` restores
  every row the records touched to the `before` of the earliest one;
  reminder rows can differ from the past (journal writes are not records).
  It first checks the records (`check_records_after`) and refuses a
  `HistoryGap`: they must be consecutive from `base.seq + 1`, the first
  linking to `base.hash` and each to its predecessor, each hash matching
  its content, and reach `head` (`Missing`, `BrokenLink`, `Tampered`,
  `HeadMismatch`).
  - `current` must be a whole-schedule snapshot (`Scope::All`).
  - `head` must be read after `current` was loaded (or in the same read
    transaction), so every record `current` reflects is included; the store
    revision cannot show this, because journal writes bump it without a
    record.
  - Records committed after `current` was loaded (after or beyond `head`)
    are harmless: a row only they touched goes back to its value at the
    load, which `current` already holds.
  - List records by following every page's `next_cursor`.

Three-way merge (`analyze_merge(base, current, ops, …)`, or
`analyze_merge_since(current, base, head, records_after_base, …)`, which
fails with `HistoryGap`): D = replay on the
base, R = replay on the current schedule. The draft's changes (base→D) and
upstream's (base→current) are compared per field. Runs: slot, bosses,
participants, channel, status, weekly timing. Weekly timings: day+time,
bosses, participants, channel, note, owner. RSVPs: state and source
together. Reminders, a run's `source`/`week_start` and RSVP timestamps are
not compared. Rows the draft created are compared as `created:<ord>.<n>`
in both replays.

- A field only one side changed, or both changed alike, takes that value;
  participants merge as sets (removals from either side, additions from
  both).
- `BothChanged`: different values on both sides.
- `UpstreamRemoved`: upstream deleted a row, retired a weekly timing, or
  cancelled/finished a run (or the run of an RSVP) that the draft changes
  differently (`Deleted`, `Retired`, `Cancelled`, `Done`). Also `Retired`
  for the timing when a drafted run's weekly timing is in the base but
  retired upstream, and `LeftRun` for a drafted answer whose member is on
  the run in the draft but not on the merged run.
- `OpRejected { ord, on_base }`: an operation fails on the current schedule
  (or on its own base). Operations replay as written, so a swap removing
  someone upstream already removed is rejected, not reinterpreted.
- `ChoicesStale`: a weekly-timing edit's per-run choices were made for a
  different set of amended runs than the current schedule lists; reported
  even when the edit is then rejected for a missing choice.
- `StaleWeeks { ord, staged, current }`: a retirement's staged boss weeks
  differ from `policy.materialised_weeks(now)` at merge (a reset happened,
  so a newly materialised week's run would stay live). The admin re-stages
  the operation; weeks are never recomputed at replay (that would leave a
  permanent `Divergent`, and drafts have no force).
- `Divergent`: R holds a field value the three-way merge does not expect
  (for example a retirement cancelling a run upstream created since).

The analysis returns the conflicts, R's row changes against the current
schedule (preview ids), the notices R would post, R's snapshot and the boss
weeks it changes. It is clean when there are no conflicts and R replayed.

## Draft storage and service (administrator drafts)

Status: implemented in `src/domain/drafts/port.rs` (`DraftStore`), both
stores (`src/infrastructure/store/{memory,sqlite}`, migration `0005`),
`src/domain/scheduler/drafts.rs` (the draft service on `SchedulerService`)
and the delivery tick (expiry). Requests are S3, cherry-pick S4.

- **Surfaces.** `Surface::{DraftMerge, RequestMerge, CherryPick}`
  (`draft_merge`, `request_merge`, `cherry_pick`) are in `Surface::ALL`;
  the unreleased `0003` `change_log.surface` CHECK was edited in place
  (parent decision: no store with real data exists). The v1 record format
  is unchanged.
- **Migration `0005`.** `drafts` (id, kind `admin|request`, title,
  author, `base_seq`/`base_hash`/`base_revision`, version, status, nullable
  `request_type`/`subject` for S3, `merged_seq` → `change_log.seq`,
  closed-by/reason, created/updated instants, nullable `expires_week`),
  `draft_ops` (`(draft_id, ord)` key, `kanade.draft_op.v1` JSON, author,
  instant), append-only `draft_events` (triggers refuse UPDATE/DELETE, as
  `0004`), and `draft_requests` (create idempotency, unique per author and
  request id). Status values: open, submitted, merged, discarded, rejected,
  withdrawn, expired. CHECKs: `status = 'merged'` exactly when
  `merged_seq` is set, and a closed status (merged, discarded, rejected,
  withdrawn, expired) exactly when `closed_by_kind`/`closed_by_id` are set
  (the merger, the closing admin, or the system `delivery` actor for
  expiry). The memory store refuses the same states (`Close` accepts only
  discarded/rejected/withdrawn/expired; only `commit_merge` sets merged);
  the draft conformance suite covers both stores.
- **`DraftStore`.** `snapshot_with_head` reads the whole schedule and the
  history head in one read transaction, so the head covers every record the
  snapshot reflects; `records_after(base)` pages every later record
  (callers refuse gaps through `check_records_after`, as `rewind` does; a
  stored row that no longer parses or matches its hash is reported as
  `HistoryGap::Tampered { seq }`, not a backend error).
  `commit_merge(expected_revision, changes, meta, draft_id,
  expected_version)` is one write transaction: the merger's request-id
  replay check, then the draft (live at the version, else `Stale`), then
  the store revision (`Conflict`), then the rows, the record with its blame
  index, the draft's `merged` status with `merged_seq`, and its `merged`
  event. An empty change set is refused.
- **Draft service** (methods on `SchedulerService`; administrator drafts
  only — every call takes an admin id, so members cannot reach merges):
  `create_draft` (title 1..=200, idempotent per request id; the caller
  supplies no scope),
  `add_draft_op` / `edit_draft_op` / `remove_draft_op`, `preview_draft`,
  `rebase_draft`, `discard_draft`, `merge_draft`, `expire_due_drafts`
  (returns `DraftExpiry { ids, notices }`; see *Member requests*).
  Every one of them refuses a `DraftKind::Request` draft with
  `DraftError::RequestDraft`; member requests merge through the request
  path (S3).
  - Staging validates each operation against the draft's base with the same
    checks the merge runs (`check_staged`: preview-id targets refused, run
    timings checked, a `create_run` whose `week_start` is not
    `policy.week_of(datetime)` refused as `ReplayError::WeekMismatch`, the
    service's own roster/channel/choice checks).
  - Removing an operation renumbers later `Target::Created` ordinals down
    by one; an edit or removal that would leave a later operation pointing
    at an operation that creates nothing is refused (`EditRefused`).
    There is no reorder operation.
  - `preview_draft` rewinds with `check_records_after` and analyses the
    merge; nothing is written. `rebase_draft` re-points the base at the
    head and bumps the version, refused while conflicts remain.
  - `merge_draft` replays with real ids on the current snapshot and
    re-checks field-by-field against the preview replay
    (`replay_equivalent`; created rows compare by `created:<ord>.<n>`, so
    an id-draw ordering tie cannot change the result). A mismatch is a
    conflict and blocks the merge. It commits one `DraftMerge` record
    (`request_id` `merge:<id>@v<version>`, `refs` the base head) with the
    draft close in one transaction, retrying up to `COMMIT_ATTEMPTS` on a
    revision race (re-analysed each attempt). A second merger reports
    `AlreadyMerged{seq}`; an exact retry reports `AlreadyApplied`. A merge
    with no staged operations is refused (`Empty`); a replay that changes
    no rows is `DraftError::NoEffect` and writes nothing (the store's own
    empty-change-set refusal is never reached).
  - Notices: one `Merged{draft}` summary notice per affected channel. A run
    is attributed to its home channel when its row or any of its RSVPs
    changed (an RSVP-only change names the run's channel); a run whose
    channel changed is named in both the old and the new channel's summary;
    a run the merge created has only its new channel; changed or retired
    weekly timings are attributed to their channel. A channel-less summary
    is emitted only when nothing changed has a channel.
  - Routine materialisation stays quiet. Replaying a timing edit
    materialises every timing's due weeks, and the merge commits those runs
    as a normal write would. A run of a timing the merge leaves unchanged is
    left out of the attribution when it matches the current schedule after a
    quiet `materialise_weeks(now)` (new runs matched by timing and boss
    week, existing runs by id, answers included). Runs of a timing the draft
    changes stay listed in that timing's channel summary, which is the
    channel a plain `FixedChanged` edit notifies.
  - **Notices are returned, not posted.** `merge_draft` RETURNS the notices
    in `MergeOutcome`; nothing enqueues them in the journal yet, and the
    service never writes the journal. The caller must enqueue them. Serve
    wiring must persist them atomically with the merge commit (an outbox
    written in the same transaction) or re-derive them: a crash between the
    commit and a separate enqueue would lose them, since the retry is
    `AlreadyApplied` and returns no notices. Not built yet.
  - Expiry scope is derived, never caller-supplied: a draft's
    `expires_week` is the earliest boss week (`policy.week_of`) touched by
    any run-level operation — `create_run` its slot; `amend_run` the source
    run's week and the destination slot's week; `set_status`,
    `swap_participants`, `set_rsvp`, `reset_to_fixed`, `set_run_bosses`,
    `ensure_reminders`, `recount_run`, `revive_run` their run's week
    (`drafts::expires_week`; `create_run` also counts its stated
    `week_start`, which staging requires to equal the slot's week). Drafts staging only weekly-timing operations
    (`add_fixed_run`, `apply_fixed_edit`, `retire_fixed_run`) have none and
    never expire by week. It is recomputed from the whole list on every add,
    edit, remove and rebase and stored in the same write; `StoredDraft.scope`
    (`DraftScope::{Week, Weekly}`) is its read-only view.
  - Expiry: live drafts whose `expires_week` is before the current boss week
    expire automatically and stay in history (closed by the system
    `delivery` actor, version unchanged). `preview_draft` and `merge_draft`
    refuse such a draft with `DraftError::Expired`. They also re-derive the
    expiry week by replaying on the current schedule, because upstream may
    have moved a drafted run into a past week since the scope was stored, and
    refuse when that week has passed. The merge closes an expired draft
    first (idempotent, as the system `delivery` actor, same as the tick), so
    the window between the reset and the tick stays closed. A draft that
    expires between that check and the commit reports `Expired` too (the
    store's `Stale(Moved{Expired})`). The delivery tick expires after `mark_done`; a
    failure never aborts the tick (throttled `DraftExpiryFailed` alert, next
    tick retries).
- The codec relies on `serde_json` without `preserve_order`; the golden
  vector guards it and must stay unchanged.

## Member requests

Status: implemented in `src/domain/requests/` (types, operations,
authorisation, requester notices) and `src/domain/scheduler/requests.rs`
(methods on `SchedulerService`, beside the draft methods). A request is a
draft of kind `request` (`request_type`, `subject` = `run:<id>` or
`fixed:<id>`) in the same `DraftStore`. No API or UI yet.

- **Not requests.** A member's direct writes (their own RSVP, this-boss-week
  moves of their own runs) stay direct edits with edit preconditions.
- **Types.** Every type needs administrator approval:
  - `new_fixed`: a new weekly run; the requester must own it and be in its
    party. Operation: `add_fixed_run`.
  - `change_fixed`: a weekly timing's day, time, party or channel (bosses
    and note are refused, `FieldNotAllowed`). Operation: `apply_fixed_edit`,
    staged with `UpdateAll`; per-run choices are made at approval.
  - `join`, `leave`, `swap` (the requester leaves, another member takes
    their place) on a run (`swap_participants`) or on a weekly timing
    (`fixed_participants`). Both are deltas, never a whole party list, so
    concurrent requests on one run or timing all merge. A join (or a swap's
    newcomer) already on the subject is refused at submit
    (`AlreadyInParty`); one added upstream after submit makes the approval
    `NoEffect` (reject it).
- **Submit** (`submit_request(member, title, spec, request_id, policy,
  directory, gate)`):
  - A frozen member is refused (`Frozen`). The freeze is the `MemberGate`
    port the caller supplies; there is no admin UI yet.
  - The requester is authorised per object on the current schedule: a guild
    member with the bossing role for `join` and `new_fixed`; a participant
    of the subject for everything else (`RequesterUnauthorised`,
    `UnknownSubject`).
  - The operations are dry-run on the current snapshot; a request that
    could not apply is refused at submit.
  - The expiry week is derived exactly as for drafts; requests touching
    only weekly timings do not expire by week.
  - The store writes the request `submitted` with its operations, `created`
    and `submitted` events and the derived expiry. In the SAME insert
    transaction it counts the member's undecided requests (at most 3) and
    submissions in the rolling 24 h (at most 6, withdrawn ones included):
    `DraftCreated::Limited(Pending | Rate)`, nothing written. Concurrent
    submits serialise on the writer, so the cap holds.
  - Idempotent per (member, request id): the exact-retry lookup runs FIRST,
    before the freeze, authorisation, dry run and expiry checks, so an exact
    retry returns the request even if things changed since; the same id for
    another request is `RequestMismatch`. The insert re-checks inside its
    transaction.
- **After submit.** The requester cannot edit; they may only withdraw
  (`withdraw_request`, requester only, while `submitted`) and resubmit. An
  administrator may replace the operations (`edit_request`): they must
  replay on the request's base, the expiry is re-derived, and the version
  goes up.
- **Preview** (`preview_request(actor, id, choices, policy, directory,
  gate)`), administrators only: the three-way analysis of the request's
  operations (with `choices` substituted) against the current schedule,
  `requester_authorised` on it, `requester_frozen`, the version and status,
  and `no_effect` (approving would change nothing: already in effect).
  Nothing is written.
- **Redundant requests.** A request already in effect (for example the
  admin added the member directly) is never closed automatically: the
  preview reports `no_effect: true`, approval is refused with
  `RequestError::NoEffect` (nothing written, still `submitted`), and the
  admin rejects it with that reason.
- **Approve = merge** (`approve_request(actor, id, reviewed_version,
  choices, policy, directory, gate)`), administrators only (`NotAdmin` for
  members and system actors):
  - The version must be the one reviewed (`Stale` otherwise), and withdrawn,
    rejected, merged or expired requests cannot be approved.
  - The requester is re-authorised on the current schedule, and again on
    `flow.current` at EVERY merge attempt (an authorisation check travels in
    `MergeInput`), so a requester removed between the check and a
    re-planned commit is `RequesterUnauthorised` and nothing is committed.
  - A frozen requester does not block approval: approval stays an
    administrator decision. `Approved.requester_frozen` (and the preview's)
    reports it so the inbox can badge it.
  - `change_fixed` needs `choices` (`ChoicesRequired`); other types refuse
    them (`ChoicesNotApplicable`). Choices made for a different set of
    amended runs than the current schedule lists are `ChoicesStale`. The
    applied choices are recorded in the `merged` event's detail after the
    record seq (`<seq> choices=<run>:<choice>,…` or
    `choices=update_all`).
  - Otherwise it runs the draft merge (shared `merge_loaded`): three-way
    analysis, where conflicts block and there is no force; the expiry
    re-derived on the current schedule; one record through
    `Surface::RequestMerge`, `request_id` `merge:<id>@v<version>`, `refs`
    the base head. The draft is closed `merged` with `merged_seq` in the
    same transaction, and blame names the approving admin with
    `via: Referenced([base])`.
  - Public merge summaries (`NoticeChange::Merged.title`) of a request use a
    code-generated description, `member request: <type> <subject>`; the
    member's title never reaches a public channel. Administrator drafts keep
    their title.
  - Once committed, an approval or rejection never returns an error: the
    requester notice is built from the snapshot read before the write, and
    nothing is read afterwards.
- **Reject** (`reject_request(actor, id, reviewed_version, reason)`):
  administrators only, with a reason of 1..=500 characters after trimming
  and without control characters (`ReasonRequired`, `ReasonInvalid`). No
  schedule record is written; the draft's `rejected` event and
  `close_reason` keep it.
- **Expiry.** Hooked into the same tick expiry: `expire_due_drafts` closes
  drafts and requests alike (system `delivery` actor) and returns
  `DraftExpiry { ids, notices }`. An approval that finds the request
  expired closes it as the system actor and returns
  `RequestError::Expired(notice)`.
- **Requester notice.** On approve, reject and expire the service RETURNS
  one `NoticeChange::RequestDecided { request, decision, reason }` notice
  in the subject's home channel as it was before the decision (a new weekly
  run's own channel). It
  lists only the requester, so it mentions nobody else; its effect is
  `notice.request.<decision>` and its dedupe context
  `request:<id>:<decision>`. Nothing enqueues it yet: serve wiring must
  persist it atomically with the decision (the same outbox as merge
  notices).
- **Kinds stay apart.** The administrator draft methods refuse request
  drafts (`RequestDraft`); the request methods refuse administrator drafts
  (`AdminDraft`).
- **API note (member-facing).** Member-facing endpoints must collapse
  `UnknownSubject`, `UnknownDraft`, `AdminDraft`, `RequesterUnauthorised`
  and `UnknownTarget` into ONE uniform not-found-or-forbidden response, so
  no one can probe which runs, timings or requests exist. `Backend` error
  text (it may hold paths) is never surfaced to anyone.

## Proposals (extractor and chatbot)

Status: storage in `src/domain/drafts/proposal.rs` (`ProposalStore`), both
stores, migration `0007`; the pure rules in `src/domain/proposals/`
(translation, `may_commit`, v4 refusal texts, card notes); the service in
`src/domain/scheduler/proposals.rs`; expiry in the delivery tick. The
`commit` family of `docs/v5/vectors/extract/` replays through it
(`tests/extract/commit.rs`). Decided (user, 2026-09-25): v4 approval — ✅
by a participant of the run, an administrator or the run's owner —
enforced by the service, not the store; TTL 24 h.

- **A proposal is a draft** of kind `DraftKind::Proposal`, created
  `submitted` with its operations (`created` + `submitted` events) and
  merged only through `DraftStore::commit_merge` (no second write path).
  Every existing draft/request service method refuses it, since they check
  for their own kind.
- **Storage (decision D-1).** The `0005` `drafts.kind` CHECK is not
  rebuilt: a proposal is stored as an `admin` row plus a `draft_proposals`
  row (`source` `extraction|chat`, `source_id`, nullable `supersede_key`,
  `expires_at`), inserted in the same transaction. Reads report
  `Proposal` when that row exists. Triggers refuse a `draft_proposals` row
  for anything but a system-authored `admin` row, and refuse UPDATE/DELETE.
  `create_draft` refuses the proposal kind in both stores.
- **Author.** Always a system actor (the extractor or chatbot component);
  anything else is `StoreError::Constraint`.
- **Idempotency.** Creating an existing id returns `Replayed` when it is a
  proposal with the same source and source id; any other existing draft id
  is `Constraint`.
- **Supersede.** In the create transaction, every live proposal with the
  same non-null `supersede_key` is closed `discarded`, reason `superseded`
  (event detail the same), by the new proposal's system author, version
  unchanged. The ids are returned. Closed proposals are left alone.
- **Card details** (migration 0011, `ProposalCardStore`; user decision
  2026-09-25): `proposal_cards` holds, per proposal, the card channel and
  the `CardDetails` JSON the card renders (v4's row: kind, run, bosses,
  party, time, the literal day/time words, answer, question flag, summary,
  also-mentioned, confidence, payload incl. `weekly_when`, evidence ids, the
  self-service line), written once after propose (the same details again
  are a no-op; different ones are `Constraint`; not in the propose
  transaction, so a crash in between leaves a proposal without a card, which
  the admin inbox still lists). `message_id`/`posted_at` are written only by
  the delivery journal's `card` bind; a trigger refuses any other change and
  every delete. Lookups: by proposal ids, by message (✅/❌), and live unposted
  cards per channel (reposted before the next card, as v4).
- **TTL.** `expires_at = created_at + ttl`; `ttl` is a per-proposal
  parameter (positive; `DEFAULT_PROPOSAL_TTL` = the decided 24 h).
  `expire_proposals(now, actor)` closes live proposals with
  `expires_at <= now` as `expired` (version unchanged). The week expiry
  (`expire_drafts`) still applies to a proposal's `expires_week`.
- **Propose** (`propose(ProposalRequest { change, source, source_id,
  supersede }, policy, directory)`). A `ProposedChange` (v4's `amendments`
  row: kind, run, channel, bosses, participants, new time, answer, typed
  payload) is translated on the current schedule into operations (v4
  `commit.py` per kind): `move` → `amend_run`, preceded by `revive_run`
  when the run is cancelled or otot (user decision: v4 `_move` revives it;
  the move then re-derives it and ends any pin as every move does — admin
  and draft `amend_run` keep their rule); `add` → `create_run` +
  `ensure_reminders`; `cancel`/`otot` → `set_status`; `sub` → a
  `swap_participants` delta (leavers not on the run and joiners already on
  it ignored, as v4); `split` → `set_run_bosses` (what stays) +
  `create_run` + `ensure_reminders` (what leaves; all bosses leaving is a
  move); `rsvp` → one `set_rsvp` (source `chat`) per named member +
  `recount_run`; `fix` → `add_fixed_run` (note `created from chat`), an
  edit → `apply_fixed_edit` (`UpdateAll`), a removal → `retire_fixed_run`
  for the materialised weeks. The operations are dry-run there.
  **Up-front refusal (user decision, `D-PROPOSE-REFUSES`):** a change that
  cannot apply now is refused with v4's `commit` text (`Refusal`, e.g. `no
  new time was agreed - use /amend to set one`), and one that would change
  nothing is `NoEffect`; nothing is written and no card is posted, and the
  caller records the reason in its extraction/chat log. v4 carded it and
  refused at ✅. A new run or timing that names nobody is staged without a
  party (a timing also without owner); approval fills in the approver, as
  v4 used the confirming member. The draft `subject` holds the target
  (sorted-key JSON: kind, run, timing, channel, bosses, named members);
  `Supersede::Older` stores its key — the run in this channel, else the
  new boss set in this channel — so the store retires same-target live
  proposals; `Supersede::Keep` stores none.
- **Approve** (`approve_proposal(id, approver, policy, directory)`):
  `Approver { user_id, has_role, is_admin }` comes from the caller (role and
  admin checks belong to it; `is_admin` also covers the guild owner). The
  service applies v4 `may_commit` on the current schedule: an admin, the
  owner of the run's (or the edited timing's) weekly timing, or, with the
  bossing role, a participant of the run (or timing), or for a change
  without one a named member (anyone with the role when it names nobody).
  Anyone else is `Unauthorised`, nothing written; this is checked first, so
  their ✅/❌ never closes anything (the same for reject). A proposal past its
  TTL answered by an allowed member is closed `expired` (system `delivery`)
  and refused (`Expired`;
  `D-EXPIRED-REFUSED`, v4 applied it). A vanished target or an answer for a
  member no longer on the run is refused with v4's text. Then the shared
  draft merge (`merge_loaded`) commits one record through
  `Surface::ExtractionApproval` or `Surface::ChatApproval` (by source),
  actor the approving member, `request_id` `approve:<id>`; the authority
  check is repeated at every merge attempt, conflicts block, and a replay
  refusal maps to v4's text. **Status at approval (parent decision, v4
  parity):** a `cancel`/`otot` target and a `recount_run` apply to the run
  as it is at approval — the run's `status` field takes the merge result
  (`analyze_merge_applying`, `MergeInput.status_at_apply`), so an upstream
  status change (a reaction, a tick recount) never conflicts with them. An
  upstream removal (done/cancelled/deleted) and every other field still
  conflict, and admin drafts and requests pass no such runs. A repeated ✅
  by the same member is `AlreadyApplied`, by another `AlreadyMerged`:
  nothing is written or posted. When the repeating member is the one who
  merged it (the draft's `closed_by`) or an administrator, and is still
  allowed to answer it on the current schedule, the idempotent follow-ups
  below are re-run first, so a crash between the merge and them is
  repaired by the next ✅; anyone else's repeat has no effect at all. The
  merge's summary notices (`NoticeChange::Merged`, title
  `<kind> proposal`) take the draft-merge outbox path. After the commit
  nothing returns an error: sibling live proposals about the same target
  that were created at or before the merge (the draft's merge time; never
  newer ones, on the first run or a re-run) are retired (v4 `commit`'s
  `supersede`, below), a new weekly timing's
  weeks are materialised (a second record, `approve:<id>:materialise`), and
  failures there are reported in `follow_up_errors`. The outcome carries
  v4's `CommitResult` facts (run, timing, created runs, old time,
  superseded, notes `adopted …'s run` / `updated N` / `cancelled N
  scheduled run(s)`).
- **Reject** (`reject_proposal(id, approver)`): the same members as for ✅;
  closed `rejected`, no schedule record.
- **Supersede** (`supersede_proposals(SupersedeScope)`, v4 `supersede`):
  live proposals about the same run — only those in `from_channel` unless
  it is the run's home channel — else, with a channel and bosses, run-less
  ones in that channel with the same boss set; closed `discarded`, reason
  `superseded`, by the source component.
- **Expiry.** The delivery tick calls `expire_due_proposals` after draft
  expiry: live proposals with `expires_at <= now` are closed once as the
  system `delivery` actor (`D-TTL-BOUNDARY`: v4 expired only strictly after
  24 h). Nothing is posted; a failure raises the throttled
  `DraftExpiryFailed` alert and the next tick retries.
- **Attendance.** The operations are the ordinary ones, so v5's pin and
  freeze rules hold: `recount_run` re-derives through `derive_run_status`
  (a pinned run keeps its hand-set status, a started run keeps its status;
  in v4-compat mode exactly v4's `compute_status`), and `set_rsvp` never
  ends a pin.

## Cherry-pick

Status: implemented in `src/domain/history/cherry_pick.rs` (pure planning,
`plan_pick`) and `src/domain/scheduler/cherry_pick.rs` (`cherry_pick`,
`preview_cherry_pick` on `SchedulerService`). No API or UI yet; the API
must enforce administrator-only access to both (the service takes an
admin id and records the pick as that administrator).

Re-applies ONE recorded change of a weekly timing's run to the same
timing's run in another boss week: `cherry_pick(admin, request_id, seq,
target_week, mode, policy, directory)`; `mode` is `Strict` (default) or
`Force(reviewed)`.

- **Source.** The record is loaded with its stored bytes hash-checked
  (`load_checked`); unknown, genesis or tampered records are refused
  (`HistoryRefusal::UnknownChange` / `Tampered`). Rollbacks are refused
  (`Rollback`: undo them with a revert). Draft and request merges are
  refused too (`Merge`): they are reviewed multi-operation changes, so pick
  the individual change or draft again. A cherry-pick record may itself be
  picked.
- **Mapping.** Each run row maps to the run with the same `fixed_run_id` in
  the target boss week (`target_week` is normalised to its week). A missing
  one is `NoTargetRun { fixed_run_id }`. All touched runs must be in one
  source week, and a record that moves a run across weeks or changes its
  timing is unsupported. A done or cancelled target is refused outright,
  strict or forced (`TargetFinished { status }`); an `otot` target takes
  party and answer steps but no slot step (`UnsupportedPick`).
- **Translation** (`PickStep`s, applied in order). A field the target
  already holds at its picked value is skipped (no step, no conflict), so
  repeating a pick, or forcing onto a target already equal to the result,
  is `NoEffect` with no commit.
  - slot → `amend_run` to the same LOCAL weekday and time in the target
    week, placed by `slot_in_week`, so it is DST-aware (21:00 GMT picked
    into a summer-time week is 21:00 BST). A source slot that is the
    timing's own placement (possibly shifted by a DST gap) maps to the
    timing's placement in the target week, so a shifted source never reads
    as a different time (no false strict conflict).
  - party → `swap_participants` with the delta (removed and added
    members). In force mode, members to remove who are no longer on the
    target (and members to add who already are) are dropped from the
    delta.
  - status → `set_status` only when it is the run's only change. A status
    changed alongside a move, a swap or answers is derived: it follows from
    them on the target (answers recount it, below).
  - answers → `set_rsvp` when the member is on the target run (after the
    party delta), then the target's status is RECOUNTED as a reaction does
    (`compute_status`: a "no" puts it at risk, a complete "yes" tally
    confirms it). An answer by a member not on the target run is
    `UnsupportedPick`. Cleared answers are side effects and are skipped only
    when the same record explains them: the members it took off the run
    (the swap clears them again on the target), or a run coming back to
    `planned` from cancelled/otot/done (the status step clears them again;
    the status is then the run's sole change). Any other cleared answer is
    `UnsupportedPick`, so nothing is dropped silently.
  - Anything else is `UnsupportedPick { reason }`: weekly-timing rows,
    one-off runs (no `fixed_run_id`), run creation or removal, bosses and
    channel changes, a derived `at_risk` status. Reminders are ignored
    (derived).
- **Precondition.** For each picked field the target's current value must
  equal the record's `before` value mapped into the target week (the slot
  mapped as above, the party as a set, the status, the answer). Otherwise
  it is a `PickConflict { run_id, field, expected, current }` (`field` is
  blame's name). `Strict` refuses with `PickError::Conflicts` and writes
  nothing.
- **Force is bound to the review.** The preview returns `force`: an
  `Expect` naming each conflicting `(run, field)` with the change that last
  set it (`seen`; `None` when the field has no history) and those changes
  as overrides `{seq, hash}`. A forced pick sends exactly that back
  (`PickMode::Force(reviewed)`):
  - every attempt re-plans, and if the conflicts' expectations differ from
    the reviewed ones it refuses with `PickStale { reviewed, current }` and
    writes nothing;
  - the reviewed expectations travel as the commit's `expect`, so the store
    re-checks those fields' versions, the overrides and the admin actor
    inside the commit transaction (a change landing in between is
    `PickStale` too);
  - the record's `refs` are [the picked record, then the overridden
    changes], and whenever at least one conflict was forced it carries the
    `notice.edit.override` marker, even when no overridden change exists
    (fields without history). Blame shows
    `via: Override { picked: Some(picked), overridden }`.
- **Refusals.** The target week is the source week (`SameWeek`: revert or
  edit instead); the target boss week has passed (`WeekPassed`).
- **Commit.** The steps replay through `apply_op` on the whole schedule
  (every normal rule applies; a refusal is `PickError::Schedule`), and a
  pick that changes nothing is `NoEffect`. One record is written with
  `Surface::CherryPick` and the admin's `request_id`. The digest covers the
  seq, the NORMALISED boss-week start and the canonical reviewed set, so a
  retry naming another instant in the same week is still `AlreadyApplied`,
  while another request with the same id is `IdempotencyMismatch`. It is
  re-planned on a revision race. Blame shows `via: Referenced([picked])` for
  an unforced pick.
- **Notices.** The replayed mutations' normal notices are RETURNED in
  `Picked.notices`; nothing is posted or claimed before (or by) the commit.
  Serve wiring enqueues them through the same outbox as merge notices.
- **Preview.** `preview_cherry_pick(seq, target_week, policy, directory)`
  returns the plan (source and resulting week, steps, conflicts), the
  notices, `no_effect`, `strict_refuses` and `force` (exactly what a forced
  pick must echo), and writes nothing. Planning is mode-independent (the
  party delta is trimmed in every mode, and an emptied delta is no step),
  so the preview never disagrees with strict mode: `plan.conflicts` are the
  ones strict refuses (`strict_refuses`) and the ones `force` covers. The
  conflict values and their versions are read against one store revision:
  if anything was committed between reading the schedule and reading the
  versions, the preview reads again (up to `COMMIT_ATTEMPTS`).
