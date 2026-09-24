"""Integrated Discord effects over the real Repo and fake transport."""

from __future__ import annotations

import asyncio
from datetime import timedelta

import pytest

from bot.agent import effects, formatting
from bot.agent.client import BossBot
from bot.domain.timeutil import utcnow
from bot.infrastructure.maintenance.coordinator import MaintenanceClosedError
from bot.infrastructure.maintenance.delivery import (
    DedupePolicy,
    DeliveryOutcomeKind,
    DeliveryTarget,
    Destination,
    observable_fingerprint,
    request_fingerprint,
)

from .conftest import RESET_TIME, RESET_WEEKDAY, TZ, kl
from .fake_bot import GUILD_ID, WATCHED_CHANNEL, FakeMessage


def _run(repo, channel_id=WATCHED_CHANNEL):
    return repo.create_run(
        kl(2026, 9, 10),
        ["HMaleficStar"],
        kl(2026, 9, 10, 21),
        ["1001", "1002"],
        channel_id=str(channel_id),
    )


def test_grouped_card_intent_and_targets_precede_one_send_and_reactions(
    fake_bot, repo, tmp_path, monkeypatch
):
    run_id = _run(repo)
    first = repo.add_reminder(run_id, "day_of", utcnow() - timedelta(minutes=1))
    second = repo.add_reminder(run_id, "countdown_60", utcnow() - timedelta(minutes=1))
    assert first and second
    channel = fake_bot.channels[WATCHED_CHANNEL]
    portrait = tmp_path / "portrait.png"
    splash = tmp_path / "splash.png"
    portrait.write_bytes(b"portrait-bytes")
    splash.write_bytes(b"splash-bytes")
    card = formatting.Card(
        content="Tonight <@1001>",
        title="Reminder",
        fields=[("First", "one"), ("Second", "two")],
        footer="Reply with a reaction",
        mention_users=["1001"],
        thumbnail_path=portrait,
        image_path=splash,
    )
    plans = []
    outcomes = []
    original_execute = repo.delivery.execute

    async def capture_plan(plan, transport):
        plans.append(plan)
        outcome = await original_execute(plan, transport)
        outcomes.append(outcome)
        return outcome

    monkeypatch.setattr(repo.delivery, "execute", capture_plan)
    original_send = channel.send
    reaction_checks = []

    async def inspect_send(content=None, **kwargs):
        assert not repo._conn.in_transaction
        row = repo._conn.execute(
            "SELECT attempt_id, state, request_fingerprint FROM delivery_attempts "
            "ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
        assert row is not None and row["state"] == "intent"
        assert plans
        assert row["request_fingerprint"] == request_fingerprint(plans[-1])
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE attempt_id = ?",
                (row["attempt_id"],),
            ).fetchone()[0]
            == 2
        )
        assert [(file.filename, file.fp.getvalue()) for file in kwargs["files"]] == [
            ("portrait.png", b"portrait-bytes"),
            ("image-splash.png", b"splash-bytes"),
        ]
        message = await original_send(content, **kwargs)
        message.mentions = []  # The receipt must use Discord-observed mentions.

        def bound_before_reaction(_emoji):
            attempt = repo._conn.execute(
                "SELECT state, message_id FROM delivery_attempts WHERE attempt_id = ?",
                (row["attempt_id"],),
            ).fetchone()
            assert not repo._conn.in_transaction
            assert attempt["state"] == "bound" and attempt["message_id"] == str(message.id)
            assert all(
                repo.get_reminder(reminder_id)["message_id"] == str(message.id)
                for reminder_id in (first, second)
            )
            reaction_checks.append(_emoji)

        message.on_add_reaction = bound_before_reaction
        return message

    monkeypatch.setattr(channel, "send", inspect_send)

    message = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="reminder",
            targets=(DeliveryTarget.reminder(first), DeliveryTarget.reminder(second)),
        )
    )

    assert message is not None
    assert channel.send_calls == 1
    assert channel.fetch_calls == 0
    assert len(plans) == len(outcomes) == 1
    plan = plans[0]
    outcome = outcomes[0]
    assert outcome.kind is DeliveryOutcomeKind.BOUND
    assert plan.destination == Destination.channel(GUILD_ID, WATCHED_CHANNEL)
    assert plan.payload.allowed_user_ids == ("1001",)
    assert plan.payload.replied_user is False
    assert [item.content for item in plan.payload.files] == [b"portrait-bytes", b"splash-bytes"]
    assert [field["name"] for field in plan.payload.embeds[0]["fields"]] == ["First", "Second"]
    assert [field["name"] for field in message.embeds[0].to_dict()["fields"]] == [
        "First",
        "Second",
    ]
    assert reaction_checks == ["✅", "❌"]
    receipt = outcome.receipt
    assert receipt is not None
    assert receipt.destination == Destination.channel(GUILD_ID, WATCHED_CHANNEL)
    assert receipt.message_id == str(message.id)
    assert receipt.observed.user_mentions == ()
    attempt = repo._conn.execute(
        "SELECT state, request_fingerprint, observable_fingerprint "
        "FROM delivery_attempts WHERE attempt_id = ?",
        (outcome.attempt_id,),
    ).fetchone()
    assert attempt["state"] == "bound"
    assert attempt["request_fingerprint"] == request_fingerprint(plan)
    assert attempt["observable_fingerprint"] == observable_fingerprint(
        receipt.destination, receipt.observed
    )


