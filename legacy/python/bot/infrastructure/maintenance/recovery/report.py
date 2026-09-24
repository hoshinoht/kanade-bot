"""Read-only, content-free inventory of persistent maintenance blockers."""

from __future__ import annotations

import sqlite3

from ..delivery.journal import unproven_retirement_exists
from ..state import read_state
from .dispositions import classify_family
from .model import (
    AttemptBlocker,
    BlockerReport,
    OrphanLease,
    RecoveryDispositionError,
    TargetClaim,
)


def build_blocker_report(
    conn: sqlite3.Connection, *, live_leases: int, persistent_blockers: bool
) -> BlockerReport:
    state = read_state(conn)
    pending_sources = 0
    if state.adoption_id is not None:
        pending_sources = int(
            conn.execute(
                "SELECT COUNT(*) FROM adoption_sources WHERE adoption_id = ? "
                "AND resolution_state = 'pending'",
                (state.adoption_id,),
            ).fetchone()[0]
        )
    attempts = []
    for row in conn.execute(
        "SELECT a.attempt_id, a.origin, a.effect_kind, a.state, a.destination_kind, "
        "a.guild_id, a.channel_id, a.recipient_id, a.intended_at, a.operation_id, "
        "l.lifecycle AS lease_lifecycle FROM delivery_attempts AS a "
        "LEFT JOIN maintenance_leases AS l ON l.operation_id = a.operation_id "
        "WHERE a.state IN ('intent', 'indeterminate') ORDER BY a.intended_at, a.attempt_id"
    ).fetchall():
        targets = tuple(
            TargetClaim(
                target["binding_type"],
                target["key_primary"],
                target["key_secondary"] or "",
                target["released_at"] is not None,
            )
            for target in conn.execute(
                "SELECT binding_type, key_primary, key_secondary, released_at "
                "FROM delivery_attempt_targets WHERE attempt_id = ? ORDER BY target_ordinal",
                (row["attempt_id"],),
            )
        )
        try:
            family = classify_family(
                row["effect_kind"], [target.binding_type for target in targets]
            ).value
        except RecoveryDispositionError:
            family = "unsupported"
        attempts.append(
            AttemptBlocker(
                attempt_id=row["attempt_id"],
                origin=row["origin"],
                effect_kind=row["effect_kind"],
                family=family,
                state=row["state"],
                destination_kind=row["destination_kind"],
                guild_id=row["guild_id"],
                channel_id=row["channel_id"],
                recipient_id=row["recipient_id"],
                intended_at=row["intended_at"],
                operation_id=row["operation_id"],
                operation_lease=row["lease_lifecycle"],
                targets=targets,
            )
        )
    orphans = tuple(
        OrphanLease(row["operation_id"], row["operation_kind"], row["orphaned_at"])
        for row in conn.execute(
            "SELECT operation_id, operation_kind, orphaned_at FROM maintenance_leases "
            "WHERE lifecycle = 'orphaned' ORDER BY started_at, operation_id"
        )
    )
    retired_cards = tuple(
        row[0]
        for row in conn.execute(
            "SELECT m.id FROM amendments AS m WHERE m.status = 'proposed' "
            "AND m.proposal_message_id IS NULL AND "
            + unproven_retirement_exists("card", "m.id")
            + " ORDER BY m.id"
        )
    )
    return BlockerReport(
        mode=state.mode.value,
        blocker_code=state.blocker_code,
        state_revision=state.state_revision,
        adoption_state=state.adoption_state,
        adoption_id=state.adoption_id,
        pending_adoption_sources=pending_sources,
        attempts=tuple(attempts),
        orphan_leases=orphans,
        retired_card_proposals=retired_cards,
        live_leases=live_leases,
        persistent_blockers_clear=not persistent_blockers,
    )
