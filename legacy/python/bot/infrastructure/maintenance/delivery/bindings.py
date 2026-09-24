"""Validation and atomic binding through Repo's existing native setters."""

from __future__ import annotations

import sqlite3
from datetime import datetime
from typing import TYPE_CHECKING

from bot.domain.timeutil import from_iso

from .model import (
    BindingType,
    DeliveryReceipt,
    DeliveryTarget,
    Destination,
    DestinationKind,
    SendPlan,
)

if TYPE_CHECKING:
    from bot.infrastructure.db import Repo


class DeliveryBindingError(RuntimeError):
    """A native target is missing, stale, or could not be bound exactly once."""


def _channel_id(destination: Destination, label: str) -> str:
    if destination.kind is not DestinationKind.CHANNEL or destination.channel_id is None:
        raise DeliveryBindingError(f"{label} requires a channel destination")
    return destination.channel_id


def _run_on_channel(repo: Repo, run_id: str, channel_id: str, label: str) -> None:
    row = repo._conn.execute("SELECT channel_id FROM runs WHERE id = ?", (run_id,)).fetchone()
    if row is None or row["channel_id"] is None or row["channel_id"] != channel_id:
        raise DeliveryBindingError(f"{label} run does not belong to the destination channel")


def validate_targets(repo: Repo, plan: SendPlan) -> None:
    """Check target state under the caller's SQLite transaction."""
    validate_target_identities(repo, plan.destination, plan.targets)


def validate_target_identities(
    repo: Repo, destination: Destination, targets: tuple[DeliveryTarget, ...]
) -> None:
    """Check persisted target identities against one requested destination."""
    conn = repo._conn
    for target in targets:
        kind = target.binding_type
        if kind is BindingType.REMINDER:
            channel_id = _channel_id(destination, "reminder")
            row = conn.execute(
                "SELECT run_id, sent_at, message_id FROM reminders WHERE id = ?",
                (target.key_primary,),
            ).fetchone()
            if row is None or row["sent_at"] is not None or row["message_id"] is not None:
                raise DeliveryBindingError("reminder target is missing or already sent")
            _run_on_channel(repo, row["run_id"], channel_id, "reminder")
        elif kind is BindingType.DIGEST:
            channel_id = _channel_id(destination, "digest")
            row = conn.execute(
                "SELECT retired_at FROM weekly_digests WHERE week_start = ?",
                (target.key_primary,),
            ).fetchone()
            if row is not None and row["retired_at"] is None:
                raise DeliveryBindingError("digest target already has an active card")
            if not channel_id:
                raise DeliveryBindingError("digest destination channel is required")
        elif kind is BindingType.DECLINE:
            channel_id = _channel_id(destination, "decline notice")
            _run_on_channel(repo, target.key_primary or "", channel_id, "decline notice")
            row = conn.execute(
                "SELECT message_id FROM decline_notices WHERE run_id = ? AND user_id = ?",
                (target.key_primary, target.key_secondary),
            ).fetchone()
            if row is not None:
                if row["message_id"] is not None:
                    raise DeliveryBindingError("decline target already has a bound message")
                if not repo.delivery._decline_target_reusable(target):
                    raise DeliveryBindingError("decline target has an unresolved historic notice")
        elif kind is BindingType.CARD:
            channel_id = _channel_id(destination, "proposal card")
            row = conn.execute(
                "SELECT channel_id, status, proposal_message_id FROM amendments WHERE id = ?",
                (target.key_primary,),
            ).fetchone()
            if (
                row is None
                or row["status"] != "proposed"
                or row["proposal_message_id"] is not None
                or row["channel_id"] != channel_id
            ):
                raise DeliveryBindingError(
                    "proposal card target is missing, stale, or on another channel"
                )
        elif kind is BindingType.DEBUG_CARD:
            channel_id = _channel_id(destination, "debug card")
            _run_on_channel(repo, target.debug_run_id or "", channel_id, "debug card")


def insert_target_claims(
    conn: sqlite3.Connection, attempt_id: str, targets: tuple[DeliveryTarget, ...]
) -> None:
    for ordinal, target in enumerate(sorted(targets, key=lambda item: item.order_key)):
        if target.binding_type is BindingType.DEBUG_CARD:
            continue
        conn.execute(
            "INSERT INTO delivery_attempt_targets "
            "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
            "VALUES (?, ?, ?, ?, ?)",
            (attempt_id, ordinal, *target.claim_key),
        )


def bind_targets(
    repo: Repo,
    plan: SendPlan,
    receipt: DeliveryReceipt,
    attempt_id: str,
    at: datetime,
) -> None:
    """Write each target through its Repo setter and verify its affected-row result."""
    bind_target_identities(repo, plan.targets, receipt, attempt_id, at)


def bind_target_identities(
    repo: Repo,
    targets: tuple[DeliveryTarget, ...],
    receipt: DeliveryReceipt,
    attempt_id: str,
    at: datetime,
) -> None:
    """Bind persisted target identities to one authoritative message."""
    channel_id = receipt.destination.channel_id
    for ordinal, target in enumerate(sorted(targets, key=lambda item: item.order_key)):
        kind = target.binding_type
        if kind is BindingType.REMINDER:
            repo.mark_reminder_sent(target.key_primary or "", receipt.message_id, at)
            _require_one_change(repo, "reminder binding")
        elif kind is BindingType.DIGEST:
            if channel_id is None:
                raise DeliveryBindingError("digest receipt has no channel ID")
            repo.set_weekly_digest(
                from_iso(target.key_primary or ""), channel_id, receipt.message_id, at=at
            )
            _require_one_change(repo, "digest binding")
        elif kind is BindingType.DECLINE:
            if channel_id is None:
                raise DeliveryBindingError("decline receipt has no channel ID")
            repo.set_decline_notice(
                target.key_primary or "",
                target.key_secondary or "",
                receipt.message_id,
                channel_id,
                at,
            )
            _require_one_change(repo, "decline binding")
        elif kind is BindingType.CARD:
            repo.set_amendment_proposal_message(target.key_primary or "", receipt.message_id)
            _require_one_change(repo, "proposal-card binding")
        elif kind is BindingType.DEBUG_CARD:
            repo.add_debug_message(
                receipt.message_id,
                target.debug_run_id or "",
                channel_id,
                target.debug_kind or "",
                at=at,
            )
            _require_one_change(repo, "debug-card binding")
            repo._conn.execute(
                "INSERT INTO delivery_attempt_targets "
                "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
                "VALUES (?, ?, 'debug_card', ?, NULL)",
                (attempt_id, ordinal, receipt.message_id),
            )


def _require_one_change(repo: Repo, label: str) -> None:
    changed = repo._conn.execute("SELECT changes()").fetchone()[0]
    if changed != 1:
        raise DeliveryBindingError(f"{label} did not affect exactly one native row")


def observable_destination_matches(plan: SendPlan, receipt: DeliveryReceipt) -> bool:
    requested = plan.destination
    actual = receipt.destination
    if requested.kind is not actual.kind or requested.guild_id != actual.guild_id:
        return False
    if requested.kind is DestinationKind.CHANNEL:
        return requested.channel_id == actual.channel_id
    return requested.recipient_id == actual.recipient_id and (
        requested.channel_id is None or requested.channel_id == actual.channel_id
    )
