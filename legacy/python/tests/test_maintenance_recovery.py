"""Closed-state operator bind/retire of runtime attempts on real synthetic stores."""

from __future__ import annotations

import asyncio
import sqlite3
from datetime import UTC, datetime, time, timedelta

import pytest

import bot.infrastructure.maintenance.recovery.runtime_resolution as runtime_resolution
from bot.agent.maintenance.evidence import (
    MAX_HISTORY_MESSAGES,
    EvidenceLookupError,
    list_candidates,
)
from bot.domain.timeutil import from_iso, to_iso, utcnow
from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.coordinator import (
    MaintenanceClosedError,
    MaintenanceStateError,
)
from bot.infrastructure.maintenance.delivery import (
    DedupePolicy,
    DeliveryTarget,
    Destination,
    ObservedMessage,
    SendPayload,
    SendPlan,
)
from bot.infrastructure.maintenance.recovery import (
    EVIDENCE_CLOCK_SKEW,
    RecoveryDispositionError,
    RecoveryError,
    RecoveryEvidenceError,
    RecoveryObservation,
    RetirementClass,
    RuntimeRecovery,
)
from bot.infrastructure.maintenance.state import MaintenanceMode
from tests.discord_rest_fakes import (
    BOT_ID,
    CHANNEL_ID,
    GUILD_ID,
    HUMAN_ID,
    HistoryChannel,
    forbidden,
    http_error,
    not_found,
)
from tests.test_migration import v14_database_with_retained_rows

GUILD = str(GUILD_ID)
CHANNEL = str(CHANNEL_ID)
BOT = str(BOT_ID)
DECLINER = "333333333333333333"
PAST_WEEK = datetime(2026, 9, 17, tzinfo=UTC)
PRIVATE_TEXT = "RECOVERY_PAYLOAD_MUST_NOT_BE_REPORTED"


class TimeoutTransport:
    def __init__(self) -> None:
        self.calls = 0

    async def send(self, plan: SendPlan):
        self.calls += 1
        raise TimeoutError("remote may have accepted the send")


def _open(path, owner_lock_dir) -> Repo:
    return Repo(str(path), owner_lock_dir=owner_lock_dir)


def _run(repo: Repo, run_id: str = "run-1") -> None:
    stamp = to_iso(utcnow())
    repo._conn.execute(
        "INSERT INTO runs "
        "(id, channel_id, week_start, bosses, datetime, participants, status, source, created_at) "
        "VALUES (?, ?, ?, '[]', ?, '[]', 'planned', 'fixed', ?)",
        (run_id, CHANNEL, stamp, stamp, stamp),
    )


def _plan(family: str, repo: Repo, week: datetime = PAST_WEEK) -> SendPlan:
    destination = Destination.channel(GUILD, CHANNEL)
    payload = SendPayload(content=PRIVATE_TEXT)
    if family == "reminder":
        _run(repo)
        due = utcnow() - timedelta(minutes=1)
        ids = [repo.add_reminder("run-1", kind, due) for kind in ("day_of", "countdown_60")]
        targets = tuple(DeliveryTarget.reminder(item) for item in ids)
        return SendPlan("reminder", destination, payload, DedupePolicy.native(), targets)
    if family == "digest":
        targets = (DeliveryTarget.digest(week),)
        return SendPlan("digest", destination, payload, DedupePolicy.native(), targets)
    if family == "decline":
        _run(repo)
        targets = (DeliveryTarget.decline("run-1", DECLINER),)
        return SendPlan("decline.notice", destination, payload, DedupePolicy.native(), targets)
    if family in {"card", "cards"}:
        stamp = to_iso(utcnow())
        ids = ["amendment-1"] if family == "card" else ["amendment-1", "amendment-2"]
        for amendment_id in ids:
            repo._conn.execute(
                "INSERT INTO amendments (id, week_start, kind, status, created_at, channel_id) "
                "VALUES (?, ?, 'add', 'proposed', ?, ?)",
                (amendment_id, stamp, stamp, CHANNEL),
            )
        targets = tuple(DeliveryTarget.card(item) for item in ids)
        return SendPlan("card", destination, payload, DedupePolicy.native(), targets)
    assert family == "operation"
    return SendPlan("say.post", destination, payload, DedupePolicy.operation())


async def _timeout(repo: Repo, plan: SendPlan) -> str:
    transport = TimeoutTransport()
    async with repo.maintenance.operation("test_send"):
        with pytest.raises(TimeoutError):
            await repo.delivery.execute(plan, transport)
    assert transport.calls == 1
    return repo._conn.execute(
        "SELECT attempt_id FROM delivery_attempts ORDER BY rowid DESC LIMIT 1"
    ).fetchone()[0]


def _blocked_store(
    tmp_path, owner_lock_dir, family: str, *, setup=None, after_timeout=None, **kwargs
) -> tuple[Repo, str]:
    """intent -> transport timeout -> indeterminate -> restart -> BLOCKED."""
    path = tmp_path / f"{family}.sqlite"
    repo = _open(path, owner_lock_dir)
    plan = _plan(family, repo, **kwargs)
    if setup is not None:
        setup(repo)
    attempt_id = asyncio.run(_timeout(repo, plan))
    assert _attempt(repo, attempt_id)["state"] == "indeterminate"
    if after_timeout is not None:
        after_timeout(repo)
    repo.close()
    repo = _open(path, owner_lock_dir)
    assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    assert repo.maintenance._has_persistent_blockers()
    return repo, attempt_id


