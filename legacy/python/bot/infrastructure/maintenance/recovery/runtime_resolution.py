"""Operator bind/retire of runtime attempts left in intent or indeterminate state.

Resolution never sends. Binding requires an authoritative observation of the
actual message; retirement records a reasoned no-replay disposition (D6). Both
run only while BLOCKED or FROZEN (D8) and never transition maintenance mode.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime
from typing import TYPE_CHECKING

from bot.domain.timeutil import from_iso

from ..adoption.resolution import validate_resolution_metadata
from ..delivery.bindings import (
    DeliveryBindingError,
    bind_target_identities,
    validate_target_identities,
)
from ..delivery.fingerprints import observable_fingerprint
from ..delivery.model import (
    BindingType,
    DeliveryReceipt,
    DeliveryTarget,
    Destination,
    DestinationKind,
    _snowflake,
)
from .dispositions import apply_retirement, classify_family, raise_digest_marker
from .model import (
    EVIDENCE_CLOCK_SKEW,
    AttemptFamily,
    BlockerReport,
    RecoveryError,
    RecoveryEvidenceError,
    RecoveryObservation,
    RetirementClass,
)

if TYPE_CHECKING:
    from bot.infrastructure.db import Repo

    from .lease import RecoveryLease

_UNRESOLVED = ("intent", "indeterminate")
_NATIVE_MESSAGE_COLUMNS = (
    ("reminders", "message_id"),
    ("weekly_digests", "message_id"),
    ("decline_notices", "message_id"),
    ("amendments", "proposal_message_id"),
    ("debug_messages", "message_id"),
)


class RuntimeRecovery:
    """Service-level entry points; construct per call site with the owning Repo."""

    def __init__(self, repo: Repo):
        self._repo = repo

    def blocker_report(self) -> BlockerReport:
        return self._repo.maintenance.blocker_report()

    async def bind_runtime_attempt(
        self,
        attempt_id: str,
        observation: RecoveryObservation,
        *,
        bot_author_id: str | int,
        actor: str,
        reason: str,
        at: datetime,
    ) -> None:
        """Bind one attempt and all its native targets to verified message evidence."""
        attempt_id = _attempt_id(attempt_id)
        if not isinstance(observation, RecoveryObservation):
            raise RecoveryEvidenceError("binding requires a typed recovery observation")
        try:
            author_id = _snowflake(bot_author_id, "configured bot author id")
        except ValueError as exc:
            raise RecoveryEvidenceError("configured bot author id is invalid") from exc
        actor, reason, resolved_at = validate_resolution_metadata(actor, reason, at)
        repo = self._repo
        repo.maintenance._runtime_recovery_transaction(
            lambda lease: _bind(
                repo, lease, attempt_id, observation, author_id, actor, reason, resolved_at
            )
        )

    async def retire_runtime_attempt(
        self,
        attempt_id: str,
        *,
        actor: str,
        reason: str,
        at: datetime,
        reason_class: RetirementClass | str = RetirementClass.OPERATOR_NO_REPLAY,
    ) -> None:
        """Retire one attempt with its family's no-replay disposition; never resend."""
        attempt_id = _attempt_id(attempt_id)
        try:
            reason_class = RetirementClass(reason_class)
        except ValueError as exc:
            raise ValueError("unsupported retirement reason class") from exc
        actor, reason, resolved_at = validate_resolution_metadata(actor, reason, at)
        recorded = f"{reason_class.value}: {reason}"
        if len(recorded) > 512:
            raise ValueError("resolution reason must be at most 512 characters with its class")
        repo = self._repo
        repo.maintenance._runtime_recovery_transaction(
            lambda lease: _retire(
                repo, lease, attempt_id, reason_class, actor, recorded, resolved_at
            )
        )


def _attempt_id(value: str) -> str:
    if not isinstance(value, str) or not value.strip() or len(value) > 128:
        raise ValueError("runtime attempt id is required")
    return value


