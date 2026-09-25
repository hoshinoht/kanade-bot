# Attendance model (v5)

A v5 behaviour change from v4 (user decisions of 2026-09-25): members often
do not answer but still come. v5 adds standing answers, a per-timing
default, derived answer states, status and ping rules built on them,
tallies, and recorded attendance.

**Status.**
- Pure rules: `src/domain/attendance/` (`AttendanceMode`,
  `AttendancePolicy`, `AttendanceDefault`, `StandingAnswer`, `AnswerState`,
  `answer_state`, `run_states`, `derive_status`, `Tally`,
  `morning_mentions`, `countdown_mentions`, `prefill`, `check_attendance`,
  `attendance_patterns`, `suggest_standing`, `StandingAction`), with unit
  tests.
- Built on the write path: standing answers and the timing default
  (`schedule::{set_standing_answer, set_attendance_default}`,
  `Op::{SetStandingAnswer, SetAttendanceDefault}`, the
  `Attributed::{set_standing_answer, set_attendance_default}` service
  methods), stored by migration `0006` on both stores with history, blame,
  preconditions and `read_versioned` (conformance: `attendance_conformance`).
- B5 (policy): `SchedulePolicy.attendance: AttendancePolicy`, built with
  `SchedulePolicy::new(reminders, reset_weekday, reset_time)` (always
  `V4_COMPAT`) and opted in with `.with_attendance(AttendancePolicy::V5)`.
  Every construction site and vector replay uses `new`, so they run in
  v4-compat mode.
