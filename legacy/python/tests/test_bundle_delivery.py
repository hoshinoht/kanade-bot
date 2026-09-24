import copy
import json
from pathlib import Path

import pytest
from pydantic import ValidationError

from bot.infrastructure.bundle import Bundle, canonical_json

FIXTURE = Path(__file__).parent / "fixtures" / "bundle" / "example.json"


def value() -> dict:
    return json.loads(FIXTURE.read_text())


def test_grouped_reminders_and_cards_use_their_own_actual_messages() -> None:
    bundle = Bundle.model_validate_json(FIXTURE.read_bytes())
    groups = {group.delivery_kind: group for group in bundle.delivery.attempts}
    assert len(groups["reminder"].targets) == 2
    assert len(groups["card"].targets) == 2
    assert groups["reminder"].message_id != groups["card"].message_id
    assert Bundle.model_validate_json(canonical_json(bundle)) == bundle


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (
            lambda candidate: candidate["delivery"]["attempts"].append(
                copy.deepcopy(candidate["delivery"]["attempts"][0])
            ),
            "duplicate bound delivery message group",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"].append(
                {
                    **copy.deepcopy(candidate["delivery"]["attempts"][0]),
                    "channel_id": "100000000000000099",
                    "message_id": "100000000000000099",
                }
            ),
            "duplicate bound delivery target",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"].append(
                {
                    **copy.deepcopy(candidate["delivery"]["attempts"][0]),
                    "channel_id": "0100000000000000003",
                    "message_id": "0100000000000000004",
                }
            ),
            "pattern",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"][0].update(targets=[]),
            "at least 1 item",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"][0]["targets"][0].update(
                reminder_id="00000000-0000-0000-0000-000000000099"
            ),
            "no authoritative binding",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"][0].update(
                message_id="100000000000000099"
            ),
            "message does not match group",
        ),
        (
            lambda candidate: candidate["delivery"]["cards"][0].update(
                channel_id="100000000000000099"
            ),
            "channel does not match group",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"][0]["targets"].pop(),
            "exactly one bound delivery group",
        ),
        (
            lambda candidate: candidate["delivery"]["reminders"][0].update(sent_at=None),
            "delivery state is partial",
        ),
    ],
)
def test_grouped_delivery_rejects_invalid_live_bindings(mutate, message: str) -> None:
    candidate = value()
    mutate(candidate)
    with pytest.raises(ValidationError, match=message):
        Bundle.model_validate_json(json.dumps(candidate))


def test_retired_targets_are_historical_and_need_not_resolve() -> None:
    candidate = value()
    candidate["delivery"]["attempts"].append(
        {
            "state": "retired",
            "delivery_kind": "reminder",
            "targets": [
                {"binding_type": "reminder", "reminder_id": "00000000-0000-0000-0000-000000000099"}
            ],
            "retired_by": "admin",
            "retired_reason": "deleted during authoritative reconciliation",
        }
    )
    attempt = Bundle.model_validate_json(json.dumps(candidate)).delivery.attempts[-1]
    assert attempt.state == "retired"


def test_pre_journal_attestation_is_the_only_non_native_retirement() -> None:
    candidate = value()
    candidate["delivery"]["attempts"].append(
        {
            "state": "retired",
            "delivery_kind": "pre_journal_attestation",
            "retired_by": "system",
            "retired_reason": "pre-journal effects reconciled by operator",
        }
    )
    assert Bundle.model_validate_json(json.dumps(candidate)).delivery.attempts[-1].targets == []


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (
            lambda candidate: candidate["delivery"]["attempts"][0].update(delivery_kind="card"),
            "must match every native target",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"][2]["targets"].append(
                copy.deepcopy(candidate["delivery"]["attempts"][2]["targets"][0])
            ),
            "requires exactly one native target",
        ),
        (
            lambda candidate: candidate["delivery"]["attempts"].append(
                {
                    "state": "retired",
                    "delivery_kind": "pre_journal_attestation",
                    "targets": [
                        {
                            "binding_type": "reminder",
                            "reminder_id": "00000000-0000-0000-0000-000000000006",
                        }
                    ],
                    "retired_by": "system",
                    "retired_reason": "invalid target claim",
                }
            ),
            "must not claim native targets",
        ),
    ],
)
def test_delivery_kind_correlation_and_cardinality_are_enforced(mutate, message: str) -> None:
    candidate = value()
    mutate(candidate)
    with pytest.raises(ValidationError, match=message):
        Bundle.model_validate_json(json.dumps(candidate))


@pytest.mark.parametrize(
    "mutate",
    [
        lambda candidate: candidate["schedule"]["amendments"][0].update(proposal_message_id=None),
        lambda candidate: candidate["schedule"]["amendments"][0].update(
            channel_id="100000000000000099"
        ),
    ],
)
def test_card_bindings_require_amendment_message_and_channel_identity(mutate) -> None:
    candidate = value()
    mutate(candidate)
    with pytest.raises(ValidationError, match="proposal identity"):
        Bundle.model_validate_json(json.dumps(candidate))


def test_unsent_future_reminder_needs_no_delivery_group() -> None:
    candidate = value()
    candidate["delivery"]["reminders"].append(
        {
            "id": "00000000-0000-0000-0000-000000000014",
            "run_id": "00000000-0000-0000-0000-000000000002",
            "fire_at": "2026-09-18T20:15:00+08:00",
            "kind": "countdown:future",
        }
    )
    assert Bundle.model_validate_json(json.dumps(candidate)).delivery.reminders[-1].sent_at is None


@pytest.mark.parametrize(
    "attempt",
    [
        {
            "state": "bound",
            "delivery_kind": "memory",
            "channel_id": "100000000000000003",
            "message_id": "100000000000000004",
            "targets": [
                {"binding_type": "reminder", "reminder_id": "00000000-0000-0000-0000-000000000006"}
            ],
        },
        {"state": "indeterminate", "delivery_kind": "reminder"},
        {
            "state": "retired",
            "delivery_kind": "generic",
            "retired_by": "admin",
            "retired_reason": "attested",
        },
        {
            "state": "retired",
            "delivery_kind": "memory",
            "retired_by": "system",
            "retired_reason": "attested",
        },
    ],
)
def test_memory_and_unresolved_delivery_variants_are_not_portable(attempt: dict) -> None:
    candidate = value()
    candidate["delivery"]["attempts"].append(attempt)
    with pytest.raises(ValidationError):
        Bundle.model_validate_json(json.dumps(candidate))