def _attempt(repo: Repo, attempt_id: str) -> sqlite3.Row:
    return repo._conn.execute(
        "SELECT * FROM delivery_attempts WHERE attempt_id = ?", (attempt_id,)
    ).fetchone()


def _claims(repo: Repo, attempt_id: str) -> list[sqlite3.Row]:
    return repo._conn.execute(
        "SELECT * FROM delivery_attempt_targets WHERE attempt_id = ? ORDER BY target_ordinal",
        (attempt_id,),
    ).fetchall()


def _observation(
    repo: Repo,
    attempt_id: str,
    *,
    message_id: str = "880000000000000001",
    author_id: str = BOT,
    guild_id: str = GUILD,
    channel_id: str = CHANNEL,
    offset: timedelta = timedelta(seconds=1),
    components: tuple[str, ...] = (),
) -> RecoveryObservation:
    intended = from_iso(_attempt(repo, attempt_id)["intended_at"])
    return RecoveryObservation(
        Destination.channel(guild_id, channel_id),
        message_id,
        author_id,
        intended + offset,
        ObservedMessage(content=PRIVATE_TEXT),
        components,
    )


def _bind(repo: Repo, attempt_id: str, observation: RecoveryObservation, **kwargs):
    values = {"bot_author_id": BOT, "actor": "operator-1", "reason": "fetched evidence"}
    values.update(kwargs)
    return asyncio.run(
        RuntimeRecovery(repo).bind_runtime_attempt(attempt_id, observation, at=utcnow(), **values)
    )


def _retire(repo: Repo, attempt_id: str, **kwargs):
    values = {"actor": "operator-1", "reason": "no evidence; do not replay"}
    values.update(kwargs)
    return asyncio.run(
        RuntimeRecovery(repo).retire_runtime_attempt(attempt_id, at=utcnow(), **values)
    )


def _assert_fk_and_no_leases(repo: Repo) -> None:
    assert repo._conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
    assert (
        repo._conn.execute(
            "SELECT COUNT(*) FROM delivery_attempt_targets AS t "
            "LEFT JOIN delivery_attempts AS a USING (attempt_id) WHERE a.attempt_id IS NULL"
        ).fetchone()[0]
        == 0
    )

    async def dangling_claim():
        with repo._guard._delivery_intent_scope():
            with pytest.raises(sqlite3.IntegrityError, match="FOREIGN KEY"):
                repo._conn.execute(
                    "INSERT INTO delivery_attempt_targets "
                    "(attempt_id, target_ordinal, binding_type, key_primary) "
                    "VALUES ('missing-attempt', 0, 'reminder', 'dangling')"
                )

    asyncio.run(dangling_claim())
    assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    assert not repo.maintenance.has_live_work()


