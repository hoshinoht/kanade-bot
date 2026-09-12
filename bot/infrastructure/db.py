"""SQLite-backed application storage."""

# ruff: noqa: E501 -- SQL schemas and statements remain readable as complete clauses.

from __future__ import annotations

import json
import logging
import math
import sqlite3
import uuid
from collections.abc import Callable, Iterable, Sequence
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

from bot.domain.bosses import BossParseError, BossTable
from bot.domain.ids import new_id
from bot.domain.timeutil import from_iso, to_iso, utcnow

log = logging.getLogger(__name__)

SCHEMA_VERSION = 13

CHAT_INTERACTIONS_KEPT = 500

MEMORY_SLOTS = (
    "answer_detail",
    "answer_format",
    "strategy_disclosure",
    "strategy_emphasis",
)
MEMORY_STATES = ("proposed", "active", "superseded", "rejected", "revoked", "expired")
MEMORY_ENROLLMENT_STATES = ("disabled", "pending_notice", "active", "opted_out")
MEMORY_EVENT_ACTIONS = (
    "enrollment_started",
    "notice_attempted",
    "enrollment_activated",
    "enrollment_disabled",
    "opted_out",
    "proposed",
    "approved",
    "rejected",
    "replaced",
    "revoked",
    "expired",
    "deleted",
    "forgot_all",
)
MEMORY_EVENT_REASONS = (
    "enroll",
    "notice",
    "opt_out",
    "proposal",
    "approval",
    "rejection",
    "replace",
    "disable",
    "expiry",
    "delete",
    "forget_all",
)
MEMORY_RETRIEVAL_REASONS = ("matched", "no_match", "inactive_enrollment", "unavailable")
MEMORY_PROPOSAL_TTL = timedelta(days=7)
MEMORY_ACTIVE_TTL = timedelta(days=180)
MEMORY_INACTIVE_RETENTION = timedelta(days=30)
MEMORY_EVENT_RETENTION = timedelta(days=365)
MEMORY_RETRIEVAL_RETENTION = timedelta(days=30)
MEMORY_RETRIEVAL_KEPT = 500

AUDIT_KEPT = 2000

#: Known audit sources; logging intentionally accepts unknown sources.
AUDIT_SURFACES = ("portal", "cli", "discord", "chat", "card", "system")

#: Persistent marker enforcing one chat follow-up per card.
CHAT_FOLLOWUP_KEY = "chat_followup_at"

RUN_STATUSES = ("planned", "confirmed", "at_risk", "otot", "done", "cancelled")
RSVP_STATES = ("yes", "no", "maybe")
#: How much a member wants to be @mentioned (DESIGN.md s3, "Mention policy").
#: `essential` is the default: only the posts that ask them to act.
PING_LEVELS = ("essential", "all", "off")
DEFAULT_PING_LEVEL = "essential"
#: Retained proposal states preserve extraction history.
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
    -- member-selected chatbot behaviour profile; NULL means the deployment default
    reply_style  TEXT,
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

-- One live digest card per boss week. Retired rows remain as the weekly log,
-- while only rows without retired_at continue to follow schedule changes.
CREATE TABLE IF NOT EXISTS weekly_digests (
    week_start TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    posted_at  TEXT NOT NULL,
    retired_at TEXT
);
CREATE INDEX IF NOT EXISTS weekly_digests_message ON weekly_digests (message_id);

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

