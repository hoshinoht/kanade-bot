# Changelog

Notable changes to the Boss Scheduler Bot, newest first.

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
