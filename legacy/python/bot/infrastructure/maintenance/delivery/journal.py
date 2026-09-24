"""Durable intent, one-call transport, and atomic native delivery binding."""

from __future__ import annotations

import hashlib
import json
import sqlite3
import uuid
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Protocol

from bot.domain.timeutil import to_iso

from .bindings import (
    bind_targets,
    insert_target_claims,
    observable_destination_matches,
    validate_targets,
)
from .fingerprints import FINGERPRINT_VERSION, observable_fingerprint, request_fingerprint
from .model import (
    DedupeScope,
    DeliveryOutcome,
    DeliveryOutcomeKind,
    DeliveryReceipt,
    DeliveryTarget,
    SendPlan,
)

if TYPE_CHECKING:
    from bot.infrastructure.db import Repo

_DIGEST_REPLACEMENT_ACTOR = "service:digest-replacement"
_DIGEST_REPLACEMENT_REASON = "confirmed Discord deletion for digest replacement"
_DEBUG_CLEANUP_ACTOR = "service:debug-cleanup"
_DEBUG_CLEANUP_REASON = "confirmed Discord deletion during debug cleanup"
_DECLINE_RETRACTION_ACTOR = "service:decline-retraction"
_DECLINE_RETRACTION_REASON = "confirmed Discord deletion for decline retraction"


def unproven_retirement_exists(binding_type: str, key_sql: str) -> str:
    """SQL EXISTS clause for an operator no-replay retirement of one native target.

    Journal-confirmed retirements always carry a message ID; a retired runtime
    attempt without one exists only through operator recovery. ``binding_type``
    and ``key_sql`` must be fixed identifiers or ``?``, never caller data.
    """
    return (
        "EXISTS (SELECT 1 FROM delivery_attempt_targets AS ut "
        "JOIN delivery_attempts AS ua ON ua.attempt_id = ut.attempt_id "
        f"WHERE ut.binding_type = '{binding_type}' AND ut.key_primary = {key_sql} "
        "AND ut.released_at IS NOT NULL AND ua.origin = 'runtime' "
        "AND ua.state = 'retired' AND ua.message_id IS NULL)"
    )


class DeliveryJournalError(RuntimeError):
    """The journal could not durably represent an outbound effect."""


class DeliveryTransport(Protocol):
    async def send(self, plan: SendPlan) -> DeliveryReceipt: ...


