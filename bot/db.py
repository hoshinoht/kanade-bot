"""SQLite storage.

Deliberately no ORM: explicit schema SQL, a ``schema_version`` table, and a thin
repository over :mod:`sqlite3`.  Lists are stored as JSON text and all datetimes
as ISO-8601 UTC strings.

Calls run synchronously on the event loop.  At this scale (a handful of rows per
guild per week, and a 30 s tick) each statement is microseconds, so the extra
machinery of a thread executor would buy nothing.  Every method here is a plain
function of ``self._conn`` though, so wrapping the class in ``asyncio.to_thread``
later is a mechanical change.
"""

from __future__ import annotations

import json
import logging
import math
import sqlite3
from collections.abc import Callable, Iterable, Sequence
from datetime import datetime
from pathlib import Path
from typing import Any

from .ids import new_id
from .timeutil import from_iso, to_iso, utcnow

log = logging.getLogger(__name__)

SCHEMA_VERSION = 8

#: How many chat interactions are kept. The table is a diagnostic, not a
#: transcript: five hundred is weeks of a guild's questions at the rate the
#: pilot is actually used, and it bounds a table nothing else ever deletes from.
CHAT_INTERACTIONS_KEPT = 500

#: How many audit rows are kept. Deliberately four times the chat log: this is
#: the answer to "who moved my run", which is asked weeks later about a night
#: nobody wrote down, and a row is three short strings rather than a prompt.
AUDIT_KEPT = 2000

#: Where a change came from. Documentation rather than a constraint -- an audit
#: row must never be the thing that fails a mutation, so nothing validates
#: against this (see :meth:`Repo.log_audit`).
AUDIT_SURFACES = ("portal", "cli", "discord", "chat", "card", "system")

#: The `amendments.payload` key that says the chat pilot has already asked what
#: this card should have said. Set once, by :meth:`Repo.claim_chat_followup`,
#: and never cleared: "one follow-up per card" is a promise about the card's
#: whole life, not about the current reaction.
CHAT_FOLLOWUP_KEY = "chat_followup_at"