def test_chat_plain_reply_journals_source_slot_before_one_observed_send(
    fake_bot, repo, monkeypatch
):
    channel = fake_bot.channels[WATCHED_CHANNEL]
    source_id = 900000000000000321
    plans = []
    original_execute = repo.delivery.execute
    original_send = channel.send

    async def capture_plan(plan, transport):
        plans.append(plan)
        return await original_execute(plan, transport)

    async def inspect_send(content=None, **kwargs):
        assert not repo._conn.in_transaction
        attempt = repo._conn.execute(
            "SELECT state, dedupe_scope, request_fingerprint FROM delivery_attempts "
            "ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
        assert attempt is not None and attempt["state"] == "intent"
        assert attempt["dedupe_scope"] == "source"
        assert plans and attempt["request_fingerprint"] == request_fingerprint(plans[-1])
        assert kwargs["reference"].message_id == source_id
        return await original_send(content, **kwargs)

    monkeypatch.setattr(repo.delivery, "execute", capture_plan)
    monkeypatch.setattr(channel, "send", inspect_send)

    posted = asyncio.run(
        BossBot.post_plain(
            fake_bot,
            channel,
            "private chat answer",
            ["1002"],
            reference_id=source_id,
            source_message_id=source_id,
            semantic_slot="chat.answer.final",
        )
    )

    assert posted is not None
    assert channel.send_calls == 1
    plan = plans[0]
    assert plan.dedupe.scope.value == "source"
    assert plan.dedupe.guild_id == str(GUILD_ID)
    assert plan.dedupe.channel_id == str(WATCHED_CHANNEL)
    assert plan.dedupe.source_id == str(source_id)
    assert plan.dedupe.semantic_slot == "chat.answer.final"
    assert plan.payload.allowed_user_ids == ("1002",)
    assert plan.payload.reference == str(source_id)
    attempt = repo._conn.execute(
        "SELECT state, message_id, request_fingerprint, observable_fingerprint "
        "FROM delivery_attempts"
    ).fetchone()
    assert attempt["state"] == "bound" and attempt["message_id"] == str(posted.id)
    assert attempt["request_fingerprint"] == request_fingerprint(plan)
    assert attempt["observable_fingerprint"] is not None
    assert posted.reference.message_id == source_id
    assert "private chat answer" not in repr(tuple(attempt))


def test_chat_plain_reply_keeps_quiet_mode_and_reply_reference(fake_bot, repo):
    repo.set_config("quiet_mode", "1")
    channel = fake_bot.channels[WATCHED_CHANNEL]
    source_id = 900000000000000322

    posted = asyncio.run(
        BossBot.post_plain(
            fake_bot,
            channel,
            "Quiet answer <@1002>",
            ["1002"],
            reference_id=source_id,
            source_message_id=source_id,
            semantic_slot="chat.answer.final",
        )
    )

    assert posted is not None
    attempt = repo._conn.execute(
        "SELECT dedupe_scope, request_fingerprint FROM delivery_attempts WHERE message_id = ?",
        (str(posted.id),),
    ).fetchone()
    assert attempt is not None and attempt["dedupe_scope"] == "source"
    assert posted.content == formatting.quiet_line("Quiet answer <@1002>")
    assert posted.mentions == []
    assert posted.reference.message_id == source_id
    assert "Quiet answer <@1002>" not in repr(tuple(attempt))


def test_operation_plain_intent_precedes_one_send_and_a_new_request_can_repeat(
    fake_bot, repo, monkeypatch
):
    channel = fake_bot.channels[WATCHED_CHANNEL]
    plans = []
    original_execute = repo.delivery.execute
    original_send = channel.send

    async def capture_plan(plan, transport):
        plans.append(plan)
        return await original_execute(plan, transport)

    async def inspect_send(content=None, **kwargs):
        assert not repo._conn.in_transaction
        attempt = repo._conn.execute(
            "SELECT attempt_id, state, dedupe_scope, effect_kind, request_fingerprint "
            "FROM delivery_attempts ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
        assert attempt is not None and attempt["state"] == "intent"
        assert attempt["dedupe_scope"] == "operation"
        assert attempt["effect_kind"].startswith("notice.run.move.moved.")
        assert "run-private-identifier" not in attempt["effect_kind"]
        assert plans and attempt["request_fingerprint"] == request_fingerprint(plans[-1])
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE attempt_id = ?",
                (attempt["attempt_id"],),
            ).fetchone()[0]
            == 0
        )
        return await original_send(content, **kwargs)

    monkeypatch.setattr(repo.delivery, "execute", capture_plan)
    monkeypatch.setattr(channel, "send", inspect_send)

    async def send():
        return await effects.send_operation_plain(
            fake_bot,
            channel,
            "run moved",
            mention_users=[],
            effect_kind="notice.run.move.moved",
            context=("run-private-identifier", "2026-09-30T12:00:00+00:00"),
        )

    first, second = asyncio.run(send()), asyncio.run(send())

    assert first is not None and second is not None and first.id != second.id
    assert channel.send_calls == 2
    assert len(plans) == 2
    assert all(plan.dedupe.scope.value == "operation" and not plan.targets for plan in plans)
    rows = list(
        repo._conn.execute(
            "SELECT operation_id, effect_ordinal, state FROM delivery_attempts "
            "WHERE effect_kind LIKE 'notice.run.move.moved.%' ORDER BY rowid"
        )
    )
    assert [row[1] for row in rows] == [0, 0]
    assert rows[0][0] != rows[1][0]
    assert all(row[2] == "bound" for row in rows)