def _load(
    conn: sqlite3.Connection, attempt_id: str
) -> tuple[sqlite3.Row, tuple[sqlite3.Row, ...], AttemptFamily]:
    attempt = conn.execute(
        "SELECT * FROM delivery_attempts WHERE attempt_id = ?", (attempt_id,)
    ).fetchone()
    if (
        attempt is None
        or attempt["origin"] != "runtime"
        or attempt["state"] not in _UNRESOLVED
        or attempt["dedupe_active"] != 1
    ):
        raise RecoveryError("attempt is not an unresolved runtime attempt")
    lease = conn.execute(
        "SELECT lifecycle FROM maintenance_leases WHERE operation_id = ?",
        (attempt["operation_id"],),
    ).fetchone()
    if lease is not None and lease["lifecycle"] == "live":
        raise RecoveryError("attempt's own operation lease is still live")
    targets = tuple(
        conn.execute(
            "SELECT * FROM delivery_attempt_targets WHERE attempt_id = ? ORDER BY target_ordinal",
            (attempt_id,),
        )
    )
    if any(target["released_at"] is not None for target in targets):
        raise RecoveryError("unresolved attempt has an already released target claim")
    family = classify_family(attempt["effect_kind"], [target["binding_type"] for target in targets])
    return attempt, targets, family