RUN_STATUSES = ("planned", "confirmed", "at_risk", "otot", "done", "cancelled")
RSVP_STATES = ("yes", "no", "maybe")
#: How much a member wants to be @mentioned (DESIGN.md s3, "Mention policy").
#: `essential` is the default: only the posts that ask them to act.
PING_LEVELS = ("essential", "all", "off")
DEFAULT_PING_LEVEL = "essential"
#: `superseded` = a newer card about the same run replaced this one, or a sibling
#: amendment for the run was committed. Kept rather than deleted so the
#: extraction log still shows what was proposed and why it never applied.
#: `withdrawn` = the card itself was deleted from Discord, so there is nothing
#: left to react to. Kept rather than deleted so the extraction log still shows
#: what was proposed and why it never applied.
AMENDMENT_STATUSES = (
    "proposed",
    "confirmed",
    "rejected",
    "expired",
    "superseded",
    "withdrawn",
)

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS members (
    user_id      TEXT PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    nickname     TEXT,
    aliases      TEXT NOT NULL DEFAULT '[]',
    has_role     INTEGER NOT NULL DEFAULT 0,
    -- how much this member wants to be @mentioned: essential | all | off
    ping_level   TEXT NOT NULL DEFAULT 'essential',
    updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fixed_runs (
    id           TEXT PRIMARY KEY,
    owner_id     TEXT NOT NULL,
    -- home channel: the channel /fixed add was invoked in. All of this run's
    -- output goes there (DESIGN.md s1, "Party channels").
    channel_id   TEXT,
    bosses       TEXT NOT NULL,
    weekday      INTEGER NOT NULL,
    time         TEXT NOT NULL,
    participants TEXT NOT NULL,
    note         TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS amendments (
    id                   TEXT PRIMARY KEY,
    week_start           TEXT NOT NULL,
    kind                 TEXT NOT NULL,
    bosses               TEXT NOT NULL DEFAULT '[]',
    run_id               TEXT,
    new_datetime         TEXT,
    participants         TEXT NOT NULL DEFAULT '[]',
    status               TEXT NOT NULL DEFAULT 'proposed',
    confidence           REAL,
    evidence_msg_ids     TEXT NOT NULL DEFAULT '[]',
    proposal_message_id  TEXT,
    created_at           TEXT NOT NULL,
    -- v3, added for the chat extractor.
    channel_id           TEXT,
    is_question          INTEGER NOT NULL DEFAULT 0,
    rsvp                 TEXT,
    -- The literal expressions the model saw, kept so a card can say "Wed, time
    -- TBD" and cite what was actually written rather than a computed guess.
    day_ref              TEXT,
    time_ref             TEXT,
    summary              TEXT,
    -- Kind-specific JSON: `split` carries the second group, `fix` the weekday
    -- and HH:MM it would create. Keeps the table stable as kinds are added.
    payload              TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS amendments_proposal
    ON amendments (proposal_message_id);
CREATE INDEX IF NOT EXISTS amendments_status ON amendments (status, created_at);

CREATE TABLE IF NOT EXISTS runs (
    id           TEXT PRIMARY KEY,
    fixed_run_id TEXT,
    channel_id   TEXT,
    week_start   TEXT NOT NULL,
    bosses       TEXT NOT NULL,
    datetime     TEXT NOT NULL,
    participants TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'planned',
    source       TEXT NOT NULL DEFAULT 'fixed',
    created_at   TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS runs_fixed_week
    ON runs (fixed_run_id, week_start) WHERE fixed_run_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS runs_by_week ON runs (week_start);

CREATE TABLE IF NOT EXISTS rsvps (
    run_id  TEXT NOT NULL,
    user_id TEXT NOT NULL,
    state   TEXT NOT NULL,
    source  TEXT NOT NULL DEFAULT 'reaction',
    at      TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);

CREATE TABLE IF NOT EXISTS reminders (
    id         TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL,
    fire_at    TEXT NOT NULL,
    kind       TEXT NOT NULL,
    sent_at    TEXT,
    message_id TEXT,
    UNIQUE (run_id, kind)
);

CREATE INDEX IF NOT EXISTS reminders_pending ON reminders (sent_at, fire_at);
CREATE INDEX IF NOT EXISTS reminders_message ON reminders (message_id);

CREATE TABLE IF NOT EXISTS messages (
    id           TEXT PRIMARY KEY,
    channel_id   TEXT NOT NULL,
    author_id    TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    content      TEXT NOT NULL,
    processed_at TEXT
);

CREATE TABLE IF NOT EXISTS extractions (
    id            TEXT PRIMARY KEY,
    at            TEXT NOT NULL,
    model         TEXT NOT NULL,
    prompt        TEXT NOT NULL,
    raw_response  TEXT NOT NULL,
    latency_ms    INTEGER,
    message_ids   TEXT NOT NULL DEFAULT '[]',
    amendment_ids TEXT NOT NULL DEFAULT '[]'
);

-- Test pings posted by /debug ping. Kept out of `reminders` so a test can never
-- satisfy or suppress a real scheduled ping, but recorded so ✅/❌ on a test
-- message still map back to the run and drive the real RSVP flow.
CREATE TABLE IF NOT EXISTS debug_messages (
    message_id TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL,
    channel_id TEXT,
    kind       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS debug_messages_run ON debug_messages (run_id);

-- One "X can't make it" notice per person per run, so a ❌ toggled on and off
-- (or spammed) never floods the channel; deleted again when they go ✅.
CREATE TABLE IF NOT EXISTS decline_notices (
    run_id      TEXT NOT NULL,
    user_id     TEXT NOT NULL,
    channel_id  TEXT,
    message_id  TEXT,
    notified_at TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);

CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- v4. One row per rescan asked for, so the portal can show the last few runs
-- after a restart. The live queue is in memory (bot/rescan.py); this is the
-- record of what was asked and what came of it.
CREATE TABLE IF NOT EXISTS rescan_jobs (
    id           TEXT PRIMARY KEY,
    channels     TEXT NOT NULL DEFAULT '[]',
    window       TEXT NOT NULL,
    source       TEXT NOT NULL DEFAULT 'manual',
    automated    INTEGER NOT NULL DEFAULT 0,
    requested_by TEXT,
    status       TEXT NOT NULL DEFAULT 'queued',
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT,
    results      TEXT NOT NULL DEFAULT '[]',
    error        TEXT
);

CREATE INDEX IF NOT EXISTS rescan_jobs_recent ON rescan_jobs (created_at DESC);

-- v6. One row per interaction the chatbot *handled*: a question that reached
-- the model and an answer (or a failure) that came back. A message the rate
-- limiter or the busy channel turned away gets no row -- nothing was asked of
-- the model, so there is nothing to account for, and counting those as
-- interactions would make every average here a lie.
--
-- The columns exist to answer "what is this costing us and where is it going
-- wrong": the model is configurable and currently remote, so tokens are money,
-- and the model/tool split is what says whether a slow answer was the model
-- thinking or the schedule being read. Pruned to CHAT_INTERACTIONS_KEPT rows.
CREATE TABLE IF NOT EXISTS chat_interactions (
    id                TEXT PRIMARY KEY,
    at                TEXT NOT NULL,
    channel_id        TEXT,
    message_id        TEXT,
    author_id         TEXT,
    model             TEXT NOT NULL DEFAULT '',
    question          TEXT NOT NULL DEFAULT '',
    reply             TEXT NOT NULL DEFAULT '',
    -- answered | failed. Nothing else is stored: see the table comment.
    outcome           TEXT NOT NULL DEFAULT 'answered',
    -- The generation's own words for what went wrong, when it did.
    error             TEXT,
    rounds            INTEGER NOT NULL DEFAULT 0,
    latency_ms        INTEGER,
    -- The split of `latency_ms`: time inside the model, time inside the tools.
    -- They do not add up to it (there is assembly either side); they are the
    -- two parts worth naming.
    model_ms          INTEGER,
    tools_ms          INTEGER,
    -- Summed across rounds, and null when the model did not report them:
    -- `prompt_eval_count` / `eval_count` are Ollama's, not the API's.
    prompt_tokens     INTEGER,
    completion_tokens INTEGER,
    -- [{name, arguments, ms, outcome, created}], one per call, in order.
    tool_calls        TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS chat_interactions_recent ON chat_interactions (at DESC);

-- v7. One row per change to the schedule, and the human behind it. The admin
-- plane is one shared token, so without this a portal edit is attributable to
-- "the portal" and nothing more -- which is no answer at all when the party
-- wants to know who moved Thursday. The actor is whoever the surface could
-- name: a tailnet login, a Discord user id, the operating-system user behind
-- `bossctl`, or the literal 'token' when the only credential was the token
-- itself. Pruned to AUDIT_KEPT rows on insert.
CREATE TABLE IF NOT EXISTS audit (
    id      TEXT PRIMARY KEY,
    at      TEXT NOT NULL,
    -- portal | cli | discord | chat | card | system (AUDIT_SURFACES).
    surface TEXT NOT NULL DEFAULT 'system',
    actor   TEXT NOT NULL DEFAULT 'token',
    -- A short verb: amend, cancel, rsvp, fixed_add, config, swap, status...
    action  TEXT NOT NULL,
    -- What was changed: a run, fixed-run or amendment id, or a config key.
    subject TEXT,
    -- One sentence a person can read without looking anything else up.
    detail  TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS audit_recent ON audit (at DESC);

-- v8. Members whose chatbot allowance is not the guild default. Sparse on
-- purpose: a row exists only where somebody was given their own numbers, and
-- deleting it is what "back to the default" means.
--
-- Its own table rather than two columns on `members`, because `members` syncs
-- from the bossing role and this is a fact about the *chat* role -- somebody
-- can hold one without the other, and a roster row invented to hold an
-- allowance would then look like a bosser to everything that reads it.
--
-- The spent windows themselves stay in memory (bot/chat/ratelimit.py): a
-- restart forgetting who has asked what is the right trade, but forgetting that
-- somebody was granted a bigger allowance is not.
CREATE TABLE IF NOT EXISTS chat_rate_limits (
    user_id    TEXT PRIMARY KEY,
    count      INTEGER NOT NULL,
    window_s   REAL NOT NULL,
    updated_at TEXT NOT NULL
);
"""


#: Tables rebuilt by the v1->v2 migration. `members`, `messages` and `config`
#: carry real state and are left untouched.
_V2_REBUILT_TABLES = (
    "debug_messages",
    "reminders",
    "rsvps",
    "runs",
    "fixed_runs",
    "amendments",
    "extractions",
)

#: Columns of `fixed_runs` carried across the v1->v2 rebuild (everything but the id).
_V2_FIXED_RUN_COLUMNS = (
    "owner_id",
    "channel_id",
    "bosses",
    "weekday",
    "time",
    "participants",
    "note",
    "created_at",
)


def _migrate_1_to_2(conn: sqlite3.Connection) -> None:
    """Integer autoincrement ids -> uuid4 text.

    Rewriting every key and foreign key in place would be a lot of machinery for
    a young database, so the scheduling tables are rebuilt instead. The one
    thing worth keeping is `fixed_runs`: those are hand-entered baselines
    (bosses, day, time, participants, home channel) that would be tedious to
    re-enter. They are carried across with fresh uuids; `runs` and `reminders`
    are derived data and are rebuilt from them by the next materialisation.
    """
    try:
        saved = [
            tuple(row)
            for row in conn.execute(f"SELECT {', '.join(_V2_FIXED_RUN_COLUMNS)} FROM fixed_runs")
        ]
    except sqlite3.OperationalError:
        saved = []

    counts: dict[str, int] = {}
    for table in _V2_REBUILT_TABLES:
        try:
            counts[table] = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]  # noqa: S608
        except sqlite3.OperationalError:
            counts[table] = 0
        conn.execute(f"DROP TABLE IF EXISTS {table}")

    conn.executescript(SCHEMA_SQL)

    placeholders = ", ".join("?" * (len(_V2_FIXED_RUN_COLUMNS) + 1))
    for row in saved:
        conn.execute(
            f"INSERT INTO fixed_runs (id, {', '.join(_V2_FIXED_RUN_COLUMNS)}) "  # noqa: S608
            f"VALUES ({placeholders})",
            (new_id(), *row),
        )

    discarded = ", ".join(f"{t}={n}" for t, n in counts.items() if n and t != "fixed_runs")
    log.warning(
        "schema v1->v2: scheduling ids are now uuid4. Carried %d fixed run(s) across with "
        "new ids; rebuilt %s (derived rows discarded: %s). members, messages and config "
        "were preserved; runs and reminders regenerate on the next materialisation.",
        len(saved),
        ", ".join(_V2_REBUILT_TABLES),
        discarded or "none",
    )


#: Columns the chat extractor added to ``amendments`` in v3 (all additive).
_V3_AMENDMENT_COLUMNS: dict[str, str] = {
    "channel_id": "TEXT",
    "is_question": "INTEGER NOT NULL DEFAULT 0",
    "rsvp": "TEXT",
    "day_ref": "TEXT",
    "time_ref": "TEXT",
    "summary": "TEXT",
    "payload": "TEXT NOT NULL DEFAULT '{}'",
}


def _migrate_2_to_3(conn: sqlite3.Connection) -> None:
    """v2 -> v3: extractor columns on ``amendments``; ``decline_notices`` table.

    Purely additive: ``ALTER TABLE ... ADD COLUMN`` for anything missing, and the
    new table arrives via ``SCHEMA_SQL`` (``CREATE TABLE IF NOT EXISTS``) after
    the step. No data is touched.
    """
    existing = {row[1] for row in conn.execute("PRAGMA table_info(amendments)")}
    added = []
    for name, decl in _V3_AMENDMENT_COLUMNS.items():
        if name not in existing:
            conn.execute(f"ALTER TABLE amendments ADD COLUMN {name} {decl}")
            added.append(name)
    log.info("schema v2->v3: amendments gained %s; decline_notices table added", added or "nothing")


#: version -> the step that upgrades *from* that version to the next.
def _migrate_3_to_4(conn: sqlite3.Connection) -> None:
    """v4 only adds ``rescan_jobs``, which ``SCHEMA_SQL`` creates on its own.

    A numbered step is still needed so :meth:`Repo.migrate` can walk past v3;
    the work is the ``CREATE TABLE IF NOT EXISTS`` that runs afterwards.
    """


def _migrate_4_to_5(conn: sqlite3.Connection) -> None:
    """v4 -> v5: ``members.ping_level``, the per-person mention preference.

    Additive, and everyone starts on the default (``essential``), which is the
    behaviour the guild already had for the posts that ask them to act.
    ``SCHEMA_SQL`` cannot do this itself: the table already exists, so its
    ``CREATE TABLE IF NOT EXISTS`` is a no-op on an upgrade.
    """
    existing = {row[1] for row in conn.execute("PRAGMA table_info(members)")}
    if not existing:
        # No `members` table yet: a database old enough to predate it walks
        # through this step on its way forward, and `SCHEMA_SQL` creates the
        # table -- with the column already on it -- once the steps are done.
        return
    if "ping_level" in existing:
        return
    conn.execute(
        f"ALTER TABLE members ADD COLUMN ping_level TEXT NOT NULL DEFAULT '{DEFAULT_PING_LEVEL}'"
    )
    log.info("schema v4->v5: members gained ping_level (everyone on %s)", DEFAULT_PING_LEVEL)


def _migrate_5_to_6(conn: sqlite3.Connection) -> None:
    """v6 only adds ``chat_interactions``, which ``SCHEMA_SQL`` creates itself.

    A numbered step is still needed so :meth:`Repo.migrate` can walk past v5, as
    with v3->v4; the work is the ``CREATE TABLE IF NOT EXISTS`` afterwards.
    """


def _migrate_6_to_7(conn: sqlite3.Connection) -> None:
    """v7 only adds ``audit``, which ``SCHEMA_SQL`` creates itself.

    A numbered step is still needed so :meth:`Repo.migrate` can walk past v6, as
    with v3->v4 and v5->v6; the work is the ``CREATE TABLE IF NOT EXISTS``
    afterwards. Nothing is backfilled: the trail starts the day it is switched
    on, and inventing rows for changes nobody recorded would be worse than a
    short history.
    """


def _migrate_7_to_8(conn: sqlite3.Connection) -> None:
    """v8 only adds ``chat_rate_limits``, which ``SCHEMA_SQL`` creates itself.

    A numbered step is still needed so :meth:`Repo.migrate` can walk past v7, as
    with v3->v4, v5->v6 and v6->v7; the work is the ``CREATE TABLE IF NOT
    EXISTS`` afterwards. Nothing is backfilled, and nothing needs to be: no rows
    means everybody is on the guild default, which is what they were on before.
    """


MIGRATIONS: dict[int, Callable[[sqlite3.Connection], None]] = {
    1: _migrate_1_to_2,
    2: _migrate_2_to_3,
    3: _migrate_3_to_4,
    4: _migrate_4_to_5,
    5: _migrate_5_to_6,
    6: _migrate_6_to_7,
    7: _migrate_7_to_8,
}


def _json_list(value: str | None) -> list:
    if not value:
        return []
    return json.loads(value)


def _dump(value: Iterable[Any]) -> str:
    return json.dumps(list(value))


def _int_or_none(value: Any) -> int | None:
    return int(value) if value is not None else None


def _percentile(ordered: Sequence[int], fraction: float) -> int | None:
    """Nearest-rank percentile of an already-sorted list, or ``None`` if empty.

    Nearest-rank rather than interpolated: every value here is a real
    measurement of a real answer, and "the 95th percentile answer took 31
    seconds" is a sentence about one of them.
    """
    if not ordered:
        return None
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


class Repo:
    """Thin repository over a single SQLite connection."""

    #: Called with a run id whenever something a posted reminder card *shows*
    #: has changed: the RSVP tally, the run's status, or its line-up. The client
    #: sets it (:meth:`bot.client.BossBot.card_needs_refresh`) so a card already
    #: in a channel is re-rendered whoever changed the answer -- a reaction,
    #: `/rsvp`, the portal, a chat-extracted "Can". Hooking the writes rather
    #: than the callers is the only way to catch all four without every one of
    #: them having to remember; a caller that forgets is exactly how the tally
    #: on a live card froze at "2/4 ✅" while the database said 4/4.
    on_run_changed: Callable[[str], None] | None = None

    def __init__(self, path: str | Path):
        self.path = str(path)
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        # `check_same_thread=False`: the connection is created wherever the
        # process happens to start (``python -m bot`` builds it inside
        # ``asyncio.run``; a test builds it on the pytest thread) and then used
        # from the bot's event loop. There is exactly one connection and one
        # user of it at a time -- every HTTP handler in `bot.api` is `async def`
        # so it runs on that loop rather than in FastAPI's worker threadpool,
        # which `tests/test_api_app.py::test_every_route_is_async` enforces.
        self._conn = sqlite3.connect(self.path, isolation_level=None, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self.migrate()

    # -- lifecycle --------------------------------------------------------
    def migrate(self) -> None:
        """Bring the database up to :data:`SCHEMA_VERSION`.

        A fresh file is created at the latest shape. An existing one is walked
        forward one numbered step at a time by :data:`MIGRATIONS`, then
        ``SCHEMA_SQL`` runs to add any table introduced since.
        """
        self._conn.execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        row = self._conn.execute("SELECT version FROM schema_version").fetchone()

        if row is None:
            self._conn.executescript(SCHEMA_SQL)
            self._conn.execute("DELETE FROM schema_version")
            self._conn.execute("INSERT INTO schema_version (version) VALUES (?)", (SCHEMA_VERSION,))
            return

        current = int(row["version"])
        if current > SCHEMA_VERSION:
            raise RuntimeError(
                f"database at {self.path} was written by a newer bot "
                f"(schema v{current} > v{SCHEMA_VERSION})"
            )
        while current < SCHEMA_VERSION:
            step = MIGRATIONS.get(current)
            if step is None:  # pragma: no cover - defensive
                raise RuntimeError(f"no migration from schema v{current}")
            log.info("migrating database %s: v%d -> v%d", self.path, current, current + 1)
            step(self._conn)
            current += 1
            self._conn.execute("UPDATE schema_version SET version = ?", (current,))
        self._conn.executescript(SCHEMA_SQL)

    def close(self) -> None:
        self._conn.close()

    def backup_to(self, path: str | Path) -> None:
        """Write a consistent snapshot of the whole database to ``path``.

        SQLite's online backup API, not a file copy: in WAL mode the ``.sqlite``
        file on its own is only whatever was last checkpointed, so copying it
        while the bot is writing can produce a file that will not open. This
        copies page by page against the live connection and takes the WAL with
        it. See :mod:`bot.backup` for when it is called.

        The copy is left in rollback-journal mode rather than the WAL mode it
        inherits, so the snapshot is one self-contained file: opening it to look
        at it does not litter the backups directory with ``-wal``/``-shm``
        siblings, and restoring it is a copy of exactly one path.
        """
        dest = sqlite3.connect(str(path))
        try:
            with dest:
                self._conn.backup(dest)
            dest.execute("PRAGMA journal_mode=DELETE")
        finally:
            dest.close()
        # Leaving WAL takes the -wal with it but not always the -shm, and both
        # belong to a file written seconds ago by this call alone.
        for sibling in (f"{path}-wal", f"{path}-shm"):
            Path(sibling).unlink(missing_ok=True)

    # -- config -----------------------------------------------------------
    def get_config(self, key: str, default: str | None = None) -> str | None:
        row = self._conn.execute("SELECT value FROM config WHERE key = ?", (key,)).fetchone()
        return row["value"] if row else default

    def set_config(self, key: str, value: str) -> None:
        self._conn.execute(
            "INSERT INTO config (key, value) VALUES (?, ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, str(value)),
        )

    def seed_config(self, defaults: dict[str, str]) -> None:
        """Insert values only if the key is absent, so runtime edits survive restarts."""
        for key, value in defaults.items():
            self._conn.execute(
                "INSERT OR IGNORE INTO config (key, value) VALUES (?, ?)", (key, str(value))
            )

    def heartbeat(self, now: datetime | None = None) -> None:
        self.set_config("heartbeat", to_iso(now or utcnow()))

    # -- members ----------------------------------------------------------
    def upsert_member(
        self,
        user_id: int | str,
        display_name: str,
        nickname: str | None,
        has_role: bool,
    ) -> None:
        self._conn.execute(
            """
            INSERT INTO members (user_id, display_name, nickname, aliases, has_role, updated_at)
            VALUES (?, ?, ?, '[]', ?, ?)
            ON CONFLICT(user_id) DO UPDATE SET
                display_name = excluded.display_name,
                nickname     = excluded.nickname,
                has_role     = excluded.has_role,
                updated_at   = excluded.updated_at
            """,
            (str(user_id), display_name, nickname, int(has_role), to_iso(utcnow())),
        )

    def sync_roster(self, members: Sequence[tuple[int | str, str, str | None, bool]]) -> None:
        """Upsert every supplied member, then clear ``has_role`` on everyone else."""
        seen = [str(m[0]) for m in members]
        for user_id, display_name, nickname, has_role in members:
            self.upsert_member(user_id, display_name, nickname, has_role)
        placeholders = ",".join("?" * len(seen)) if seen else "''"
        self._conn.execute(
            f"UPDATE members SET has_role = 0, updated_at = ? "  # noqa: S608 - ids are ours
            f"WHERE has_role = 1 AND user_id NOT IN ({placeholders})",
            (to_iso(utcnow()), *seen),
        )

    def get_member(self, user_id: int | str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM members WHERE user_id = ?", (str(user_id),)
        ).fetchone()
        return self._member(row) if row else None

    def list_members(self, with_role: bool = True) -> list[dict]:
        sql = "SELECT * FROM members"
        if with_role:
            sql += " WHERE has_role = 1"
        sql += " ORDER BY display_name COLLATE NOCASE"
        return [self._member(r) for r in self._conn.execute(sql)]

    def add_alias(self, user_id: int | str, alias: str) -> list[str]:
        member = self.get_member(user_id)
        aliases = list(member["aliases"]) if member else []
        if alias not in aliases:
            aliases.append(alias)
        if member is None:
            self.upsert_member(user_id, alias, None, False)
        self._conn.execute(
            "UPDATE members SET aliases = ?, updated_at = ? WHERE user_id = ?",
            (_dump(aliases), to_iso(utcnow()), str(user_id)),
        )
        return aliases

    def has_role(self, user_id: int | str) -> bool:
        member = self.get_member(user_id)
        return bool(member and member["has_role"])

    def get_ping_level(self, user_id: int | str) -> str:
        """How much this person wants to be @mentioned.

        Somebody the roster has never seen (a guest on a run, a member who left)
        gets the default rather than an error: a missing row must never stop a
        reminder going out.
        """
        member = self.get_member(user_id)
        return member["ping_level"] if member else DEFAULT_PING_LEVEL

    def set_ping_level(self, user_id: int | str, level: str) -> str:
        """Record a member's mention preference; returns the level that was stored."""
        if level not in PING_LEVELS:
            raise ValueError(f"ping level must be one of {', '.join(PING_LEVELS)}, not {level!r}")
        updated = self._conn.execute(
            "UPDATE members SET ping_level = ?, updated_at = ? WHERE user_id = ?",
            (level, to_iso(utcnow()), str(user_id)),
        ).rowcount
        if not updated:
            raise KeyError(str(user_id))
        return level

    @staticmethod
    def _member(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["aliases"] = _json_list(data["aliases"])
        data["has_role"] = bool(data["has_role"])
        data["ping_level"] = data.get("ping_level") or DEFAULT_PING_LEVEL
        return data

    # -- chatbot allowances ------------------------------------------------
    def set_rate_limit(self, user_id: int | str, count: int, window_s: float) -> None:
        """Give one member their own chatbot allowance, replacing any it had."""
        self._conn.execute(
            "INSERT INTO chat_rate_limits (user_id, count, window_s, updated_at) "
            "VALUES (?, ?, ?, ?) ON CONFLICT(user_id) DO UPDATE SET "
            "count = excluded.count, window_s = excluded.window_s, "
            "updated_at = excluded.updated_at",
            (str(user_id), int(count), float(window_s), to_iso(utcnow())),
        )

    def clear_rate_limit(self, user_id: int | str) -> bool:
        """Put one member back on the guild default. False if they were already."""
        cursor = self._conn.execute(
            "DELETE FROM chat_rate_limits WHERE user_id = ?", (str(user_id),)
        )
        return cursor.rowcount > 0

    def list_rate_limits(self) -> list[dict]:
        """Every member with their own allowance, for loading into the limiter."""
        rows = self._conn.execute(
            "SELECT * FROM chat_rate_limits ORDER BY updated_at DESC, user_id"
        )
        return [
            {
                "user_id": row["user_id"],
                "count": int(row["count"]),
                "window_s": float(row["window_s"]),
                "updated_at": row["updated_at"],
            }
            for row in rows
        ]

    # -- fixed runs -------------------------------------------------------
    def add_fixed_run(
        self,
        owner_id: int | str,
        bosses: Sequence[str],
        weekday: int,
        time_hhmm: str,
        participants: Sequence[int | str],
        note: str | None = None,
        channel_id: int | str | None = None,
    ) -> str:
        fixed_id = new_id()
        self._conn.execute(
            """
            INSERT INTO fixed_runs
                (id, owner_id, channel_id, bosses, weekday, time, participants, note, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                fixed_id,
                str(owner_id),
                str(channel_id) if channel_id is not None else None,
                _dump(bosses),
                int(weekday),
                time_hhmm,
                _dump(str(p) for p in participants),
                note,
                to_iso(utcnow()),
            ),
        )
        return fixed_id

    def get_fixed_run(self, fixed_run_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM fixed_runs WHERE id = ?", (fixed_run_id,)
        ).fetchone()
        return self._fixed(row) if row else None

    def list_fixed_runs(
        self, participant: str | None = None, involving: int | str | None = None
    ) -> list[dict]:
        """Every baseline timing, ordered by when it happens.

        ``participant`` narrows to runs someone is *on*.  ``involving`` is the
        wider "mine": the runs someone is on **or owns**.  Setting a party's
        timing up does not put you on it (`/fixed add` deliberately does not add
        the invoker), so filtering on participation alone hides a pilot's or an
        admin's own timings from them, which reads as data loss.
        """
        if involving is not None:
            rows = self._conn.execute(
                """
                SELECT * FROM fixed_runs
                WHERE owner_id = :uid
                   OR EXISTS (SELECT 1 FROM json_each(fixed_runs.participants) WHERE value = :uid)
                ORDER BY weekday, time, id
                """,
                {"uid": str(involving)},
            )
            return [self._fixed(r) for r in rows]
        rows = self._conn.execute("SELECT * FROM fixed_runs ORDER BY weekday, time, id")
        out = [self._fixed(r) for r in rows]
        if participant is not None:
            out = [f for f in out if str(participant) in f["participants"]]
        return out

    def update_fixed_run(self, fixed_run_id: str, **fields: Any) -> None:
        allowed = {"bosses", "weekday", "time", "participants", "note", "owner_id", "channel_id"}
        sets, values = [], []
        for key, value in fields.items():
            if key not in allowed or value is None:
                continue
            if key in ("bosses", "participants"):
                value = _dump(str(v) for v in value) if key == "participants" else _dump(value)
            sets.append(f"{key} = ?")
            values.append(value)
        if not sets:
            return
        values.append(fixed_run_id)
        self._conn.execute(f"UPDATE fixed_runs SET {', '.join(sets)} WHERE id = ?", values)

    def delete_fixed_run(self, fixed_run_id: str) -> None:
        self._conn.execute("DELETE FROM fixed_runs WHERE id = ?", (fixed_run_id,))

    @staticmethod
    def _fixed(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["bosses"] = _json_list(data["bosses"])
        data["participants"] = _json_list(data["participants"])
        return data

    # -- runs -------------------------------------------------------------
    def create_run(
        self,
        week_start: datetime,
        bosses: Sequence[str],
        run_at: datetime,
        participants: Sequence[int | str],
        status: str = "planned",
        source: str = "fixed",
        fixed_run_id: str | None = None,
        channel_id: int | str | None = None,
    ) -> str:
        run_id = new_id()
        self._conn.execute(
            """
            INSERT INTO runs
                (id, fixed_run_id, channel_id, week_start, bosses, datetime, participants,
                 status, source, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                run_id,
                fixed_run_id,
                str(channel_id) if channel_id is not None else None,
                to_iso(week_start),
                _dump(bosses),
                to_iso(run_at),
                _dump(str(p) for p in participants),
                status,
                source,
                to_iso(utcnow()),
            ),
        )
        return run_id

    def get_run(self, run_id: str) -> dict | None:
        row = self._conn.execute("SELECT * FROM runs WHERE id = ?", (run_id,)).fetchone()
        return self._run(row) if row else None

    def run_for_fixed(self, fixed_run_id: str, week_start: datetime) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM runs WHERE fixed_run_id = ? AND week_start = ?",
            (fixed_run_id, to_iso(week_start)),
        ).fetchone()
        return self._run(row) if row else None

    def list_runs(
        self,
        week_start: datetime | None = None,
        participant: str | None = None,
        channel_id: int | str | None = None,
        include_cancelled: bool = True,
        involving: int | str | None = None,
        statuses: Sequence[str] | None = None,
    ) -> list[dict]:
        """Runs, newest-slot last.

        ``participant`` is "on this run"; ``involving`` also counts runs
        materialised from a fixed timing the person owns, which is what "mine"
        means everywhere a human reads it.  ``statuses`` keeps only those
        statuses -- the callers that hide `done`/`cancelled` pass it.
        """
        sql = "SELECT * FROM runs"
        params: list[Any] = []
        if week_start is not None:
            sql += " WHERE week_start = ?"
            params.append(to_iso(week_start))
        sql += " ORDER BY datetime, id"
        runs = [self._run(r) for r in self._conn.execute(sql, params)]
        if participant is not None:
            runs = [r for r in runs if str(participant) in r["participants"]]
        if involving is not None:
            uid = str(involving)
            owned = {f["id"] for f in self.list_fixed_runs(involving=uid)}
            runs = [
                r for r in runs if uid in r["participants"] or (r["fixed_run_id"] or "") in owned
            ]
        if channel_id is not None:
            runs = [r for r in runs if r["channel_id"] == str(channel_id)]
        if not include_cancelled:
            runs = [r for r in runs if r["status"] != "cancelled"]
        if statuses is not None:
            wanted = set(statuses)
            runs = [r for r in runs if r["status"] in wanted]
        return runs

    def _run_changed(self, run_id: str) -> None:
        """Tell whoever is listening that a card about this run is now out of date."""
        if self.on_run_changed is None:
            return
        try:
            self.on_run_changed(str(run_id))
        except Exception:
            # A stale card is a cosmetic problem; failing the write that caused
            # it would be a real one.
            log.exception("the run-changed hook failed for run %s", run_id)

    def set_run_status(self, run_id: str, status: str) -> None:
        if status not in RUN_STATUSES:
            raise ValueError(f"unknown run status {status!r}")
        self._conn.execute("UPDATE runs SET status = ? WHERE id = ?", (status, run_id))
        self._run_changed(run_id)

    def set_run_datetime(self, run_id: str, run_at: datetime, week_start: datetime) -> None:
        self._conn.execute(
            "UPDATE runs SET datetime = ?, week_start = ? WHERE id = ?",
            (to_iso(run_at), to_iso(week_start), run_id),
        )

    def set_run_channel(self, run_id: str, channel_id: int | str | None) -> None:
        self._conn.execute(
            "UPDATE runs SET channel_id = ? WHERE id = ?",
            (str(channel_id) if channel_id is not None else None, run_id),
        )

    def set_run_fixed(self, run_id: str, fixed_run_id: str | None) -> None:
        """Link a run to the weekly timing that now produces it, or unlink it.

        The partial unique index on ``(fixed_run_id, week_start)`` is the guard:
        a week cannot end up with two runs claiming the same timing, whichever
        route wrote the second one.
        """
        self._conn.execute("UPDATE runs SET fixed_run_id = ? WHERE id = ?", (fixed_run_id, run_id))

    def set_run_bosses(self, run_id: str, bosses: Sequence[str]) -> None:
        self._conn.execute("UPDATE runs SET bosses = ? WHERE id = ?", (_dump(bosses), run_id))

    def set_run_participants(self, run_id: str, participants: Sequence[int | str]) -> None:
        self._conn.execute(
            "UPDATE runs SET participants = ? WHERE id = ?",
            (_dump(str(p) for p in participants), run_id),
        )
        self._run_changed(run_id)

    @staticmethod
    def _run(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["bosses"] = _json_list(data["bosses"])
        data["participants"] = _json_list(data["participants"])
        data["datetime"] = from_iso(data["datetime"])
        data["week_start"] = from_iso(data["week_start"])
        return data

    # -- rsvps ------------------------------------------------------------
    def set_rsvp(
        self, run_id: str, user_id: int | str, state: str, source: str = "reaction"
    ) -> None:
        if state not in RSVP_STATES:
            raise ValueError(f"unknown rsvp state {state!r}")
        self._conn.execute(
            """
            INSERT INTO rsvps (run_id, user_id, state, source, at) VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(run_id, user_id) DO UPDATE SET
                state = excluded.state, source = excluded.source, at = excluded.at
            """,
            (run_id, str(user_id), state, source, to_iso(utcnow())),
        )
        self._run_changed(run_id)

    def clear_rsvp(self, run_id: str, user_id: int | str) -> None:
        self._conn.execute(
            "DELETE FROM rsvps WHERE run_id = ? AND user_id = ?", (run_id, str(user_id))
        )
        self._run_changed(run_id)

    def get_rsvps(self, run_id: str) -> dict[str, str]:
        rows = self._conn.execute("SELECT user_id, state FROM rsvps WHERE run_id = ?", (run_id,))
        return {r["user_id"]: r["state"] for r in rows}

    # -- reminders --------------------------------------------------------
    def add_reminder(
        self, run_id: str, kind: str, fire_at: datetime, sent_at: datetime | None = None
    ) -> str | None:
        """Insert a reminder, ignoring the insert if ``(run_id, kind)`` already exists."""
        reminder_id = new_id()
        cur = self._conn.execute(
            "INSERT OR IGNORE INTO reminders (id, run_id, kind, fire_at, sent_at) "
            "VALUES (?, ?, ?, ?, ?)",
            (reminder_id, run_id, kind, to_iso(fire_at), to_iso(sent_at) if sent_at else None),
        )
        return reminder_id if cur.rowcount else None

    def get_reminder(self, reminder_id: str) -> dict | None:
        row = self._conn.execute("SELECT * FROM reminders WHERE id = ?", (reminder_id,)).fetchone()
        return self._reminder(row) if row else None

    def list_reminders(self, run_id: str) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM reminders WHERE run_id = ? ORDER BY fire_at", (run_id,)
        )
        return [self._reminder(r) for r in rows]

    def due_reminders(self, now: datetime) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM reminders WHERE sent_at IS NULL AND fire_at <= ? ORDER BY fire_at, id",
            (to_iso(now),),
        )
        return [self._reminder(r) for r in rows]

    def unsent_reminders(self, kind: str | None = None) -> list[dict]:
        sql = "SELECT * FROM reminders WHERE sent_at IS NULL"
        params: list[Any] = []
        if kind is not None:
            sql += " AND kind = ?"
            params.append(kind)
        return [self._reminder(r) for r in self._conn.execute(sql + " ORDER BY fire_at", params)]

    def delete_reminders(self, run_id: str) -> None:
        """Drop every reminder for a run, sent ones included."""
        self._conn.execute("DELETE FROM reminders WHERE run_id = ?", (run_id,))

    def delete_unsent_reminders(self, run_id: str, keep_kinds: Sequence[str] = ()) -> None:
        params: list[Any] = [run_id]
        sql = "DELETE FROM reminders WHERE run_id = ? AND sent_at IS NULL"
        if keep_kinds:
            sql += f" AND kind NOT IN ({','.join('?' * len(keep_kinds))})"
            params.extend(keep_kinds)
        self._conn.execute(sql, params)

    def set_reminder_fire_at(self, reminder_id: str, fire_at: datetime) -> None:
        self._conn.execute(
            "UPDATE reminders SET fire_at = ? WHERE id = ?", (to_iso(fire_at), reminder_id)
        )

    def mark_reminder_sent(
        self, reminder_id: str, message_id: int | str | None = None, at: datetime | None = None
    ) -> None:
        self._conn.execute(
            "UPDATE reminders SET sent_at = ?, message_id = ? WHERE id = ?",
            (to_iso(at or utcnow()), str(message_id) if message_id else None, reminder_id),
        )

    def reminders_by_message(self, message_id: int | str) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM reminders WHERE message_id = ?", (str(message_id),)
        )
        return [self._reminder(r) for r in rows]

    @staticmethod
    def _reminder(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["fire_at"] = from_iso(data["fire_at"])
        data["sent_at"] = from_iso(data["sent_at"]) if data["sent_at"] else None
        return data

    # -- debug test messages ----------------------------------------------
    def add_debug_message(
        self, message_id: int | str, run_id: str, channel_id: int | str | None, kind: str
    ) -> None:
        self._conn.execute(
            """
            INSERT OR REPLACE INTO debug_messages
                (message_id, run_id, channel_id, kind, created_at)
            VALUES (?, ?, ?, ?, ?)
            """,
            (
                str(message_id),
                run_id,
                str(channel_id) if channel_id is not None else None,
                kind,
                to_iso(utcnow()),
            ),
        )

    def debug_messages_for_run(self, run_id: str) -> list[dict]:
        """Test cards posted for one run, oldest first.

        A `/debug ping` is a real card carrying a real tally -- its ✅/❌ drive
        the genuine RSVP flow -- so it goes stale like any other and has to be
        re-rendered with it.
        """
        rows = self._conn.execute(
            "SELECT * FROM debug_messages WHERE run_id = ? ORDER BY created_at", (run_id,)
        )
        return [dict(r) for r in rows]

    def debug_messages_for(self, message_id: int | str) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM debug_messages WHERE message_id = ?", (str(message_id),)
        )
        return [dict(r) for r in rows]

    def recent_debug_messages(
        self, since: datetime, channel_id: int | str | None = None
    ) -> list[dict]:
        sql = "SELECT * FROM debug_messages WHERE created_at >= ?"
        params: list[Any] = [to_iso(since)]
        if channel_id is not None:
            sql += " AND channel_id = ?"
            params.append(str(channel_id))
        return [dict(r) for r in self._conn.execute(sql, params)]

    def delete_debug_message(self, message_id: int | str) -> None:
        self._conn.execute("DELETE FROM debug_messages WHERE message_id = ?", (str(message_id),))

    # -- messages (phase 2 groundwork) ------------------------------------
    # -- decline notices ---------------------------------------------------
    def get_decline_notice(self, run_id: str, user_id: int | str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM decline_notices WHERE run_id = ? AND user_id = ?",
            (run_id, str(user_id)),
        ).fetchone()
        if row is None:
            return None
        data = dict(row)
        data["notified_at"] = from_iso(data["notified_at"])
        return data

    def set_decline_notice(
        self,
        run_id: str,
        user_id: int | str,
        message_id: int | str | None,
        channel_id: int | str | None,
        at: datetime | None = None,
    ) -> None:
        self._conn.execute(
            "INSERT INTO decline_notices (run_id, user_id, channel_id, message_id, notified_at) "
            "VALUES (?, ?, ?, ?, ?) ON CONFLICT(run_id, user_id) DO UPDATE SET "
            "channel_id = excluded.channel_id, message_id = excluded.message_id, "
            "notified_at = excluded.notified_at",
            (
                run_id,
                str(user_id),
                str(channel_id) if channel_id else None,
                str(message_id) if message_id else None,
                to_iso(at or utcnow()),
            ),
        )

    def clear_decline_notice_message(self, run_id: str, user_id: int | str) -> None:
        """Forget the posted message (it was deleted) but keep the timestamp for the cooldown."""
        self._conn.execute(
            "UPDATE decline_notices SET message_id = NULL WHERE run_id = ? AND user_id = ?",
            (run_id, str(user_id)),
        )

    def record_message(
        self,
        message_id: int | str,
        channel_id: int | str,
        author_id: int | str,
        created_at: datetime,
        content: str,
    ) -> None:
        self._conn.execute(
            """
            INSERT OR IGNORE INTO messages (id, channel_id, author_id, created_at, content)
            VALUES (?, ?, ?, ?, ?)
            """,
            (
                str(message_id),
                str(channel_id),
                str(author_id),
                to_iso(created_at),
                content,
            ),
        )

    def get_message(self, message_id: int | str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM messages WHERE id = ?", (str(message_id),)
        ).fetchone()
        return self._message(row) if row else None

    def recent_messages(
        self,
        channel_id: int | str,
        since: datetime,
        until: datetime | None = None,
        unprocessed_only: bool = False,
        limit: int | None = None,
    ) -> list[dict]:
        """One channel's stored messages in ``[since, until)``, oldest first.

        ``limit`` keeps the *newest* rows (the tail of the window), because that
        is what a prompt wants when a channel has been busy.
        """
        sql = "SELECT * FROM messages WHERE channel_id = ? AND created_at >= ?"
        params: list[Any] = [str(channel_id), to_iso(since)]
        if until is not None:
            sql += " AND created_at < ?"
            params.append(to_iso(until))
        if unprocessed_only:
            sql += " AND processed_at IS NULL"
        sql += " ORDER BY created_at, id"
        rows = [self._message(r) for r in self._conn.execute(sql, params)]
        return rows[-limit:] if limit is not None and limit >= 0 else rows

    def mark_messages_processed(
        self, message_ids: Sequence[int | str], at: datetime | None = None
    ) -> None:
        stamp = to_iso(at or utcnow())
        self._conn.executemany(
            "UPDATE messages SET processed_at = ? WHERE id = ?",
            [(stamp, str(mid)) for mid in message_ids],
        )

    @staticmethod
    def _message(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["created_at"] = from_iso(data["created_at"])
        data["processed_at"] = from_iso(data["processed_at"]) if data["processed_at"] else None
        return data

    # -- amendments (the chat extractor's proposals) -----------------------
    def create_amendment(
        self,
        week_start: datetime,
        kind: str,
        bosses: Sequence[str] = (),
        run_id: str | None = None,
        new_datetime: datetime | None = None,
        participants: Sequence[int | str] = (),
        confidence: float | None = None,
        evidence_msg_ids: Sequence[int | str] = (),
        channel_id: int | str | None = None,
        is_question: bool = False,
        rsvp: str | None = None,
        day_ref: str | None = None,
        time_ref: str | None = None,
        summary: str | None = None,
        payload: dict | None = None,
        status: str = "proposed",
    ) -> str:
        if status not in AMENDMENT_STATUSES:
            raise ValueError(f"unknown amendment status {status!r}")
        amendment_id = new_id()
        self._conn.execute(
            """
            INSERT INTO amendments
                (id, week_start, kind, bosses, run_id, new_datetime, participants, status,
                 confidence, evidence_msg_ids, proposal_message_id, created_at, channel_id,
                 is_question, rsvp, day_ref, time_ref, summary, payload)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                amendment_id,
                to_iso(week_start),
                kind,
                _dump(bosses),
                run_id,
                to_iso(new_datetime) if new_datetime else None,
                _dump(str(p) for p in participants),
                status,
                confidence,
                _dump(str(m) for m in evidence_msg_ids),
                to_iso(utcnow()),
                str(channel_id) if channel_id is not None else None,
                int(bool(is_question)),
                rsvp,
                day_ref,
                time_ref,
                summary,
                json.dumps(payload or {}),
            ),
        )
        return amendment_id

    def get_amendment(self, amendment_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM amendments WHERE id = ?", (amendment_id,)
        ).fetchone()
        return self._amendment(row) if row else None

    def amendments_by_message(self, message_id: int | str) -> list[dict]:
        """Every amendment on one proposal card -- a burst posts one card for all of them."""
        rows = self._conn.execute(
            "SELECT * FROM amendments WHERE proposal_message_id = ? ORDER BY created_at, id",
            (str(message_id),),
        )
        return [self._amendment(r) for r in rows]

    def list_amendments(
        self,
        status: str | None = None,
        channel_id: int | str | None = None,
        week_start: datetime | None = None,
    ) -> list[dict]:
        sql = "SELECT * FROM amendments"
        clauses: list[str] = []
        params: list[Any] = []
        if status is not None:
            clauses.append("status = ?")
            params.append(status)
        if channel_id is not None:
            clauses.append("channel_id = ?")
            params.append(str(channel_id))
        if week_start is not None:
            clauses.append("week_start = ?")
            params.append(to_iso(week_start))
        if clauses:
            sql += " WHERE " + " AND ".join(clauses)
        sql += " ORDER BY created_at, id"
        return [self._amendment(r) for r in self._conn.execute(sql, params)]

    def set_amendment_status(self, amendment_id: str, status: str) -> None:
        if status not in AMENDMENT_STATUSES:
            raise ValueError(f"unknown amendment status {status!r}")
        self._conn.execute("UPDATE amendments SET status = ? WHERE id = ?", (status, amendment_id))

    def set_amendment_proposal_message(
        self, amendment_id: str, message_id: int | str | None
    ) -> None:
        self._conn.execute(
            "UPDATE amendments SET proposal_message_id = ? WHERE id = ?",
            (str(message_id) if message_id is not None else None, amendment_id),
        )

    def set_amendment_datetime(self, amendment_id: str, new_datetime: datetime | None) -> None:
        """Overwrite the instant a proposal would move a run to.

        Used by the portal's "edit, then approve": the reader corrects the time
        the extractor read, and the correction -- not the model's value -- is
        what `commit()` then applies.
        """
        self._conn.execute(
            "UPDATE amendments SET new_datetime = ? WHERE id = ?",
            (to_iso(new_datetime) if new_datetime else None, amendment_id),
        )

    def set_amendment_run(self, amendment_id: str, run_id: str | None) -> None:
        self._conn.execute("UPDATE amendments SET run_id = ? WHERE id = ?", (run_id, amendment_id))

    def claim_chat_followup(self, amendment_id: str, at: datetime | None = None) -> bool:
        """Take the right to follow up on this card, once and for all.

        ``True`` the first time it is asked about a card and ``False`` for ever
        after, so a member who reacts ❌, un-reacts and reacts again gets one
        question rather than three -- and two ❌ arriving together produce one
        between them.

        The check and the set are one statement, so there is no window between
        them to lose: the ``WHERE`` clause *is* the check. It lives in
        :data:`CHAT_FOLLOWUP_KEY` inside the amendment's own ``payload`` rather
        than in a table of its own, because it is a fact about one card, it dies
        with that card, and a marker in a side table is a thing to forget to
        delete. Nothing else reads the key: the commit handlers take only the
        keys their kind writes.
        """
        # `json_valid` guards the one row shape this cannot assume: `payload` is
        # NOT NULL DEFAULT '{}', but a v1 database rebuilt by hand could hold
        # something else, and a JSON error here would unwind into a rejection.
        payload = "CASE WHEN json_valid(payload) THEN payload ELSE '{}' END"
        cursor = self._conn.execute(
            f"""
            UPDATE amendments
               SET payload = json_set({payload}, '$.{CHAT_FOLLOWUP_KEY}', ?)
             WHERE id = ?
               AND json_extract({payload}, '$.{CHAT_FOLLOWUP_KEY}') IS NULL
            """,  # noqa: S608 - `payload` is a literal above, not caller input
            (to_iso(at or utcnow()), amendment_id),
        )
        return bool(cursor.rowcount)

    def proposed_for_run(self, run_id: str, exclude: str | None = None) -> list[dict]:
        """Still-`proposed` amendments targeting one run, newest last."""
        rows = self._conn.execute(
            "SELECT * FROM amendments WHERE status = 'proposed' AND run_id = ? "
            "ORDER BY created_at, id",
            (run_id,),
        )
        return [self._amendment(r) for r in rows if r["id"] != exclude]

    def proposed_for_bosses(
        self, channel_id: int | str, bosses: Sequence[str], exclude: str | None = None
    ) -> list[dict]:
        """Still-`proposed` amendments in one channel that create the same thing.

        Used for `add`/`fix`, which have no run to key on yet: two cards for the
        same new run in the same channel are the same proposal.
        """
        wanted = {str(b) for b in bosses}
        rows = self._conn.execute(
            "SELECT * FROM amendments WHERE status = 'proposed' AND channel_id = ? "
            "AND run_id IS NULL ORDER BY created_at, id",
            (str(channel_id),),
        )
        return [
            row
            for row in (self._amendment(r) for r in rows)
            if row["id"] != exclude and {str(b) for b in row["bosses"]} == wanted
        ]

    def stale_amendments(self, before: datetime, status: str = "proposed") -> list[dict]:
        """Proposals older than ``before`` -- the tick expires these."""
        rows = self._conn.execute(
            "SELECT * FROM amendments WHERE status = ? AND created_at < ? ORDER BY created_at",
            (status, to_iso(before)),
        )
        return [self._amendment(r) for r in rows]

    @staticmethod
    def _amendment(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["bosses"] = _json_list(data["bosses"])
        data["participants"] = _json_list(data["participants"])
        data["evidence_msg_ids"] = _json_list(data["evidence_msg_ids"])
        data["payload"] = json.loads(data["payload"] or "{}")
        data["is_question"] = bool(data["is_question"])
        data["week_start"] = from_iso(data["week_start"])
        data["created_at"] = from_iso(data["created_at"])
        data["new_datetime"] = from_iso(data["new_datetime"]) if data["new_datetime"] else None
        return data

    # -- extraction log ----------------------------------------------------
    def log_extraction(
        self,
        model: str,
        prompt: str,
        raw_response: str,
        latency_ms: int | None = None,
        message_ids: Sequence[int | str] = (),
        amendment_ids: Sequence[str] = (),
        at: datetime | None = None,
    ) -> str:
        """Record one model call. This is the prompt-tuning tool (DESIGN.md §5)."""
        extraction_id = new_id()
        self._conn.execute(
            """
            INSERT INTO extractions
                (id, at, model, prompt, raw_response, latency_ms, message_ids, amendment_ids)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                extraction_id,
                to_iso(at or utcnow()),
                model,
                prompt,
                raw_response,
                int(latency_ms) if latency_ms is not None else None,
                _dump(str(m) for m in message_ids),
                _dump(amendment_ids),
            ),
        )
        return extraction_id

    def set_extraction_amendments(self, extraction_id: str, amendment_ids: Sequence[str]) -> None:
        self._conn.execute(
            "UPDATE extractions SET amendment_ids = ? WHERE id = ?",
            (_dump(amendment_ids), extraction_id),
        )

    # -- rescan jobs -------------------------------------------------------
    def create_rescan_job(
        self,
        job_id: str,
        channels: Sequence[str],
        window: str,
        source: str = "manual",
        automated: bool = False,
        requested_by: int | str | None = None,
        at: datetime | None = None,
    ) -> str:
        """Record a rescan the moment it is asked for, before it runs."""
        self._conn.execute(
            """
            INSERT OR REPLACE INTO rescan_jobs
                (id, channels, window, source, automated, requested_by, status, created_at)
            VALUES (?, ?, ?, ?, ?, ?, 'queued', ?)
            """,
            (
                job_id,
                _dump(str(c) for c in channels),
                window,
                source,
                int(bool(automated)),
                str(requested_by) if requested_by is not None else None,
                to_iso(at or utcnow()),
            ),
        )
        return job_id

    def update_rescan_job(self, job_id: str, **fields: Any) -> None:
        allowed = {
            "status",
            "started_at",
            "finished_at",
            "results",
            "error",
            "channels",
            "window",
        }
        sets, values = [], []
        for key, value in fields.items():
            if key not in allowed:
                continue
            if key in ("started_at", "finished_at") and isinstance(value, datetime):
                value = to_iso(value)
            if key in ("results", "channels") and not isinstance(value, str):
                value = json.dumps(value)
            sets.append(f"{key} = ?")
            values.append(value)
        if not sets:
            return
        values.append(job_id)
        self._conn.execute(f"UPDATE rescan_jobs SET {', '.join(sets)} WHERE id = ?", values)

    def get_rescan_job(self, job_id: str) -> dict | None:
        row = self._conn.execute("SELECT * FROM rescan_jobs WHERE id = ?", (job_id,)).fetchone()
        return self._rescan_job(row) if row else None

    def recent_rescan_jobs(self, limit: int = 10) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM rescan_jobs ORDER BY created_at DESC, id DESC LIMIT ?", (int(limit),)
        )
        return [self._rescan_job(r) for r in rows]

    @staticmethod
    def _rescan_job(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["channels"] = _json_list(data["channels"])
        data["results"] = json.loads(data["results"] or "[]")
        data["automated"] = bool(data["automated"])
        data["created_at"] = from_iso(data["created_at"])
        for key in ("started_at", "finished_at"):
            data[key] = from_iso(data[key]) if data[key] else None
        return data

    def recent_extractions(self, limit: int = 20) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM extractions ORDER BY at DESC, id DESC LIMIT ?", (int(limit),)
        )
        return [self._extraction(row) for row in rows]

    def get_extraction(self, extraction_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM extractions WHERE id = ?", (extraction_id,)
        ).fetchone()
        return self._extraction(row) if row else None

    def list_extraction_ids(self) -> list[str]:
        """Every extraction id, newest first -- what a short-prefix lookup resolves against."""
        return [r["id"] for r in self._conn.execute("SELECT id FROM extractions ORDER BY at DESC")]

    @staticmethod
    def _extraction(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["at"] = from_iso(data["at"])
        data["message_ids"] = _json_list(data["message_ids"])
        data["amendment_ids"] = _json_list(data["amendment_ids"])
        return data

    # -- chat interactions -------------------------------------------------
    def log_chat_interaction(
        self,
        *,
        model: str,
        question: str,
        reply: str,
        outcome: str,
        rounds: int = 0,
        channel_id: int | str | None = None,
        message_id: int | str | None = None,
        author_id: int | str | None = None,
        error: str | None = None,
        latency_ms: int | None = None,
        model_ms: int | None = None,
        tools_ms: int | None = None,
        prompt_tokens: int | None = None,
        completion_tokens: int | None = None,
        tool_calls: Sequence[dict] = (),
        at: datetime | None = None,
        keep: int = CHAT_INTERACTIONS_KEPT,
    ) -> str:
        """Record one handled interaction, then prune the log back to ``keep``.

        Pruning on insert rather than on a timer for the same reason
        :mod:`bot.backup` prunes as it writes: the only moment the table is
        certainly growing is the moment something was added to it, and a
        separate sweep is one more thing that can quietly stop running.
        """
        interaction_id = new_id()
        self._conn.execute(
            """
            INSERT INTO chat_interactions
                (id, at, channel_id, message_id, author_id, model, question, reply, outcome,
                 error, rounds, latency_ms, model_ms, tools_ms, prompt_tokens,
                 completion_tokens, tool_calls)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                interaction_id,
                to_iso(at or utcnow()),
                str(channel_id) if channel_id is not None else None,
                str(message_id) if message_id is not None else None,
                str(author_id) if author_id is not None else None,
                model,
                question,
                reply,
                outcome,
                error,
                int(rounds),
                _int_or_none(latency_ms),
                _int_or_none(model_ms),
                _int_or_none(tools_ms),
                _int_or_none(prompt_tokens),
                _int_or_none(completion_tokens),
                json.dumps(list(tool_calls)),
            ),
        )
        self.prune_chat_interactions(keep)
        return interaction_id

    def prune_chat_interactions(self, keep: int = CHAT_INTERACTIONS_KEPT) -> int:
        """Delete all but the newest ``keep`` rows; returns how many went."""
        cursor = self._conn.execute(
            """
            DELETE FROM chat_interactions WHERE id NOT IN (
                SELECT id FROM chat_interactions ORDER BY at DESC, id DESC LIMIT ?
            )
            """,
            (max(int(keep), 0),),
        )
        return cursor.rowcount if cursor.rowcount and cursor.rowcount > 0 else 0

    def recent_chat_interactions(self, limit: int = 50) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM chat_interactions ORDER BY at DESC, id DESC LIMIT ?", (int(limit),)
        )
        return [self._chat_interaction(row) for row in rows]

    def get_chat_interaction(self, interaction_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM chat_interactions WHERE id = ?", (interaction_id,)
        ).fetchone()
        return self._chat_interaction(row) if row else None

    def chat_interaction_for_amendment(self, amendment_id: str) -> dict | None:
        """The interaction whose tool calls raised ``amendment_id``, if one did.

        This is the provenance the rejection follow-up runs on: a card the
        chatbot posted has an interaction here naming it in a tool call's
        ``created`` list, and a card the *extractor* raised has none at all. It
        also answers the other half of the question -- ``author_id`` is the
        member who asked for the card, which is the only person a follow-up is
        ever addressed to.

        The ``LIKE`` narrows the scan; the loop is what decides, because a
        substring of a JSON blob is not an id. Newest first, so a card somehow
        named twice is attributed to the interaction that last touched it.
        Nothing older than :data:`CHAT_INTERACTIONS_KEPT` interactions is
        findable, which is the same as saying a card outlived the log of how it
        came to exist -- proposals expire in a day and the log holds weeks.
        """
        wanted = str(amendment_id)
        rows = self._conn.execute(
            "SELECT * FROM chat_interactions WHERE tool_calls LIKE ? ORDER BY at DESC, id DESC",
            (f"%{wanted}%",),
        )
        for row in rows:
            interaction = self._chat_interaction(row)
            if any(wanted in (call.get("created") or []) for call in interaction["tool_calls"]):
                return interaction
        return None

    def list_chat_interaction_ids(self) -> list[str]:
        """Every interaction id, newest first -- what a short-prefix lookup resolves against."""
        return [
            r["id"] for r in self._conn.execute("SELECT id FROM chat_interactions ORDER BY at DESC")
        ]

    def count_chat_interactions(self) -> int:
        return int(self._conn.execute("SELECT COUNT(*) FROM chat_interactions").fetchone()[0])

    def chat_interaction_stats(self) -> list[dict]:
        """Per model: how many, how they went, how slow, and what they cost.

        Aggregated in Python rather than SQL because a percentile is not
        something SQLite computes, and the alternative -- an average alone --
        hides exactly the answer worth knowing about, the one that took forty
        seconds. The table is capped at :data:`CHAT_INTERACTIONS_KEPT` rows, so
        reading all of them costs less than the query planner would.
        """
        by_model: dict[str, dict] = {}
        for row in self._conn.execute(
            "SELECT model, outcome, latency_ms, prompt_tokens, completion_tokens "
            "FROM chat_interactions"
        ):
            stat = by_model.setdefault(
                row["model"] or "",
                {
                    "model": row["model"] or "",
                    "count": 0,
                    "answered": 0,
                    "failed": 0,
                    "prompt_tokens": 0,
                    "completion_tokens": 0,
                    "_latencies": [],
                },
            )
            stat["count"] += 1
            stat["answered" if row["outcome"] == "answered" else "failed"] += 1
            stat["prompt_tokens"] += int(row["prompt_tokens"] or 0)
            stat["completion_tokens"] += int(row["completion_tokens"] or 0)
            if row["latency_ms"] is not None:
                stat["_latencies"].append(int(row["latency_ms"]))

        stats = []
        for stat in by_model.values():
            latencies = sorted(stat.pop("_latencies"))
            stat["avg_latency_ms"] = round(sum(latencies) / len(latencies)) if latencies else None
            stat["p95_latency_ms"] = _percentile(latencies, 0.95)
            stats.append(stat)
        stats.sort(key=lambda s: (-s["count"], s["model"]))
        return stats

    @staticmethod
    def _chat_interaction(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["at"] = from_iso(data["at"])
        data["tool_calls"] = json.loads(data["tool_calls"] or "[]")
        return data

    # -- audit trail -------------------------------------------------------
    def log_audit(
        self,
        *,
        surface: str,
        actor: str,
        action: str,
        subject: str | None = None,
        detail: str = "",
        at: datetime | None = None,
        keep: int = AUDIT_KEPT,
    ) -> str:
        """Record one change and who made it, then prune back to ``keep``.

        Nothing here validates: an unknown surface is stored as it arrives.
        This is written on the way *out* of a mutation that has already
        happened, so raising would turn a bad label into a failed edit -- see
        :func:`bot.audit.record`, which is how every caller reaches this.

        Pruned on insert for the same reason
        :meth:`log_chat_interaction` is: the only moment the table is certainly
        growing is the moment something was added to it.
        """
        audit_id = new_id()
        self._conn.execute(
            """
            INSERT INTO audit (id, at, surface, actor, action, subject, detail)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            (
                audit_id,
                to_iso(at or utcnow()),
                str(surface),
                str(actor),
                str(action),
                str(subject) if subject is not None else None,
                detail or "",
            ),
        )
        self.prune_audit(keep)
        return audit_id

    def prune_audit(self, keep: int = AUDIT_KEPT) -> int:
        """Delete all but the newest ``keep`` rows; returns how many went."""
        cursor = self._conn.execute(
            """
            DELETE FROM audit WHERE id NOT IN (
                SELECT id FROM audit ORDER BY at DESC, id DESC LIMIT ?
            )
            """,
            (max(int(keep), 0),),
        )
        return cursor.rowcount if cursor.rowcount and cursor.rowcount > 0 else 0

    def list_audit(self, limit: int = 200) -> list[dict]:
        """The most recent changes, newest first -- what the Audit page lists."""
        rows = self._conn.execute(
            "SELECT * FROM audit ORDER BY at DESC, id DESC LIMIT ?", (int(limit),)
        )
        return [self._audit(row) for row in rows]

    def count_audit(self) -> int:
        return int(self._conn.execute("SELECT COUNT(*) FROM audit").fetchone()[0])

    @staticmethod
    def _audit(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["at"] = from_iso(data["at"])
        return data