def test_decline_notice_binds_before_send_and_retraction_retires_exact_claim(
    fake_bot, repo, monkeypatch
):
    from bot.agent import client as client_module

    run_id = _run(repo)
    run = repo.get_run(run_id)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    now = utcnow()
    monkeypatch.setattr(client_module, "utcnow", lambda: now)
    original_send = channel.send

    async def inspect_send(content=None, **kwargs):
        assert not repo._conn.in_transaction
        attempt = repo._conn.execute(
            "SELECT attempt_id, state, effect_kind FROM delivery_attempts "
            "ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
        assert attempt is not None and attempt["state"] == "intent"
        assert attempt["effect_kind"].startswith("decline.notice.")
        native = repo.get_decline_notice(run_id, "1001")
        assert native is None or native["message_id"] is None
        claim = repo._conn.execute(
            "SELECT binding_type, key_primary, key_secondary FROM delivery_attempt_targets "
            "WHERE attempt_id = ?",
            (attempt["attempt_id"],),
        ).fetchone()
        assert tuple(claim) == ("decline", run_id, "1001")
        return await original_send(content, **kwargs)

    monkeypatch.setattr(channel, "send", inspect_send)

    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))

    row = repo.get_decline_notice(run_id, "1001")
    assert row is not None and row["message_id"] is not None
    message = channel.messages[int(row["message_id"])]
    attempt = repo._conn.execute(
        "SELECT attempt_id, state, dedupe_active FROM delivery_attempts WHERE message_id = ?",
        (row["message_id"],),
    ).fetchone()
    attempt_id = attempt["attempt_id"]
    notified_at = row["notified_at"]
    assert tuple(attempt)[1:] == ("bound", 1)

    asyncio.run(BossBot.retract_decline(fake_bot, run, "1001"))

    assert message.deleted
    cleared = repo.get_decline_notice(run_id, "1001")
    assert cleared is not None and cleared["message_id"] is None
    assert cleared["notified_at"] == notified_at
    retired = repo._conn.execute(
        "SELECT state, dedupe_active, resolved_by, resolution_reason "
        "FROM delivery_attempts WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert tuple(retired) == (
        "retired",
        0,
        "service:decline-retraction",
        "confirmed Discord deletion for decline retraction",
    )
    target = repo._conn.execute(
        "SELECT released_at, release_actor, release_reason FROM delivery_attempt_targets "
        "WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert target["released_at"] is not None
    assert target["release_actor"] == "service:decline-retraction"
    assert target["release_reason"] == "confirmed Discord deletion for decline retraction"

    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))
    assert channel.send_calls == 1

    from bot.agent.client import DECLINE_NOTICE_COOLDOWN

    monkeypatch.setattr(
        client_module,
        "utcnow",
        lambda: notified_at + DECLINE_NOTICE_COOLDOWN + timedelta(seconds=1),
    )
    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))
    assert channel.send_calls == 2
    assert repo.get_decline_notice(run_id, "1001")["message_id"] != row["message_id"]


