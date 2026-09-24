"""Actual-Repo delivery journaling with deterministic fake transports."""

from __future__ import annotations

import asyncio
import re
from datetime import UTC, datetime, timedelta

import pytest

from bot.domain.timeutil import to_iso, utcnow
from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.coordinator import MaintenanceClosedError
from bot.infrastructure.maintenance.delivery import (
    BindingType,
    DedupePolicy,
    DeliveryOutcomeKind,
    DeliveryReceipt,
    DeliveryTarget,
    Destination,
    ObservedAttachment,
    ObservedMessage,
    SendFile,
    SendPayload,
    SendPlan,
)

CHANNEL = Destination.channel("1", "2")


def _run(repo: Repo, run_id: str = "run-1", channel_id: str = "2") -> None:
    repo._conn.execute(
        "INSERT INTO runs "
        "(id, channel_id, week_start, bosses, datetime, participants, status, source, created_at) "
        "VALUES (?, ?, ?, '[]', ?, '[]', 'planned', 'fixed', ?)",
        (run_id, channel_id, to_iso(utcnow()), to_iso(utcnow()), to_iso(utcnow())),
    )


def _amendment(repo: Repo, amendment_id: str, channel_id: str = "2") -> None:
    repo._conn.execute(
        "INSERT INTO amendments (id, week_start, kind, status, created_at, channel_id) "
        "VALUES (?, ?, 'add', 'proposed', ?, ?)",
        (amendment_id, to_iso(utcnow()), to_iso(utcnow()), channel_id),
    )


def _plan(
    targets: tuple[DeliveryTarget, ...] = (),
    *,
    destination: Destination = CHANNEL,
    dedupe: DedupePolicy | None = None,
    content: str = "test message",
    embeds: tuple[dict, ...] = (),
    files: tuple[SendFile, ...] = (),
    effect_kind: str = "test_effect",
) -> SendPlan:
    return SendPlan(
        effect_kind,
        destination,
        SendPayload(content=content, embeds=embeds, files=files),
        dedupe or (DedupePolicy.native() if targets else DedupePolicy.operation()),
        targets,
    )


def _receipt(plan: SendPlan, message_id: str = "9001") -> DeliveryReceipt:
    destination = plan.destination
    if destination.kind.value == "dm" and destination.channel_id is None:
        destination = Destination.dm(destination.recipient_id or "3", channel_id="500")
    return DeliveryReceipt(
        destination,
        message_id,
        ObservedMessage(
            content=plan.payload.content,
            embeds=plan.payload.embeds,
            user_mentions=plan.payload.allowed_user_ids,
            role_mentions=plan.payload.allowed_role_ids,
            everyone_mentioned=plan.payload.allow_everyone,
            replied_user=plan.payload.replied_user,
            reference=plan.payload.reference,
            attachments=tuple(
                ObservedAttachment(item.name, len(item.content)) for item in plan.payload.files
            ),
        ),
    )


