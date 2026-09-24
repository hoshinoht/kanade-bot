"""Private, transaction-scoped adoption binding and reasoned retirement.

The accepted evidence is synthetic input, not proof of remote authorship. A
later trusted reconciler must fetch and authenticate Discord messages before
calling these offline primitives.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import uuid
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from bot.domain.timeutil import to_iso

from ..delivery.fingerprints import FINGERPRINT_VERSION, observable_fingerprint
from ..delivery.model import Destination
from .classification import classify_source_snapshot
from .evidence import MessageObservation, _snowflake
from .fingerprints import EVIDENCE_HASH_VERSION, source_evidence_hash
from .model import AdoptionConflictError, SourceFamily, SourceKey

if TYPE_CHECKING:
    from ..guard import WriteGuard, _Authority


class AdoptionResolutionError(RuntimeError):
    """A synthetic observation or native adoption group cannot be resolved safely."""


class AdoptionEvidenceError(AdoptionResolutionError):
    """The observation is incomplete, ambiguous, or does not match the candidate identity."""


@dataclass(frozen=True, slots=True)
class PreparedMessageEvidence:
    family: SourceFamily
    guild_id: str
    channel_id: str
    message_id: str
    observation: MessageObservation
    observable_hash: str


_FAMILY_BINDING = {
    SourceFamily.REMINDERS: "reminder",
    SourceFamily.AMENDMENTS: "card",
    SourceFamily.WEEKLY_DIGESTS: "digest",
    SourceFamily.DECLINE_NOTICES: "decline",
    SourceFamily.DEBUG_MESSAGES: "debug_card",
}
_GROUPABLE_FAMILIES = frozenset({SourceFamily.REMINDERS, SourceFamily.AMENDMENTS})


def validate_resolution_metadata(actor: str, reason: str, at: datetime) -> tuple[str, str, str]:
    return (
        _bounded_text(actor, "resolution actor", 128),
        _bounded_text(reason, "resolution reason", 512),
        _aware_iso(at),
    )


def prepare_message_evidence(
    family: SourceFamily | str,
    channel_id: str | int,
    message_id: str | int,
    guild_id: str | int,
    bot_author_id: str | int,
    observations: tuple[MessageObservation, ...],
) -> PreparedMessageEvidence:
    """Select exactly one complete bot-authored observation for a stored group."""
    try:
        source_family = SourceFamily(family)
    except (TypeError, ValueError) as exc:
        raise AdoptionEvidenceError("unsupported adoption source family") from exc
    try:
        destination = Destination.channel(guild_id, channel_id)
        expected_message_id = _snowflake(message_id, "candidate message id")
        expected_author_id = _snowflake(bot_author_id, "configured bot author id")
    except ValueError as exc:
        raise AdoptionEvidenceError("candidate Discord identity is invalid") from exc
    if not isinstance(observations, tuple) or any(
        not isinstance(item, MessageObservation) for item in observations
    ):
        raise AdoptionEvidenceError("message observations must be a typed immutable tuple")

    matches = [
        item
        for item in observations
        if item.channel_id == destination.channel_id and item.message_id == expected_message_id
    ]
    if not matches:
        raise AdoptionEvidenceError("no observed message matches the candidate identity")
    if len(matches) != 1:
        raise AdoptionEvidenceError("multiple observed messages match the candidate identity")
    observation = matches[0]
    if observation.guild_id != destination.guild_id:
        raise AdoptionEvidenceError("observed message belongs to another guild")
    if observation.author_id != expected_author_id:
        raise AdoptionEvidenceError("observed message was not authored by the configured bot")
    if observation.unsupported_components:
        raise AdoptionEvidenceError("observed message contains unsupported components")

    return PreparedMessageEvidence(
        source_family,
        destination.guild_id or "",
        destination.channel_id or "",
        expected_message_id,
        observation,
        observable_fingerprint(destination, observation.observed),
    )


def bind_verified_group(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    *,
    adoption_id: str,
    instance_id: str,
    evidence: PreparedMessageEvidence,
    actor: str,
    reason: str,
    resolved_at: str,
) -> str:
    """Bind the entire persisted family/message group to one adopted attempt."""
    _require_transaction(conn)
    rows = _load_message_group(conn, adoption_id, evidence)
    existing_attempt_ids = {row["adopted_attempt_id"] for row in rows}
    idempotent_attempt_id: str | None = None
    if all(row["resolution_state"] == "verified_bound" for row in rows):
        if len(existing_attempt_ids) != 1 or None in existing_attempt_ids:
            raise AdoptionResolutionError("verified source group has inconsistent attempt claims")
        idempotent_attempt_id = next(iter(existing_attempt_ids))
    elif any(row["resolution_state"] != "pending" for row in rows):
        raise AdoptionResolutionError("message group is not wholly pending")

    current = _validate_current_sources(
        conn,
        adoption_id,
        rows,
        exclude_attempt_id=idempotent_attempt_id,
        expected_group=(evidence.family, evidence.channel_id, evidence.message_id),
    )
    keys = tuple(_source_key(row) for row in rows)
    if any(
        row["reason"] != "candidate_message" or current[key].reason.value != "candidate_message"
        for row, key in zip(rows, keys, strict=True)
    ):
        raise AdoptionResolutionError("message group includes a non-candidate source")
    claims = _target_claims(evidence.family, keys)
    _validate_target_group_cardinality(evidence.family, claims)
    _require_unique_claims(claims)
    _assert_no_competing_claims(conn, claims, allow_attempt_id=idempotent_attempt_id)
    fingerprint = _group_source_fingerprint(adoption_id, evidence.family, rows)
    dedupe_key = _native_dedupe_key(claims)

    if idempotent_attempt_id is not None:
        _assert_no_message_attempt_conflict(conn, evidence, allow_attempt_id=idempotent_attempt_id)
        _verify_idempotent_attempt(
            conn,
            adoption_id,
            evidence,
            rows,
            claims,
            idempotent_attempt_id,
            fingerprint,
            dedupe_key,
            actor,
            reason,
        )
        return idempotent_attempt_id

    _assert_no_message_attempt_conflict(conn, evidence)
    attempt_id = str(uuid.uuid4())
    operation_id = f"adoption:{adoption_id}"
    effect_ordinal = int(
        conn.execute(
            "SELECT COALESCE(MAX(effect_ordinal), -1) + 1 FROM delivery_attempts "
            "WHERE operation_id = ?",
            (operation_id,),
        ).fetchone()[0]
    )
    try:
        with guard._adoption_group_insert_scope(authority):
            conn.execute(
                "INSERT INTO delivery_attempts "
                "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, "
                "effect_kind, "
                "dedupe_scope, dedupe_key, dedupe_active, state, destination_kind, guild_id, "
                "channel_id, recipient_id, message_id, fingerprint_version, request_fingerprint, "
                "observable_fingerprint, intended_at, resolved_at, resolved_by, resolution_reason) "
                "VALUES (?, ?, ?, ?, 'adoption', ?, 'native', ?, 1, 'bound', 'channel', ?, ?, "
                "NULL, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    attempt_id,
                    operation_id,
                    effect_ordinal,
                    instance_id,
                    f"adoption.{evidence.family.value}",
                    dedupe_key,
                    evidence.guild_id,
                    evidence.channel_id,
                    evidence.message_id,
                    FINGERPRINT_VERSION,
                    fingerprint,
                    evidence.observable_hash,
                    resolved_at,
                    resolved_at,
                    actor,
                    reason,
                ),
            )
        for ordinal, claim in enumerate(claims):
            with guard._adoption_group_insert_scope(authority):
                conn.execute(
                    "INSERT INTO delivery_attempt_targets "
                    "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
                    "VALUES (?, ?, ?, ?, ?)",
                    (attempt_id, ordinal, *claim),
                )
        for row in rows:
            _bind_source_cas(
                conn,
                guard,
                authority,
                row,
                attempt_id,
                actor,
                reason,
                resolved_at,
            )
    except sqlite3.IntegrityError as exc:
        raise AdoptionResolutionError("adoption binding conflicts with an existing claim") from exc
    return attempt_id


def retire_pending_source(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    *,
    adoption_id: str,
    key: SourceKey,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    """Reasonedly tombstone one pending source without creating a send attempt."""
    _require_transaction(conn)
    row = conn.execute(
        "SELECT * FROM adoption_sources WHERE adoption_id = ? AND source_family = ? "
        "AND source_primary = ? AND source_secondary = ?",
        (adoption_id, key.family, key.primary, key.secondary),
    ).fetchone()
    if row is None or row["resolution_state"] != "pending" or row["adopted_attempt_id"] is not None:
        raise AdoptionResolutionError("source is not pending for reasoned retirement")
    _validate_current_sources(conn, adoption_id, (row,))
    claim = _target_claims(SourceFamily(key.family), (key,))[0]
    _assert_no_competing_claims(conn, (claim,))
    _retire_source_cas(conn, guard, authority, row, actor, reason, resolved_at)


def retire_adopted_attempt(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    *,
    adoption_id: str,
    attempt_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    """Atomically release and retire one exact adopted attempt and all its sources."""
    _require_transaction(conn)
    attempt = conn.execute(
        "SELECT * FROM delivery_attempts WHERE attempt_id = ?", (attempt_id,)
    ).fetchone()
    if (
        attempt is None
        or attempt["origin"] != "adoption"
        or attempt["state"] != "bound"
        or attempt["dedupe_active"] != 1
        or attempt["destination_kind"] != "channel"
    ):
        raise AdoptionResolutionError("attempt is not an active bound adoption attempt")

    rows = conn.execute(
        "SELECT * FROM adoption_sources WHERE adoption_id = ? AND adopted_attempt_id = ? "
        "AND resolution_state = 'verified_bound' ORDER BY source_family, source_primary, "
        "source_secondary",
        (adoption_id, attempt_id),
    ).fetchall()
    if not rows:
        raise AdoptionResolutionError("adoption attempt has no bound source claims")
    families = {row["source_family"] for row in rows}
    groups = {(row["legacy_channel_id"], row["legacy_message_id"]) for row in rows}
    if len(families) != 1 or len(groups) != 1:
        raise AdoptionResolutionError("adoption attempt crosses source families or messages")
    family = SourceFamily(next(iter(families)))
    channel_id, message_id = next(iter(groups))
    if (
        channel_id != attempt["channel_id"]
        or message_id != attempt["message_id"]
        or attempt["operation_id"] != f"adoption:{adoption_id}"
        or attempt["effect_kind"] != f"adoption.{family.value}"
    ):
        raise AdoptionResolutionError("adoption attempt does not match its exact source group")

    _validate_current_sources(
        conn,
        adoption_id,
        rows,
        exclude_attempt_id=attempt_id,
        expected_group=(family, channel_id, message_id),
    )
    keys = tuple(_source_key(row) for row in rows)
    claims = _target_claims(family, keys)
    _validate_target_group_cardinality(family, claims)
    _require_unique_claims(claims)
    _assert_no_competing_claims(conn, claims, allow_attempt_id=attempt_id)
    source_fingerprint = _group_source_fingerprint(adoption_id, family, rows)
    if attempt["request_fingerprint"] != source_fingerprint:
        raise AdoptionConflictError("adopted attempt source evidence no longer matches")

    stored_claims = tuple(
        (row["binding_type"], row["key_primary"], row["key_secondary"] or "")
        for row in conn.execute(
            "SELECT binding_type, key_primary, key_secondary FROM delivery_attempt_targets "
            "WHERE attempt_id = ? ORDER BY target_ordinal",
            (attempt_id,),
        )
    )
    if tuple(sorted(stored_claims)) != tuple(sorted(claims)) or len(stored_claims) != len(claims):
        raise AdoptionResolutionError("adopted attempt does not own the full source group")

    released = _release_attempt_targets(
        conn, guard, authority, attempt_id, actor, reason, resolved_at
    )
    if released != len(claims):
        raise AdoptionResolutionError("adopted attempt did not release every native claim")
    retired = _retire_attempt_cas(conn, guard, authority, attempt_id, actor, reason, resolved_at)
    if retired != 1:
        raise AdoptionResolutionError("adopted attempt was not retired exactly once")
    for row in rows:
        _retire_bound_source_cas(
            conn, guard, authority, row, attempt_id, actor, reason, resolved_at
        )


def _load_message_group(
    conn: sqlite3.Connection, adoption_id: str, evidence: PreparedMessageEvidence
) -> tuple[sqlite3.Row, ...]:
    families = {
        row[0]
        for row in conn.execute(
            "SELECT DISTINCT source_family FROM adoption_sources WHERE adoption_id = ? "
            "AND legacy_channel_id = ? AND legacy_message_id = ?",
            (adoption_id, evidence.channel_id, evidence.message_id),
        )
    }
    if families != {evidence.family.value}:
        raise AdoptionResolutionError("message identity is missing or claimed by multiple families")
    rows = tuple(
        conn.execute(
            "SELECT * FROM adoption_sources WHERE adoption_id = ? AND source_family = ? "
            "AND legacy_channel_id = ? AND legacy_message_id = ? "
            "ORDER BY source_primary, source_secondary",
            (adoption_id, evidence.family.value, evidence.channel_id, evidence.message_id),
        )
    )
    if not rows:
        raise AdoptionResolutionError("candidate message has no persisted native group")
    if any(row["reason"] != "candidate_message" for row in rows):
        raise AdoptionResolutionError("message group includes a non-candidate source")
    return rows


def _validate_current_sources(
    conn: sqlite3.Connection,
    adoption_id: str,
    rows: tuple[sqlite3.Row, ...],
    *,
    exclude_attempt_id: str | None = None,
    expected_group: tuple[SourceFamily, str, str] | None = None,
) -> dict[SourceKey, object]:
    captured = {row["captured_at"] for row in rows}
    if len(captured) != 1:
        raise AdoptionConflictError("source group has inconsistent capture times")
    captured_at = next(iter(captured))
    try:
        pinned_at = datetime.fromisoformat(captured_at)
        if pinned_at.tzinfo is None or pinned_at.utcoffset() is None:
            raise ValueError
        pinned_at = pinned_at.astimezone(UTC)
    except (OverflowError, TypeError, ValueError) as exc:
        raise AdoptionConflictError("source evidence has an invalid capture time") from exc
    current_records = classify_source_snapshot(
        conn,
        adoption_id,
        pinned_at,
        captured_at,
        "",
        exclude_attempt_id=exclude_attempt_id,
    )
    current = {record.key: record for record in current_records}
    for row in rows:
        key = _source_key(row)
        record = current.get(key)
        if (
            record is None
            or row["evidence_hash_version"] != EVIDENCE_HASH_VERSION
            or row["evidence_hash"] != record.evidence_hash
            or row["reason"] != record.reason.value
        ):
            raise AdoptionConflictError(
                "source facts or journal claims changed at "
                f"{key.family}:{key.primary}:{key.secondary}"
            )
    if expected_group is not None:
        family, channel_id, message_id = expected_group
        persisted_keys = tuple(sorted(_source_key(row) for row in rows))
        current_keys = tuple(
            sorted(
                record.key
                for record in current_records
                if record.key.family == family.value
                and record.legacy_channel_id == channel_id
                and record.legacy_message_id == message_id
            )
        )
        if current_keys != persisted_keys:
            raise AdoptionConflictError("current native message group changed after source capture")
    return current


def _source_key(row: sqlite3.Row) -> SourceKey:
    return SourceKey(row["source_family"], row["source_primary"], row["source_secondary"])


def _target_claims(
    family: SourceFamily, keys: tuple[SourceKey, ...]
) -> tuple[tuple[str, str, str], ...]:
    binding = _FAMILY_BINDING[family]
    claims = []
    for key in sorted(keys):
        if key.family != family.value:
            raise AdoptionResolutionError("native group crosses source families")
        if family is SourceFamily.DECLINE_NOTICES:
            if not key.secondary:
                raise AdoptionResolutionError("decline source is missing its user identity")
            primary, secondary = key.primary, key.secondary
        else:
            if key.secondary:
                raise AdoptionResolutionError("source family has an unsupported secondary key")
            primary, secondary = key.primary, ""
        claims.append((binding, primary, secondary))
    return tuple(claims)


def _require_unique_claims(claims: tuple[tuple[str, str, str], ...]) -> None:
    if not claims or len(claims) != len(set(claims)):
        raise AdoptionResolutionError("native group contains duplicate target claims")


def _validate_target_group_cardinality(
    family: SourceFamily, claims: tuple[tuple[str, str, str], ...]
) -> None:
    if len(claims) > 1 and family not in _GROUPABLE_FAMILIES:
        raise AdoptionResolutionError("only reminder or card groups may contain multiple targets")


def _assert_no_competing_claims(
    conn: sqlite3.Connection,
    claims: tuple[tuple[str, str, str], ...],
    *,
    allow_attempt_id: str | None = None,
) -> None:
    for binding_type, primary, secondary in claims:
        rows = conn.execute(
            "SELECT attempt_id FROM delivery_attempt_targets WHERE binding_type = ? "
            "AND key_primary = ? AND COALESCE(key_secondary, '') = ? AND released_at IS NULL",
            (binding_type, primary, secondary),
        )
        if any(row[0] != allow_attempt_id for row in rows):
            raise AdoptionResolutionError("native target has a conflicting delivery claim")


def _assert_no_message_attempt_conflict(
    conn: sqlite3.Connection,
    evidence: PreparedMessageEvidence,
    *,
    allow_attempt_id: str | None = None,
) -> None:
    rows = conn.execute(
        "SELECT attempt_id FROM delivery_attempts WHERE destination_kind = 'channel' "
        "AND guild_id = ? AND channel_id = ? AND message_id = ?",
        (evidence.guild_id, evidence.channel_id, evidence.message_id),
    )
    if any(row[0] != allow_attempt_id for row in rows):
        raise AdoptionResolutionError("message identity already belongs to another attempt")


def _group_source_fingerprint(
    adoption_id: str, family: SourceFamily, rows: tuple[sqlite3.Row, ...]
) -> str:
    return source_evidence_hash(
        {
            "evidence_kind": "adoption-bound-source-group:v1",
            "adoption_id": adoption_id,
            "family": family.value,
            "sources": [
                {
                    "family": row["source_family"],
                    "primary": row["source_primary"],
                    "secondary": row["source_secondary"],
                    "evidence_hash_version": row["evidence_hash_version"],
                    "evidence_hash": row["evidence_hash"],
                }
                for row in sorted(rows, key=lambda item: _source_key(item))
            ],
        }
    )


def _native_dedupe_key(claims: tuple[tuple[str, str, str], ...]) -> str:
    encoded = json.dumps(
        ["kanade.delivery.dedupe.v1", "native", sorted(claims)], separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _verify_idempotent_attempt(
    conn: sqlite3.Connection,
    adoption_id: str,
    evidence: PreparedMessageEvidence,
    rows: tuple[sqlite3.Row, ...],
    claims: tuple[tuple[str, str, str], ...],
    attempt_id: str,
    fingerprint: str,
    dedupe_key: str,
    actor: str,
    reason: str,
) -> None:
    attempt = conn.execute(
        "SELECT * FROM delivery_attempts WHERE attempt_id = ?", (attempt_id,)
    ).fetchone()
    if (
        attempt is None
        or attempt["origin"] != "adoption"
        or attempt["state"] != "bound"
        or attempt["dedupe_active"] != 1
        or attempt["operation_id"] != f"adoption:{adoption_id}"
        or attempt["effect_kind"] != f"adoption.{evidence.family.value}"
        or attempt["destination_kind"] != "channel"
        or attempt["guild_id"] != evidence.guild_id
        or attempt["channel_id"] != evidence.channel_id
        or attempt["message_id"] != evidence.message_id
        or attempt["request_fingerprint"] != fingerprint
        or attempt["observable_fingerprint"] != evidence.observable_hash
        or attempt["dedupe_key"] != dedupe_key
        or attempt["resolved_by"] != actor
        or attempt["resolution_reason"] != reason
    ):
        raise AdoptionResolutionError("repeated adoption evidence conflicts with its bound attempt")
    if any(row["resolution_actor"] != actor or row["resolution_reason"] != reason for row in rows):
        raise AdoptionResolutionError("repeated adoption decision conflicts with stored resolution")
    stored = tuple(
        (row["binding_type"], row["key_primary"], row["key_secondary"] or "")
        for row in conn.execute(
            "SELECT binding_type, key_primary, key_secondary FROM delivery_attempt_targets "
            "WHERE attempt_id = ? ORDER BY target_ordinal",
            (attempt_id,),
        )
    )
    if tuple(sorted(stored)) != tuple(sorted(claims)) or len(stored) != len(claims):
        raise AdoptionResolutionError("repeated adoption attempt has different target claims")


def _bind_source_cas(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    row: sqlite3.Row,
    attempt_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    with guard._adoption_source_bind_scope(authority):
        changed = conn.execute(
            "UPDATE adoption_sources SET resolution_state = 'verified_bound', "
            "adopted_attempt_id = ?, resolution_actor = ?, resolution_at = ?, "
            "resolution_reason = ? WHERE adoption_id = ? AND source_family = ? "
            "AND source_primary = ? AND source_secondary = ? AND resolution_state = 'pending' "
            "AND evidence_hash_version = ? AND evidence_hash = ? "
            "AND legacy_channel_id = ? AND legacy_message_id = ?",
            (
                attempt_id,
                actor,
                resolved_at,
                reason,
                row["adoption_id"],
                row["source_family"],
                row["source_primary"],
                row["source_secondary"],
                EVIDENCE_HASH_VERSION,
                row["evidence_hash"],
                row["legacy_channel_id"],
                row["legacy_message_id"],
            ),
        ).rowcount
    if changed != 1:
        raise AdoptionConflictError("source compare-and-set failed during group binding")


def _retire_source_cas(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    row: sqlite3.Row,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    with guard._adoption_source_retire_scope(authority):
        changed = conn.execute(
            "UPDATE adoption_sources SET resolution_state = 'reasoned_retired', "
            "resolution_actor = ?, resolution_at = ?, resolution_reason = ? "
            "WHERE adoption_id = ? AND source_family = ? AND source_primary = ? "
            "AND source_secondary = ? AND resolution_state = 'pending' "
            "AND evidence_hash_version = ? AND evidence_hash = ?",
            (
                actor,
                resolved_at,
                reason,
                row["adoption_id"],
                row["source_family"],
                row["source_primary"],
                row["source_secondary"],
                EVIDENCE_HASH_VERSION,
                row["evidence_hash"],
            ),
        ).rowcount
    if changed != 1:
        raise AdoptionConflictError("source compare-and-set failed during retirement")


def _release_attempt_targets(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    attempt_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> int:
    with guard._adoption_attempt_retire_scope(authority):
        return conn.execute(
            "UPDATE delivery_attempt_targets SET released_at = ?, release_actor = ?, "
            "release_reason = ? WHERE attempt_id = ? AND released_at IS NULL",
            (resolved_at, actor, reason, attempt_id),
        ).rowcount


def _retire_attempt_cas(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    attempt_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> int:
    with guard._adoption_attempt_retire_scope(authority):
        return conn.execute(
            "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, "
            "resolved_at = ?, resolved_by = ?, resolution_reason = ? "
            "WHERE attempt_id = ? AND origin = 'adoption' AND state = 'bound' "
            "AND dedupe_active = 1",
            (resolved_at, actor, reason, attempt_id),
        ).rowcount


def _retire_bound_source_cas(
    conn: sqlite3.Connection,
    guard: WriteGuard,
    authority: _Authority,
    row: sqlite3.Row,
    attempt_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    with guard._adoption_source_retire_scope(authority):
        changed = conn.execute(
            "UPDATE adoption_sources SET resolution_state = 'reasoned_retired', "
            "adopted_attempt_id = NULL, resolution_actor = ?, resolution_at = ?, "
            "resolution_reason = ? WHERE adoption_id = ? AND source_family = ? "
            "AND source_primary = ? AND source_secondary = ? "
            "AND resolution_state = 'verified_bound' AND adopted_attempt_id = ? "
            "AND evidence_hash_version = ? AND evidence_hash = ?",
            (
                actor,
                resolved_at,
                reason,
                row["adoption_id"],
                row["source_family"],
                row["source_primary"],
                row["source_secondary"],
                attempt_id,
                EVIDENCE_HASH_VERSION,
                row["evidence_hash"],
            ),
        ).rowcount
    if changed != 1:
        raise AdoptionConflictError("source compare-and-set failed during attempt retirement")


def _bounded_text(value: str, label: str, limit: int) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be text")
    value = value.strip()
    if not value or len(value) > limit:
        raise ValueError(f"{label} must be nonempty and at most {limit} characters")
    return value


def _aware_iso(value: datetime) -> str:
    if not isinstance(value, datetime) or value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("resolution time must be timezone-aware")
    return to_iso(value)


def _require_transaction(conn: sqlite3.Connection) -> None:
    if not conn.in_transaction:
        raise RuntimeError("adoption resolution requires its stable write transaction")