- B6 (wiring): every status recount goes through
  `schedule::derive_run_status` (= `attendance::derive_status` over
  `schedule::answer_states`): reactions, roster changes (swaps, party
  edits and deltas, adoption, reset), and the cherry-pick answer recount.
  The rules come from the `Draft`'s policy
  (`Draft::with_attendance`), and there is ONE source: the scheduler's
  `SchedulerService::with_attendance(policy.attendance)` (default
  `V4_COMPAT`). Every call that also takes a `SchedulePolicy` (the
  attributed ops carrying one, the draft and request methods,
  `preview_fixed_edit`, cherry-pick) is refused with
  `ScheduleError::AttendanceMismatch { service, policy }` when its
  `attendance` differs, so draft replay and cherry-pick (which read the
  policy they are given) always derive under the scheduler's rules. The tick recounts after `mark_done`
  (`Attributed::recount_attendance`, `Op::RecountAttendance`, v5 only; a
  failure raises the throttled `AdminAlert::AttendanceRecountFailed` and
  never aborts the tick; `TickReport.recounted`). Dispatch reads
  `DeliverySettings.attendance`: the morning ping uses `morning_mentions`
  in v5 and v4's everyone-on-the-runs in v4-compat; countdowns use
  `countdown_mentions` (identical to v4's `not_declined`). `render(intent,
  schedule, attendance, quiet)` adds ` · <tally>` (and `, expected` for a
  confirmation resting on assumed answers) to run lines, and the digest
  lists the week's live runs with their tallies, in v5 only. Hand-set
  planned/confirmed statuses are pinned and kept (see *Hand-set status*).
  Standing-answer and default changes re-derive their timing's live,
  not-yet-started runs in their own commit (see *Status*).
- Standing answers and timing defaults can be written in V4_COMPAT mode
  (with history and blame) but have NO effect on answer states, status,
  pings or rendering until the mode is V5; the UI labels them "active in
  v5 mode".
- Previews that re-read on a revision race (cherry-pick preview, like the
  commits) give up after `COMMIT_ATTEMPTS`; journal-only writes (delivery
  claims and binds) also bump the store revision, so a busy tick can
  exhaust them. Callers should retry the preview.
- B7 (attendance): `Attributed::record_attendance(run_id, &attended)`
  (`Op::RecordAttendance`, `schedule::record_attendance`) and the store
  query `AttendanceHistory::member_history(member, per_timing)` behind
  `SchedulerService::member_attendance(viewer, member)`; conformance on
  both stores (`attendance_conformance`).
- NOT built: the Discord "✅ every week" control (port only,
  `StandingAction`) and the API/PWA surfaces.

## Modes

- `AttendanceMode::V4Compat`: no standing answers and every timing opt-in;
  every rule reduces exactly to v4's (`derive_status` equals
  `schedule::compute_status` for every status and answer combination,
  tested). Frozen v4 vectors (RSVP, status, digest, mentions) replay in this
  mode and must stay green.
- `AttendanceMode::V5`: the rules below.
- `AttendancePolicy { mode, unknown_window }`; the unknown window defaults
  to 12 h. It is the `SchedulePolicy.attendance` field, `V4_COMPAT` unless
  set (so every v4 vector replays in v4-compat mode). Serve wiring must
  hand the same value to `SchedulerService::with_attendance` and to the
  delivery config.

## Standing answers

- A member may set or clear "always in" per weekly timing:
  `set_standing_answer(member, fixed_run_id, on | off)`.
- It applies to that timing's runs in materialised and future weeks, unless
  the member reacted explicitly on a run (an explicit reaction always wins).
- It is removed automatically when the member leaves the timing's party:
  `fixed_participants` (party delta), `apply_fixed_edit` (party edit),
  `retire_fixed_run`, and swaps on the timing that remove them.
- It goes through the scheduler write path:
  `service.as_origin(origin).set_standing_answer(member, fixed_run_id, on)`.
  A member may set only their own (`SchedulerError::Forbidden` otherwise);
  an administrator may set it for anyone. Only a member in the timing's
  party may have one (`ScheduleError::NotInParty`). Setting it again keeps
  the original; clearing an absent one does nothing; neither records
  anything.
- The answers live ON the weekly timing (`FixedRun.standing`, sorted by
  user: `user_id`, `set_by` as `<kind>:<id>`, `at`), so every change is part
  of the timing's row in the change record, and blame field
  `standing:<user>` on the weekly timing names it (see `history.md`,
  *Blame* and *Edit preconditions*; preconditions accept `standing:<user>`).
- Auto-removal happens in the same commit as the party change, because
  `Draft::update_fixed_run` drops the answers of members no longer in the
  party: `fixed_participants` (a request's leave or swap on a timing),
  `apply_fixed_edit` and `edit_fixed_run` party edits. Retiring a timing
  deletes it with its answers. Swaps on one run (one-off substitutions) and
  cherry-picked party steps change a run, not the timing's party, so they
  keep the standing answer.
- Card control "✅ every week": `StandingAction { fixed_run_id, on }` with
  custom id `standing:<fixed_run_id>:on|off`. Adapter change needed (not
  wired): render it as a button on weekly-run cards, route presses through
  `commands::spawn_interaction` to `set_standing_answer` for the pressing
  member (a participant of the timing), and reply ephemerally.

## Per-timing default

- `attendance_default: opt_in | assume_coming` on a weekly timing
  (`FixedRun.attendance_default`), set with
  `service.as_origin(admin).set_attendance_default(fixed_run_id, default)`,
  administrators only (`Forbidden`). Blame field `attendance_default` on the
  weekly timing (listed once it has been set); preconditions accept it.
  One-off runs are always opt-in. Timings imported from v4 start `opt_in`.
- Record encoding: the change record's weekly-timing row carries
  `attendance_default` only when it is not `opt_in` and `standing` only when
  non-empty, so every v4-shaped row (and the golden history vector)
  encodes byte for byte as before; older rows decode with the defaults.

## Answer states

`AnswerState::{Confirmed (explicit ✅), Assumed(Standing | Default),
Unknown, Declined (explicit ❌)}`, from ONE pure function
`answer_state(facts, mode)`:

1. an explicit reaction wins (✅ confirmed, ❌ declined);
2. else a standing answer: assumed (standing);
3. else an `assume_coming` weekly timing: assumed (default);
4. else unknown.

Removing a reaction reverts the member to assumed or unknown. The v5 rule
still holds: removed members' answers are dropped.

## Status

`derive_status(current, states, start, now, policy)`:

- cancelled, otot and done are kept;
- at risk only from an explicit ❌, or (v5) from any unknown member once
  `now >= start - unknown_window`. The window is ELAPSED time on UTC
  instants, so across a DST change it is still 12 real hours;
- confirmed when every participant is confirmed or assumed, with the
  presentation flag `expected: true` when any is assumed (the UI says
  "expected" instead of "confirmed");
- otherwise planned. In v5 status is FULLY derived: there is no sticky
  confirmed, so withdrawn assumptions (the default back to opt-in, a
  cleared standing answer) take a confirmation back to planned (or at
  risk under the rules above). v4-compat keeps v4's rule that silence
  does not undo a confirmation.
- Standing-answer and default changes re-derive every live, not-yet-started
  run of that timing at `now` in the same commit (v5 only), so the change
  shows at once; the tick's recount covers the unknown window.
- Derivation goes through `schedule::derive_run_status`, which in v5 first
  applies the two rules below (in v4-compat mode neither exists).

### Hand-set status (v5)

- `set_status` to `planned` or `confirmed` (directly, from a draft, or a
  cherry-picked `Status` step) pins it on the run: `Run.status_pin:
  Option<StatusPin { status, at }>`. Who set it is the blame of the run's
  `status_pin` field (the change's actor), so the pin does not repeat it.
- While pinned, derivation keeps the pinned status: recounts, standing
  answers, defaults and the unknown window do not override it.
- The pin ends, and the run is re-derived in the same commit, when:
  someone reacts explicitly (✅ or ❌ added, or a reaction removed) on that
  run; the run's participant list changes before the start (swap, party
  edit or delta, reset, adoption, fixed-edit push); the run moves to a new
  slot (`amend_run`, `reset_to_fixed` even when only slot, bosses or
  channel differ, or a weekly weekday/time edit pushed onto it), re-derived
  from answers for the new slot; an administrator sets the status again (planned or
  confirmed re-pins; done, cancelled or otot clear it); or the run leaves
  the live statuses (`Draft::set_run_status` clears it). Answers written
  by `set_rsvp` (portal/chat, draft and cherry-pick replays) keep v4's
  `set_rsvp` semantics and do not end it. An approved chat proposal's
  answers are followed by `recount_run`, which re-derives through
  `derive_run_status`: it keeps a pin (and a started run's status).
- Cards and the admin UI say "confirmed (set by admin)": `status_label`
  gives `StatusLabel::SetByAdmin(status)` (a pin wins over `Expected`),
  and `tally_text` appends it (`0/2, confirmed (set by admin)`), v5 only.
- V4_COMPAT: `set_status` never writes a pin and derivation ignores one,
  so behaviour, the golden vector and v4 encodings are unchanged.

### Frozen at start (v5)

- Once `now >= run.datetime`, answers, reactions, swaps and recounts no
  longer change the run's status (`schedule::is_frozen`); only a hand-set
  status (done, cancelled, otot, or planned/confirmed) does. Reactions and
  participant changes after the start do not end a pin (its blame stays);
  only a manual status change touches it. The tick's recount and the timing
  re-derive already skip started runs. V4_COMPAT is unchanged (a swap
  after the start may still re-derive, as v4).

## Pings

- The morning ping mentions only unknown members (`morning_mentions`).
- Countdown pings keep v4: everyone who has not declined
  (`countdown_mentions`).

## Tallies

`Tally { confirmed, assumed, unknown, declined, total }` for cards, digest,
board and API. Text: coming over party, then notes, e.g. `4/4 (2 assumed)`
or `2/4 (1 assumed, 1 out)`. Cards and digest render it with a softer mark
for assumed answers.

## Attendance

- `service.as_origin(origin).record_attendance(run_id, &attended)` at or
  after marking the run done; `attended` is the whole attended set as the
  caller sees it. Returns the `RunState`. A participant without an entry
  stands at the prefill (everyone not declined, `recorded_or_prefill`), so
  the screen starts from it for one-tap correction.
- `check_attendance` against that standing set: the run is done
  (`ScheduleError::Attendance(NotDone)`); attendees are on the run
  (`NotOnRun`); a member may change only their own entry
  (`SchedulerError::Forbidden`); a system actor is `Forbidden`.
- Writes: an administrator writes an entry for every participant, a
  member only their own; an entry that already says the same is kept as
  recorded (who and when), so its blame does not move.
- Entries follow the run: a participant change drops the leavers' entries
  (swaps are allowed on a done run; a newcomer starts at the prefill), and
  reviving a done run (`set_status` away from done) clears them, in the
  same commit.
- Stored ON the run (`Run.attendance: Vec<AttendanceRecord { user_id,
  attended, recorded_by, at }>`, sorted by user), like standing answers on
  the timing: every change is part of the run's row in the change record
  (key `attendance`, only when non-empty, so v4-shaped rows encode as
  before), blame field `attended:<user>` on the run, preconditions accept
  it, and `read_versioned` returns the row with it.
- `SchedulerService::member_attendance(viewer, member)` →
  `MemberAttendance { member, history, patterns, suggest_standing }`, read
  from `AttendanceHistory::member_history(member,
  RECENT_RUNS_PER_TIMING = 4)`: the member's recorded entries on done runs
  with their explicit answer, newest first, at most 4 per timing (one-off
  runs share one group). Runs without an entry for the member are not
  counted. Only an administrator or the member themselves may read it
  (`Forbidden` otherwise; the API must enforce it too).
- `attendance_patterns`: attended without answering, answered in but
  absent, answered out but attended.
- `suggest_standing`: suggested for a timing the member is in and holds no
  standing answer on, when they attended without answering on at least 3
  of their last 4 runs of it. It is a derived suggestion, never applied
  automatically.
- No Discord voice-state inference.

## Storage (migration `0006_attendance`, unreleased)

- `fixed_runs.attendance_default TEXT NOT NULL DEFAULT 'opt_in'`
  (checked: `opt_in`, `assume_coming`);
- `standing_answers (fixed_run_id, user_id, set_by, at)`, primary key
  `(fixed_run_id, user_id)`, deferred FK to `fixed_runs`, index on
  `user_id`. Written with the timing's row (deleted and re-inserted when
  the timing is written; loaded onto `FixedRun.standing`);
- `run_status_pins (run_id PRIMARY KEY, status, at)` (checked: `planned`,
  `confirmed`), deferred FK to `runs`: the run's `status_pin`, written with
  the run's row and loaded onto `Run.status_pin`; the change record's run
  row carries `status_pin` only when set;
- `run_attendance (run_id, user_id, attended, recorded_by, at)`, primary
  key `(run_id, user_id)`, deferred FK to `runs`, index `(user_id,
  run_id)`: written with the run's row (deleted and re-inserted when the
  run is written; loaded onto `Run.attendance`), read by `member_history`
  (one constant query with `ROW_NUMBER()` per timing);
- the memory store keeps the same fields on its rows; the
  `attendance_conformance` suite runs on both stores.

## API notes

- Public (members): set or clear their own standing answer on a timing
  they are in; confirm their own attendance on a done run; read their own
  patterns and suggestions.
- Admin: set a timing's default; record attendance for anyone; read any
  member's patterns.
- Role and object checks belong to the API layer (see `history.md`, *Role
  checks belong to the API*).