class FakeTransport:
    def __init__(
        self,
        repo: Repo,
        *,
        error: BaseException | None = None,
        started: asyncio.Event | None = None,
        release: asyncio.Event | None = None,
        receipt_destination: Destination | None = None,
    ):
        self.repo = repo
        self.error = error
        self.started = started
        self.release = release
        self.receipt_destination = receipt_destination
        self.calls = 0

    async def send(self, plan: SendPlan) -> DeliveryReceipt:
        self.calls += 1
        assert not self.repo._conn.in_transaction
        attempt = self.repo._conn.execute(
            "SELECT attempt_id, state, request_fingerprint FROM delivery_attempts "
            "ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
        assert attempt is not None
        assert attempt["state"] == "intent"
        assert re.fullmatch(r"[0-9a-f]{64}", attempt["request_fingerprint"])
        target_count = self.repo._conn.execute(
            "SELECT COUNT(*) FROM delivery_attempt_targets WHERE attempt_id = ?",
            (attempt["attempt_id"],),
        ).fetchone()[0]
        expected_count = sum(target.binding_type.value != "debug_card" for target in plan.targets)
        assert target_count == expected_count
        if self.started is not None:
            self.started.set()
        if self.release is not None:
            await self.release.wait()
            assert not self.repo._conn.in_transaction
        if self.error is not None:
            raise self.error
        receipt = _receipt(plan, message_id=str(9000 + self.calls))
        if self.receipt_destination is not None:
            receipt = DeliveryReceipt(
                self.receipt_destination,
                receipt.message_id,
                receipt.observed,
            )
        return receipt


async def _execute(repo: Repo, plan: SendPlan, transport: FakeTransport):
    assert repo.maintenance is not None
    assert repo.delivery is not None
    async with repo.maintenance.operation("test_delivery"):
        return await repo.delivery.execute(plan, transport)


def _family_plan(repo: Repo, family: str) -> SendPlan:
    if family == "reminder":
        _run(repo)
        reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
        assert reminder_id is not None
        return _plan((DeliveryTarget.reminder(reminder_id),), effect_kind=family)
    if family == "digest":
        return _plan(
            (DeliveryTarget.digest(datetime(2026, 9, 24, tzinfo=UTC)),),
            effect_kind=family,
        )
    if family == "decline":
        _run(repo)
        return _plan((DeliveryTarget.decline("run-1", "3"),), effect_kind=family)
    if family == "card":
        _amendment(repo, "amendment-1")
        return _plan((DeliveryTarget.card("amendment-1"),), effect_kind=family)
    if family == "debug_card":
        _run(repo)
        return _plan(
            (DeliveryTarget.debug_card("run-1", "countdown_60"),),
            dedupe=DedupePolicy.operation(),
            effect_kind=family,
        )
    raise AssertionError(f"unknown family {family}")


@pytest.mark.parametrize(
    "family",
    ["reminder", "digest", "decline", "card", "debug_card"],
)
def test_each_native_target_family_finalizes_on_actual_repo(repo: Repo, family: str):
    plan = _family_plan(repo, family)
    transport = FakeTransport(repo)

    outcome = asyncio.run(_execute(repo, plan, transport))

    assert outcome.kind is DeliveryOutcomeKind.BOUND
    assert outcome.receipt is not None
    assert transport.calls == 1
    attempt = repo._conn.execute(
        "SELECT state, message_id, request_fingerprint, observable_fingerprint, "
        "fingerprint_version "
        "FROM delivery_attempts WHERE attempt_id = ?",
        (outcome.attempt_id,),
    ).fetchone()
    assert attempt["state"] == "bound"
    assert attempt["message_id"] == outcome.receipt.message_id
    assert attempt["fingerprint_version"] == 1
    assert re.fullmatch(r"[0-9a-f]{64}", attempt["request_fingerprint"])
    assert re.fullmatch(r"[0-9a-f]{64}", attempt["observable_fingerprint"])
    claim = repo._conn.execute(
        "SELECT binding_type, key_primary, key_secondary FROM delivery_attempt_targets "
        "WHERE attempt_id = ?",
        (outcome.attempt_id,),
    ).fetchone()
    assert claim is not None
    assert claim["binding_type"] == family

    if family == "reminder":
        row = repo.get_reminder(plan.targets[0].key_primary or "")
        assert row is not None and row["message_id"] == outcome.receipt.message_id
    elif family == "digest":
        row = repo.get_weekly_digest(datetime(2026, 9, 24, tzinfo=UTC))
        assert row is not None and row["message_id"] == outcome.receipt.message_id
    elif family == "decline":
        row = repo.get_decline_notice("run-1", "3")
        assert row is not None and row["message_id"] == outcome.receipt.message_id
    elif family == "card":
        row = repo._conn.execute(
            "SELECT proposal_message_id FROM amendments WHERE id = 'amendment-1'"
        ).fetchone()
        assert row[0] == outcome.receipt.message_id
    else:
        row = repo.debug_messages_for(outcome.receipt.message_id)
        assert len(row) == 1 and row[0]["run_id"] == "run-1"


@pytest.mark.parametrize("binding_type", ["memory_notice", "memory_proposal"])
def test_removed_memory_binding_types_cannot_be_planned(binding_type: str):
    assert binding_type not in {member.value for member in BindingType}
    with pytest.raises(ValueError):
        DeliveryTarget(binding_type, key_primary="1", key_secondary="3")


@pytest.mark.parametrize("family", ["reminder", "card"])
def test_grouped_targets_share_timestamp_and_response_identity(repo: Repo, family: str):
    if family == "reminder":
        _run(repo)
        first = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
        second = repo.add_reminder("run-1", "countdown_60", utcnow() - timedelta(minutes=1))
        assert first and second
        targets = (DeliveryTarget.reminder(second), DeliveryTarget.reminder(first))
    else:
        _amendment(repo, "amendment-a")
        _amendment(repo, "amendment-b")
        targets = (DeliveryTarget.card("amendment-b"), DeliveryTarget.card("amendment-a"))
    plan = _plan(targets, effect_kind=family)
    transport = FakeTransport(repo)

    outcome = asyncio.run(_execute(repo, plan, transport))

    assert outcome.receipt is not None
    claims = list(
        repo._conn.execute(
            "SELECT target_ordinal FROM delivery_attempt_targets WHERE attempt_id = ? "
            "ORDER BY target_ordinal",
            (outcome.attempt_id,),
        )
    )
    assert [row[0] for row in claims] == [0, 1]
    if family == "reminder":
        rows = [repo.get_reminder(key) for key in (first, second)]
        assert all(row is not None for row in rows)
        assert {row["message_id"] for row in rows if row is not None} == {
            outcome.receipt.message_id
        }
        assert len({row["sent_at"] for row in rows if row is not None}) == 1
    else:
        rows = list(
            repo._conn.execute(
                "SELECT proposal_message_id FROM amendments "
                "WHERE id IN ('amendment-a', 'amendment-b')"
            )
        )
        assert {row[0] for row in rows} == {outcome.receipt.message_id}


def test_duplicate_native_and_source_keys_suppress_transport(repo: Repo):
    _run(repo)
    first = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    second = repo.add_reminder("run-1", "countdown_60", utcnow() - timedelta(minutes=1))
    assert first and second
    plan = _plan((DeliveryTarget.reminder(first), DeliveryTarget.reminder(second)))
    source_plan = _plan(
        dedupe=DedupePolicy.source("1", "2", "7001", "chat.reply"),
        content="source-keyed reply",
    )
    native_transport = FakeTransport(repo)
    source_transport = FakeTransport(repo)

    async def run():
        assert repo.maintenance is not None and repo.delivery is not None
        async with repo.maintenance.operation("duplicate-test"):
            bound = await repo.delivery.execute(plan, native_transport)
            reversed_group = _plan(tuple(reversed(plan.targets)))
            suppressed = await repo.delivery.execute(reversed_group, native_transport)
            source_bound = await repo.delivery.execute(source_plan, source_transport)
            source_suppressed = await repo.delivery.execute(source_plan, source_transport)
            return bound, suppressed, source_bound, source_suppressed

    bound, suppressed, source_bound, source_suppressed = asyncio.run(run())
    assert bound.kind is DeliveryOutcomeKind.BOUND
    assert suppressed.kind is DeliveryOutcomeKind.SUPPRESSED
    assert suppressed.attempt_id == bound.attempt_id
    assert source_bound.kind is DeliveryOutcomeKind.BOUND
    assert source_suppressed.kind is DeliveryOutcomeKind.SUPPRESSED
    assert source_suppressed.attempt_id == source_bound.attempt_id
    assert native_transport.calls == 1
    assert source_transport.calls == 1


def test_chat_source_slots_allow_deliberate_replies_and_dedupe_after_restart(
    tmp_path, owner_lock_dir
):
    path = tmp_path / "chat-source.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    plans = [
        _plan(
            dedupe=DedupePolicy.source("1", "2", "7001", slot),
            content=f"reply for {slot}",
        )
        for slot in (
            "chat.answer.staging",
            "chat.answer.final",
            "chat.rejection_followup.reply.1",
            "chat.rejection_followup.reply.2",
        )
    ]
    transport = FakeTransport(repo)

    async def send_distinct_slots():
        async with repo.maintenance.operation("chat-source-slots"):
            return [await repo.delivery.execute(plan, transport) for plan in plans]

    outcomes = asyncio.run(send_distinct_slots())
    assert all(outcome.kind is DeliveryOutcomeKind.BOUND for outcome in outcomes)
    assert transport.calls == 4
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        retry_transport = FakeTransport(reopened)

        async def retry_after_restart():
            async with reopened.maintenance.operation("chat-source-restart"):
                return [await reopened.delivery.execute(plan, retry_transport) for plan in plans]

        suppressed = asyncio.run(retry_after_restart())
        assert all(outcome.kind is DeliveryOutcomeKind.SUPPRESSED for outcome in suppressed)
        assert {outcome.attempt_id for outcome in suppressed} == {
            outcome.attempt_id for outcome in outcomes
        }
        assert retry_transport.calls == 0
        assert (
            reopened._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempts "
                "WHERE state = 'bound' AND dedupe_scope = 'source'"
            ).fetchone()[0]
            == 4
        )
    finally:
        reopened.close()