def _bind(
    repo: Repo,
    lease: RecoveryLease,
    attempt_id: str,
    observation: RecoveryObservation,
    bot_author_id: str,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    conn = repo._conn
    attempt, target_rows, family = _load(conn, attempt_id)
    if family is AttemptFamily.MEMORY:
        raise RecoveryError("personal-memory attempts cannot be bound; retire as feature_removed")
    requested = _attempt_destination(attempt)
    _validate_evidence(conn, attempt, requested, observation, bot_author_id)
    targets = tuple(_delivery_target(row) for row in target_rows)
    receipt = DeliveryReceipt(observation.destination, observation.message_id, observation.observed)
    guard = repo._guard
    with guard._runtime_recovery_bind_scope(lease.authority):
        try:
            validate_target_identities(repo, requested, targets)
            # Native timestamps record the actual send, not the resolution time.
            bind_target_identities(repo, targets, receipt, attempt_id, observation.created_at)
        except DeliveryBindingError as exc:
            raise RecoveryError(f"native targets cannot be bound: {exc}") from exc
        if family is AttemptFamily.DIGEST:
            # The tick would otherwise delete and repost the operator-bound digest.
            raise_digest_marker(conn, target_rows[0]["key_primary"])
        destination = observation.destination
        changed = conn.execute(
            "UPDATE delivery_attempts SET state = 'bound', guild_id = ?, channel_id = ?, "
            "recipient_id = ?, message_id = ?, observable_fingerprint = ?, resolved_at = ?, "
            "resolved_by = ?, resolution_reason = ? WHERE attempt_id = ? AND origin = 'runtime' "
            "AND state IN ('intent', 'indeterminate') AND dedupe_active = 1",
            (
                destination.guild_id,
                destination.channel_id,
                destination.recipient_id,
                observation.message_id,
                observable_fingerprint(destination, observation.observed),
                resolved_at,
                actor,
                reason,
                attempt_id,
            ),
        ).rowcount
        if changed != 1:
            raise RecoveryError("runtime attempt was not bound exactly once")
        _bump_revision(conn, lease)


def _retire(
    repo: Repo,
    lease: RecoveryLease,
    attempt_id: str,
    reason_class: RetirementClass,
    actor: str,
    reason: str,
    resolved_at: str,
) -> None:
    conn = repo._conn
    attempt, targets, family = _load(conn, attempt_id)
    if (family is AttemptFamily.MEMORY) != (reason_class is RetirementClass.FEATURE_REMOVED):
        raise RecoveryError(
            "feature_removed retirement applies exactly to retained personal-memory attempts"
        )
    with repo._guard._runtime_recovery_retire_scope(lease.authority):
        apply_retirement(conn, family, attempt, targets, resolved_at)
        released = conn.execute(
            "UPDATE delivery_attempt_targets SET released_at = ?, release_actor = ?, "
            "release_reason = ? WHERE attempt_id = ? AND released_at IS NULL",
            (resolved_at, actor, reason, attempt_id),
        ).rowcount
        if released != len(targets):
            raise RecoveryError("runtime attempt did not release every target claim")
        retired = conn.execute(
            "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, resolved_at = ?, "
            "resolved_by = ?, resolution_reason = ? WHERE attempt_id = ? AND origin = 'runtime' "
            "AND state IN ('intent', 'indeterminate') AND dedupe_active = 1",
            (resolved_at, actor, reason, attempt_id),
        ).rowcount
        if retired != 1:
            raise RecoveryError("runtime attempt was not retired exactly once")
        _bump_revision(conn, lease)


def _bump_revision(conn: sqlite3.Connection, lease: RecoveryLease) -> None:
    # Resolution changes retained state, so any prior checkpoint revision is stale.
    changed = conn.execute(
        "UPDATE maintenance_state SET state_revision = state_revision + 1 "
        "WHERE id = 1 AND generation = ? AND mode IN ('BLOCKED', 'FROZEN')",
        (lease.generation,),
    ).rowcount
    if changed != 1:
        raise RecoveryError("maintenance state changed during runtime recovery")


def _attempt_destination(attempt: sqlite3.Row) -> Destination:
    try:
        if attempt["destination_kind"] == DestinationKind.CHANNEL.value:
            return Destination.channel(attempt["guild_id"], attempt["channel_id"])
        return Destination.dm(attempt["recipient_id"], channel_id=attempt["channel_id"])
    except ValueError as exc:
        raise RecoveryError("attempt destination is not a canonical Discord identity") from exc


def _validate_evidence(
    conn: sqlite3.Connection,
    attempt: sqlite3.Row,
    requested: Destination,
    observation: RecoveryObservation,
    bot_author_id: str,
) -> None:
    if observation.unsupported_components:
        raise RecoveryEvidenceError("observed message contains unsupported components")
    if observation.author_id != bot_author_id:
        raise RecoveryEvidenceError("observed message was not authored by the configured bot")
    actual = observation.destination
    if actual.kind is not requested.kind:
        raise RecoveryEvidenceError("observed message has another destination kind")
    if requested.kind is DestinationKind.CHANNEL:
        if actual.guild_id != requested.guild_id:
            raise RecoveryEvidenceError("observed message belongs to another guild")
        if actual.channel_id != requested.channel_id:
            raise RecoveryEvidenceError("observed message belongs to another channel")
    elif actual.recipient_id != requested.recipient_id or (
        requested.channel_id is not None and actual.channel_id != requested.channel_id
    ):
        raise RecoveryEvidenceError("observed message belongs to another DM recipient")
    try:
        intended_at = from_iso(attempt["intended_at"])
    except (TypeError, ValueError) as exc:
        raise RecoveryError("attempt intent time is malformed") from exc
    if observation.created_at < intended_at - EVIDENCE_CLOCK_SKEW:
        raise RecoveryEvidenceError("observed message predates the recorded intent")
    message_id = observation.message_id
    if conn.execute(
        "SELECT 1 FROM delivery_attempts WHERE message_id = ? LIMIT 1", (message_id,)
    ).fetchone():
        raise RecoveryEvidenceError("observed message already belongs to another attempt")
    for table, column in _NATIVE_MESSAGE_COLUMNS:
        if conn.execute(
            f"SELECT 1 FROM {table} WHERE {column} = ? LIMIT 1",  # noqa: S608 - fixed identifiers
            (message_id,),
        ).fetchone():
            raise RecoveryEvidenceError("observed message is already bound to a native row")


def _delivery_target(row: sqlite3.Row) -> DeliveryTarget:
    try:
        kind = BindingType(row["binding_type"])
        if kind is BindingType.DECLINE:
            return DeliveryTarget.decline(row["key_primary"], row["key_secondary"] or "")
        if kind is BindingType.DIGEST:
            return DeliveryTarget.digest(row["key_primary"])
        if kind is BindingType.REMINDER:
            return DeliveryTarget.reminder(row["key_primary"])
        if kind is BindingType.CARD:
            return DeliveryTarget.card(row["key_primary"])
    except ValueError as exc:
        raise RecoveryError("persisted target claim is malformed") from exc
    raise RecoveryError("persisted target claim has no native binding")