def test_decline_timeout_keeps_only_the_uncertain_journal_claim(fake_bot, repo):
    run_id = _run(repo)
    run = repo.get_run(run_id)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    fake_bot.digest_fails = True

    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))

    fake_bot.digest_fails = False
    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))

    assert channel.send_calls == 1
    assert repo.get_decline_notice(run_id, "1001") is None
    attempt = repo._conn.execute(
        "SELECT attempt_id, state, dedupe_active FROM delivery_attempts "
        "WHERE effect_kind LIKE 'decline.notice.%'"
    ).fetchone()
    assert tuple(attempt)[1:] == ("indeterminate", 1)
    assert (
        repo._conn.execute(
            "SELECT released_at FROM delivery_attempt_targets WHERE attempt_id = ?",
            (attempt["attempt_id"],),
        ).fetchone()[0]
        is None
    )


def test_legacy_null_decline_row_keeps_cooldown_and_never_becomes_safe_by_age(
    fake_bot, repo, monkeypatch
):
    from bot.agent import client as client_module
    from bot.agent.client import DECLINE_NOTICE_COOLDOWN

    run_id = _run(repo)
    run = repo.get_run(run_id)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    now = utcnow()
    repo.set_decline_notice(run_id, "1001", None, WATCHED_CHANNEL, at=now)
    monkeypatch.setattr(client_module, "utcnow", lambda: now)

    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))

    monkeypatch.setattr(
        client_module, "utcnow", lambda: now + DECLINE_NOTICE_COOLDOWN + timedelta(seconds=1)
    )
    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))

    assert channel.send_calls == 0
    assert repo.get_decline_notice(run_id, "1001")["message_id"] is None
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0