def test_operation_scope_allocates_repeatable_effect_ordinals(repo: Repo):
    plan = _plan(content="deliberate repeat", dedupe=DedupePolicy.operation())
    transport = FakeTransport(repo)

    async def run():
        assert repo.maintenance is not None and repo.delivery is not None
        async with repo.maintenance.operation("repeat-test"):
            first = await repo.delivery.execute(plan, transport)
            second = await repo.delivery.execute(plan, transport)
            repeated_first = await repo.delivery.execute(
                _plan(content="deliberate repeat", dedupe=DedupePolicy.operation(0)), transport
            )
            return first, second, repeated_first

    first, second, repeated_first = asyncio.run(run())
    assert first.effect_ordinal == 0
    assert second.effect_ordinal == 1
    assert repeated_first.kind is DeliveryOutcomeKind.SUPPRESSED
    assert repeated_first.attempt_id == first.attempt_id
    assert transport.calls == 2


def test_operation_scope_allows_a_new_request_to_repeat_a_targetless_post(repo: Repo):
    plan = _plan(content="deliberate repeat", dedupe=DedupePolicy.operation())
    transport = FakeTransport(repo)

    async def run():
        first = await _execute(repo, plan, transport)
        second = await _execute(repo, plan, transport)
        return first, second

    first, second = asyncio.run(run())

    assert first.kind is second.kind is DeliveryOutcomeKind.BOUND
    assert first.effect_ordinal == second.effect_ordinal == 0
    assert transport.calls == 2
    assert (
        repo._conn.execute(
            "SELECT COUNT(DISTINCT operation_id) FROM delivery_attempts WHERE effect_kind = ?",
            (plan.effect_kind,),
        ).fetchone()[0]
        == 2
    )


