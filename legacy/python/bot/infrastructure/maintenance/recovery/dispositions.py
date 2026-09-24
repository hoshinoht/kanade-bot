"""Per-family classification and no-replay native dispositions for retirement.

Every disposition runs inside the caller's recovery transaction and guard scope.
None fabricates a Discord message identity.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Sequence
from datetime import UTC, datetime

from bot.domain.timeutil import from_iso, to_iso

from .model import AttemptFamily, RecoveryDispositionError

#: The weekly tick's own "already posted for this boss week" key (client.CFG_LAST_DIGEST).
DIGEST_MARKER_KEY = "last_digest_week"

_MEMORY_BINDINGS = frozenset({"memory_notice", "memory_proposal"})
_NATIVE_FAMILIES = {
    "reminder": AttemptFamily.REMINDER,
    "digest": AttemptFamily.DIGEST,
    "decline": AttemptFamily.DECLINE,
    "card": AttemptFamily.CARD,
}


def classify_family(effect_kind: str, binding_types: Sequence[str]) -> AttemptFamily:
    """Classify by persisted target claims; targetless attempts are operation-scoped."""
    kinds = set(binding_types)
    if kinds & _MEMORY_BINDINGS or (not kinds and effect_kind.startswith("memory")):
        if kinds - _MEMORY_BINDINGS:
            raise RecoveryDispositionError("attempt mixes memory and native target claims")
        return AttemptFamily.MEMORY
    if not kinds:
        return AttemptFamily.OPERATION
    if len(kinds) != 1:
        raise RecoveryDispositionError("attempt mixes native target families")
    family = _NATIVE_FAMILIES.get(next(iter(kinds)))
    if family is None:
        raise RecoveryDispositionError("attempt has an unsupported pre-receipt target claim")
    if family in {AttemptFamily.DIGEST, AttemptFamily.DECLINE} and len(binding_types) != 1:
        raise RecoveryDispositionError("digest and decline attempts have exactly one target")
    return family


def apply_retirement(
    conn: sqlite3.Connection,
    family: AttemptFamily,
    attempt: sqlite3.Row,
    targets: Sequence[sqlite3.Row],
    resolved_at: str,
) -> None:
    if family is AttemptFamily.REMINDER:
        for target in targets:
            _suppress_reminder(conn, target["key_primary"], resolved_at)
    elif family is AttemptFamily.DECLINE:
        _cooldown_decline(conn, attempt, targets[0], resolved_at)
    elif family is AttemptFamily.DIGEST:
        raise_digest_marker(conn, targets[0]["key_primary"])
    elif family is AttemptFamily.CARD:
        for target in targets:
            _leave_card_proposed(conn, target["key_primary"])
    # Operation-scoped and removed memory kinds have no native row to suppress.


def _leave_card_proposed(conn: sqlite3.Connection, amendment_id: str) -> None:
    # The amendment stays as-is; the retired runtime attempt without a message is
    # the durable no-replay record that pipeline._repost_stranded consults.
    row = conn.execute(
        "SELECT proposal_message_id FROM amendments WHERE id = ?", (amendment_id,)
    ).fetchone()
    if row is not None and row["proposal_message_id"] is not None:
        raise RecoveryDispositionError("card target is already bound to another message")


def _suppress_reminder(conn: sqlite3.Connection, reminder_id: str, resolved_at: str) -> None:
    row = conn.execute(
        "SELECT sent_at, message_id FROM reminders WHERE id = ?", (reminder_id,)
    ).fetchone()
    if row is None or (row["sent_at"] is not None and row["message_id"] is None):
        return
    if row["message_id"] is not None:
        raise RecoveryDispositionError("reminder target is already bound to another message")
    # The existing skipped shape: sent without a message, so due_reminders never returns it.
    changed = conn.execute(
        "UPDATE reminders SET sent_at = ? WHERE id = ? AND sent_at IS NULL AND message_id IS NULL",
        (resolved_at, reminder_id),
    ).rowcount
    if changed != 1:
        raise RecoveryDispositionError("reminder disposition lost its native row")


def _cooldown_decline(
    conn: sqlite3.Connection, attempt: sqlite3.Row, target: sqlite3.Row, resolved_at: str
) -> None:
    """Leave a message-less cooldown row for this (run, user).

    Intended no-replay behavior: without a journal-confirmed retraction,
    notify_decline treats this row as an unresolved historic notice and never
    posts another decline notice for the same run and member.
    """
    run_id, user_id = target["key_primary"], target["key_secondary"] or ""
    row = conn.execute(
        "SELECT message_id FROM decline_notices WHERE run_id = ? AND user_id = ?",
        (run_id, user_id),
    ).fetchone()
    if row is not None and row["message_id"] is not None:
        raise RecoveryDispositionError("decline target is already bound to another message")
    conn.execute(
        "INSERT INTO decline_notices (run_id, user_id, channel_id, message_id, notified_at) "
        "VALUES (?, ?, ?, NULL, ?) ON CONFLICT(run_id, user_id) DO UPDATE SET "
        "notified_at = excluded.notified_at WHERE decline_notices.message_id IS NULL",
        (run_id, user_id, attempt["channel_id"], resolved_at),
    )


def raise_digest_marker(conn: sqlite3.Connection, week_key: str) -> None:
    """Record a resolved current-or-past reset week so the weekly tick skips it.

    The marker never moves backwards and is never set to a future week: a future
    week's reset tick posts its scheduled digest, which is a new send rather
    than a replay (an earlier preview, if it landed, may need manual deletion).
    """
    try:
        week = from_iso(week_key)
    except (TypeError, ValueError) as exc:
        raise RecoveryDispositionError("digest target is not an ISO week start") from exc
    if week.tzinfo is None:
        raise RecoveryDispositionError("digest target is not an ISO week start")
    if week > datetime.now(UTC):
        return
    row = conn.execute("SELECT value FROM config WHERE key = ?", (DIGEST_MARKER_KEY,)).fetchone()
    if row is not None:
        try:
            marker = from_iso(row["value"])
        except (TypeError, ValueError) as exc:
            raise RecoveryDispositionError("digest marker is malformed") from exc
        if marker.tzinfo is None:
            raise RecoveryDispositionError("digest marker is malformed")
        if marker >= week:
            return
    conn.execute(
        "INSERT INTO config (key, value) VALUES (?, ?) "
        "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (DIGEST_MARKER_KEY, to_iso(week)),
    )