-- Debug pings remain isolated from scheduled reminders.
CREATE TABLE IF NOT EXISTS debug_messages (
    message_id TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL,
    channel_id TEXT,
    kind       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS debug_messages_run ON debug_messages (run_id);

-- At most one decline notice per member and run.
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

-- Persist rescan outcomes; the live queue remains in memory.
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

-- Handled model interactions only; pruned to CHAT_INTERACTIONS_KEPT rows.
CREATE TABLE IF NOT EXISTS chat_interactions (
    id                TEXT PRIMARY KEY,
    at                TEXT NOT NULL,
    channel_id        TEXT,
    message_id        TEXT,
    author_id         TEXT,
    model             TEXT NOT NULL DEFAULT '',
    question          TEXT NOT NULL DEFAULT '',
    reply             TEXT NOT NULL DEFAULT '',
    -- answered | failed
    outcome           TEXT NOT NULL DEFAULT 'answered',
    -- Provider or generation failure.
    error             TEXT,
    rounds            INTEGER NOT NULL DEFAULT 0,
    latency_ms        INTEGER,
    -- Partial latency split; assembly time is excluded.
    model_ms          INTEGER,
    tools_ms          INTEGER,
    -- Summed provider counters across rounds.
    prompt_tokens     INTEGER,
    completion_tokens INTEGER,
    -- Ordered tool-call diagnostics as JSON.
    tool_calls        TEXT NOT NULL DEFAULT '[]',
    -- Provider responses by round; prompts are never stored.
    model_rounds      TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS chat_interactions_recent ON chat_interactions (at DESC);

-- Schedule changes with the best actor identity available; pruned on insert.
CREATE TABLE IF NOT EXISTS audit (
    id      TEXT PRIMARY KEY,
    at      TEXT NOT NULL,
    -- See AUDIT_SURFACES.
    surface TEXT NOT NULL DEFAULT 'system',
    actor   TEXT NOT NULL DEFAULT 'token',
    -- Short action verb.
    action  TEXT NOT NULL,
    -- Changed entity id or config key.
    subject TEXT,
    -- Human-readable summary.
    detail  TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS audit_recent ON audit (at DESC);

-- Sparse persisted allowance overrides; spent windows remain in memory.
CREATE TABLE IF NOT EXISTS chat_rate_limits (
    user_id    TEXT PRIMARY KEY,
    count      INTEGER NOT NULL,
    window_s   REAL NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_memory_enrollments (
    guild_id            TEXT NOT NULL,
    user_id             TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN ('disabled', 'pending_notice', 'active', 'opted_out')),
    admin_actor_id      TEXT,
    policy_version      TEXT NOT NULL,
    notice_attempted_at TEXT,
    notice_message_id   TEXT,
    notice_sent_at      TEXT,
    opt_out_at          TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (guild_id, user_id)
);

CREATE INDEX IF NOT EXISTS chat_memory_enrollments_active
    ON chat_memory_enrollments (guild_id, user_id) WHERE state = 'active';

CREATE TABLE IF NOT EXISTS chat_memories (
    id                TEXT PRIMARY KEY,
    guild_id          TEXT NOT NULL,
    user_id           TEXT NOT NULL,
    slot              TEXT NOT NULL CHECK (slot IN ('answer_detail', 'answer_format', 'strategy_disclosure', 'strategy_emphasis')),
    value             TEXT NOT NULL,
    boss_token        TEXT,
    state             TEXT NOT NULL CHECK (state IN ('proposed', 'active', 'superseded', 'rejected', 'revoked', 'expired')),
    source_message_id TEXT,
    source_channel_id TEXT,
    proposer_id       TEXT,
    proposal_message_id TEXT,
    reviewer_id       TEXT,
    created_at        TEXT NOT NULL,
    reviewed_at       TEXT,
    expires_at        TEXT NOT NULL,
    state_at          TEXT NOT NULL,
    CHECK (
        (slot = 'answer_detail' AND value IN ('concise', 'standard', 'detailed')) OR
        (slot = 'answer_format' AND value IN ('prose', 'bullets', 'steps')) OR
        (slot = 'strategy_disclosure' AND value IN ('none', 'hints', 'full')) OR
        (slot = 'strategy_emphasis' AND value IN ('mechanics', 'survival', 'party_roles'))
    ),
    CHECK (boss_token IS NULL OR slot IN ('strategy_disclosure', 'strategy_emphasis')),
    CHECK (boss_token IS NULL OR (length(boss_token) BETWEEN 2 AND 64 AND substr(boss_token, 1, 1) GLOB '[A-Za-z]' AND boss_token NOT GLOB '*[^A-Za-z0-9]*'))
);

CREATE UNIQUE INDEX IF NOT EXISTS chat_memories_one_active_scope
    ON chat_memories (guild_id, user_id, slot, COALESCE(boss_token, ''))
    WHERE state = 'active';
CREATE INDEX IF NOT EXISTS chat_memories_retrieval
    ON chat_memories (guild_id, user_id, state, expires_at, slot, boss_token);
CREATE INDEX IF NOT EXISTS chat_memories_inactive_purge
    ON chat_memories (state, state_at);
CREATE INDEX IF NOT EXISTS chat_memories_proposal_message
    ON chat_memories (proposal_message_id);

CREATE TABLE IF NOT EXISTS chat_memory_events (
    id                 TEXT PRIMARY KEY,
    guild_id           TEXT NOT NULL,
    user_id            TEXT NOT NULL,
    actor_id           TEXT,
    action             TEXT NOT NULL CHECK (action IN ('enrollment_started', 'notice_attempted', 'enrollment_activated', 'enrollment_disabled', 'opted_out', 'proposed', 'approved', 'rejected', 'replaced', 'revoked', 'expired', 'deleted', 'forgot_all')),
    memory_id          TEXT,
    enrollment_guild_id TEXT,
    enrollment_user_id TEXT,
    reason             TEXT NOT NULL CHECK (reason IN ('enroll', 'notice', 'opt_out', 'proposal', 'approval', 'rejection', 'replace', 'disable', 'expiry', 'delete', 'forget_all')),
    at                 TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_memory_events_recent ON chat_memory_events (at DESC, id DESC);
CREATE INDEX IF NOT EXISTS chat_memory_events_subject ON chat_memory_events (guild_id, user_id, at DESC);

CREATE TABLE IF NOT EXISTS chat_memory_retrievals (
    id                  TEXT PRIMARY KEY,
    guild_id            TEXT NOT NULL,
    user_id             TEXT NOT NULL,
    selected_memory_ids TEXT NOT NULL DEFAULT '[]' CHECK (length(selected_memory_ids) <= 4096),
    reason              TEXT NOT NULL CHECK (reason IN ('matched', 'no_match', 'inactive_enrollment', 'unavailable')),
    latency_ms          INTEGER,
    result_count        INTEGER NOT NULL CHECK (result_count >= 0 AND result_count <= 4),
    at                  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_memory_retrievals_recent ON chat_memory_retrievals (at DESC, id DESC);
"""


def _migrate_9_to_10(conn: sqlite3.Connection) -> None:
    """v9 -> v10: nullable member-selected chatbot reply style."""
    existing = {row[1] for row in conn.execute("PRAGMA table_info(members)")}
    if existing and "reply_style" not in existing:
        conn.execute("ALTER TABLE members ADD COLUMN reply_style TEXT")
        log.info("schema v9->v10: members gained reply_style")


#: v10 -> v11: the `Star` boss key becomes `MaleficStar`.
_RENAMED_TOKENS = {"NStar": "NMaleficStar", "HStar": "HMaleficStar"}


def _rewrite_tokens(items: list) -> list:
    return [_RENAMED_TOKENS.get(item, item) for item in items]


def _migrate_10_to_11(conn: sqlite3.Connection) -> None:
    """v10 -> v11: stored `NStar`/`HStar` tokens become `NMaleficStar`/`HMaleficStar`.

    The old spellings keep parsing (``star`` stays an alias), so this only
    rewrites what is already stored: the ``bosses`` JSON lists on runs, fixed
    runs and amendments, plus the ``bosses`` array a ``split`` card keeps in
    its amendment payload. Anything else holding boss text (extraction logs,
    audit lines, rescan reports) is history and is left alone.
    """
    tables = (("runs", "id"), ("fixed_runs", "id"), ("amendments", "id"))
    existing_tables = {
        row["name"]
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
        )
    }
    rewritten = 0
    for table, key in tables:
        if table not in existing_tables:
            continue
        columns = {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
        if "bosses" not in columns:
            continue
        for row in conn.execute(f"SELECT {key}, bosses FROM {table}"):
            try:
                items = json.loads(row["bosses"] or "[]")
            except (json.JSONDecodeError, TypeError):
                continue
            if not isinstance(items, list) or not any(item in _RENAMED_TOKENS for item in items):
                continue
            conn.execute(
                f"UPDATE {table} SET bosses = ? WHERE {key} = ?",
                (json.dumps(_rewrite_tokens(items)), row[key]),
            )
            rewritten += 1
    if "amendments" in existing_tables:
        for row in conn.execute("SELECT id, payload FROM amendments"):
            try:
                payload = json.loads(row["payload"] or "{}")
            except (json.JSONDecodeError, TypeError):
                continue
            bosses = payload.get("bosses")
            if (
                not isinstance(payload, dict)
                or not isinstance(bosses, list)
                or not any(item in _RENAMED_TOKENS for item in bosses)
            ):
                continue
            payload["bosses"] = _rewrite_tokens(bosses)
            conn.execute(
                "UPDATE amendments SET payload = ? WHERE id = ?",
                (json.dumps(payload), row["id"]),
            )
            rewritten += 1
    log.info("schema v10->v11: rewrote %d stored boss list(s) to MaleficStar", rewritten)


def _migrate_11_to_12(conn: sqlite3.Connection) -> None:
    """v11 -> v12: add empty governed-memory storage without backfilling data."""
    del conn


def _migrate_12_to_13(conn: sqlite3.Connection) -> None:
    """v12 -> v13: add empty weekly digest tracking without guessing old posts."""
    del conn


def _json_list(value: str | None) -> list:
    if not value:
        return []
    return json.loads(value)


def _dump(value: Iterable[Any]) -> str:
    return json.dumps(list(value))


def _int_or_none(value: Any) -> int | None:
    return int(value) if value is not None else None


def _memory_time(value: datetime | None = None) -> datetime:
    value = utcnow() if value is None else value
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("memory timestamps must be timezone-aware")
    return value.astimezone(UTC)


def _stored_memory_time(value: object) -> datetime | None:
    """Parse only aware ISO storage values; damaged rows are unavailable."""
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
        if parsed.tzinfo is None or parsed.utcoffset() is None:
            return None
        return parsed.astimezone(UTC)
    except (TypeError, ValueError):
        return None


def _memory_preference(
    slot: str, value: str, boss_token: str | None, boss_table: BossTable | None = None
) -> tuple[str, str, str | None]:
    slot = str(slot).strip().lower()
    value = str(value).strip().lower()
    allowed = {
        "answer_detail": {"concise", "standard", "detailed"},
        "answer_format": {"prose", "bullets", "steps"},
        "strategy_disclosure": {"none", "hints", "full"},
        "strategy_emphasis": {"mechanics", "survival", "party_roles"},
    }
    if value not in allowed.get(slot, set()):
        raise ValueError("invalid governed-memory slot or value")
    token = str(boss_token).strip() if boss_token is not None else None
    if token is not None and (slot not in {"strategy_disclosure", "strategy_emphasis"}):
        raise ValueError("boss scope is valid only for strategy preferences")
    if token is not None:
        if not isinstance(boss_table, BossTable):
            raise ValueError("a BossTable is required for a boss-scoped preference")
        if not token.isascii():
            raise ValueError("boss token must be ASCII")
        try:
            token = boss_table.parse_token(token)
        except BossParseError as exc:
            raise ValueError("invalid canonical boss token") from exc
        if not token.isascii():
            raise ValueError("canonical boss token must be ASCII")
    return slot, value, token


def _like_escape(term: str) -> str:
    r"""Escape SQL ``LIKE`` wildcard characters."""
    return term.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def _search_where(columns: Sequence[str], q: str) -> tuple[str, list[str]]:
    """Return a ``LIKE`` clause and parameters for a text search."""
    term = (q or "").strip()
    if not term:
        return "", []
    ors = " OR ".join(f"{column} LIKE ? ESCAPE '\\'" for column in columns)
    return f"({ors})", [f"%{_like_escape(term)}%"] * len(columns)


def _percentile(ordered: Sequence[int], fraction: float) -> int | None:
    """Return a nearest-rank percentile, or ``None`` for an empty sequence."""
    if not ordered:
        return None
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


class Repo:
    """Thin repository over a single SQLite connection."""

    #: Called after a run change that requires card re-rendering.
    on_run_changed: Callable[[str], None] | None = None

    def __init__(self, path: str | Path):
        self.path = str(path)
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        # One event-loop-owned connection; async routes avoid worker threads.
        self._conn = sqlite3.connect(self.path, isolation_level=None, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._memory_savepoint = 0
        self.migrate()

    def migrate(self) -> None:
        """Create or upgrade the database to the supported schema version."""
        existing_tables = {
            row["name"]
            for row in self._conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
            )
        }
        if existing_tables and "schema_version" not in existing_tables:
            raise RuntimeError(
                f"database at {self.path} has no schema version; refusing to treat existing "
                f"application tables as a fresh v{SCHEMA_VERSION} database"
            )
        self._conn.execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        row = self._conn.execute("SELECT version FROM schema_version").fetchone()

        if row is None:
            application_tables = existing_tables - {"schema_version"}
            if application_tables:
                raise RuntimeError(
                    f"database at {self.path} has no schema version; refusing to treat existing "
                    f"application tables as a fresh v{SCHEMA_VERSION} database"
                )
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
        if current < 9:
            raise RuntimeError(
                f"database at {self.path} is schema v{current}; this release supports "
                "upgrades from v9 only"
            )
        if current == 9:
            log.info("migrating database %s: v9 -> v10", self.path)
            _migrate_9_to_10(self._conn)
            self._conn.execute("UPDATE schema_version SET version = 10")
            current = 10
        if current == 10:
            log.info("migrating database %s: v10 -> v11", self.path)
            _migrate_10_to_11(self._conn)
            self._conn.execute("UPDATE schema_version SET version = 11")
            current = 11
        if current == 11:
            log.info("migrating database %s: v11 -> v12", self.path)
            _migrate_11_to_12(self._conn)
            self._conn.execute("UPDATE schema_version SET version = 12")
            current = 12
        if current == 12:
            log.info("migrating database %s: v12 -> v13", self.path)
            _migrate_12_to_13(self._conn)
            self._conn.execute("UPDATE schema_version SET version = 13")
        # Some unreleased v12 databases predate the card binding column.  Add it
        # before SCHEMA_SQL creates its index, otherwise SQLite rejects startup.
        if "chat_memories" in existing_tables:
            columns = {row[1] for row in self._conn.execute("PRAGMA table_info(chat_memories)")}
            if "proposal_message_id" not in columns:
                self._conn.execute("ALTER TABLE chat_memories ADD COLUMN proposal_message_id TEXT")
        self._conn.executescript(SCHEMA_SQL)
        self._conn.execute(
            "CREATE INDEX IF NOT EXISTS chat_memories_proposal_message ON chat_memories (proposal_message_id)"
        )

    def close(self) -> None:
        self._conn.close()

    @contextmanager
    def _memory_transaction(self):
        """Serialize a governed-memory lifecycle transition on the owned connection."""
        if self._conn.in_transaction:
            self._memory_savepoint += 1
            savepoint = f"memory_{self._memory_savepoint}"
            self._conn.execute(f"SAVEPOINT {savepoint}")  # noqa: S608 - generated counter only
            try:
                yield
            except Exception:
                self._conn.execute(f"ROLLBACK TO SAVEPOINT {savepoint}")  # noqa: S608
                self._conn.execute(f"RELEASE SAVEPOINT {savepoint}")  # noqa: S608
                raise
            else:
                self._conn.execute(f"RELEASE SAVEPOINT {savepoint}")  # noqa: S608
            return
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            yield
        except Exception:
            self._conn.execute("ROLLBACK")
            raise
        else:
            self._conn.execute("COMMIT")

    def backup_to(self, path: str | Path) -> None:
        """Write a consistent, self-contained database snapshot."""
        dest = sqlite3.connect(str(path))
        try:
            with dest:
                self._conn.backup(dest)
            dest.execute("PRAGMA journal_mode=DELETE")
        finally:
            dest.close()
        # Remove WAL sidecars left after switching the copy's journal mode.
        for sibling in (f"{path}-wal", f"{path}-shm"):
            Path(sibling).unlink(missing_ok=True)

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
        """Return a member's mention preference or the default."""
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

    def get_reply_style(self, user_id: int | str) -> str | None:
        member = self.get_member(user_id)
        return member["reply_style"] if member else None

    def set_reply_style(self, user_id: int | str, style: str | None) -> str | None:
        """Store a profile name or NULL; catalog membership is a caller concern."""
        stored = str(style).strip().lower() if style is not None else None
        if stored == "":
            stored = None
        updated = self._conn.execute(
            "UPDATE members SET reply_style = ?, updated_at = ? WHERE user_id = ?",
            (stored, to_iso(utcnow()), str(user_id)),
        ).rowcount
        if not updated:
            raise KeyError(str(user_id))
        return stored

    @staticmethod
    def _member(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["aliases"] = _json_list(data["aliases"])
        data["has_role"] = bool(data["has_role"])
        data["ping_level"] = data.get("ping_level") or DEFAULT_PING_LEVEL
        data["reply_style"] = data.get("reply_style") or None
        return data

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
        """Return baseline timings, optionally by participant or owner."""
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
        """Return runs ordered by time, with optional filters."""
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

    def run_move_conflict(self, run: dict, week_start: datetime) -> dict | None:
        """The other run blocking a cross-week move, if any.

        Each weekly timing has at most one run per boss week
        (``runs_fixed_week``). Moving a run into a week that already holds its
        weekly's run would violate that, so callers check this first and refuse
        with a message naming the blocker instead of hitting the database
        constraint. Standalone runs (``fixed_run_id IS NULL``) never conflict.
        """
        fixed_run_id = run.get("fixed_run_id")
        if not fixed_run_id:
            return None
        existing = self.run_for_fixed(str(fixed_run_id), week_start)
        if existing is not None and existing["id"] != run["id"]:
            return existing
        return None

    def set_run_datetime(self, run_id: str, run_at: datetime, week_start: datetime) -> None:
        try:
            self._conn.execute(
                "UPDATE runs SET datetime = ?, week_start = ? WHERE id = ?",
                (to_iso(run_at), to_iso(week_start), run_id),
            )
        except sqlite3.IntegrityError as exc:
            # Only the (fixed_run_id, week_start) uniqueness can fail here.
            # Raise a domain error so callers can refuse the move instead of
            # leaking a 500 with the raw constraint name.
            raise ValueError("that weekly already has a run in that boss week") from exc

    def set_run_channel(self, run_id: str, channel_id: int | str | None) -> None:
        self._conn.execute(
            "UPDATE runs SET channel_id = ? WHERE id = ?",
            (str(channel_id) if channel_id is not None else None, run_id),
        )

    def set_run_fixed(self, run_id: str, fixed_run_id: str | None) -> None:
        """Link or unlink a run's fixed timing."""
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

    def reschedule_unposted_reminder(
        self, reminder_id: str, fire_at: datetime, now: datetime
    ) -> bool:
        """Reschedule a reminder that has not posted a card."""
        fire_at_iso = to_iso(fire_at)
        if fire_at > now:
            cursor = self._conn.execute(
                """
                UPDATE reminders SET fire_at = ?, sent_at = NULL
                WHERE id = ? AND message_id IS NULL
                  AND (fire_at != ? OR sent_at IS NOT NULL)
                """,
                (fire_at_iso, reminder_id, fire_at_iso),
            )
        else:
            cursor = self._conn.execute(
                """
                UPDATE reminders SET fire_at = ?, sent_at = COALESCE(sent_at, ?)
                WHERE id = ? AND message_id IS NULL
                  AND (fire_at != ? OR sent_at IS NULL)
                """,
                (fire_at_iso, to_iso(now), reminder_id, fire_at_iso),
            )
        return bool(cursor.rowcount)

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

    def set_weekly_digest(
        self,
        week_start: datetime,
        channel_id: int | str,
        message_id: int | str,
        *,
        at: datetime | None = None,
    ) -> None:
        """Bind a boss week to its one live digest card, retaining the row as its log."""
        self._conn.execute(
            """
            INSERT INTO weekly_digests
                (week_start, channel_id, message_id, posted_at, retired_at)
            VALUES (?, ?, ?, ?, NULL)
            ON CONFLICT(week_start) DO UPDATE SET
                channel_id = excluded.channel_id,
                message_id = excluded.message_id,
                posted_at = excluded.posted_at,
                retired_at = NULL
            """,
            (to_iso(week_start), str(channel_id), str(message_id), to_iso(at or utcnow())),
        )

    def get_weekly_digest(self, week_start: datetime, *, active_only: bool = False) -> dict | None:
        sql = "SELECT * FROM weekly_digests WHERE week_start = ?"
        if active_only:
            sql += " AND retired_at IS NULL"
        row = self._conn.execute(sql, (to_iso(week_start),)).fetchone()
        if row is None:
            return None
        digest = dict(row)
        digest["week_start"] = from_iso(digest["week_start"])
        digest["posted_at"] = from_iso(digest["posted_at"])
        digest["retired_at"] = from_iso(digest["retired_at"]) if digest["retired_at"] else None
        return digest

    def retire_weekly_digest(self, week_start: datetime, *, at: datetime | None = None) -> bool:
        cursor = self._conn.execute(
            "UPDATE weekly_digests SET retired_at = ? WHERE week_start = ? AND retired_at IS NULL",
            (to_iso(at or utcnow()), to_iso(week_start)),
        )
        return bool(cursor.rowcount)

    def retire_weekly_digests_before(
        self, week_start: datetime, *, at: datetime | None = None
    ) -> int:
        """Stop old cards following live state while preserving their final bindings."""
        cursor = self._conn.execute(
            "UPDATE weekly_digests SET retired_at = ? WHERE week_start < ? AND retired_at IS NULL",
            (to_iso(at or utcnow()), to_iso(week_start)),
        )
        return cursor.rowcount

    def retire_weekly_digest_by_message(
        self, message_id: int | str, *, at: datetime | None = None
    ) -> bool:
        cursor = self._conn.execute(
            "UPDATE weekly_digests SET retired_at = ? WHERE message_id = ? AND retired_at IS NULL",
            (to_iso(at or utcnow()), str(message_id)),
        )
        return bool(cursor.rowcount)

    @staticmethod
    def _reminder(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["fire_at"] = from_iso(data["fire_at"])
        data["sent_at"] = from_iso(data["sent_at"]) if data["sent_at"] else None
        return data

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
        """Return test cards for a run, oldest first."""
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
        """Return one channel's messages in ``[since, until)``, oldest first."""
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
        """Overwrite the instant a proposal would move a run to."""
        self._conn.execute(
            "UPDATE amendments SET new_datetime = ? WHERE id = ?",
            (to_iso(new_datetime) if new_datetime else None, amendment_id),
        )

    def set_amendment_run(self, amendment_id: str, run_id: str | None) -> None:
        self._conn.execute("UPDATE amendments SET run_id = ? WHERE id = ?", (run_id, amendment_id))

    def claim_chat_followup(self, amendment_id: str, at: datetime | None = None) -> bool:
        """Atomically claim a card's one permitted chatbot follow-up."""
        # Guard malformed legacy payloads so a rejection cannot raise.
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

    def _memory_event(
        self,
        guild_id: int | str,
        user_id: int | str,
        *,
        actor_id: int | str | None,
        action: str,
        reason: str,
        memory_id: str | None = None,
        at: datetime,
    ) -> str:
        if action not in MEMORY_EVENT_ACTIONS or reason not in MEMORY_EVENT_REASONS:
            raise ValueError("invalid governed-memory event")
        event_id = new_id()
        self._conn.execute(
            "INSERT INTO chat_memory_events "
            "(id, guild_id, user_id, actor_id, action, memory_id, enrollment_guild_id, "
            "enrollment_user_id, reason, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                event_id,
                str(guild_id),
                str(user_id),
                str(actor_id) if actor_id is not None else None,
                action,
                memory_id,
                str(guild_id),
                str(user_id),
                reason,
                to_iso(at),
            ),
        )
        return event_id

    def begin_memory_enrollment(
        self,
        guild_id: int | str,
        user_id: int | str,
        admin_actor_id: int | str,
        policy_version: str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Start one notification-first enrollment; active members are unchanged."""
        stamp = _memory_time(now)
        if not str(policy_version).strip():
            raise ValueError("policy version is required")
        with self._memory_transaction():
            cursor = self._conn.execute(
                "INSERT INTO chat_memory_enrollments "
                "(guild_id, user_id, state, admin_actor_id, policy_version, created_at, updated_at) "
                "VALUES (?, ?, 'pending_notice', ?, ?, ?, ?) "
                "ON CONFLICT(guild_id, user_id) DO UPDATE SET state = 'pending_notice', "
                "admin_actor_id = excluded.admin_actor_id, policy_version = excluded.policy_version, "
                "notice_attempted_at = NULL, notice_message_id = NULL, notice_sent_at = NULL, "
                "opt_out_at = NULL, updated_at = excluded.updated_at "
                "WHERE chat_memory_enrollments.state != 'active'",
                (
                    str(guild_id),
                    str(user_id),
                    str(admin_actor_id),
                    str(policy_version),
                    to_iso(stamp),
                    to_iso(stamp),
                ),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=admin_actor_id,
                action="enrollment_started",
                reason="enroll",
                at=stamp,
            )
            return True

    def record_memory_notice_attempt(
        self,
        guild_id: int | str,
        user_id: int | str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Audit an explicit notice delivery attempt while the member is ineligible."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "UPDATE chat_memory_enrollments SET notice_attempted_at = ?, updated_at = ? "
                "WHERE guild_id = ? AND user_id = ? AND state = 'pending_notice'",
                (to_iso(stamp), to_iso(stamp), str(guild_id), str(user_id)),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="notice_attempted",
                reason="notice",
                at=stamp,
            )
            return True

    def activate_memory_enrollment(
        self,
        guild_id: int | str,
        user_id: int | str,
        actor_id: int | str,
        notice_message_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """CAS a successfully delivered notice from pending to active."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "UPDATE chat_memory_enrollments SET state = 'active', notice_attempted_at = ?, "
                "notice_message_id = ?, notice_sent_at = ?, updated_at = ? "
                "WHERE guild_id = ? AND user_id = ? AND state = 'pending_notice'",
                (
                    to_iso(stamp),
                    str(notice_message_id),
                    to_iso(stamp),
                    to_iso(stamp),
                    str(guild_id),
                    str(user_id),
                ),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="enrollment_activated",
                reason="notice",
                at=stamp,
            )
            return True

    def memory_enrollment_active(self, guild_id: int | str, user_id: int | str) -> bool:
        return bool(
            self._conn.execute(
                "SELECT 1 FROM chat_memory_enrollments WHERE guild_id = ? AND user_id = ? AND state = 'active'",
                (str(guild_id), str(user_id)),
            ).fetchone()
        )

    def get_memory_enrollment(self, guild_id: int | str, user_id: int | str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM chat_memory_enrollments WHERE guild_id = ? AND user_id = ?",
            (str(guild_id), str(user_id)),
        ).fetchone()
        return self._memory_enrollment(row) if row else None

    @staticmethod
    def _memory_enrollment(row: sqlite3.Row) -> dict:
        data = dict(row)
        for key in (
            "notice_attempted_at",
            "notice_sent_at",
            "opt_out_at",
            "created_at",
            "updated_at",
        ):
            data[key] = _stored_memory_time(data[key])
        return data

    def opt_out_memory(
        self,
        guild_id: int | str,
        user_id: int | str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Make a member ineligible and revoke live content in the same transaction."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            self._conn.execute(
                "INSERT INTO chat_memory_enrollments (guild_id, user_id, state, admin_actor_id, policy_version, opt_out_at, created_at, updated_at) "
                "VALUES (?, ?, 'opted_out', ?, '', ?, ?, ?) ON CONFLICT(guild_id, user_id) DO UPDATE SET "
                "state = 'opted_out', opt_out_at = excluded.opt_out_at, updated_at = excluded.updated_at",
                (
                    str(guild_id),
                    str(user_id),
                    str(actor_id),
                    to_iso(stamp),
                    to_iso(stamp),
                    to_iso(stamp),
                ),
            )
            rows = list(
                self._conn.execute(
                    "SELECT id FROM chat_memories WHERE guild_id = ? AND user_id = ? AND state IN ('proposed', 'active')",
                    (str(guild_id), str(user_id)),
                )
            )
            self._conn.execute(
                "UPDATE chat_memories SET state = 'revoked', state_at = ?, reviewed_at = COALESCE(reviewed_at, ?), reviewer_id = COALESCE(reviewer_id, ?) WHERE guild_id = ? AND user_id = ? AND state IN ('proposed', 'active')",
                (to_iso(stamp), to_iso(stamp), str(actor_id), str(guild_id), str(user_id)),
            )
            self._memory_event(
                guild_id, user_id, actor_id=actor_id, action="opted_out", reason="opt_out", at=stamp
            )
            for row in rows:
                self._memory_event(
                    guild_id,
                    user_id,
                    actor_id=actor_id,
                    action="revoked",
                    reason="opt_out",
                    memory_id=row["id"],
                    at=stamp,
                )
            return True

    def disable_memory_enrollment(
        self,
        guild_id: int | str,
        user_id: int | str,
        admin_actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Disable one enrollment and revoke its live content without deleting it."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "UPDATE chat_memory_enrollments SET state = 'disabled', admin_actor_id = ?, "
                "updated_at = ? WHERE guild_id = ? AND user_id = ? AND state != 'disabled'",
                (str(admin_actor_id), to_iso(stamp), str(guild_id), str(user_id)),
            )
            if not cursor.rowcount:
                return False
            rows = list(
                self._conn.execute(
                    "SELECT id FROM chat_memories WHERE guild_id = ? AND user_id = ? "
                    "AND state IN ('proposed', 'active')",
                    (str(guild_id), str(user_id)),
                )
            )
            self._conn.execute(
                "UPDATE chat_memories SET state = 'revoked', state_at = ?, "
                "reviewed_at = COALESCE(reviewed_at, ?), reviewer_id = COALESCE(reviewer_id, ?) "
                "WHERE guild_id = ? AND user_id = ? AND state IN ('proposed', 'active')",
                (to_iso(stamp), to_iso(stamp), str(admin_actor_id), str(guild_id), str(user_id)),
            )
            self._memory_event(
                guild_id,
                user_id,
                actor_id=admin_actor_id,
                action="enrollment_disabled",
                reason="disable",
                at=stamp,
            )
            for row in rows:
                self._memory_event(
                    guild_id,
                    user_id,
                    actor_id=admin_actor_id,
                    action="revoked",
                    reason="disable",
                    memory_id=row["id"],
                    at=stamp,
                )
            return True

    def create_memory_proposal(
        self,
        guild_id: int | str,
        user_id: int | str,
        slot: str,
        value: str,
        *,
        boss_token: str | None = None,
        boss_table: BossTable | None = None,
        source_message_id: int | str | None = None,
        source_channel_id: int | str | None = None,
        proposer_id: int | str | None = None,
        now: datetime | None = None,
    ) -> str | None:
        """Store a seven-day proposal only while its exact subject is active."""
        slot, value, boss_token = _memory_preference(slot, value, boss_token, boss_table)
        stamp = _memory_time(now)
        memory_id = new_id()
        with self._memory_transaction():
            if not self.memory_enrollment_active(guild_id, user_id):
                return None
            self._conn.execute(
                "INSERT INTO chat_memories (id, guild_id, user_id, slot, value, boss_token, state, source_message_id, source_channel_id, proposer_id, created_at, expires_at, state_at) VALUES (?, ?, ?, ?, ?, ?, 'proposed', ?, ?, ?, ?, ?, ?)",
                (
                    memory_id,
                    str(guild_id),
                    str(user_id),
                    slot,
                    value,
                    boss_token,
                    str(source_message_id) if source_message_id is not None else None,
                    str(source_channel_id) if source_channel_id is not None else None,
                    str(proposer_id) if proposer_id is not None else None,
                    to_iso(stamp),
                    to_iso(stamp + MEMORY_PROPOSAL_TTL),
                    to_iso(stamp),
                ),
            )
            self._memory_event(
                guild_id,
                user_id,
                actor_id=proposer_id,
                action="proposed",
                reason="proposal",
                memory_id=memory_id,
                at=stamp,
            )
        return memory_id

    def bind_memory_proposal_message(
        self, guild_id: int | str, user_id: int | str, memory_id: str, message_id: int | str
    ) -> bool:
        """Bind one posted card to its still-pending proposal."""
        with self._memory_transaction():
            return bool(
                self._conn.execute(
                    "UPDATE chat_memories SET proposal_message_id = ? WHERE id = ? AND guild_id = ? "
                    "AND user_id = ? AND state = 'proposed' AND proposal_message_id IS NULL",
                    (str(message_id), memory_id, str(guild_id), str(user_id)),
                ).rowcount
            )

    def memory_proposal_by_message(self, message_id: int | str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM chat_memories WHERE proposal_message_id = ?", (str(message_id),)
        ).fetchone()
        return self._memory(row) if row else None

    def discard_memory_proposal(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Remove only an unbound proposal whose review card was never made usable."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "DELETE FROM chat_memories WHERE id = ? AND guild_id = ? AND user_id = ? "
                "AND state = 'proposed' AND proposal_message_id IS NULL",
                (memory_id, str(guild_id), str(user_id)),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=None,
                action="deleted",
                reason="delete",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def approve_memory_proposal(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        reviewer_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """CAS an unexpired proposal, superseding the matching active scope atomically."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            proposal = self._conn.execute(
                "SELECT slot, boss_token, expires_at FROM chat_memories WHERE id = ? "
                "AND guild_id = ? AND user_id = ? AND state = 'proposed'",
                (memory_id, str(guild_id), str(user_id)),
            ).fetchone()
            if (
                proposal is None
                or (expires_at := _stored_memory_time(proposal["expires_at"])) is None
                or expires_at <= stamp
                or not self.memory_enrollment_active(guild_id, user_id)
            ):
                return False
            active = list(
                self._conn.execute(
                    "SELECT id FROM chat_memories WHERE guild_id = ? AND user_id = ? AND slot = ? AND COALESCE(boss_token, '') = COALESCE(?, '') AND state = 'active'",
                    (str(guild_id), str(user_id), proposal["slot"], proposal["boss_token"]),
                )
            )
            self._conn.execute(
                "UPDATE chat_memories SET state = 'superseded', state_at = ?, reviewed_at = ?, reviewer_id = ? WHERE guild_id = ? AND user_id = ? AND slot = ? AND COALESCE(boss_token, '') = COALESCE(?, '') AND state = 'active'",
                (
                    to_iso(stamp),
                    to_iso(stamp),
                    str(reviewer_id),
                    str(guild_id),
                    str(user_id),
                    proposal["slot"],
                    proposal["boss_token"],
                ),
            )
            cursor = self._conn.execute(
                "UPDATE chat_memories SET state = 'active', reviewed_at = ?, reviewer_id = ?, "
                "expires_at = ?, state_at = ? WHERE id = ? AND guild_id = ? AND user_id = ? "
                "AND state = 'proposed'",
                (
                    to_iso(stamp),
                    str(reviewer_id),
                    to_iso(stamp + MEMORY_ACTIVE_TTL),
                    to_iso(stamp),
                    memory_id,
                    str(guild_id),
                    str(user_id),
                ),
            )
            if not cursor.rowcount:
                raise RuntimeError("memory proposal changed during approval")
            for row in active:
                self._memory_event(
                    guild_id,
                    user_id,
                    actor_id=reviewer_id,
                    action="replaced",
                    reason="replace",
                    memory_id=row["id"],
                    at=stamp,
                )
            self._memory_event(
                guild_id,
                user_id,
                actor_id=reviewer_id,
                action="approved",
                reason="approval",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def reject_memory_proposal(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        reviewer_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        stamp = _memory_time(now)
        with self._memory_transaction():
            candidate = self._conn.execute(
                "SELECT expires_at FROM chat_memories WHERE id = ? AND guild_id = ? AND user_id = ? "
                "AND state = 'proposed'",
                (memory_id, str(guild_id), str(user_id)),
            ).fetchone()
            if (
                candidate is None
                or (expires_at := _stored_memory_time(candidate["expires_at"])) is None
            ):
                return False
            if expires_at <= stamp:
                return False
            cursor = self._conn.execute(
                "UPDATE chat_memories SET state = 'rejected', reviewer_id = ?, reviewed_at = ?, "
                "state_at = ? WHERE id = ? AND guild_id = ? AND user_id = ? AND state = 'proposed'",
                (
                    str(reviewer_id),
                    to_iso(stamp),
                    to_iso(stamp),
                    memory_id,
                    str(guild_id),
                    str(user_id),
                ),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=reviewer_id,
                action="rejected",
                reason="rejection",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def replace_memory(
        self,
        guild_id: int | str,
        user_id: int | str,
        slot: str,
        value: str,
        admin_actor_id: int | str,
        *,
        boss_token: str | None = None,
        boss_table: BossTable | None = None,
        now: datetime | None = None,
    ) -> str | None:
        """Create an immediately active administrative value for an active member."""
        slot, value, boss_token = _memory_preference(slot, value, boss_token, boss_table)
        stamp = _memory_time(now)
        memory_id = new_id()
        with self._memory_transaction():
            if not self.memory_enrollment_active(guild_id, user_id):
                return None
            old = list(
                self._conn.execute(
                    "SELECT id FROM chat_memories WHERE guild_id = ? AND user_id = ? AND slot = ? AND COALESCE(boss_token, '') = COALESCE(?, '') AND state = 'active'",
                    (str(guild_id), str(user_id), slot, boss_token),
                )
            )
            self._conn.execute(
                "UPDATE chat_memories SET state = 'superseded', state_at = ?, reviewed_at = ?, reviewer_id = ? WHERE guild_id = ? AND user_id = ? AND slot = ? AND COALESCE(boss_token, '') = COALESCE(?, '') AND state = 'active'",
                (
                    to_iso(stamp),
                    to_iso(stamp),
                    str(admin_actor_id),
                    str(guild_id),
                    str(user_id),
                    slot,
                    boss_token,
                ),
            )
            self._conn.execute(
                "INSERT INTO chat_memories (id, guild_id, user_id, slot, value, boss_token, state, proposer_id, reviewer_id, created_at, reviewed_at, expires_at, state_at) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?, ?)",
                (
                    memory_id,
                    str(guild_id),
                    str(user_id),
                    slot,
                    value,
                    boss_token,
                    str(admin_actor_id),
                    str(admin_actor_id),
                    to_iso(stamp),
                    to_iso(stamp),
                    to_iso(stamp + MEMORY_ACTIVE_TTL),
                    to_iso(stamp),
                ),
            )
            for row in old:
                self._memory_event(
                    guild_id,
                    user_id,
                    actor_id=admin_actor_id,
                    action="replaced",
                    reason="replace",
                    memory_id=row["id"],
                    at=stamp,
                )
            self._memory_event(
                guild_id,
                user_id,
                actor_id=admin_actor_id,
                action="approved",
                reason="replace",
                memory_id=memory_id,
                at=stamp,
            )
        return memory_id

    def list_memories(self, guild_id: int | str, user_id: int | str) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM chat_memories WHERE guild_id = ? AND user_id = ? ORDER BY created_at DESC, id DESC",
            (str(guild_id), str(user_id)),
        )
        return [self._memory(row) for row in rows]

    def list_memory_subjects(
        self,
        guild_id: int | str,
        *,
        enrollment_state: str | None = None,
        lifecycle: str | None = None,
        slot: str | None = None,
        boss_token: str | None = None,
        expires_before: datetime | None = None,
    ) -> list[dict]:
        """List exact-guild memory subjects with their enrollment and typed rows."""
        if enrollment_state is not None and enrollment_state not in MEMORY_ENROLLMENT_STATES:
            raise ValueError("invalid memory enrollment state")
        if lifecycle is not None and lifecycle not in MEMORY_STATES:
            raise ValueError("invalid memory lifecycle")
        if slot is not None and slot not in MEMORY_SLOTS:
            raise ValueError("invalid memory slot")
        subjects = self._conn.execute(
            "SELECT user_id FROM chat_memory_enrollments WHERE guild_id = ? "
            "UNION SELECT user_id FROM chat_memories WHERE guild_id = ? ORDER BY user_id",
            (str(guild_id), str(guild_id)),
        )
        output = []
        for subject in subjects:
            user_id = subject["user_id"]
            enrollment = self.get_memory_enrollment(guild_id, user_id)
            rows = self.list_memories(guild_id, user_id)
            if enrollment_state is not None and (enrollment or {}).get("state") != enrollment_state:
                continue
            filtered = [
                row
                for row in rows
                if (lifecycle is None or row["state"] == lifecycle)
                and (slot is None or row["slot"] == slot)
                and (boss_token is None or row["boss_token"] == boss_token)
                and (
                    expires_before is None
                    or (row["expires_at"] is not None and row["expires_at"] <= expires_before)
                )
            ]
            if (
                lifecycle is not None
                or slot is not None
                or boss_token is not None
                or expires_before is not None
            ) and not filtered:
                continue
            output.append({"user_id": user_id, "enrollment": enrollment, "memories": filtered})
        return output

    def get_memory(self, guild_id: int | str, user_id: int | str, memory_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM chat_memories WHERE id = ? AND guild_id = ? AND user_id = ?",
            (memory_id, str(guild_id), str(user_id)),
        ).fetchone()
        return self._memory(row) if row else None

    @staticmethod
    def _memory(row: sqlite3.Row) -> dict:
        data = dict(row)
        for key in ("created_at", "reviewed_at", "expires_at", "state_at"):
            data[key] = _stored_memory_time(data[key])
        return data

    def delete_memory(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Physically remove a preference without requiring present consent."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "DELETE FROM chat_memories WHERE id = ? AND guild_id = ? AND user_id = ?",
                (memory_id, str(guild_id), str(user_id)),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="deleted",
                reason="delete",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def revoke_memory(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Revoke one still-live exact-scope memory and record a bounded event."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "UPDATE chat_memories SET state = 'revoked', state_at = ?, reviewer_id = COALESCE(reviewer_id, ?), "
                "reviewed_at = COALESCE(reviewed_at, ?) WHERE id = ? AND guild_id = ? AND user_id = ? "
                "AND state IN ('proposed', 'active')",
                (
                    to_iso(stamp),
                    str(actor_id),
                    to_iso(stamp),
                    memory_id,
                    str(guild_id),
                    str(user_id),
                ),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="revoked",
                reason="disable",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def expire_memory(
        self,
        guild_id: int | str,
        user_id: int | str,
        memory_id: str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> bool:
        """Expire one still-live exact-scope memory before its scheduled expiry."""
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "UPDATE chat_memories SET state = 'expired', state_at = ?, reviewer_id = COALESCE(reviewer_id, ?), "
                "reviewed_at = COALESCE(reviewed_at, ?) WHERE id = ? AND guild_id = ? AND user_id = ? "
                "AND state IN ('proposed', 'active')",
                (
                    to_iso(stamp),
                    str(actor_id),
                    to_iso(stamp),
                    memory_id,
                    str(guild_id),
                    str(user_id),
                ),
            )
            if not cursor.rowcount:
                return False
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="expired",
                reason="expiry",
                memory_id=memory_id,
                at=stamp,
            )
            return True

    def forget_all_memories(
        self,
        guild_id: int | str,
        user_id: int | str,
        actor_id: int | str,
        *,
        now: datetime | None = None,
    ) -> int:
        stamp = _memory_time(now)
        with self._memory_transaction():
            cursor = self._conn.execute(
                "DELETE FROM chat_memories WHERE guild_id = ? AND user_id = ?",
                (str(guild_id), str(user_id)),
            )
            self._memory_event(
                guild_id,
                user_id,
                actor_id=actor_id,
                action="forgot_all",
                reason="forget_all",
                at=stamp,
            )
            return max(cursor.rowcount, 0)

    def list_memory_events(self, guild_id: int | str, user_id: int | str) -> list[dict]:
        """Content-free lifecycle metadata for an exact member scope."""
        rows = self._conn.execute(
            "SELECT * FROM chat_memory_events WHERE guild_id = ? AND user_id = ? ORDER BY at DESC, id DESC",
            (str(guild_id), str(user_id)),
        )
        return [self._memory_event_row(row) for row in rows]

    @staticmethod
    def _memory_event_row(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["at"] = _stored_memory_time(data["at"])
        return data

    def retrieve_memories(
        self, guild_id: int | str, user_id: int | str, *, now: datetime | None = None
    ) -> list[dict]:
        """Return only exact-scope active, unexpired preferences for an active member."""
        stamp = _memory_time(now)
        rows = self._conn.execute(
            "SELECT m.* FROM chat_memories AS m WHERE m.guild_id = ? AND m.user_id = ? "
            "AND m.state = 'active' AND EXISTS (SELECT 1 FROM "
            "chat_memory_enrollments AS e WHERE e.guild_id = m.guild_id AND e.user_id = m.user_id "
            "AND e.state = 'active') ORDER BY m.reviewed_at DESC, m.id DESC",
            (str(guild_id), str(user_id)),
        )
        return [
            self._memory(row)
            for row in rows
            if (expires_at := _stored_memory_time(row["expires_at"])) is not None
            and expires_at > stamp
        ]

    def log_memory_retrieval(
        self,
        guild_id: int | str,
        user_id: int | str,
        selected_memory_ids: Sequence[str],
        reason: str,
        *,
        latency_ms: int | None = None,
        now: datetime | None = None,
    ) -> str:
        """Store bounded, content-free retrieval diagnostics and prune them on insert."""
        if reason not in MEMORY_RETRIEVAL_REASONS:
            raise ValueError("invalid memory retrieval reason")
        selected = list(selected_memory_ids)
        if len(selected) > 4 or len(set(selected)) != len(selected):
            raise ValueError("invalid selected memory ids")
        for memory_id in selected:
            try:
                parsed = uuid.UUID(memory_id)
            except (AttributeError, ValueError) as exc:
                raise ValueError("invalid selected memory ids") from exc
            if parsed.version != 4 or str(parsed) != memory_id:
                raise ValueError("invalid selected memory ids")
        stamp = _memory_time(now)
        retrieval_id = new_id()
        with self._memory_transaction():
            for memory_id in selected:
                if not self._conn.execute(
                    "SELECT 1 FROM chat_memories WHERE id = ? AND guild_id = ? AND user_id = ?",
                    (memory_id, str(guild_id), str(user_id)),
                ).fetchone():
                    raise ValueError("selected memory ids must belong to this member")
            self._conn.execute(
                "INSERT INTO chat_memory_retrievals (id, guild_id, user_id, selected_memory_ids, reason, latency_ms, result_count, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    retrieval_id,
                    str(guild_id),
                    str(user_id),
                    json.dumps(selected),
                    reason,
                    _int_or_none(latency_ms),
                    len(selected),
                    to_iso(stamp),
                ),
            )
            self._conn.execute(
                "DELETE FROM chat_memory_retrievals WHERE at < ?",
                (to_iso(stamp - MEMORY_RETRIEVAL_RETENTION),),
            )
            self._conn.execute(
                "DELETE FROM chat_memory_retrievals WHERE id NOT IN (SELECT id FROM chat_memory_retrievals ORDER BY at DESC, id DESC LIMIT ?)",
                (MEMORY_RETRIEVAL_KEPT,),
            )
        return retrieval_id

    def list_memory_retrievals(self, guild_id: int | str, user_id: int | str) -> list[dict]:
        rows = self._conn.execute(
            "SELECT * FROM chat_memory_retrievals WHERE guild_id = ? AND user_id = ? "
            "ORDER BY at DESC, id DESC",
            (str(guild_id), str(user_id)),
        )
        output = []
        for row in rows:
            data = dict(row)
            data["selected_memory_ids"] = _json_list(data["selected_memory_ids"])
            data["at"] = _stored_memory_time(data["at"])
            output.append(data)
        return output

    def cleanup_memories(self, *, now: datetime | None = None) -> dict[str, int]:
        """Expire eligible rows and prune governed-memory retention windows."""
        stamp = _memory_time(now)
        expired = purged = 0
        with self._memory_transaction():
            rows = list(
                self._conn.execute(
                    "SELECT id, guild_id, user_id, expires_at FROM chat_memories "
                    "WHERE state IN ('proposed', 'active')"
                )
            )
            for row in rows:
                expires_at = _stored_memory_time(row["expires_at"])
                if expires_at is not None and expires_at > stamp:
                    continue
                cursor = self._conn.execute(
                    "UPDATE chat_memories SET state = 'expired', state_at = ? WHERE id = ? "
                    "AND state IN ('proposed', 'active')",
                    (to_iso(stamp), row["id"]),
                )
                if cursor.rowcount:
                    expired += 1
                    self._memory_event(
                        row["guild_id"],
                        row["user_id"],
                        actor_id=None,
                        action="expired",
                        reason="expiry",
                        memory_id=row["id"],
                        at=stamp,
                    )
            cursor = self._conn.execute(
                "DELETE FROM chat_memories WHERE state NOT IN ('proposed', 'active') AND state_at < ?",
                (to_iso(stamp - MEMORY_INACTIVE_RETENTION),),
            )
            purged = max(cursor.rowcount, 0)
            self._conn.execute(
                "DELETE FROM chat_memory_retrievals WHERE at < ?",
                (to_iso(stamp - MEMORY_RETRIEVAL_RETENTION),),
            )
            self._conn.execute(
                "DELETE FROM chat_memory_retrievals WHERE id NOT IN (SELECT id FROM chat_memory_retrievals ORDER BY at DESC, id DESC LIMIT ?)",
                (MEMORY_RETRIEVAL_KEPT,),
            )
            self._conn.execute(
                "DELETE FROM chat_memory_events WHERE at < ?",
                (to_iso(stamp - MEMORY_EVENT_RETENTION),),
            )
        return {"expired": expired, "purged": purged}

    def set_extraction_amendments(self, extraction_id: str, amendment_ids: Sequence[str]) -> None:
        self._conn.execute(
            "UPDATE extractions SET amendment_ids = ? WHERE id = ?",
            (_dump(amendment_ids), extraction_id),
        )

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

    #: What the Extractions page searches. The prompt and the response are the
    #: point of the page -- it is the prompt-tuning tool, and "find the call
    #: where it said Wednesday" is the question it exists to answer.
    EXTRACTION_SEARCH = ("model", "prompt", "raw_response")

    def recent_extractions(self, limit: int = 20, offset: int = 0, q: str = "") -> list[dict]:
        where, params = _search_where(self.EXTRACTION_SEARCH, q)
        sql = "SELECT * FROM extractions"
        if where:
            sql += f" WHERE {where}"
        sql += " ORDER BY at DESC, id DESC LIMIT ? OFFSET ?"
        rows = self._conn.execute(sql, [*params, int(limit), max(int(offset), 0)])
        return [self._extraction(row) for row in rows]

    def count_extractions(self, q: str = "") -> int:
        where, params = _search_where(self.EXTRACTION_SEARCH, q)
        sql = "SELECT COUNT(*) FROM extractions" + (f" WHERE {where}" if where else "")
        return int(self._conn.execute(sql, params).fetchone()[0])

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
        model_rounds: Sequence[dict] = (),
        at: datetime | None = None,
        keep: int = CHAT_INTERACTIONS_KEPT,
    ) -> str:
        """Record one handled interaction, then prune the log back to ``keep``.

        Pruning on insert rather than on a timer for the same reason
        :mod:`bot.infrastructure.backup` prunes as it writes: the only moment the table is
        certainly growing is the moment something was added to it, and a
        separate sweep is one more thing that can quietly stop running.
        """
        interaction_id = new_id()
        self._conn.execute(
            """
            INSERT INTO chat_interactions
                (id, at, channel_id, message_id, author_id, model, question, reply, outcome,
                 error, rounds, latency_ms, model_ms, tools_ms, prompt_tokens,
                  completion_tokens, tool_calls, model_rounds)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                json.dumps(list(model_rounds)),
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

    #: Name-derived member/channel matches arrive separately through ``ids``.
    CHAT_SEARCH = ("model", "question", "reply", "outcome")

    def _chat_where(self, q: str, ids: Sequence[str] = ()) -> tuple[str, list[Any]]:
        where, params = _search_where(self.CHAT_SEARCH, q)
        if not where:
            return "", []
        wanted = [str(i) for i in ids]
        if wanted:
            marks = ",".join("?" * len(wanted))
            where = f"({where} OR author_id IN ({marks}) OR channel_id IN ({marks}))"
            params = [*params, *wanted, *wanted]
        return where, params

    def recent_chat_interactions(
        self, limit: int = 50, offset: int = 0, q: str = "", ids: Sequence[str] = ()
    ) -> list[dict]:
        where, params = self._chat_where(q, ids)
        sql = "SELECT * FROM chat_interactions"
        if where:
            sql += f" WHERE {where}"
        sql += " ORDER BY at DESC, id DESC LIMIT ? OFFSET ?"
        rows = self._conn.execute(sql, [*params, int(limit), max(int(offset), 0)])
        return [self._chat_interaction(row) for row in rows]

    def count_chat_interactions(self, q: str = "", ids: Sequence[str] = ()) -> int:
        where, params = self._chat_where(q, ids)
        sql = "SELECT COUNT(*) FROM chat_interactions" + (f" WHERE {where}" if where else "")
        return int(self._conn.execute(sql, params).fetchone()[0])

    def get_chat_interaction(self, interaction_id: str) -> dict | None:
        row = self._conn.execute(
            "SELECT * FROM chat_interactions WHERE id = ?", (interaction_id,)
        ).fetchone()
        return self._chat_interaction(row) if row else None

    def chat_interaction_for_amendment(self, amendment_id: str) -> dict | None:
        """Return the chatbot interaction that created an amendment, if any."""
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

    def chat_interaction_stats(self) -> list[dict]:
        """Return per-model interaction counts, latency, and token totals."""
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
        data["model_rounds"] = json.loads(data["model_rounds"] or "[]")
        return data

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
        """Record a change and actor, then prune the audit log."""
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

    #: What the Audit page's search box looks in. Every column of the row: an
    #: audit entry is five short strings, and which one holds the thing being
    #: looked for is exactly what the reader does not know.
    AUDIT_SEARCH = ("surface", "actor", "action", "subject", "detail")

    def list_audit(self, limit: int = 200, offset: int = 0, q: str = "") -> list[dict]:
        """The most recent changes, newest first -- what the Audit page lists.

        ``offset`` and ``q`` are what the portal pages and searches with; the
        JSON API passes neither and gets what it always did.
        """
        where, params = _search_where(self.AUDIT_SEARCH, q)
        sql = "SELECT * FROM audit"
        if where:
            sql += f" WHERE {where}"
        sql += " ORDER BY at DESC, id DESC LIMIT ? OFFSET ?"
        rows = self._conn.execute(sql, [*params, int(limit), max(int(offset), 0)])
        return [self._audit(row) for row in rows]

    def count_audit(self, q: str = "") -> int:
        """How many rows a search matches -- what "page 2 of 7" is counted from."""
        where, params = _search_where(self.AUDIT_SEARCH, q)
        sql = "SELECT COUNT(*) FROM audit" + (f" WHERE {where}" if where else "")
        return int(self._conn.execute(sql, params).fetchone()[0])

    @staticmethod
    def _audit(row: sqlite3.Row) -> dict:
        data = dict(row)
        data["at"] = from_iso(data["at"])
        return data
