"""Cross-reference grouped sends to authoritative native bindings."""

from datetime import datetime

from pydantic import Field, model_validator

from ..common import AwareModel
from .attempt import BoundDeliveryAttempt, DeliveryAttempt, RetiredDeliveryAttempt
from .binding import CardBinding, DebugCardBinding, DeclineBinding, DigestBinding, ReminderBinding
from .refs import CardTarget, DebugCardTarget, DeclineTarget, DigestTarget, ReminderTarget


class DeliveryLedger(AwareModel):
    current_week_start: datetime
    pinned_at: datetime
    reminders: list[ReminderBinding]
    digests: list[DigestBinding]
    declines: list[DeclineBinding]
    cards: list[CardBinding]
    debug_cards: list[DebugCardBinding]
    attempts: list[DeliveryAttempt]
    retired_out_of_week_bindings: int = Field(ge=0)

    @model_validator(mode="after")
    def _attempts_match_authoritative_bindings(self) -> "DeliveryLedger":
        reminders = {row.id: row for row in self.reminders}
        digests = {row.week_start: row for row in self.digests}
        declines = {(row.run_id, row.user_id): row for row in self.declines}
        cards = {row.amendment_id: row for row in self.cards}
        debug_cards = {row.message_id: row for row in self.debug_cards}
        delivered = set()
        for name, rows, source in (
            ("reminder", reminders, self.reminders),
            ("digest", digests, self.digests),
            ("decline", declines, self.declines),
            ("card", cards, self.cards),
            ("debug_card", debug_cards, self.debug_cards),
        ):
            if len(rows) != len(source):
                raise ValueError(f"duplicate authoritative {name} binding")
        for reminder_id, row in reminders.items():
            if (row.sent_at is None) != (row.message_id is None):
                raise ValueError("reminder delivery state is partial")
            if row.sent_at is not None:
                delivered.add(("reminder", reminder_id))
        for key, row in declines.items():
            if (row.channel_id is None) != (row.message_id is None):
                raise ValueError("decline delivery state is partial")
            if row.message_id is None:
                raise ValueError("decline delivery state is unresolved")
            delivered.add(("decline", key))
        delivered.update(("digest", key) for key in digests)
        delivered.update(("card", key) for key in cards)
        delivered.update(("debug_card", key) for key in debug_cards)

        grouped = set()
        targets = set()
        for attempt in self.attempts:
            if isinstance(attempt, RetiredDeliveryAttempt):
                _validate_retired_targets(attempt)
                continue
            if (
                attempt.delivery_kind in {"digest", "decline", "debug_card"}
                and len(attempt.targets) != 1
            ):
                raise ValueError("bound delivery kind requires exactly one native target")
            if any(target.binding_type != attempt.delivery_kind for target in attempt.targets):
                raise ValueError("bound delivery kind must match every native target")
            if not isinstance(attempt, BoundDeliveryAttempt):
                continue
            group = (attempt.channel_id, attempt.message_id)
            if group in grouped:
                raise ValueError("duplicate bound delivery message group")
            grouped.add(group)
            for target in attempt.targets:
                key, row = _resolve_target(target, reminders, digests, declines, cards, debug_cards)
                if key in targets:
                    raise ValueError("duplicate bound delivery target")
                targets.add(key)
                if row.message_id != attempt.message_id:
                    raise ValueError("bound delivery target message does not match group")
                if getattr(row, "channel_id", None) not in {None, attempt.channel_id}:
                    raise ValueError("bound delivery target channel does not match group")
        if targets != delivered:
            raise ValueError(
                "every delivered binding must belong to exactly one bound delivery group"
            )
        return self


def _resolve_target(target, reminders, digests, declines, cards, debug_cards):
    if isinstance(target, ReminderTarget):
        row = reminders.get(target.reminder_id)
        key = ("reminder", target.reminder_id)
    elif isinstance(target, DigestTarget):
        row = digests.get(target.week_start)
        key = ("digest", target.week_start)
    elif isinstance(target, DeclineTarget):
        key = ("decline", (target.run_id, target.user_id))
        row = declines.get(key[1])
    elif isinstance(target, CardTarget):
        row = cards.get(target.amendment_id)
        key = ("card", target.amendment_id)
    elif isinstance(target, DebugCardTarget):
        row = debug_cards.get(target.message_id)
        key = ("debug_card", target.message_id)
    else:  # pragma: no cover - the discriminated union makes this unreachable.
        raise TypeError("unsupported native delivery target")
    if row is None:
        raise ValueError("bound delivery target has no authoritative binding")
    return key, row


def _validate_retired_targets(attempt: RetiredDeliveryAttempt) -> None:
    if attempt.delivery_kind == "pre_journal_attestation":
        if attempt.targets:
            raise ValueError("pre-journal attestation must not claim native targets")
        return
    if any(target.binding_type != attempt.delivery_kind for target in attempt.targets):
        raise ValueError("retired delivery kind must match every native target")