def test_legacy_bound_decline_without_a_journal_claim_is_not_deleted(fake_bot, repo):
    run_id = _run(repo)
    run = repo.get_run(run_id)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    message = FakeMessage(900000000000000345, channel, fake_bot)
    channel.messages[message.id] = message
    repo.set_decline_notice(run_id, "1001", message.id, WATCHED_CHANNEL)

    asyncio.run(BossBot.retract_decline(fake_bot, run, "1001"))

    assert not message.deleted
    assert channel.fetch_calls == 0
    assert repo.get_decline_notice(run_id, "1001")["message_id"] == str(message.id)
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0


def test_failed_decline_deletion_retains_binding_and_suppresses_repost(fake_bot, repo, monkeypatch):
    from bot.agent import client as client_module
    from bot.agent.client import DECLINE_NOTICE_COOLDOWN

    run_id = _run(repo)
    run = repo.get_run(run_id)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    now = utcnow()
    monkeypatch.setattr(client_module, "utcnow", lambda: now)
    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))
    row = repo.get_decline_notice(run_id, "1001")
    message = channel.messages[int(row["message_id"])]
    message.fail_delete = True

    asyncio.run(BossBot.retract_decline(fake_bot, run, "1001"))

    assert not message.deleted
    assert repo.get_decline_notice(run_id, "1001")["message_id"] == row["message_id"]
    assert repo.delivery.has_active_claim(DeliveryTarget.decline(run_id, "1001"))
    monkeypatch.setattr(
        client_module,
        "utcnow",
        lambda: row["notified_at"] + DECLINE_NOTICE_COOLDOWN + timedelta(seconds=1),
    )
    asyncio.run(BossBot.notify_decline(fake_bot, run, "1001", "Alvin"))
    assert channel.send_calls == 1


def test_reaction_failure_after_binding_does_not_replay_the_card(fake_bot, repo, monkeypatch):
    run_id = _run(repo)
    reminder_id = repo.add_reminder(run_id, "day_of", utcnow() - timedelta(minutes=1))
    assert reminder_id is not None
    channel = fake_bot.channels[WATCHED_CHANNEL]
    original_send = channel.send

    async def reaction_failure_send(content=None, **kwargs):
        message = await original_send(content, **kwargs)

        def fail_reaction(_emoji):
            raise OSError("reaction endpoint is unavailable")

        message.on_add_reaction = fail_reaction
        return message

    monkeypatch.setattr(channel, "send", reaction_failure_send)
    card = formatting.Card("bound before reactions")

    first = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="reminder",
            targets=(DeliveryTarget.reminder(reminder_id),),
        )
    )
    retry = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="reminder",
            targets=(DeliveryTarget.reminder(reminder_id),),
        )
    )

    assert first is not None and retry is None
    assert channel.send_calls == 1
    assert first.reactions == []
    assert repo.get_reminder(reminder_id)["message_id"] == str(first.id)
    assert (
        repo._conn.execute(
            "SELECT state FROM delivery_attempts WHERE effect_kind = 'reminder'"
        ).fetchone()[0]
        == "bound"
    )


def test_reminder_client_callsites_group_and_bind_native_rows(fake_bot, repo, monkeypatch):
    run_id = _run(repo)
    run = repo.get_run(run_id)
    second_run_id = _run(repo)
    second_run = repo.get_run(second_run_id)
    first = repo.add_reminder(run_id, "day_of", utcnow() - timedelta(minutes=1))
    second = repo.add_reminder(second_run_id, "day_of", utcnow() - timedelta(minutes=1))
    countdown = repo.add_reminder(run_id, "countdown_60", utcnow() - timedelta(minutes=1))
    assert first and second and countdown
    card = formatting.Card("grouped reminder")
    monkeypatch.setattr(fake_bot, "day_of_card_for", lambda _runs: card, raising=False)
    monkeypatch.setattr(fake_bot, "countdown_card_for", lambda _run, _minutes: card, raising=False)
    channel = fake_bot.channels[WATCHED_CHANNEL]

    asyncio.run(
        BossBot._send_day_of(
            fake_bot,
            str(WATCHED_CHANNEL),
            [(repo.get_reminder(first), run), (repo.get_reminder(second), second_run)],
        )
    )
    grouped_message_ids = {
        repo.get_reminder(reminder_id)["message_id"] for reminder_id in (first, second)
    }
    assert len(grouped_message_ids) == 1 and None not in grouped_message_ids
    asyncio.run(BossBot._send_countdown(fake_bot, repo.get_reminder(countdown), run))
    assert repo.get_reminder(countdown)["message_id"] is not None
    assert channel.send_calls == 2
    assert (
        repo._conn.execute(
            "SELECT COUNT(*) FROM delivery_attempts "
            "WHERE effect_kind = 'reminder' AND state = 'bound'"
        ).fetchone()[0]
        == 2
    )


