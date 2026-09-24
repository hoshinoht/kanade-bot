"""Read-only source classification and idempotent v15 adoption seeding."""

from __future__ import annotations

import sqlite3
from collections import defaultdict
from datetime import UTC, datetime, time
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from bot.domain.timeutil import to_iso
from bot.domain.weeks import current_week_start

from .fingerprints import EVIDENCE_HASH_VERSION, source_evidence_hash
from .model import (
    AdoptionConflictError,
    AdoptionSeedReport,
    AdoptionSource,
    CandidateGroup,
    ResolutionState,
    SourceFamily,
    SourceKey,
    SourceReason,
)

_CLASSIFIER_ACTOR = "offline-classifier"
_MARKER_SOURCE_SECONDARY = "marker"
_TERMINAL_AMENDMENT_STATUSES = frozenset(
    {"confirmed", "rejected", "expired", "superseded", "withdrawn"}
)


def validate_seed_inputs(
    timezone: ZoneInfo, reset_weekday: int, reset_time: time, pinned_at: datetime
) -> tuple[ZoneInfo, datetime, str]:
    """Accept only a validated IANA timezone, reset, and one aware instant."""
    if not isinstance(timezone, ZoneInfo) or not timezone.key:
        raise ValueError("adoption requires a validated IANA timezone")
    try:
        validated_timezone = ZoneInfo(timezone.key)
    except (ZoneInfoNotFoundError, ValueError) as exc:
        raise ValueError("adoption timezone is no longer available") from exc
    if type(reset_weekday) is not int or not 0 <= reset_weekday <= 6:
        raise ValueError("reset weekday must be an integer from 0 through 6")
    if not isinstance(reset_time, time) or reset_time.tzinfo is not None:
        raise ValueError("reset time must be a timezone-naive wall-clock time")
    if not isinstance(pinned_at, datetime) or pinned_at.tzinfo is None:
        raise ValueError("adoption instant must be timezone-aware")
    try:
        if pinned_at.utcoffset() is None:
            raise ValueError("adoption instant must be timezone-aware")
        boss_week_start = to_iso(
            current_week_start(validated_timezone, reset_weekday, reset_time, pinned_at)
        )
    except (OverflowError, ValueError) as exc:
        raise ValueError("adoption reset or instant is invalid") from exc
    return validated_timezone, pinned_at.astimezone(UTC), boss_week_start


def classify_source_snapshot(
    conn: sqlite3.Connection,
    adoption_id: str,
    pinned_at: datetime,
    captured_at: str,
    boss_week_start: str,
    *,
    exclude_attempt_id: str | None = None,
) -> tuple[AdoptionSource, ...]:
    """Classify the five non-memory legacy send families from one SQL snapshot."""
    if not conn.in_transaction:
        raise RuntimeError("adoption source scan requires its stable write transaction")

    claims = _source_claims(conn, exclude_attempt_id=exclude_attempt_id)
    records: list[AdoptionSource] = []
    records.extend(_classify_reminders(conn, claims, adoption_id, pinned_at, captured_at))
    records.extend(_classify_amendments(conn, claims, adoption_id, captured_at))
    records.extend(_classify_digests(conn, claims, adoption_id, captured_at))
    records.extend(_classify_declines(conn, claims, adoption_id, captured_at))
    records.extend(_classify_debug_messages(conn, claims, adoption_id, captured_at))
    records.sort(key=lambda item: item.key)
    return tuple(records)