def test_no_plaintext_payload_or_file_data_is_persisted(repo: Repo):
    _run(repo)
    reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    plan = _plan(
        (DeliveryTarget.reminder(reminder_id),),
        content="private message body",
        embeds=({"description": "private embed text"},),
        files=(SendFile("private-name.txt", b"private file contents"),),
    )
    outcome = asyncio.run(_execute(repo, plan, FakeTransport(repo)))
    rows = list(repo._conn.execute("SELECT * FROM delivery_attempts"))
    rows.extend(repo._conn.execute("SELECT * FROM delivery_attempt_targets"))
    stored = repr([tuple(row) for row in rows])

    for plaintext in (
        "private message body",
        "private embed text",
        "private-name.txt",
        "private file contents",
    ):
        assert plaintext not in stored
    assert outcome.receipt is not None
    assert re.fullmatch(r"[0-9a-f]{64}", rows[0]["request_fingerprint"])
    assert re.fullmatch(r"[0-9a-f]{64}", rows[0]["observable_fingerprint"])


def test_timeout_marks_uncertainty_and_suppresses_same_key_before_reopen(tmp_path, owner_lock_dir):
    path = tmp_path / "delivery-timeout.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    _run(repo)
    reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    plan = _plan((DeliveryTarget.reminder(reminder_id),))
    timeout_transport = FakeTransport(repo, error=TimeoutError("remote accepted, local timed out"))
    retry_transport = FakeTransport(repo)

    async def run():
        assert repo.maintenance is not None and repo.delivery is not None
        async with repo.maintenance.operation("timeout-test"):
            with pytest.raises(TimeoutError):
                await repo.delivery.execute(plan, timeout_transport)
            return await repo.delivery.execute(plan, retry_transport)

    suppressed = asyncio.run(run())
    assert suppressed.kind is DeliveryOutcomeKind.SUPPRESSED
    assert suppressed.state == "indeterminate"
    assert timeout_transport.calls == 1
    assert retry_transport.calls == 0
    attempt = repo._conn.execute(
        "SELECT attempt_id, dedupe_key, dedupe_active, state FROM delivery_attempts"
    ).fetchone()
    assert tuple(attempt)[2:] == (1, "indeterminate")
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        row = reopened._conn.execute(
            "SELECT attempt_id, dedupe_key, dedupe_active, state FROM delivery_attempts"
        ).fetchone()
        assert tuple(row) == tuple(attempt)
        assert (
            reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
        )
        assert (
            reopened._conn.execute("SELECT COUNT(*) FROM delivery_attempt_targets").fetchone()[0]
            == 1
        )

        async def denied_retry():
            assert reopened.maintenance is not None and reopened.delivery is not None
            async with reopened.maintenance.operation("post-reopen-retry"):
                return await reopened.delivery.execute(plan, retry_transport)

        with pytest.raises(MaintenanceClosedError):
            asyncio.run(denied_retry())
        assert retry_transport.calls == 0
    finally:
        reopened.close()