def test_quiet_mode_wording_and_allowlist_are_in_the_journalled_plan(fake_bot, repo, monkeypatch):
    run_id = _run(repo)
    repo.set_config("quiet_mode", "1")
    channel = fake_bot.channels[WATCHED_CHANNEL]
    card = formatting.Card(
        "Test <@1001>", title="Quiet", footer="Existing footer", mention_users=["1001"]
    )
    plans = []
    original_execute = repo.delivery.execute

    async def capture_plan(plan, transport):
        plans.append(plan)
        return await original_execute(plan, transport)

    monkeypatch.setattr(repo.delivery, "execute", capture_plan)

    message = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="debug_card",
            targets=(DeliveryTarget.debug_card(run_id, "day_of"),),
            dedupe=DedupePolicy.operation(),
            react=False,
        )
    )

    assert message is not None
    assert plans[0].payload.allowed_user_ids == ()
    assert formatting.QUIET_MARKER in message.embeds[0].footer.text
    assert fake_bot.posts[-1].mentions == []
    assert fake_bot.repo.debug_messages_for(message.id)
    assert repo.list_reminders(run_id) == []


def test_ambiguous_timeout_is_durable_and_suppresses_the_next_send(fake_bot, repo):
    channel = fake_bot.channels[WATCHED_CHANNEL]
    week = kl(2026, 9, 24)
    plan = dict(
        effect_kind="digest",
        targets=(DeliveryTarget.digest(week),),
        react=False,
    )
    fake_bot.digest_fails = True
    first = asyncio.run(effects.send_card(fake_bot, channel, formatting.Card("digest"), **plan))
    fake_bot.digest_fails = False
    second = asyncio.run(effects.send_card(fake_bot, channel, formatting.Card("digest"), **plan))

    assert first is second is None
    assert channel.send_calls == 1
    attempt = repo._conn.execute(
        "SELECT state, dedupe_active FROM delivery_attempts WHERE effect_kind = 'digest'"
    ).fetchone()
    assert tuple(attempt) == ("indeterminate", 1)


def test_unjournalled_active_digest_is_not_deleted_or_replaced(fake_bot, repo):
    week = kl(2026, 9, 24)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    message = FakeMessage(900000000000000321, channel, fake_bot)
    channel.messages[message.id] = message
    repo.set_weekly_digest(week, WATCHED_CHANNEL, message.id)

    deleted = asyncio.run(
        effects.retire_deleted_digest(fake_bot, week, WATCHED_CHANNEL, message.id, at=utcnow())
    )

    assert deleted is False
    assert not message.deleted
    assert channel.fetch_calls == 0
    assert channel.send_calls == 0
    assert repo.get_weekly_digest(week, active_only=True) is not None
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0