def test_bind_clears_blocker_and_binds_every_grouped_target_atomically(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    try:
        report = repo.maintenance.blocker_report()
        assert not report.persistent_blockers_clear
        [blocker] = report.attempts
        assert (blocker.attempt_id, blocker.family, blocker.state) == (
            attempt_id,
            "reminder",
            "indeterminate",
        )
        assert (blocker.guild_id, blocker.channel_id, blocker.operation_lease) == (
            GUILD,
            CHANNEL,
            None,
        )
        assert len(blocker.targets) == 2
        assert PRIVATE_TEXT not in repr(report)
        revision = report.state_revision

        observation = _observation(repo, attempt_id)
        _bind(repo, attempt_id, observation)

        attempt = _attempt(repo, attempt_id)
        assert (attempt["state"], attempt["message_id"], attempt["resolved_by"]) == (
            "bound",
            observation.message_id,
            "operator-1",
        )
        assert attempt["observable_fingerprint"]
        reminders = repo._conn.execute("SELECT sent_at, message_id FROM reminders").fetchall()
        assert {row["message_id"] for row in reminders} == {observation.message_id}
        assert {row["sent_at"] for row in reminders} == {to_iso(observation.created_at)}
        assert all(row["released_at"] is None for row in _claims(repo, attempt_id))
        after = repo.maintenance.blocker_report()
        assert after.persistent_blockers_clear and after.attempts == ()
        assert after.mode == "BLOCKED"
        assert after.state_revision == revision + 1
        assert not repo.maintenance._has_persistent_blockers()
        _assert_fk_and_no_leases(repo)
        assert repo.due_reminders(utcnow() + timedelta(days=1)) == []
    finally:
        repo.close()


def test_retired_reminder_is_never_due_and_stays_clear_across_restart(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    path = repo.path
    _retire(repo, attempt_id)
    attempt = _attempt(repo, attempt_id)
    assert (attempt["state"], attempt["dedupe_active"]) == ("retired", 0)
    assert attempt["resolution_reason"] == "operator_no_replay: no evidence; do not replay"
    claims = _claims(repo, attempt_id)
    assert all(row["released_at"] and row["release_actor"] == "operator-1" for row in claims)
    rows = repo._conn.execute("SELECT sent_at, message_id FROM reminders").fetchall()
    assert all(row["sent_at"] is not None and row["message_id"] is None for row in rows)
    assert repo.due_reminders(utcnow() + timedelta(days=30)) == []
    assert repo.maintenance.blocker_report().persistent_blockers_clear
    _assert_fk_and_no_leases(repo)
    repo.close()

    reopened = _open(path, owner_lock_dir)
    try:
        report = reopened.maintenance.blocker_report()
        # Clear does not reopen admission; BLOCKED -> OPEN belongs to the resume controller.
        assert report.persistent_blockers_clear and report.mode == "BLOCKED"
        assert reopened.due_reminders(utcnow() + timedelta(days=30)) == []
    finally:
        reopened.close()


def test_retired_decline_leaves_a_message_less_cooldown_row(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "decline")
    try:
        _retire(repo, attempt_id)
        row = repo.get_decline_notice("run-1", DECLINER)
        assert row is not None and row["message_id"] is None and row["channel_id"] == CHANNEL
        target = DeliveryTarget.decline("run-1", DECLINER)
        assert not repo.delivery._decline_target_reusable(target)
        assert not repo.delivery.has_active_claim(target)
        assert repo.maintenance.blocker_report().persistent_blockers_clear
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_retired_digest_sets_the_week_marker_without_a_digest_row(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "digest")
    try:
        _retire(repo, attempt_id)
        assert repo.get_config("last_digest_week") == to_iso(PAST_WEEK)
        assert repo._conn.execute("SELECT COUNT(*) FROM weekly_digests").fetchone()[0] == 0
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


def test_retired_digest_never_moves_a_newer_marker_back(tmp_path, owner_lock_dir):
    newer = to_iso(PAST_WEEK + timedelta(days=7))
    repo, attempt_id = _blocked_store(
        tmp_path,
        owner_lock_dir,
        "digest",
        setup=lambda repo: repo.set_config("last_digest_week", newer),
    )
    try:
        _retire(repo, attempt_id)
        assert repo.get_config("last_digest_week") == newer
    finally:
        repo.close()


def test_future_week_digest_retire_releases_without_a_future_marker(tmp_path, owner_lock_dir):
    future = datetime.now(UTC).replace(microsecond=0) + timedelta(days=30)
    older = to_iso(PAST_WEEK)
    repo, attempt_id = _blocked_store(
        tmp_path,
        owner_lock_dir,
        "digest",
        week=future,
        setup=lambda repo: repo.set_config("last_digest_week", older),
    )
    try:
        _retire(repo, attempt_id)
        assert _attempt(repo, attempt_id)["state"] == "retired"
        assert all(row["released_at"] for row in _claims(repo, attempt_id))
        # That week's reset tick posts its scheduled digest; nothing marks it done.
        assert repo.get_config("last_digest_week") == older
        assert repo._conn.execute("SELECT COUNT(*) FROM weekly_digests").fetchone()[0] == 0
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


def test_future_week_digest_bind_records_the_row_without_a_future_marker(tmp_path, owner_lock_dir):
    future = datetime.now(UTC).replace(microsecond=0) + timedelta(days=30)
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "digest", week=future)
    try:
        observation = _observation(repo, attempt_id)
        _bind(repo, attempt_id, observation)
        digest = repo.get_weekly_digest(future, active_only=True)
        assert digest["message_id"] == observation.message_id
        assert repo.get_config("last_digest_week") is None
        assert repo.maintenance.blocker_report().persistent_blockers_clear
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_operation_scoped_retire_touches_only_the_attempt(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")
    try:
        assert _claims(repo, attempt_id) == []
        _retire(repo, attempt_id)
        assert _attempt(repo, attempt_id)["state"] == "retired"
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


def test_card_retire_leaves_the_proposal_pending_and_card_bind_binds_it(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "card")
    try:
        _retire(repo, attempt_id)
        assert _attempt(repo, attempt_id)["state"] == "retired"
        assert all(row["released_at"] for row in _claims(repo, attempt_id))
        amendment = repo.get_amendment("amendment-1")
        assert (amendment["status"], amendment["proposal_message_id"]) == ("proposed", None)
        assert repo.delivery.card_retired_unproven("amendment-1")
        assert not repo.delivery.has_active_claim(DeliveryTarget.card("amendment-1"))
        report = repo.maintenance.blocker_report()
        assert report.retired_card_proposals == ("amendment-1",)
        assert report.persistent_blockers_clear
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()

    repo, attempt_id = _blocked_store(tmp_path / "bound", owner_lock_dir, "card")
    try:
        observation = _observation(repo, attempt_id)
        _bind(repo, attempt_id, observation)
        amendment = repo.get_amendment("amendment-1")
        assert (amendment["status"], amendment["proposal_message_id"]) == (
            "proposed",
            observation.message_id,
        )
        report = repo.maintenance.blocker_report()
        assert report.persistent_blockers_clear and report.retired_card_proposals == ()
        assert not repo.delivery.card_retired_unproven("amendment-1")
    finally:
        repo.close()


def test_retired_card_is_never_reposted_after_restart_but_unposted_rows_still_are(
    tmp_path, owner_lock_dir, bosses
):
    from bot.agent import effects, formatting
    from tests.fake_bot import WATCHED_CHANNEL, FakeBot
    from tests.test_rescan_window import effects_pipeline

    path = tmp_path / "cards.sqlite"
    repo = _open(path, owner_lock_dir)
    channel_id = str(WATCHED_CHANNEL)

    def proposal() -> str:
        return repo.create_amendment(
            week_start=PAST_WEEK,
            kind="add",
            bosses=["NMaleficStar"],
            channel_id=channel_id,
            confidence=0.9,
        )

    retired, never_posted = proposal(), proposal()
    bot = FakeBot(repo, bosses)
    channel = bot.channels[WATCHED_CHANNEL]
    bot.digest_fails = True
    sent = asyncio.run(
        effects.send_card(
            bot,
            channel,
            formatting.Card("possibly accepted"),
            effect_kind="card",
            targets=(DeliveryTarget.card(retired),),
        )
    )
    assert sent is None and channel.send_calls == 1
    attempt_id = repo._conn.execute("SELECT attempt_id FROM delivery_attempts").fetchone()[0]
    assert _attempt(repo, attempt_id)["state"] == "indeterminate"
    repo.close()

    repo = _open(path, owner_lock_dir)
    _retire(repo, attempt_id)
    assert repo.maintenance.blocker_report().persistent_blockers_clear
    repo.close()
    # Synthetic stand-in for the checked resume controller (G7), which owns BLOCKED -> OPEN.
    conn = sqlite3.connect(path)
    conn.execute("UPDATE maintenance_state SET mode = 'OPEN', blocker_code = NULL")
    conn.commit()
    conn.close()

    repo = _open(path, owner_lock_dir)
    try:
        assert repo.maintenance.blocker_report().mode == "OPEN"
        bot = FakeBot(repo, bosses)
        channel = bot.channels[WATCHED_CHANNEL]
        asyncio.run(effects_pipeline(bot).apply_plan(channel_id, [], [], PAST_WEEK, ""))

        assert channel.send_calls == 1
        assert repo.get_amendment(never_posted)["proposal_message_id"] is not None
        kept = repo.get_amendment(retired)
        assert (kept["status"], kept["proposal_message_id"]) == ("proposed", None)
        assert repo.maintenance.blocker_report().retired_card_proposals == (retired,)

        # A second pass stays quiet: the durable journal record, not memory, suppresses it.
        asyncio.run(effects_pipeline(bot).apply_plan(channel_id, [], [], PAST_WEEK, ""))
        assert channel.send_calls == 1
    finally:
        repo.close()


def test_open_mode_refuses_resolution(tmp_path, owner_lock_dir):
    repo = _open(tmp_path / "open.sqlite", owner_lock_dir)
    try:
        attempt_id = asyncio.run(_timeout(repo, _plan("operation", repo)))
        with pytest.raises(MaintenanceClosedError, match="BLOCKED or FROZEN"):
            _retire(repo, attempt_id)
        with pytest.raises(MaintenanceClosedError, match="BLOCKED or FROZEN"):
            _bind(repo, attempt_id, _observation(repo, attempt_id))
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_live_application_and_own_attempt_leases_refuse_resolution(tmp_path, owner_lock_dir):
    repo = _open(tmp_path / "live.sqlite", owner_lock_dir)
    plan = _plan("operation", repo)

    async def scenario():
        holding = asyncio.Event()
        release = asyncio.Event()
        attempt: dict[str, str] = {}

        async def owner():
            async with repo.maintenance.operation("slow_send"):
                with pytest.raises(TimeoutError):
                    await repo.delivery.execute(plan, TimeoutTransport())
                attempt["id"] = repo._conn.execute(
                    "SELECT attempt_id FROM delivery_attempts"
                ).fetchone()[0]
                holding.set()
                await release.wait()

        task = asyncio.create_task(owner())
        await holding.wait()
        assert not await repo.maintenance.prepare(timeout=0)
        assert repo.maintenance.blocker_report().mode == "BLOCKED"
        recovery = RuntimeRecovery(repo)
        with pytest.raises(MaintenanceClosedError, match="live work"):
            await recovery.retire_runtime_attempt(
                attempt["id"], actor="operator-1", reason="own lease live", at=utcnow()
            )
        report = repo.maintenance.blocker_report()
        assert report.attempts[0].operation_lease == "live"
        release.set()
        await task
        return attempt["id"]

    try:
        attempt_id = asyncio.run(scenario())
        # Once the application lease ends, the BLOCKED store is resolvable.
        _retire(repo, attempt_id)
        assert repo.maintenance.blocker_report().persistent_blockers_clear
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_resolution_requires_a_task_and_rejects_copied_context(
    tmp_path, owner_lock_dir, monkeypatch
):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    coroutine = RuntimeRecovery(repo).retire_runtime_attempt(
        attempt_id, actor="operator-1", reason="taskless", at=utcnow()
    )
    try:
        with pytest.raises(MaintenanceStateError, match="asyncio task"):
            coroutine.send(None)
    finally:
        coroutine.close()

    authorities = []
    real_register = repo._guard._register_runtime_recovery

    def capture(*args, **kwargs):
        authority = real_register(*args, **kwargs)
        authorities.append(authority)
        return authority

    monkeypatch.setattr(repo._guard, "_register_runtime_recovery", capture)
    real_apply = runtime_resolution.apply_retirement
    children = []

    def spawn_copied_child(conn, *args):
        async def copied_child():
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                conn.execute("UPDATE reminders SET message_id = 'forged'")
            with pytest.raises(RuntimeError, match="not live"):
                with repo._guard._runtime_recovery_retire_scope(authorities[0]):
                    pass

        children.append(asyncio.get_running_loop().create_task(copied_child()))
        with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
            conn.execute("UPDATE runs SET status = 'forged'")
        return real_apply(conn, *args)

    monkeypatch.setattr(runtime_resolution, "apply_retirement", spawn_copied_child)

    async def scenario():
        await RuntimeRecovery(repo).retire_runtime_attempt(
            attempt_id, actor="operator-1", reason="copied context", at=utcnow()
        )
        await asyncio.gather(*children)
        with pytest.raises(RuntimeError, match="not live"):
            with repo._guard._runtime_recovery_bind_scope(authorities[0]):
                pass

    try:
        asyncio.run(scenario())
        assert _attempt(repo, attempt_id)["state"] == "retired"
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM reminders WHERE message_id = 'forged'"
            ).fetchone()[0]
            == 0
        )
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