def test_cancelled_transport_records_uncertainty_before_lease_release(repo: Repo):
    _run(repo)
    reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    plan = _plan((DeliveryTarget.reminder(reminder_id),))
    transport = FakeTransport(repo, error=asyncio.CancelledError())

    with pytest.raises(asyncio.CancelledError):
        asyncio.run(_execute(repo, plan, transport))

    assert transport.calls == 1
    assert (
        repo._conn.execute("SELECT state FROM delivery_attempts").fetchone()[0] == "indeterminate"
    )
    assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0


def test_bound_dedupe_suppresses_after_repo_reopen(tmp_path, owner_lock_dir):
    path = tmp_path / "delivery-reopen.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    _run(repo)
    reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    plan = _plan((DeliveryTarget.reminder(reminder_id),))
    first_transport = FakeTransport(repo)
    first = asyncio.run(_execute(repo, plan, first_transport))
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        retry_transport = FakeTransport(reopened)
        suppressed = asyncio.run(_execute(reopened, plan, retry_transport))
        assert suppressed.kind is DeliveryOutcomeKind.SUPPRESSED
        assert suppressed.attempt_id == first.attempt_id
        assert retry_transport.calls == 0
        assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "OPEN"
    finally:
        reopened.close()


def test_finalization_failure_rolls_back_group_then_marks_indeterminate(repo: Repo, monkeypatch):
    _run(repo)
    first = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    second = repo.add_reminder("run-1", "countdown_60", utcnow() - timedelta(minutes=1))
    assert first and second
    plan = _plan((DeliveryTarget.reminder(first), DeliveryTarget.reminder(second)))
    transport = FakeTransport(repo)
    original = repo.mark_reminder_sent
    calls = 0

    def fail_second(reminder_id, message_id=None, at=None):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise RuntimeError("injected native binding failure")
        return original(reminder_id, message_id, at)

    monkeypatch.setattr(repo, "mark_reminder_sent", fail_second)

    async def run():
        assert repo.maintenance is not None and repo.delivery is not None
        async with repo.maintenance.operation("finalize-failure"):
            with pytest.raises(RuntimeError, match="injected native binding failure"):
                await repo.delivery.execute(plan, transport)
            return await repo.delivery.execute(plan, FakeTransport(repo))

    suppressed = asyncio.run(run())
    assert suppressed.kind is DeliveryOutcomeKind.SUPPRESSED
    assert transport.calls == 1
    assert calls == 2
    assert all(repo.get_reminder(key)["message_id"] is None for key in (first, second))
    assert (
        repo._conn.execute("SELECT state FROM delivery_attempts").fetchone()[0] == "indeterminate"
    )
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempt_targets").fetchone()[0] == 2