def test_confirmed_digest_delete_retires_and_releases_exact_attempt(fake_bot, repo):
    week = kl(2026, 9, 24)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    card = formatting.Card("weekly digest", title="Digest")

    first = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="digest",
            targets=(DeliveryTarget.digest(week),),
            react=False,
        )
    )
    assert first is not None
    attempt_id = repo._conn.execute(
        "SELECT attempt_id FROM delivery_attempts WHERE state = 'bound'"
    ).fetchone()[0]
    first.fail_delete = True
    at = utcnow()

    assert not asyncio.run(
        effects.retire_deleted_digest(fake_bot, week, WATCHED_CHANNEL, first.id, at=at)
    )
    assert not first.deleted
    assert repo.get_weekly_digest(week, active_only=True) is not None
    assert repo._conn.execute(
        "SELECT state, dedupe_active FROM delivery_attempts WHERE attempt_id = ?", (attempt_id,)
    ).fetchone()[:] == ("bound", 1)
    assert channel.send_calls == 1

    first.fail_delete = False
    assert asyncio.run(
        effects.retire_deleted_digest(fake_bot, week, WATCHED_CHANNEL, first.id, at=at)
    )
    retired = repo._conn.execute(
        "SELECT state, dedupe_active, resolved_by, resolution_reason "
        "FROM delivery_attempts WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert tuple(retired) == (
        "retired",
        0,
        "service:digest-replacement",
        "confirmed Discord deletion for digest replacement",
    )
    target = repo._conn.execute(
        "SELECT released_at, release_actor, release_reason FROM delivery_attempt_targets "
        "WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert target["released_at"] is not None
    assert target["release_actor"] == "service:digest-replacement"
    assert target["release_reason"] == "confirmed Discord deletion for digest replacement"
    assert repo.get_weekly_digest(week, active_only=True) is None

    replacement = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            card,
            effect_kind="digest",
            targets=(DeliveryTarget.digest(week),),
            react=False,
        )
    )
    assert replacement is not None and replacement.id != first.id
    assert channel.send_calls == 2
    assert repo.get_weekly_digest(week, active_only=True)["message_id"] == str(replacement.id)


def test_digest_retirement_failure_sends_nothing_until_safe_retry(fake_bot, repo, monkeypatch):
    from bot.agent import client as client_module
    from bot.domain.weeks import current_week_start

    now = kl(2026, 9, 24, 0, 5)
    monkeypatch.setattr(client_module, "utcnow", lambda: now)
    week = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, now)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    first = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            formatting.Card("first digest"),
            effect_kind="digest",
            targets=(DeliveryTarget.digest(week),),
            react=False,
        )
    )
    assert first is not None

    def fail_retirement(*_args, **_kwargs):
        raise RuntimeError("injected retirement transaction failure")

    with monkeypatch.context() as patcher:
        patcher.setattr(repo.delivery, "_retire_digest_replacement", fail_retirement)
        assert asyncio.run(fake_bot.post_digest()) is None

    assert first.deleted
    assert channel.send_calls == 1
    assert repo.get_weekly_digest(week, active_only=True) is not None
    claim = repo._conn.execute(
        "SELECT state, dedupe_active FROM delivery_attempts WHERE message_id = ?",
        (str(first.id),),
    ).fetchone()
    assert tuple(claim) == ("bound", 1)

    replacement = asyncio.run(fake_bot.post_digest())
    assert replacement is not None and replacement.id != first.id
    assert channel.send_calls == 2
    assert (
        repo._conn.execute(
            "SELECT state FROM delivery_attempts WHERE message_id = ?", (str(first.id),)
        ).fetchone()[0]
        == "retired"
    )


def test_debug_cleanup_retires_and_releases_its_exact_bound_attempt(fake_bot, repo):
    run_id = _run(repo)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    message = asyncio.run(
        effects.send_card(
            fake_bot,
            channel,
            formatting.Card("debug cleanup target"),
            effect_kind="debug_card",
            targets=(DeliveryTarget.debug_card(run_id, "day_of"),),
            dedupe=DedupePolicy.operation(),
            react=False,
        )
    )
    assert message is not None
    attempt_id = repo._conn.execute(
        "SELECT attempt_id FROM delivery_attempts WHERE message_id = ?", (str(message.id),)
    ).fetchone()[0]

    assert asyncio.run(effects.delete_debug_message(fake_bot, WATCHED_CHANNEL, message.id))

    assert message.deleted
    assert repo.debug_messages_for(message.id) == []
    retired = repo._conn.execute(
        "SELECT state, dedupe_active, resolved_by, resolution_reason "
        "FROM delivery_attempts WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert tuple(retired) == (
        "retired",
        0,
        "service:debug-cleanup",
        "confirmed Discord deletion during debug cleanup",
    )
    target = repo._conn.execute(
        "SELECT release_actor, release_reason FROM delivery_attempt_targets WHERE attempt_id = ?",
        (attempt_id,),
    ).fetchone()
    assert tuple(target) == (
        "service:debug-cleanup",
        "confirmed Discord deletion during debug cleanup",
    )