def persist_source_snapshot(
    conn: sqlite3.Connection,
    adoption_id: str,
    records: tuple[AdoptionSource, ...],
    captured_at: str,
    boss_week_start: str,
) -> AdoptionSeedReport:
    """Insert a complete seed once; never overwrite a prior decision or evidence."""
    if not conn.in_transaction:
        raise RuntimeError("adoption source persistence requires its stable transaction")

    proposed = {record.key: record for record in records}
    if len(proposed) != len(records):
        raise AdoptionConflictError("source snapshot contains duplicate native identities")

    existing_rows = conn.execute(
        "SELECT source_family, source_primary, source_secondary, evidence_hash_version, "
        "evidence_hash FROM adoption_sources WHERE adoption_id = ?",
        (adoption_id,),
    ).fetchall()
    existing = {SourceKey(row[0], row[1], row[2]): (row[3], row[4]) for row in existing_rows}

    changed: set[SourceKey] = set()
    if existing:
        changed.update(set(existing).symmetric_difference(proposed))
        changed.update(
            key
            for key in set(existing).intersection(proposed)
            if existing[key] != (EVIDENCE_HASH_VERSION, proposed[key].evidence_hash)
        )
    if changed:
        first = min(changed)
        raise AdoptionConflictError(
            f"adoption source snapshot changed at {first.family}:{first.primary}:{first.secondary}"
        )

    inserted = 0
    for record in records:
        if record.key in existing:
            continue
        resolution_at = captured_at if record.resolution_actor is not None else None
        resolution_reason = record.resolution_reason
        conn.execute(
            "INSERT INTO adoption_sources "
            "(adoption_id, source_family, source_primary, source_secondary, reason, "
            "resolution_state, captured_at, classified_at, evidence_hash_version, evidence_hash, "
            "legacy_channel_id, legacy_message_id, resolution_actor, resolution_at, "
            "resolution_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                adoption_id,
                record.key.family,
                record.key.primary,
                record.key.secondary,
                record.reason.value,
                record.resolution_state.value,
                captured_at,
                captured_at,
                EVIDENCE_HASH_VERSION,
                record.evidence_hash,
                record.legacy_channel_id,
                record.legacy_message_id,
                record.resolution_actor,
                resolution_at,
                resolution_reason,
            ),
        )
        inserted += 1

    groups = _candidate_groups(conn, adoption_id)
    return AdoptionSeedReport(
        adoption_id=adoption_id,
        pinned_at=captured_at,
        boss_week_start=boss_week_start,
        classified_sources=len(records),
        inserted_sources=inserted,
        unchanged_sources=len(records) - inserted,
        candidate_groups=groups,
    )


def _source_claims(
    conn: sqlite3.Connection, *, exclude_attempt_id: str | None = None
) -> dict[tuple[str, str, str], tuple[dict[str, Any], ...]]:
    claims: defaultdict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    rows = conn.execute(
        "SELECT t.binding_type, t.key_primary, COALESCE(t.key_secondary, ''), "
        "t.attempt_id, t.released_at, a.origin, a.state, a.destination_kind, "
        "a.channel_id, a.recipient_id, a.message_id "
        "FROM delivery_attempt_targets AS t "
        "JOIN delivery_attempts AS a ON a.attempt_id = t.attempt_id "
        "WHERE t.binding_type IN ('reminder', 'digest', 'decline', 'card', 'debug_card') "
        "AND (? IS NULL OR a.attempt_id != ?) "
        "ORDER BY t.binding_type, t.key_primary, COALESCE(t.key_secondary, ''), t.attempt_id",
        (exclude_attempt_id, exclude_attempt_id),
    )
    for row in rows:
        key = (row[0], row[1], row[2])
        claims[key].append(
            {
                "attempt_id": row[3],
                "released_at": row[4],
                "origin": row[5],
                "state": row[6],
                "destination_kind": row[7],
                "channel_id": row[8],
                "recipient_id": row[9],
                "message_id": row[10],
            }
        )
    return {key: tuple(value) for key, value in claims.items()}