def test_finalize_after_prepare_timeout_keeps_accepted_lease_authority(repo: Repo):
    _run(repo)
    reminder_id = repo.add_reminder("run-1", "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    plan = _plan((DeliveryTarget.reminder(reminder_id),))
    started = asyncio.Event()
    release = asyncio.Event()
    transport = FakeTransport(repo, started=started, release=release)

    async def run():
        assert repo.maintenance is not None and repo.delivery is not None

        async def send():
            async with repo.maintenance.operation("accepted-before-prepare"):
                return await repo.delivery.execute(plan, transport)

        task = asyncio.create_task(send())
        await started.wait()
        assert not await repo.maintenance.prepare(timeout=0)
        assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
        release.set()
        outcome = await task
        return outcome

    outcome = asyncio.run(run())
    assert outcome.kind is DeliveryOutcomeKind.BOUND
    assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    assert repo._conn.execute("SELECT state FROM delivery_attempts").fetchone()[0] == "bound"
    assert repo.get_reminder(reminder_id)["message_id"] == outcome.receipt.message_id


def test_send_requires_the_current_task_lease(repo: Repo):
    plan = _plan()
    transport = FakeTransport(repo)
    assert repo.delivery is not None

    with pytest.raises(MaintenanceClosedError):
        asyncio.run(repo.delivery.execute(plan, transport))

    assert transport.calls == 0
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0


def test_empty_or_over_limit_payload_is_rejected_before_journal_or_transport(repo: Repo):
    transport = FakeTransport(repo)
    invalid_plans = (
        lambda: SendPlan("test_effect", CHANNEL, SendPayload(), DedupePolicy.operation()),
        lambda: SendPlan("test_effect", CHANNEL, SendPayload(content=""), DedupePolicy.operation()),
        lambda: SendPlan(
            "test_effect",
            CHANNEL,
            SendPayload(content="body", embeds=tuple({} for _ in range(11))),
            DedupePolicy.operation(),
        ),
        lambda: SendPlan(
            "test_effect",
            CHANNEL,
            SendPayload(
                content="body",
                files=tuple(SendFile(f"file-{index}", b"x") for index in range(11)),
            ),
            DedupePolicy.operation(),
        ),
    )

    for build_plan in invalid_plans:
        with pytest.raises(ValueError):
            build_plan()

    assert SendPayload(files=(SendFile("only-file.bin", b"x"),)).files
    assert len(SendPayload(embeds=tuple({"title": str(i)} for i in range(10))).embeds) == 10
    assert len(SendPayload(files=tuple(SendFile(f"file-{i}", b"x") for i in range(10))).files) == 10
    assert transport.calls == 0
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0