def test_closed_maintenance_denies_edits_deletes_reactions_and_sends_before_io(fake_bot, repo):
    run_id = _run(repo)
    channel = fake_bot.channels[WATCHED_CHANNEL]
    message = FakeMessage(900000000000000321, channel, fake_bot)
    channel.messages[message.id] = message
    repo.add_debug_message(message.id, run_id, WATCHED_CHANNEL, "day_of")
    assert asyncio.run(repo.maintenance.prepare())

    async def effects_are_denied():
        assert (
            await effects.send_card(
                fake_bot,
                channel,
                formatting.Card("blocked"),
                effect_kind="debug_card",
                targets=(DeliveryTarget.debug_card(run_id, "day_of"),),
                dedupe=DedupePolicy.operation(),
            )
            is None
        )
        assert (
            await effects.send_plain(
                fake_bot,
                channel,
                "blocked chat reply",
                mention_users=[],
                source_message_id="900000000000000322",
                semantic_slot="chat.answer.final",
            )
            is None
        )
        with pytest.raises(MaintenanceClosedError):
            await effects.send_operation_plain(
                fake_bot,
                channel,
                "blocked guide or say",
                mention_users=[],
                effect_kind="say.post",
            )
        assert not await effects.add_reactions(fake_bot, message, ("✅",))
        assert not await effects.add_chat_reaction(fake_bot, message, "👀")
        assert not await effects.remove_chat_reaction(fake_bot, message, "👀")
        assert not await effects.remove_reaction(
            fake_bot, WATCHED_CHANNEL, message.id, "1001", "✅"
        )
        assert not await effects.edit_plain(fake_bot, message, "edited placeholder")
        assert not await effects.delete_placeholder(fake_bot, message)
        assert not await effects.edit_card(
            fake_bot, WATCHED_CHANNEL, message.id, formatting.Card("edited")
        )
        assert not await effects.annotate_message(
            fake_bot, WATCHED_CHANNEL, message.id, "annotation"
        )
        assert not await effects.delete_debug_message(fake_bot, WATCHED_CHANNEL, message.id)
        assert not await effects.retire_deleted_digest(
            fake_bot, kl(2026, 9, 24), WATCHED_CHANNEL, message.id, at=utcnow()
        )

    asyncio.run(effects_are_denied())
    assert channel.send_calls == 0
    assert channel.fetch_calls == 0
    assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0
    assert message.edits == []
    assert message.deleted is False
    assert message.reactions == []


@pytest.mark.parametrize(
    "failure",
    [effects.DeliveryUncertainError("timeout"), MaintenanceClosedError("closed")],
)
def test_card_notice_failure_does_not_abort_the_committed_card_loop(fake_bot, repo, failure):
    run = repo.get_run(_run(repo))
    client = BossBot.__new__(BossBot)
    client.repo = repo
    client.tz = TZ
    attempts: list[str] = []

    async def channel_for(_channel_id):
        return fake_bot.channels[WATCHED_CHANNEL]

    async def failing_post(_channel, _content, _mentions, **kw):
        attempts.append(kw["effect_kind"])
        raise failure

    client.post_channel = channel_for
    client.post_plain = failing_post

    asyncio.run(client._announce_move(run, run["datetime"] - timedelta(hours=1)))
    asyncio.run(
        client._post_courtesy_notice(
            fake_bot.channels[WATCHED_CHANNEL],
            "⚠️ problem",
            [],
            effect_kind="notice.card.apply.problem",
            effect_context=("1", "commit_failed"),
        )
    )

    assert attempts == ["notice.run.move.approved", "notice.card.apply.problem"]