def _classify_reminders(
    conn: sqlite3.Connection,
    claims: dict[tuple[str, str, str], tuple[dict[str, Any], ...]],
    adoption_id: str,
    pinned_at: datetime,
    captured_at: str,
) -> list[AdoptionSource]:
    records = []
    rows = conn.execute(
        "SELECT r.id, r.run_id, r.fire_at, r.sent_at, r.message_id, "
        "rn.id, rn.channel_id FROM reminders AS r "
        "LEFT JOIN runs AS rn ON rn.id = r.run_id ORDER BY r.id"
    )
    for row in rows:
        source_id, run_id, fire_raw, sent_raw, message_raw, matched_run, channel_raw = row
        fire_at = _aware_timestamp(fire_raw)
        sent_at = _aware_timestamp(sent_raw) if sent_raw is not None else None
        message_id = _optional_text(message_raw)
        channel_id = _optional_text(channel_raw)
        facts = {
            "run_id": run_id,
            "run_exists": matched_run is not None,
            "fire_at": _canonical_timestamp(fire_raw),
            "sent_at": _canonical_timestamp(sent_raw),
            "message_id": message_id,
            "channel_id": channel_id,
        }
        has_sent = sent_raw is not None
        has_message = message_raw is not None and bool(message_id and message_id.strip())
        if sent_raw is None and message_raw is None:
            if fire_at is None:
                reason = SourceReason.INCONSISTENT_SOURCE
                state = ResolutionState.PENDING
            elif fire_at > pinned_at:
                reason = SourceReason.FUTURE_UNSENT
                state = ResolutionState.PROVEN_UNSENT
            else:
                reason = SourceReason.DUE_UNBOUND
                state = ResolutionState.PENDING
        elif not has_sent or not has_message:
            reason = SourceReason.PARTIAL_BINDING
            state = ResolutionState.PENDING
        elif sent_at is None:
            reason = SourceReason.INCONSISTENT_SOURCE
            state = ResolutionState.PENDING
        elif not channel_id:
            reason = SourceReason.PARTIAL_BINDING
            state = ResolutionState.PENDING
        else:
            reason = SourceReason.CANDIDATE_MESSAGE
            state = ResolutionState.PENDING
        records.append(
            _record(
                adoption_id,
                SourceFamily.REMINDERS,
                source_id,
                "",
                reason,
                state,
                facts,
                claims.get(("reminder", source_id, ""), ()),
                captured_at,
                channel_id if message_raw is not None else None,
                message_id if message_raw is not None else None,
            )
        )
    return records


def _classify_amendments(
    conn: sqlite3.Connection,
    claims: dict[tuple[str, str, str], tuple[dict[str, Any], ...]],
    adoption_id: str,
    captured_at: str,
) -> list[AdoptionSource]:
    records = []
    rows = conn.execute(
        "SELECT id, status, channel_id, proposal_message_id FROM amendments ORDER BY id"
    )
    for row in rows:
        source_id, status, channel_raw, message_raw = row
        channel_id = _optional_text(channel_raw)
        message_id = _optional_text(message_raw)
        facts = {"status": status, "channel_id": channel_id, "message_id": message_id}
        message_present = bool(message_id and message_id.strip())
        channel_present = bool(channel_id and channel_id.strip())
        if status == "proposed":
            if message_present and channel_present:
                reason = SourceReason.CANDIDATE_MESSAGE
                state = ResolutionState.PENDING
            elif message_present:
                reason = SourceReason.PARTIAL_BINDING
                state = ResolutionState.PENDING
            else:
                reason = SourceReason.PROPOSED_UNBOUND
                state = ResolutionState.PENDING
        elif status in _TERMINAL_AMENDMENT_STATUSES:
            if message_present:
                if channel_present:
                    reason = SourceReason.CANDIDATE_MESSAGE
                    state = ResolutionState.PENDING
                else:
                    reason = SourceReason.PARTIAL_BINDING
                    state = ResolutionState.PENDING
            else:
                reason = SourceReason.TERMINAL_UNBOUND
                state = ResolutionState.TERMINAL_NO_REPLAY
        else:
            reason = SourceReason.INCONSISTENT_SOURCE
            state = ResolutionState.PENDING
        records.append(
            _record(
                adoption_id,
                SourceFamily.AMENDMENTS,
                source_id,
                "",
                reason,
                state,
                facts,
                claims.get(("card", source_id, ""), ()),
                captured_at,
                channel_id if message_present else None,
                message_id if message_present else None,
            )
        )
    return records