class DeliveryJournal:
    """Persist one actual send before transport and bind it without replay."""

    def __init__(self, repo: Repo):
        self._repo = repo

    async def execute(self, plan: SendPlan, transport: DeliveryTransport) -> DeliveryOutcome:
        """Send once under the current task's live lease; ambiguity stays durable."""
        if not isinstance(plan, SendPlan):
            raise TypeError("delivery execution requires an immutable SendPlan")
        attempt_id, effect_ordinal, suppressed = self._persist_intent(plan)
        if suppressed is not None:
            return suppressed

        conn = self._repo._conn
        if conn.in_transaction:
            raise DeliveryJournalError("delivery intent transaction remained open before transport")
        try:
            receipt = await transport.send(plan)
            if not isinstance(receipt, DeliveryReceipt):
                raise DeliveryJournalError("transport returned an invalid delivery receipt")
            if not observable_destination_matches(plan, receipt):
                raise DeliveryJournalError(
                    "transport receipt destination does not match the request"
                )
            observed_hash = observable_fingerprint(receipt.destination, receipt.observed)
        except BaseException:
            self._record_indeterminate(attempt_id)
            raise

        try:
            self._finalize(plan, attempt_id, receipt, observed_hash)
        except BaseException:
            self._record_indeterminate(attempt_id)
            raise
        return DeliveryOutcome(
            DeliveryOutcomeKind.BOUND,
            attempt_id,
            effect_ordinal,
            "bound",
            receipt,
        )

    def _persist_intent(self, plan: SendPlan) -> tuple[str, int, DeliveryOutcome | None]:
        coordinator = self._repo.maintenance
        lease = coordinator._assert_current_live_lease()
        conn = self._repo._conn
        attempt_id = str(uuid.uuid4())
        request_hash = request_fingerprint(plan)
        now = to_iso(datetime.now(UTC))
        policy = plan.dedupe
        guard = self._repo._guard

        with guard._delivery_intent_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                next_ordinal = int(
                    conn.execute(
                        "SELECT COALESCE(MAX(effect_ordinal), -1) + 1 "
                        "FROM delivery_attempts WHERE operation_id = ?",
                        (lease.operation_id,),
                    ).fetchone()[0]
                )
                effect_ordinal = (
                    policy.effect_ordinal
                    if policy.scope is DedupeScope.OPERATION and policy.effect_ordinal is not None
                    else next_ordinal
                )
                if effect_ordinal > next_ordinal:
                    raise ValueError("operation effect ordinals must be allocated without gaps")

                existing = self._operation_attempt(lease.operation_id, effect_ordinal)
                if existing is not None:
                    conn.execute("ROLLBACK")
                    return attempt_id, effect_ordinal, self._suppressed(existing)

                dedupe_key = self._dedupe_key(plan, lease.operation_id, effect_ordinal)
                existing = self._active_dedupe(dedupe_key)
                if existing is not None:
                    conn.execute("ROLLBACK")
                    return attempt_id, effect_ordinal, self._suppressed(existing)

                validate_targets(self._repo, plan)
                conn.execute(
                    "INSERT INTO delivery_attempts "
                    "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, "
                    "effect_kind, dedupe_scope, dedupe_key, dedupe_active, state, "
                    "destination_kind, guild_id, channel_id, recipient_id, fingerprint_version, "
                    "request_fingerprint, intended_at) "
                    "VALUES (?, ?, ?, ?, 'runtime', ?, ?, ?, 1, 'intent', ?, ?, ?, ?, ?, ?, ?)",
                    (
                        attempt_id,
                        lease.operation_id,
                        effect_ordinal,
                        coordinator.instance_id,
                        plan.effect_kind,
                        policy.scope.value,
                        dedupe_key,
                        plan.destination.kind.value,
                        plan.destination.guild_id,
                        plan.destination.channel_id,
                        plan.destination.recipient_id,
                        FINGERPRINT_VERSION,
                        request_hash,
                        now,
                    ),
                )
                insert_target_claims(conn, attempt_id, plan.targets)
                conn.execute("COMMIT")
            except sqlite3.IntegrityError:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                duplicate = self._active_dedupe(dedupe_key) if "dedupe_key" in locals() else None
                if duplicate is None:
                    duplicate = self._claimed_target(plan)
                if duplicate is None:
                    duplicate = self._operation_attempt(lease.operation_id, effect_ordinal)
                if duplicate is None:
                    raise
                return attempt_id, effect_ordinal, self._suppressed(duplicate)
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise
        return attempt_id, effect_ordinal, None

    def _finalize(
        self,
        plan: SendPlan,
        attempt_id: str,
        receipt: DeliveryReceipt,
        observed_hash: str,
    ) -> None:
        self._repo.maintenance._assert_current_live_lease()
        conn = self._repo._conn
        stamp = datetime.now(UTC)
        resolved_at = to_iso(stamp)
        destination = receipt.destination
        guard = self._repo._guard
        with guard._delivery_finalize_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                validate_targets(self._repo, plan)
                bind_targets(self._repo, plan, receipt, attempt_id, stamp)
                changed = conn.execute(
                    "UPDATE delivery_attempts SET state = 'bound', guild_id = ?, channel_id = ?, "
                    "recipient_id = ?, message_id = ?, observable_fingerprint = ?, resolved_at = ? "
                    "WHERE attempt_id = ? AND state = 'intent'",
                    (
                        destination.guild_id,
                        destination.channel_id,
                        destination.recipient_id,
                        receipt.message_id,
                        observed_hash,
                        resolved_at,
                        attempt_id,
                    ),
                ).rowcount
                if changed != 1:
                    raise DeliveryJournalError("delivery intent was not finalized exactly once")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise

    def _record_indeterminate(self, attempt_id: str) -> None:
        self._repo.maintenance._assert_current_live_lease()
        conn = self._repo._conn
        with self._repo._guard._delivery_indeterminate_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                changed = conn.execute(
                    "UPDATE delivery_attempts SET state = 'indeterminate' "
                    "WHERE attempt_id = ? AND state = 'intent'",
                    (attempt_id,),
                ).rowcount
                if changed != 1:
                    raise DeliveryJournalError("delivery intent could not be marked indeterminate")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise

    def _dedupe_key(self, plan: SendPlan, operation_id: str, ordinal: int) -> str:
        policy = plan.dedupe
        if policy.scope is DedupeScope.NATIVE:
            identity: object = sorted(target.claim_key for target in plan.targets)
        elif policy.scope is DedupeScope.SOURCE:
            identity = [
                policy.guild_id,
                policy.channel_id,
                policy.source_id,
                policy.semantic_slot,
            ]
        else:
            identity = [operation_id, ordinal]
        payload = json.dumps(
            ["kanade.delivery.dedupe.v1", policy.scope.value, identity],
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        return hashlib.sha256(payload).hexdigest()

    def _active_dedupe(self, dedupe_key: str) -> sqlite3.Row | None:
        return self._repo._conn.execute(
            "SELECT attempt_id, effect_ordinal, state FROM delivery_attempts "
            "WHERE dedupe_key = ? AND dedupe_active = 1",
            (dedupe_key,),
        ).fetchone()

    def _operation_attempt(self, operation_id: str, ordinal: int) -> sqlite3.Row | None:
        return self._repo._conn.execute(
            "SELECT attempt_id, effect_ordinal, state FROM delivery_attempts "
            "WHERE operation_id = ? AND effect_ordinal = ?",
            (operation_id, ordinal),
        ).fetchone()

    def _claimed_target(self, plan: SendPlan) -> sqlite3.Row | None:
        for target in sorted(plan.targets, key=lambda item: item.order_key):
            if target.binding_type.value == "debug_card":
                continue
            row = self._repo._conn.execute(
                "SELECT a.attempt_id, a.effect_ordinal, a.state "
                "FROM delivery_attempt_targets t JOIN delivery_attempts a USING (attempt_id) "
                "WHERE t.binding_type = ? AND t.key_primary = ? "
                "AND COALESCE(t.key_secondary, '') = ? AND t.released_at IS NULL "
                "AND a.dedupe_active = 1",
                target.claim_key,
            ).fetchone()
            if row is not None:
                return row
        return None

    def has_active_claim(self, target: DeliveryTarget) -> bool:
        """Whether a live or uncertain attempt still owns this native target."""
        if target.binding_type.value == "debug_card":
            return False
        return (
            self._repo._conn.execute(
                "SELECT 1 FROM delivery_attempt_targets t "
                "JOIN delivery_attempts a USING (attempt_id) "
                "WHERE t.binding_type = ? AND t.key_primary = ? "
                "AND COALESCE(t.key_secondary, '') = ? AND t.released_at IS NULL "
                "AND a.dedupe_active = 1 LIMIT 1",
                target.claim_key,
            ).fetchone()
            is not None
        )

    def card_retired_unproven(self, amendment_id: str) -> bool:
        """Whether operator recovery retired a card send whose outcome was never proven."""
        return self._retired_unproven("card", amendment_id)

    def reminder_retired_unproven(self, reminder_id: str) -> bool:
        """Whether operator recovery retired a reminder send whose outcome was never proven."""
        return self._retired_unproven("reminder", reminder_id)

    def _retired_unproven(self, binding_type: str, key: str) -> bool:
        return bool(
            self._repo._conn.execute(
                f"SELECT {unproven_retirement_exists(binding_type, '?')}", (str(key),)
            ).fetchone()[0]
        )

    def _decline_target_reusable(self, target: DeliveryTarget) -> bool:
        """Allow a cleared native row only after this journal confirmed deletion."""
        if target.binding_type.value != "decline":
            return False
        return (
            self._repo._conn.execute(
                "SELECT 1 FROM delivery_attempt_targets t "
                "JOIN delivery_attempts a USING (attempt_id) "
                "WHERE t.binding_type = 'decline' AND t.key_primary = ? "
                "AND COALESCE(t.key_secondary, '') = ? AND t.released_at IS NOT NULL "
                "AND a.state = 'retired' AND a.dedupe_active = 0 "
                "AND a.resolved_by = ? AND a.resolution_reason = ? LIMIT 1",
                (
                    target.key_primary,
                    target.key_secondary or "",
                    _DECLINE_RETRACTION_ACTOR,
                    _DECLINE_RETRACTION_REASON,
                ),
            ).fetchone()
            is not None
        )

    def _decline_retraction_claim(
        self, run_id: str, user_id: int | str, channel_id: int | str, message_id: int | str
    ) -> sqlite3.Row | None:
        self._repo.maintenance._assert_current_live_lease()
        return self._repo._conn.execute(
            "SELECT a.attempt_id, a.guild_id FROM decline_notices d "
            "JOIN delivery_attempt_targets t ON t.binding_type = 'decline' "
            "AND t.key_primary = d.run_id AND t.key_secondary = d.user_id "
            "JOIN delivery_attempts a USING (attempt_id) "
            "WHERE d.run_id = ? AND d.user_id = ? AND d.channel_id = ? AND d.message_id = ? "
            "AND t.released_at IS NULL AND a.state = 'bound' AND a.dedupe_active = 1 "
            "AND a.destination_kind = 'channel' AND a.channel_id = d.channel_id "
            "AND a.message_id = d.message_id AND a.effect_kind LIKE 'decline.notice.%'",
            (str(run_id), str(user_id), str(channel_id), str(message_id)),
        ).fetchone()

    def _retire_decline_retraction(
        self,
        attempt_id: str,
        run_id: str,
        user_id: int | str,
        channel_id: int | str,
        message_id: int | str,
        at: datetime,
    ) -> None:
        """Clear one deleted decline and release/retire its exact bound attempt."""
        self._repo.maintenance._assert_current_live_lease()
        conn = self._repo._conn
        run_key = str(run_id)
        user_key = str(user_id)
        channel_key = str(channel_id)
        message_key = str(message_id)
        resolved_at = to_iso(at)
        with self._repo._guard._decline_retraction_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                claim = conn.execute(
                    "SELECT 1 FROM decline_notices d "
                    "JOIN delivery_attempt_targets t ON t.binding_type = 'decline' "
                    "AND t.key_primary = d.run_id AND t.key_secondary = d.user_id "
                    "JOIN delivery_attempts a USING (attempt_id) "
                    "WHERE d.run_id = ? AND d.user_id = ? AND d.channel_id = ? "
                    "AND d.message_id = ? AND t.attempt_id = ? AND t.released_at IS NULL "
                    "AND a.state = 'bound' AND a.dedupe_active = 1 "
                    "AND a.destination_kind = 'channel' AND a.channel_id = d.channel_id "
                    "AND a.message_id = d.message_id "
                    "AND a.effect_kind LIKE 'decline.notice.%'",
                    (run_key, user_key, channel_key, message_key, attempt_id),
                ).fetchone()
                if claim is None:
                    raise DeliveryJournalError("bound decline retraction claim changed")
                cleared = conn.execute(
                    "UPDATE decline_notices SET message_id = NULL "
                    "WHERE run_id = ? AND user_id = ? AND channel_id = ? AND message_id = ?",
                    (run_key, user_key, channel_key, message_key),
                ).rowcount
                if cleared != 1:
                    raise DeliveryJournalError("decline retraction lost its native row")
                released = conn.execute(
                    "UPDATE delivery_attempt_targets SET released_at = ?, release_actor = ?, "
                    "release_reason = ? WHERE attempt_id = ? AND binding_type = 'decline' "
                    "AND key_primary = ? AND key_secondary = ? AND released_at IS NULL",
                    (
                        resolved_at,
                        _DECLINE_RETRACTION_ACTOR,
                        _DECLINE_RETRACTION_REASON,
                        attempt_id,
                        run_key,
                        user_key,
                    ),
                ).rowcount
                if released != 1:
                    raise DeliveryJournalError("decline retraction claim was not released once")
                retired = conn.execute(
                    "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, "
                    "resolved_at = ?, resolved_by = ?, resolution_reason = ? "
                    "WHERE attempt_id = ? AND state = 'bound' AND dedupe_active = 1 "
                    "AND channel_id = ? AND message_id = ? "
                    "AND effect_kind LIKE 'decline.notice.%'",
                    (
                        resolved_at,
                        _DECLINE_RETRACTION_ACTOR,
                        _DECLINE_RETRACTION_REASON,
                        attempt_id,
                        channel_key,
                        message_key,
                    ),
                ).rowcount
                if retired != 1:
                    raise DeliveryJournalError("decline retraction attempt was not retired once")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise

    def _retire_debug_cleanup(
        self, message_id: int | str, channel_id: int | str, at: datetime
    ) -> None:
        """Delete one debug binding and retire its exact attempt, when journalled."""
        self._repo.maintenance._assert_current_live_lease()
        conn = self._repo._conn
        message_key = str(message_id)
        channel_key = str(channel_id)
        resolved_at = to_iso(at)
        with self._repo._guard._debug_cleanup_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                native = conn.execute(
                    "SELECT 1 FROM debug_messages WHERE message_id = ? AND channel_id = ?",
                    (message_key, channel_key),
                ).fetchone()
                if native is None:
                    raise DeliveryJournalError("debug cleanup lost its native message row")
                claim = conn.execute(
                    "SELECT a.attempt_id, a.state, a.channel_id, a.message_id "
                    "FROM delivery_attempt_targets t JOIN delivery_attempts a USING (attempt_id) "
                    "WHERE t.binding_type = 'debug_card' AND t.key_primary = ? "
                    "AND t.released_at IS NULL AND a.dedupe_active = 1",
                    (message_key,),
                ).fetchone()
                if claim is not None:
                    if (
                        claim["state"] != "bound"
                        or claim["channel_id"] != channel_key
                        or claim["message_id"] != message_key
                    ):
                        raise DeliveryJournalError(
                            "debug cleanup attempt does not match its binding"
                        )
                    released = conn.execute(
                        "UPDATE delivery_attempt_targets SET released_at = ?, release_actor = ?, "
                        "release_reason = ? WHERE attempt_id = ? AND binding_type = 'debug_card' "
                        "AND key_primary = ? AND released_at IS NULL",
                        (
                            resolved_at,
                            _DEBUG_CLEANUP_ACTOR,
                            _DEBUG_CLEANUP_REASON,
                            claim["attempt_id"],
                            message_key,
                        ),
                    ).rowcount
                    if released != 1:
                        raise DeliveryJournalError("debug cleanup target was not released once")
                    retired = conn.execute(
                        "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, "
                        "resolved_at = ?, resolved_by = ?, resolution_reason = ? "
                        "WHERE attempt_id = ? AND state = 'bound' AND dedupe_active = 1 "
                        "AND effect_kind = 'debug_card' AND channel_id = ? AND message_id = ?",
                        (
                            resolved_at,
                            _DEBUG_CLEANUP_ACTOR,
                            _DEBUG_CLEANUP_REASON,
                            claim["attempt_id"],
                            channel_key,
                            message_key,
                        ),
                    ).rowcount
                    if retired != 1:
                        raise DeliveryJournalError("debug cleanup attempt was not retired once")
                deleted = conn.execute(
                    "DELETE FROM debug_messages WHERE message_id = ? AND channel_id = ?",
                    (message_key, channel_key),
                ).rowcount
                if deleted != 1:
                    raise DeliveryJournalError("debug cleanup lost its native message row")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise

    def _digest_replacement_claim(
        self, week_start: datetime, channel_id: int | str, message_id: int | str
    ) -> sqlite3.Row | None:
        self._repo.maintenance._assert_current_live_lease()
        return self._repo._conn.execute(
            "SELECT a.attempt_id, a.guild_id FROM weekly_digests d "
            "JOIN delivery_attempt_targets t ON t.binding_type = 'digest' "
            "AND t.key_primary = d.week_start AND COALESCE(t.key_secondary, '') = '' "
            "JOIN delivery_attempts a USING (attempt_id) "
            "WHERE d.week_start = ? AND d.channel_id = ? AND d.message_id = ? "
            "AND d.retired_at IS NULL AND t.released_at IS NULL "
            "AND a.effect_kind = 'digest' "
            "AND a.state = 'bound' AND a.dedupe_active = 1 "
            "AND a.destination_kind = 'channel' AND a.channel_id = d.channel_id "
            "AND a.message_id = d.message_id",
            (to_iso(week_start), str(channel_id), str(message_id)),
        ).fetchone()

    def _retire_digest_replacement(
        self,
        attempt_id: str,
        week_start: datetime,
        channel_id: int | str,
        message_id: int | str,
        at: datetime,
    ) -> None:
        """Atomically retire one confirmed-deleted digest and release its exact claim."""
        self._repo.maintenance._assert_current_live_lease()
        conn = self._repo._conn
        week_key = to_iso(week_start)
        channel_key = str(channel_id)
        message_key = str(message_id)
        resolved_at = to_iso(at)
        with self._repo._guard._digest_replacement_scope():
            conn.execute("BEGIN IMMEDIATE")
            try:
                claim = conn.execute(
                    "SELECT 1 FROM weekly_digests d "
                    "JOIN delivery_attempt_targets t ON t.binding_type = 'digest' "
                    "AND t.key_primary = d.week_start AND COALESCE(t.key_secondary, '') = '' "
                    "JOIN delivery_attempts a USING (attempt_id) "
                    "WHERE d.week_start = ? AND d.channel_id = ? AND d.message_id = ? "
                    "AND d.retired_at IS NULL AND t.attempt_id = ? AND t.released_at IS NULL "
                    "AND a.effect_kind = 'digest' "
                    "AND a.state = 'bound' AND a.dedupe_active = 1 "
                    "AND a.destination_kind = 'channel' AND a.channel_id = d.channel_id "
                    "AND a.message_id = d.message_id",
                    (week_key, channel_key, message_key, attempt_id),
                ).fetchone()
                if claim is None:
                    raise DeliveryJournalError("bound digest replacement claim changed")
                changed = conn.execute(
                    "UPDATE weekly_digests SET retired_at = ? "
                    "WHERE week_start = ? AND channel_id = ? AND message_id = ? "
                    "AND retired_at IS NULL",
                    (resolved_at, week_key, channel_key, message_key),
                ).rowcount
                if changed != 1:
                    raise DeliveryJournalError("digest replacement lost its native row")
                released = conn.execute(
                    "UPDATE delivery_attempt_targets SET released_at = ?, release_actor = ?, "
                    "release_reason = ? WHERE attempt_id = ? AND binding_type = 'digest' "
                    "AND key_primary = ? AND COALESCE(key_secondary, '') = '' "
                    "AND released_at IS NULL",
                    (
                        resolved_at,
                        _DIGEST_REPLACEMENT_ACTOR,
                        _DIGEST_REPLACEMENT_REASON,
                        attempt_id,
                        week_key,
                    ),
                ).rowcount
                if released != 1:
                    raise DeliveryJournalError("digest replacement claim was not released once")
                retired = conn.execute(
                    "UPDATE delivery_attempts SET state = 'retired', dedupe_active = 0, "
                    "resolved_at = ?, resolved_by = ?, resolution_reason = ? "
                    "WHERE attempt_id = ? AND state = 'bound' AND dedupe_active = 1 "
                    "AND effect_kind = 'digest' AND channel_id = ? AND message_id = ?",
                    (
                        resolved_at,
                        _DIGEST_REPLACEMENT_ACTOR,
                        _DIGEST_REPLACEMENT_REASON,
                        attempt_id,
                        channel_key,
                        message_key,
                    ),
                ).rowcount
                if retired != 1:
                    raise DeliveryJournalError("digest replacement attempt was not retired once")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise

    @staticmethod
    def _suppressed(row: sqlite3.Row) -> DeliveryOutcome:
        return DeliveryOutcome(
            DeliveryOutcomeKind.SUPPRESSED,
            row["attempt_id"],
            int(row["effect_ordinal"]),
            row["state"],
        )
