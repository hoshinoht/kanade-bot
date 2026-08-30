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
import sqlite3
from collections.abc import Callable, Iterable, Sequence
from datetime import datetime
from pathlib import Path
from typing import Any

from .ids import new_id
from .timeutil import from_iso, to_iso, utcnow

log = logging.getLogger(__name__)

SCHEMA_VERSION = 3

RUN_STATUSES = ("planned", "confirmed", "at_risk", "otot", "done", "cancelled")
RSVP_STATES = ("yes", "no", "maybe")
AMENDMENT_STATUSES = ("proposed", "confirmed", "rejected", "expired")

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
MIGRATIONS: dict[int, Callable[[sqlite3.Connection], None]] = {
    1: _migrate_1_to_2,
    2: _migrate_2_to_3,
}


def _json_list(value: str | None) -> list:
    if not value:
        return []
    return json.loads(value)


def _dump(value: Iterable[Any]) -> str:
    return json.dumps(list(value))


class Repo:
    """Thin repository over a single SQLite connection."""

    def __init__(self, path: str | Path):
        self.path = str(path)
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.path, isolation_level=None)
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

    @staticmethod
    def _member(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["aliases"] = _json_list(data["aliases"])
        data["has_role"] = bool(data["has_role"])
        return data

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

    def list_fixed_runs(self, participant: str | None = None) -> list[dict]:
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
    ) -> list[dict]:
        sql = "SELECT * FROM runs"
        params: list[Any] = []
        if week_start is not None:
            sql += " WHERE week_start = ?"
            params.append(to_iso(week_start))
        sql += " ORDER BY datetime, id"
        runs = [self._run(r) for r in self._conn.execute(sql, params)]
        if participant is not None:
            runs = [r for r in runs if str(participant) in r["participants"]]
        if channel_id is not None:
            runs = [r for r in runs if r["channel_id"] == str(channel_id)]
        if not include_cancelled:
            runs = [r for r in runs if r["status"] != "cancelled"]
        return runs

    def set_run_status(self, run_id: str, status: str) -> None:
        if status not in RUN_STATUSES:
            raise ValueError(f"unknown run status {status!r}")
        self._conn.execute("UPDATE runs SET status = ? WHERE id = ?", (status, run_id))

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

    def set_run_bosses(self, run_id: str, bosses: Sequence[str]) -> None:
        self._conn.execute("UPDATE runs SET bosses = ? WHERE id = ?", (_dump(bosses), run_id))

    def set_run_participants(self, run_id: str, participants: Sequence[int | str]) -> None:
        self._conn.execute(
            "UPDATE runs SET participants = ? WHERE id = ?",
            (_dump(str(p) for p in participants), run_id),
        )

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

    def clear_rsvp(self, run_id: str, user_id: int | str) -> None:
        self._conn.execute(
            "DELETE FROM rsvps WHERE run_id = ? AND user_id = ?", (run_id, str(user_id))
        )

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