def _classify_digests(
    conn: sqlite3.Connection,
    claims: dict[tuple[str, str, str], tuple[dict[str, Any], ...]],
    adoption_id: str,
    captured_at: str,
) -> list[AdoptionSource]:
    rows = conn.execute(
        "SELECT week_start, channel_id, message_id, retired_at FROM weekly_digests "
        "ORDER BY week_start"
    ).fetchall()
    by_week = {row[0]: row for row in rows}
    marker_row = conn.execute("SELECT value FROM config WHERE key = 'last_digest_week'").fetchone()
    marker = marker_row[0] if marker_row is not None else None
    records = []
    for row in rows:
        week_start, channel_raw, message_raw, retired_at = row
        channel_id = _optional_text(channel_raw)
        message_id = _optional_text(message_raw)
        retired = retired_at is not None
        reason = SourceReason.DIGEST_RETIRED_MISMATCH if retired else SourceReason.CANDIDATE_MESSAGE
        facts = {
            "week_start": week_start,
            "channel_id": channel_id,
            "message_id": message_id,
            "retired_at": retired_at,
        }
        records.append(
            _record(
                adoption_id,
                SourceFamily.WEEKLY_DIGESTS,
                week_start,
                "",
                reason,
                ResolutionState.PENDING,
                facts,
                claims.get(("digest", week_start, ""), ()),
                captured_at,
                channel_id,
                message_id,
            )
        )

    gap: tuple[str, str, dict[str, object]] | None = None
    if marker is None:
        gap = ("marker:none", "missing", {"last_digest_week": None})
    elif marker not in by_week:
        gap = (f"marker:{marker}", "missing", {"last_digest_week": marker, "row": None})
    else:
        marker_digest = by_week[marker]
        if marker_digest[3] is not None:
            gap = (
                f"marker_gap:{marker}",
                "retired",
                {
                    "last_digest_week": marker,
                    "week_start": marker_digest[0],
                    "channel_id": marker_digest[1],
                    "message_id": marker_digest[2],
                    "retired_at": marker_digest[3],
                },
            )
        elif any(row[0] != marker and row[3] is None for row in rows):
            mismatched = [
                {
                    "week_start": row[0],
                    "channel_id": row[1],
                    "message_id": row[2],
                }
                for row in rows
                if row[0] != marker and row[3] is None
            ]
            gap = (
                f"marker_mismatch:{marker}",
                "active_rows_disagree",
                {"last_digest_week": marker, "active_rows": mismatched},
            )
    if gap is not None:
        primary, condition, facts = gap
        _append_gap_record(
            records,
            adoption_id,
            primary,
            condition,
            facts,
            captured_at,
        )
    return records


def _append_gap_record(
    records: list[AdoptionSource],
    adoption_id: str,
    primary: str,
    condition: str,
    facts: dict[str, object],
    captured_at: str,
) -> None:
    records.append(
        _record(
            adoption_id,
            SourceFamily.WEEKLY_DIGESTS,
            primary,
            _MARKER_SOURCE_SECONDARY,
            SourceReason.DIGEST_MARKER_GAP,
            ResolutionState.PENDING,
            {"condition": condition, **facts},
            (),
            captured_at,
        )
    )


def _classify_declines(
    conn: sqlite3.Connection,
    claims: dict[tuple[str, str, str], tuple[dict[str, Any], ...]],
    adoption_id: str,
    captured_at: str,
) -> list[AdoptionSource]:
    records = []
    rows = conn.execute(
        "SELECT run_id, user_id, channel_id, message_id, notified_at "
        "FROM decline_notices ORDER BY run_id, user_id"
    )
    for row in rows:
        run_id, user_id, channel_raw, message_raw, notified_at = row
        channel_id = _optional_text(channel_raw)
        message_id = _optional_text(message_raw)
        if channel_id and channel_id.strip() and message_id and message_id.strip():
            reason = SourceReason.CANDIDATE_MESSAGE
        else:
            reason = SourceReason.PARTIAL_BINDING
        records.append(
            _record(
                adoption_id,
                SourceFamily.DECLINE_NOTICES,
                run_id,
                user_id,
                reason,
                ResolutionState.PENDING,
                {
                    "run_id": run_id,
                    "user_id": user_id,
                    "channel_id": channel_id,
                    "message_id": message_id,
                    "notified_at": _canonical_timestamp(notified_at),
                },
                claims.get(("decline", run_id, user_id), ()),
                captured_at,
                channel_id,
                message_id,
            )
        )
    return records