@pytest.mark.parametrize(
    ("overrides", "match"),
    [
        ({"author_id": "444444444444444444"}, "configured bot"),
        ({"guild_id": "999999999999999999"}, "another guild"),
        ({"channel_id": "999999999999999999"}, "another channel"),
        ({"offset": -EVIDENCE_CLOCK_SKEW - timedelta(seconds=1)}, "predates"),
        ({"components": ("button",)}, "unsupported"),
    ],
)
def test_bind_rejects_unauthoritative_evidence(tmp_path, owner_lock_dir, overrides, match):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    try:
        with pytest.raises(RecoveryEvidenceError, match=match):
            _bind(repo, attempt_id, _observation(repo, attempt_id, **overrides))
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM reminders WHERE message_id IS NOT NULL"
            ).fetchone()[0]
            == 0
        )
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_bind_within_skew_is_accepted(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")
    try:
        offset = -EVIDENCE_CLOCK_SKEW + timedelta(seconds=1)
        _bind(repo, attempt_id, _observation(repo, attempt_id, offset=offset))
        assert _attempt(repo, attempt_id)["state"] == "bound"
    finally:
        repo.close()


def _foreign_reminder_message(repo: Repo) -> None:
    stamp = to_iso(utcnow())
    repo._conn.execute(
        "INSERT INTO reminders (id, run_id, fire_at, kind, sent_at, message_id) "
        "VALUES ('other', 'run-1', ?, 'x', ?, '880000000000000009')",
        (stamp, stamp),
    )


def test_bind_rejects_duplicates_and_messages_bound_elsewhere(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(
        tmp_path, owner_lock_dir, "decline", setup=_foreign_reminder_message
    )
    try:
        with pytest.raises(RecoveryEvidenceError, match="native row"):
            _bind(repo, attempt_id, _observation(repo, attempt_id, message_id="880000000000000009"))
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"

        _bind(repo, attempt_id, _observation(repo, attempt_id))
        with pytest.raises(RecoveryError, match="not an unresolved"):
            _bind(repo, attempt_id, _observation(repo, attempt_id, message_id="880000000000000002"))
        with pytest.raises(RecoveryError, match="not an unresolved"):
            _retire(repo, attempt_id)
        row = repo.get_decline_notice("run-1", DECLINER)
        assert row["message_id"] == "880000000000000001"
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_bind_rejects_a_message_owned_by_another_attempt(tmp_path, owner_lock_dir):
    path = tmp_path / "two.sqlite"
    repo = _open(path, owner_lock_dir)
    first = asyncio.run(_timeout(repo, _plan("operation", repo)))
    second = asyncio.run(_timeout(repo, _plan("operation", repo)))
    repo.close()
    repo = _open(path, owner_lock_dir)
    try:
        _bind(repo, first, _observation(repo, first))
        with pytest.raises(RecoveryEvidenceError, match="another attempt"):
            _bind(repo, second, _observation(repo, second))
        assert _attempt(repo, second)["state"] == "indeterminate"
        assert not repo.maintenance.blocker_report().persistent_blockers_clear
        _retire(repo, second)
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


@pytest.mark.parametrize(
    ("actor", "reason"),
    [("", "reason"), ("   ", "reason"), ("x" * 129, "reason"), ("op", ""), ("op", "r" * 513)],
)
def test_actor_and_reason_are_bounded_before_any_lease(tmp_path, owner_lock_dir, actor, reason):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")
    try:
        with pytest.raises(ValueError):
            _retire(repo, attempt_id, actor=actor, reason=reason)
        with pytest.raises(ValueError):
            _bind(repo, attempt_id, _observation(repo, attempt_id), actor=actor, reason=reason)
        # The class prefix must also fit the stored 512-character bound.
        with pytest.raises(ValueError, match="512"):
            _retire(repo, attempt_id, reason="r" * 505)
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def _v15_store_with_memory_attempts(path) -> None:
    v14_database_with_retained_rows(path, version=15, live_lease=False)
    conn = sqlite3.connect(path)
    for attempt_id, kind, dedupe, destination in (
        ("memory-notice-1", "memory_notice", "1", ("dm", None, None, "700000000000000001")),
        ("memory-proposal-1", "memory_proposal", "2", ("channel", GUILD, CHANNEL, None)),
    ):
        conn.execute(
            "INSERT INTO delivery_attempts "
            "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind, "
            "dedupe_scope, dedupe_key, state, destination_kind, guild_id, channel_id, "
            "recipient_id, fingerprint_version, request_fingerprint, intended_at) "
            "VALUES (?, ?, 0, 'instance-old', 'runtime', ?, 'native', ?, 'indeterminate', "
            "?, ?, ?, ?, 1, ?, '2026-09-22T00:00:00+00:00')",
            (attempt_id, f"op-{attempt_id}", kind, dedupe * 64, *destination, "f" * 64),
        )
        conn.execute(
            "INSERT INTO delivery_attempt_targets "
            "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
            "VALUES (?, 0, ?, 'memory-7', '')",
            (attempt_id, kind),
        )
    conn.commit()
    conn.close()


def test_retained_v15_memory_attempts_are_retire_only_as_feature_removed(tmp_path, owner_lock_dir):
    path = tmp_path / "v15-memory.sqlite"
    _v15_store_with_memory_attempts(path)
    repo = _open(path, owner_lock_dir)
    try:
        report = repo.maintenance.blocker_report()
        assert report.mode == "BLOCKED"
        assert {item.family for item in report.attempts} == {"memory"}
        with pytest.raises(RecoveryError, match="cannot be bound"):
            _bind(
                repo,
                "memory-proposal-1",
                RecoveryObservation(
                    Destination.channel(GUILD, CHANNEL),
                    "880000000000000001",
                    BOT,
                    datetime(2026, 9, 22, 0, 0, 1, tzinfo=UTC),
                    ObservedMessage(content="x"),
                ),
            )
        with pytest.raises(RecoveryError, match="feature_removed"):
            _retire(repo, "memory-notice-1")
        for attempt_id in ("memory-notice-1", "memory-proposal-1"):
            _retire(repo, attempt_id, reason_class=RetirementClass.FEATURE_REMOVED)
            assert _attempt(repo, attempt_id)["resolution_reason"].startswith("feature_removed: ")
            assert all(row["released_at"] for row in _claims(repo, attempt_id))
        assert repo.maintenance.blocker_report().persistent_blockers_clear
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_feature_removed_class_is_rejected_for_non_memory_attempts(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")
    try:
        with pytest.raises(RecoveryError, match="feature_removed"):
            _retire(repo, attempt_id, reason_class="feature_removed")
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
    finally:
        repo.close()


def test_frozen_store_resolution_keeps_frozen_mode(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    try:

        async def freeze():
            # Synthetic: the real freeze controller (G7) never freezes with blockers.
            repo.maintenance._transition(
                MaintenanceMode.FROZEN, expected=MaintenanceMode.BLOCKED, blocker_code=None
            )

        asyncio.run(freeze())
        _retire(repo, attempt_id)
        report = repo.maintenance.blocker_report()
        assert report.mode == "FROZEN" and report.persistent_blockers_clear
        assert repo._guard.mode is MaintenanceMode.FROZEN
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def test_orphan_leases_remain_reported_and_blocking(tmp_path, owner_lock_dir):
    path = tmp_path / "orphan.sqlite"
    v14_database_with_retained_rows(path, version=15, live_lease=True)
    repo = _open(path, owner_lock_dir)
    try:
        report = repo.maintenance.blocker_report()
        assert [lease.operation_id for lease in report.orphan_leases] == ["lease-legacy-1"]
        assert not report.persistent_blockers_clear

        async def retire_orphan():
            repo.maintenance.retire_orphan("lease-legacy-1", "operator-1", "stale admin lease")

        asyncio.run(retire_orphan())
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


def test_direct_closed_state_writes_remain_denied_after_resolution(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "reminder")
    try:
        _retire(repo, attempt_id)
        for sql in (
            "UPDATE reminders SET sent_at = NULL",
            "UPDATE delivery_attempts SET state = 'indeterminate'",
            "UPDATE maintenance_state SET mode = 'OPEN'",
        ):
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(sql)
    finally:
        repo.close()


# -- advisory history listing -------------------------------------------------

INTENT = datetime(2026, 9, 24, 3, 0, tzinfo=UTC)
DESTINATION = Destination.channel(GUILD, CHANNEL)


def _list(channel: HistoryChannel, **kwargs):
    return asyncio.run(
        list_candidates(channel, DESTINATION, intended_at=INTENT, bot_author_id=BOT, **kwargs)
    )


def test_history_listing_is_bounded_and_lists_only_bot_authored_candidates():
    channel = HistoryChannel()
    channel.add(900000000000000001, INTENT - timedelta(minutes=5))
    channel.add(900000000000000002, INTENT + timedelta(seconds=2))
    channel.add(900000000000000003, INTENT + timedelta(seconds=3), author_id=HUMAN_ID)
    channel.add(900000000000000004, INTENT + timedelta(minutes=16))

    listing = _list(channel)

    assert [item.message_id for item in listing.candidates] == ["900000000000000002"]
    assert listing.scanned == 2 and not listing.truncated
    [call] = channel.history_calls
    assert call["limit"] == MAX_HISTORY_MESSAGES and call["oldest_first"] is True
    assert call["after"] == INTENT - EVIDENCE_CLOCK_SKEW
    assert call["before"] - INTENT <= timedelta(minutes=15)
    assert listing.candidates[0].destination == DESTINATION


def test_history_listing_reports_truncation_and_rejects_unbounded_requests():
    channel = HistoryChannel()
    for index in range(5):
        channel.add(900000000000000010 + index, INTENT + timedelta(seconds=index))
    listing = _list(channel, limit=3)
    assert listing.scanned == 3 and listing.truncated
    for kwargs in (
        {"limit": 101},
        {"limit": 0},
        {"window": timedelta(minutes=16)},
        {"window": timedelta(0)},
    ):
        with pytest.raises(ValueError):
            _list(channel, **kwargs)
    channel.overrun = True
    with pytest.raises(EvidenceLookupError, match="more messages"):
        _list(channel, limit=3)


def test_zero_candidates_is_a_listing_not_an_unsent_verdict():
    listing = _list(HistoryChannel())
    assert listing.candidates == () and listing.scanned == 0
    assert not hasattr(listing, "unsent")


@pytest.mark.parametrize("error", [forbidden, not_found, lambda: http_error(503)])
@pytest.mark.parametrize("fail_after", [0, 1])
def test_history_failures_surface_as_failures(error, fail_after):
    channel = HistoryChannel()
    channel.add(900000000000000001, INTENT + timedelta(seconds=1))
    channel.add(900000000000000002, INTENT + timedelta(seconds=2))
    channel.error = error()
    channel.fail_after = fail_after
    with pytest.raises(EvidenceLookupError, match="outcome remains unknown"):
        _list(channel)


def test_listed_candidate_can_be_bound_by_an_operator(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")
    try:
        intended = from_iso(_attempt(repo, attempt_id)["intended_at"])
        channel = HistoryChannel()
        channel.add(900000000000000042, intended + timedelta(seconds=1), content=PRIVATE_TEXT)
        listing = asyncio.run(
            list_candidates(channel, DESTINATION, intended_at=intended, bot_author_id=BOT)
        )
        [candidate] = listing.candidates
        _bind(repo, attempt_id, candidate)
        assert _attempt(repo, attempt_id)["message_id"] == "900000000000000042"
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


# -- replay regressions after resume -----------------------------------------


def _resume_synthetically(path, owner_lock_dir) -> Repo:
    """Stand-in for the checked resume controller (G7), which owns BLOCKED -> OPEN."""
    conn = sqlite3.connect(path)
    conn.execute("UPDATE maintenance_state SET mode = 'OPEN', blocker_code = NULL")
    conn.commit()
    conn.close()
    repo = _open(path, owner_lock_dir)
    assert repo.maintenance.blocker_report().mode == "OPEN"
    return repo


RUN_AT = utcnow().replace(microsecond=0) + timedelta(days=3)


def _future_runs_with_a_plain_skip(repo: Repo) -> None:
    repo._conn.execute("UPDATE runs SET datetime = ? WHERE id = 'run-1'", (to_iso(RUN_AT),))
    _run(repo, "run-2")
    repo._conn.execute("UPDATE runs SET datetime = ? WHERE id = 'run-2'", (to_iso(RUN_AT),))
    # An ordinary skip with no delivery attempt at all.
    repo.add_reminder("run-2", "day_of", utcnow() - timedelta(hours=1), sent_at=utcnow())


def _day_of(repo: Repo, run_id: str) -> sqlite3.Row:
    return repo._conn.execute(
        "SELECT * FROM reminders WHERE run_id = ? AND kind = 'day_of'", (run_id,)
    ).fetchone()


def test_ping_time_reconcile_never_rearms_a_retired_reminder(tmp_path, owner_lock_dir):
    from zoneinfo import ZoneInfo

    from bot.agent.materialise import reconcile_day_of

    tz = ZoneInfo("Asia/Kuala_Lumpur")
    repo, attempt_id = _blocked_store(
        tmp_path, owner_lock_dir, "reminder", setup=_future_runs_with_a_plain_skip
    )
    path = repo.path
    retired_id = _day_of(repo, "run-1")["id"]
    _retire(repo, attempt_id)
    repo.close()

    repo = _resume_synthetically(path, owner_lock_dir)
    try:
        assert repo.delivery.reminder_retired_unproven(retired_id)
        assert not repo.delivery.reminder_retired_unproven(_day_of(repo, "run-2")["id"])
        before = dict(_day_of(repo, "run-1"))
        assert reconcile_day_of(repo, tz, time(9, 0), now=utcnow()) == 1
        assert dict(_day_of(repo, "run-1")) == before
        # The plain skip still re-arms exactly as before.
        assert _day_of(repo, "run-2")["sent_at"] is None
        due = {row["id"] for row in repo.due_reminders(RUN_AT + timedelta(days=1))}
        assert due == {_day_of(repo, "run-2")["id"]}
    finally:
        repo.close()

    repo = _open(path, owner_lock_dir)
    try:
        reconcile_day_of(repo, tz, time(10, 30), now=utcnow())
        assert _day_of(repo, "run-1")["sent_at"] is not None
        due = {row["run_id"] for row in repo.due_reminders(RUN_AT + timedelta(days=1))}
        assert due == {"run-2"}
    finally:
        repo.close()


def test_a_run_move_rebuilds_fresh_reminders_for_the_new_schedule(tmp_path, owner_lock_dir):
    """Documented: a move deletes every reminder, bound or retired, and schedules new ones."""
    from zoneinfo import ZoneInfo

    from bot.agent.materialise import refresh_run_reminders

    repo, attempt_id = _blocked_store(
        tmp_path, owner_lock_dir, "reminder", setup=_future_runs_with_a_plain_skip
    )
    path = repo.path
    old_ids = {row["key_primary"] for row in _claims(repo, attempt_id)}
    _retire(repo, attempt_id)
    repo.close()

    repo = _resume_synthetically(path, owner_lock_dir)
    try:
        moved = RUN_AT + timedelta(days=1)
        repo._conn.execute("UPDATE runs SET datetime = ? WHERE id = 'run-1'", (to_iso(moved),))
        refresh_run_reminders(repo, "run-1", ZoneInfo("Asia/Kuala_Lumpur"), time(9, 0), [60])
        rows = repo.list_reminders("run-1")
        assert {row["id"] for row in rows}.isdisjoint(old_ids)
        assert all(row["sent_at"] is None for row in rows)
        assert not any(repo.delivery.reminder_retired_unproven(row["id"]) for row in rows)
    finally:
        repo.close()


def test_bound_current_week_digest_is_not_replaced_by_the_weekly_tick(
    tmp_path, owner_lock_dir, bosses, monkeypatch
):
    from bot.agent import client as client_module
    from bot.agent import effects, formatting
    from bot.agent.client import CFG_LAST_DIGEST, BossBot
    from bot.domain.weeks import current_week_start
    from tests.conftest import RESET_TIME, RESET_WEEKDAY, TZ
    from tests.fake_bot import WATCHED_CHANNEL, FakeBot, FakeMessage

    now = utcnow()
    week = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, now)
    path = tmp_path / "digest-tick.sqlite"
    repo = _open(path, owner_lock_dir)
    repo.set_config(CFG_LAST_DIGEST, to_iso(week - timedelta(days=7)))
    bot = FakeBot(repo, bosses)
    channel = bot.channels[WATCHED_CHANNEL]
    bot.digest_fails = True
    sent = asyncio.run(
        effects.send_card(
            bot,
            channel,
            formatting.Card("weekly digest"),
            effect_kind="digest",
            targets=(DeliveryTarget.digest(week),),
            react=False,
        )
    )
    assert sent is None
    attempt_id = repo._conn.execute("SELECT attempt_id FROM delivery_attempts").fetchone()[0]
    repo.close()

    repo = _open(path, owner_lock_dir)
    message_id = "900000000000000777"
    _bind(repo, attempt_id, _observation(repo, attempt_id, message_id=message_id))
    assert repo.get_config(CFG_LAST_DIGEST) == to_iso(week)
    repo.close()

    repo = _resume_synthetically(path, owner_lock_dir)
    try:
        bot = FakeBot(repo, bosses)
        channel = bot.channels[WATCHED_CHANNEL]
        message = FakeMessage(int(message_id), channel, bot)
        channel.messages[message.id] = message
        monkeypatch.setattr(client_module, "utcnow", lambda: now)

        assert asyncio.run(BossBot.post_week_digest(bot, now)) is None

        assert channel.send_calls == 0 and channel.fetch_calls == 0
        assert not message.deleted
        assert repo.get_weekly_digest(week, active_only=True)["message_id"] == message_id
    finally:
        repo.close()


def test_grouped_card_retirement_skips_every_amendment_on_repost(tmp_path, owner_lock_dir, bosses):
    from tests.fake_bot import WATCHED_CHANNEL, FakeBot
    from tests.test_rescan_window import effects_pipeline

    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "cards")
    path = repo.path
    assert len(_claims(repo, attempt_id)) == 2
    _retire(repo, attempt_id)
    assert repo.maintenance.blocker_report().retired_card_proposals == (
        "amendment-1",
        "amendment-2",
    )
    repo.close()

    repo = _resume_synthetically(path, owner_lock_dir)
    try:
        bot = FakeBot(repo, bosses)
        asyncio.run(effects_pipeline(bot)._repost_stranded(CHANNEL, None))
        assert bot.channels[WATCHED_CHANNEL].send_calls == 0
        for amendment_id in ("amendment-1", "amendment-2"):
            row = repo.get_amendment(amendment_id)
            assert (row["status"], row["proposal_message_id"]) == ("proposed", None)
    finally:
        repo.close()


# -- additional refusals -------------------------------------------------------


def test_a_persisted_live_lease_row_refuses_recovery(tmp_path, owner_lock_dir):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")

    async def foreign_lease(insert: bool):
        # A live row this coordinator does not hold, as another instance would leave.
        if insert:
            with repo._guard._runtime_recovery_lease_insert_scope():
                repo._conn.execute(
                    "INSERT INTO maintenance_leases (operation_id, instance_id, "
                    "owner_token_hash, generation, operation_kind, started_at, owner_task_id, "
                    "lifecycle) VALUES ('foreign', 'other-instance', ?, 0, 'foreign', ?, 1, "
                    "'live')",
                    ("c" * 64, to_iso(utcnow())),
                )
        else:
            with repo._guard._runtime_recovery_lease_delete_scope():
                repo._conn.execute("DELETE FROM maintenance_leases WHERE operation_id = 'foreign'")

    try:
        asyncio.run(foreign_lease(True))
        assert not repo.maintenance.has_live_work()
        with pytest.raises(MaintenanceClosedError, match="another lease is live"):
            _retire(repo, attempt_id)
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM maintenance_leases WHERE operation_kind = 'runtime_recovery'"
            ).fetchone()[0]
            == 0
        )
        asyncio.run(foreign_lease(False))
        _retire(repo, attempt_id)
        assert repo.maintenance.blocker_report().persistent_blockers_clear
    finally:
        repo.close()


@pytest.mark.parametrize("mode", [MaintenanceMode.PREPARING, MaintenanceMode.RESUMING])
def test_preparing_and_resuming_refuse_recovery(tmp_path, owner_lock_dir, mode):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, "operation")

    async def move():
        repo.maintenance._transition(mode, expected=MaintenanceMode.BLOCKED, blocker_code=None)

    try:
        asyncio.run(move())
        with pytest.raises(MaintenanceClosedError, match="BLOCKED or FROZEN"):
            _retire(repo, attempt_id)
        with pytest.raises(MaintenanceClosedError, match="BLOCKED or FROZEN"):
            _bind(repo, attempt_id, _observation(repo, attempt_id))
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()


def _bind_reminders_elsewhere(repo: Repo) -> None:
    repo._conn.execute("UPDATE reminders SET message_id = '880000000000000055'")


def _bind_card_elsewhere(repo: Repo) -> None:
    repo._conn.execute("UPDATE amendments SET proposal_message_id = '880000000000000055'")


@pytest.mark.parametrize(
    ("family", "after_timeout"),
    [("reminder", _bind_reminders_elsewhere), ("card", _bind_card_elsewhere)],
)
def test_retire_refuses_a_target_that_already_has_a_message(
    tmp_path, owner_lock_dir, family, after_timeout
):
    repo, attempt_id = _blocked_store(tmp_path, owner_lock_dir, family, after_timeout=after_timeout)
    try:
        with pytest.raises(RecoveryDispositionError, match="already bound"):
            _retire(repo, attempt_id)
        assert _attempt(repo, attempt_id)["state"] == "indeterminate"
        assert all(row["released_at"] is None for row in _claims(repo, attempt_id))
        _assert_fk_and_no_leases(repo)
    finally:
        repo.close()