def _classify_debug_messages(
    conn: sqlite3.Connection,
    claims: dict[tuple[str, str, str], tuple[dict[str, Any], ...]],
    adoption_id: str,
    captured_at: str,
) -> list[AdoptionSource]:
    records = []
    rows = conn.execute(
        "SELECT message_id, run_id, channel_id FROM debug_messages ORDER BY message_id"
    )
    for row in rows:
        message_id, run_id, channel_raw = row
        channel_id = _optional_text(channel_raw)
        reason = (
            SourceReason.CANDIDATE_MESSAGE
            if channel_id and channel_id.strip()
            else SourceReason.DEBUG_MISSING_CHANNEL
        )
        records.append(
            _record(
                adoption_id,
                SourceFamily.DEBUG_MESSAGES,
                message_id,
                "",
                reason,
                ResolutionState.PENDING,
                {"message_id": message_id, "run_id": run_id, "channel_id": channel_id},
                claims.get(("debug_card", message_id, ""), ()),
                captured_at,
                channel_id,
                message_id,
            )
        )
    return records


def _record(
    adoption_id: str,
    family: SourceFamily,
    primary: str,
    secondary: str,
    reason: SourceReason,
    state: ResolutionState,
    facts: dict[str, object],
    claims: tuple[dict[str, Any], ...],
    captured_at: str,
    channel_id: str | None = None,
    message_id: str | None = None,
) -> AdoptionSource:
    evidence = {
        "domain": "kanade-bot:adoption-source-evidence:v1",
        "family": family.value,
        "identity": {"primary": primary, "secondary": secondary},
        "facts": facts,
        "journal_claims": list(claims),
    }
    if claims:
        reason = SourceReason.JOURNAL_CLAIM_CONFLICT
        state = ResolutionState.PENDING
    resolution_actor = None
    resolution_reason = None
    if state is ResolutionState.PROVEN_UNSENT:
        resolution_actor = _CLASSIFIER_ACTOR
        resolution_reason = (
            "both delivery bindings are null and fire_at is after the pinned instant"
        )
    elif state is ResolutionState.TERMINAL_NO_REPLAY:
        resolution_actor = _CLASSIFIER_ACTOR
        resolution_reason = "terminal amendment has no proposal message; do not replay"
    return AdoptionSource(
        key=SourceKey(family.value, primary, secondary),
        reason=reason,
        resolution_state=state,
        evidence_hash=source_evidence_hash(evidence),
        legacy_channel_id=channel_id,
        legacy_message_id=message_id,
        resolution_actor=resolution_actor,
        resolution_reason=resolution_reason,
    )


def _candidate_groups(conn: sqlite3.Connection, adoption_id: str) -> tuple[CandidateGroup, ...]:
    rows = conn.execute(
        "SELECT source_family, source_primary, source_secondary, legacy_channel_id, "
        "legacy_message_id FROM adoption_sources WHERE adoption_id = ? "
        "AND reason = 'candidate_message' AND resolution_state = 'pending' "
        "AND legacy_channel_id IS NOT NULL AND legacy_channel_id != '' "
        "AND legacy_message_id IS NOT NULL AND legacy_message_id != '' "
        "ORDER BY source_family, legacy_channel_id, legacy_message_id, source_primary, "
        "source_secondary",
        (adoption_id,),
    )
    grouped: defaultdict[tuple[str, str, str], list[SourceKey]] = defaultdict(list)
    for row in rows:
        family, primary, secondary, channel_id, message_id = row
        grouped[(family, channel_id, message_id)].append(SourceKey(family, primary, secondary))
    return tuple(
        CandidateGroup(family, channel_id, message_id, tuple(targets))
        for (family, channel_id, message_id), targets in sorted(grouped.items())
    )


def _optional_text(value: object) -> str | None:
    return None if value is None else str(value)


def _aware_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
        if parsed.tzinfo is None or parsed.utcoffset() is None:
            return None
        return parsed.astimezone(UTC)
    except (OverflowError, ValueError):
        return None


def _canonical_timestamp(value: object) -> str | None:
    if value is None:
        return None
    parsed = _aware_timestamp(value)
    return to_iso(parsed) if parsed is not None else str(value)
