"""Offline source seeding on synthetic v13/v14 repositories."""

from __future__ import annotations

import asyncio
import hashlib
import sqlite3
from contextlib import contextmanager
from datetime import UTC, datetime, time, timedelta
from zoneinfo import ZoneInfo

import pytest

import bot.infrastructure.maintenance.adoption.classification as adoption_classification
import bot.infrastructure.maintenance.adoption.resolution as adoption_resolution
from bot.domain.timeutil import to_iso
from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.adoption import AdoptionConflictError
from bot.infrastructure.maintenance.adoption.evidence import (
    CompleteAttachmentEvidence,
    MessageObservation,
)
from bot.infrastructure.maintenance.adoption.fingerprints import source_evidence_hash
from bot.infrastructure.maintenance.adoption.model import SourceFamily, SourceKey
from bot.infrastructure.maintenance.adoption.resolution import (
    AdoptionEvidenceError,
    AdoptionResolutionError,
)
from bot.infrastructure.maintenance.coordinator import (
    MaintenanceClosedError,
    MaintenanceStateError,
)
from bot.infrastructure.maintenance.delivery import (
    DedupePolicy,
    Destination,
    SendPayload,
    SendPlan,
)
from tests.test_migration import v13_database_with_retained_rows, v14_database_with_retained_rows

NOW = datetime(2026, 9, 23, 12, tzinfo=UTC)
TZ = ZoneInfo("Asia/Kuala_Lumpur")
RESET_WEEKDAY = 3
RESET_TIME = time(0, 0)
PRIVATE_TEXT = "ADOPTION_PLAINTEXT_MUST_NOT_BE_STORED"


def _seed_sources(
    path,
    *,
    marker: str | None = "2026-09-17T00:00:00+00:00",
    mixed_group: bool = False,
    multi_target_family: str | None = None,
) -> None:
    conn = sqlite3.connect(path)
    stamp = to_iso(NOW)
    old_stamp = to_iso(NOW - timedelta(hours=1))
    conn.execute(
        "INSERT INTO runs "
        "(id, channel_id, week_start, bosses, datetime, participants, status, source, created_at) "
        "VALUES ('run-main', 'channel-1', ?, '[]', ?, '[]', 'planned', 'fixed', ?)",
        (stamp, stamp, stamp),
    )
    conn.execute(
        "INSERT INTO runs "
        "(id, channel_id, week_start, bosses, datetime, participants, status, source, created_at) "
        "VALUES ('run-canonical', '200', ?, '[]', ?, '[]', 'planned', 'fixed', ?)",
        (stamp, stamp, stamp),
    )
    reminders = (
        ("future", "future", to_iso(NOW + timedelta(minutes=1)), None, None),
        ("due", "due", stamp, None, None),
        ("partial", "partial", to_iso(NOW + timedelta(hours=1)), old_stamp, None),
        ("message-only", "message-only", stamp, None, "partial-reminder-message"),
        ("reminder-a", "reminder-a", stamp, old_stamp, "shared-reminder-message"),
        ("reminder-b", "reminder-b", stamp, old_stamp, "shared-reminder-message"),
        ("malformed historic id", "malformed", stamp, old_stamp, "malformed-message-id"),
    )
    conn.executemany(
        "INSERT INTO reminders (id, run_id, fire_at, kind, sent_at, message_id) "
        "VALUES (?, 'run-main', ?, ?, ?, ?)",
        (
            (source_id, fire_at, kind, sent_at, message_id)
            for source_id, kind, fire_at, sent_at, message_id in reminders
        ),
    )
    conn.executemany(
        "INSERT INTO reminders (id, run_id, fire_at, kind, sent_at, message_id) "
        "VALUES (?, 'run-canonical', ?, ?, ?, ?)",
        (
            ("adopt-rem-a", stamp, "day_of", old_stamp, "300"),
            ("adopt-rem-b", stamp, "countdown_60", old_stamp, "300"),
        ),
    )
    amendments = (
        ("card-a", "proposed", "channel-1", "shared-card-message", PRIVATE_TEXT),
        ("card-b", "proposed", "channel-1", "shared-card-message", "unused private payload"),
        ("card-proposed-null", "proposed", "channel-1", None, "unused evidence"),
        ("card-terminal-null", "rejected", None, None, "unused payload"),
        ("card-partial", "proposed", None, "partial-card-message", "unused payload"),
    )
    conn.executemany(
        "INSERT INTO amendments "
        "(id, week_start, kind, status, created_at, channel_id, proposal_message_id, summary) "
        "VALUES (?, ?, 'add', ?, ?, ?, ?, ?)",
        (
            (source_id, stamp, status, stamp, channel_id, message_id, summary)
            for source_id, status, channel_id, message_id, summary in amendments
        ),
    )
    canonical_cards = [
        ("adopt-card-a", "proposed", "400"),
        ("adopt-card-b", "proposed", "400"),
    ]
    if mixed_group:
        canonical_cards.append(("adopt-card-mixed", "proposed", "300"))
    conn.executemany(
        "INSERT INTO amendments "
        "(id, week_start, kind, status, created_at, channel_id, proposal_message_id, summary) "
        "VALUES (?, ?, 'add', ?, ?, '200', ?, ?)",
        (
            (source_id, stamp, status, stamp, message_id, "synthetic summary")
            for source_id, status, message_id in canonical_cards
        ),
    )
    conn.executemany(
        "INSERT INTO weekly_digests (week_start, channel_id, message_id, posted_at, retired_at) "
        "VALUES (?, 'channel-1', ?, ?, ?)",
        (
            ("2026-09-17T00:00:00+00:00", "retired-digest-message", old_stamp, stamp),
            ("2026-09-24T00:00:00+00:00", "active-digest-message", stamp, None),
        ),
    )
    single_digest_message = (
        "500" if multi_target_family == SourceFamily.WEEKLY_DIGESTS.value else "600"
    )
    conn.execute(
        "INSERT INTO weekly_digests "
        "(week_start, channel_id, message_id, posted_at, retired_at) "
        "VALUES ('2026-10-01T00:00:00+00:00', '200', ?, ?, NULL)",
        (single_digest_message, stamp),
    )
    if multi_target_family == SourceFamily.WEEKLY_DIGESTS.value:
        conn.execute(
            "INSERT INTO weekly_digests "
            "(week_start, channel_id, message_id, posted_at, retired_at) "
            "VALUES ('2026-10-08T00:00:00+00:00', '200', '500', ?, NULL)",
            (stamp,),
        )
    if marker is None:
        conn.execute(
            "INSERT INTO weekly_digests "
            "(week_start, channel_id, message_id, posted_at, retired_at) "
            "VALUES ('marker:none', 'channel-1', 'malformed-digest-message', ?, NULL)",
            (stamp,),
        )
    else:
        conn.execute("INSERT INTO config (key, value) VALUES ('last_digest_week', ?)", (marker,))
    decline_rows = [
        ("run-main", "user-partial", "channel-1", None, old_stamp),
        ("run-main", "user-message-only", None, "partial-decline-message", old_stamp),
        ("run-main", "user-complete", "channel-1", "complete-decline-message", old_stamp),
        ("run-canonical", "single-user", "200", "700", old_stamp),
    ]
    if multi_target_family == SourceFamily.DECLINE_NOTICES.value:
        decline_rows.extend(
            (
                ("run-canonical", "multi-user-a", "200", "500", old_stamp),
                ("run-canonical", "multi-user-b", "200", "500", old_stamp),
            )
        )
    conn.executemany(
        "INSERT INTO decline_notices (run_id, user_id, channel_id, message_id, notified_at) "
        "VALUES (?, ?, ?, ?, ?)",
        decline_rows,
    )
    conn.executemany(
        "INSERT INTO debug_messages (message_id, run_id, channel_id, kind, created_at) "
        "VALUES (?, 'run-main', ?, 'test', ?)",
        (
            ("debug-bound-message", "channel-1", old_stamp),
            ("debug-no-channel-message", None, old_stamp),
        ),
    )
    conn.commit()
    conn.close()


def _pending_repo(
    path,
    version: int,
    owner_lock_dir,
    *,
    mixed_group: bool = False,
    conflicting_claim: bool = False,
    multi_target_family: str | None = None,
) -> Repo:
    if version == 13:
        v13_database_with_retained_rows(path)
    else:
        v14_database_with_retained_rows(path)
    _seed_sources(
        path,
        mixed_group=mixed_group,
        multi_target_family=multi_target_family,
    )
    if conflicting_claim:
        if version != 14:
            raise ValueError("the synthetic conflicting journal claim requires a v14 store")
        with sqlite3.connect(path) as conn:
            conn.execute(
                "INSERT INTO delivery_attempts "
                "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, "
                "effect_kind, dedupe_scope, dedupe_key, state, destination_kind, guild_id, "
                "channel_id, message_id, fingerprint_version, request_fingerprint, intended_at) "
                "VALUES ('runtime-duplicate', 'runtime-duplicate-op', 0, 'instance-old', "
                "'runtime', 'reminder', 'native', ?, 'bound', 'channel', '100', '200', '999', "
                "1, ?, ?)",
                ("e" * 64, "f" * 64, to_iso(NOW)),
            )
            conn.execute(
                "INSERT INTO delivery_attempt_targets "
                "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
                "VALUES ('runtime-duplicate', 0, 'reminder', 'adopt-rem-a', '')"
            )
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    assert tuple(
        repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
    ) == ("BLOCKED", "pending")
    return repo


def _seed(repo: Repo, *, now: datetime = NOW):
    return asyncio.run(repo.maintenance.seed_adoption_sources(TZ, RESET_WEEKDAY, RESET_TIME, now))


def _observation(
    message_id: str = "300",
    *,
    channel_id: str = "200",
    guild_id: str = "100",
    author_id: str = "900",
    unsupported: tuple[str, ...] = (),
    attachments: tuple[CompleteAttachmentEvidence, ...] = (),
) -> MessageObservation:
    return MessageObservation(
        guild_id=guild_id,
        channel_id=channel_id,
        message_id=message_id,
        author_id=author_id,
        content=PRIVATE_TEXT,
        embeds=(),
        user_mentions=(),
        role_mentions=(),
        everyone_mentioned=False,
        replied_user=False,
        reference=None,
        attachments=attachments,
        unsupported_components=unsupported,
    )


def test_fresh_open_repo_cannot_use_adoption_seed_authority():
    repo = Repo(":memory:")
    try:
        with pytest.raises(MaintenanceClosedError, match="BLOCKED pending adoption"):
            _seed(repo)
        assert tuple(
            repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("OPEN", "complete")
        assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        repo.close()


@pytest.mark.parametrize("version", [13, 14])
def test_pending_v13_v14_classification_groups_only_recorded_messages(
    tmp_path, owner_lock_dir, version
):
    repo = _pending_repo(tmp_path / f"v{version}.sqlite", version, owner_lock_dir)
    before_attempts = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]
    try:
        report = _seed(repo)
        rows = {
            (row["source_family"], row["source_primary"], row["source_secondary"]): row
            for row in repo._conn.execute(
                "SELECT * FROM adoption_sources WHERE adoption_id = ?",
                (report.adoption_id,),
            )
        }
        assert rows[("reminders", "future", "")]["reason"] == "future_unsent"
        assert rows[("reminders", "future", "")]["resolution_state"] == "proven_unsent"
        assert rows[("reminders", "future", "")]["resolution_actor"] == "offline-classifier"
        assert rows[("reminders", "due", "")]["reason"] == "due_unbound"
        assert rows[("reminders", "due", "")]["resolution_state"] == "pending"
        assert rows[("reminders", "partial", "")]["reason"] == "partial_binding"
        assert rows[("reminders", "message-only", "")]["reason"] == "partial_binding"
        assert rows[("reminders", "malformed historic id", "")]["reason"] == ("candidate_message")
        assert rows[("amendments", "card-proposed-null", "")]["reason"] == "proposed_unbound"
        assert rows[("amendments", "card-terminal-null", "")]["resolution_state"] == (
            "terminal_no_replay"
        )
        assert rows[("amendments", "card-terminal-null", "")]["resolution_actor"] == (
            "offline-classifier"
        )
        assert rows[("amendments", "card-partial", "")]["reason"] == "partial_binding"
        assert rows[("weekly_digests", "2026-09-17T00:00:00+00:00", "")]["reason"] == (
            "digest_retired_mismatch"
        )
        assert (
            rows[("weekly_digests", "marker_gap:2026-09-17T00:00:00+00:00", "marker")]["reason"]
            == "digest_marker_gap"
        )
        assert rows[("decline_notices", "run-main", "user-partial")]["reason"] == (
            "partial_binding"
        )
        assert rows[("debug_messages", "debug-no-channel-message", "")]["reason"] == (
            "debug_missing_channel"
        )

        reminder_group = next(
            group
            for group in report.candidate_groups
            if group.family == "reminders" and group.message_id == "shared-reminder-message"
        )
        assert {(key.primary, key.secondary) for key in reminder_group.targets} == {
            ("reminder-a", ""),
            ("reminder-b", ""),
        }
        amendment_group = next(
            group
            for group in report.candidate_groups
            if group.family == "amendments" and group.message_id == "shared-card-message"
        )
        assert {key.primary for key in amendment_group.targets} == {"card-a", "card-b"}
        assert (
            len(
                [
                    group
                    for group in report.candidate_groups
                    if group.message_id == "shared-card-message"
                ]
            )
            == 1
        )

        if version == 14:
            assert rows[("reminders", "reminder-legacy-1", "")]["reason"] == (
                "journal_claim_conflict"
            )
        assert {
            row[0]
            for row in repo._conn.execute("SELECT DISTINCT source_family FROM adoption_sources")
        } <= {
            "reminders",
            "amendments",
            "weekly_digests",
            "decline_notices",
            "debug_messages",
        }
        assert all(
            row["evidence_hash_version"] == 1
            and len(row["evidence_hash"]) == 64
            and int(row["evidence_hash"], 16) >= 0
            for row in rows.values()
        )
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            before_attempts
        )
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM maintenance_leases "
                "WHERE operation_kind = 'adoption_seed' AND lifecycle = 'live'"
            ).fetchone()[0]
            == 0
        )
        if version == 14:
            assert (
                repo._conn.execute(
                    "SELECT lifecycle FROM maintenance_leases WHERE operation_id = 'lease-legacy-1'"
                ).fetchone()[0]
                == "orphaned"
            )
        assert tuple(
            repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        stored = " ".join(
            str(value)
            for row in repo._conn.execute("SELECT * FROM adoption_sources")
            for value in row
        )
        assert PRIVATE_TEXT not in stored
        assert "unused private payload" not in stored
    finally:
        repo.close()


def test_missing_digest_marker_is_a_stable_source_blocker(tmp_path, owner_lock_dir):
    path = tmp_path / "marker-missing.sqlite"
    v13_database_with_retained_rows(path)
    _seed_sources(path, marker=None)
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        report = _seed(repo)
        row = repo._conn.execute(
            "SELECT reason, resolution_state, legacy_message_id FROM adoption_sources "
            "WHERE adoption_id = ? AND source_family = 'weekly_digests' "
            "AND source_primary = 'marker:none' AND source_secondary = 'marker'",
            (report.adoption_id,),
        ).fetchone()
        assert tuple(row) == ("digest_marker_gap", "pending", None)
        malformed = repo._conn.execute(
            "SELECT reason, resolution_state, legacy_message_id FROM adoption_sources "
            "WHERE adoption_id = ? AND source_family = 'weekly_digests' "
            "AND source_primary = 'marker:none' AND source_secondary = ''",
            (report.adoption_id,),
        ).fetchone()
        assert tuple(malformed) == (
            "candidate_message",
            "pending",
            "malformed-digest-message",
        )
    finally:
        repo.close()


def test_adoption_evidence_fingerprint_is_canonical_and_domain_separated():
    payload = {"b": 2, "a": 1}
    assert source_evidence_hash(payload) == source_evidence_hash({"a": 1, "b": 2})
    assert source_evidence_hash(payload) != hashlib.sha256(b'{"a":1,"b":2}').hexdigest()


def test_seed_is_idempotent_across_reopen_and_uses_one_pinned_instant(tmp_path, owner_lock_dir):
    path = tmp_path / "repeat.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    try:
        first = _seed(repo)
        stored_first = [
            tuple(row)
            for row in repo._conn.execute(
                "SELECT source_family, source_primary, source_secondary, reason, resolution_state, "
                "captured_at, classified_at, evidence_hash FROM adoption_sources "
                "ORDER BY source_family, source_primary, source_secondary"
            )
        ]
        second = _seed(repo)
        assert second.inserted_sources == 0
        assert second.unchanged_sources == first.classified_sources
        assert second.pinned_at == first.pinned_at == to_iso(NOW)
        later = _seed(repo, now=NOW + timedelta(minutes=2))
        assert later.inserted_sources == 0
        assert later.unchanged_sources == first.classified_sources
        assert tuple(
            repo._conn.execute(
                "SELECT reason, resolution_state FROM adoption_sources "
                "WHERE source_family = 'reminders' AND source_primary = 'future'"
            ).fetchone()
        ) == ("future_unsent", "proven_unsent")
        assert [
            tuple(row)
            for row in repo._conn.execute(
                "SELECT source_family, source_primary, source_secondary, reason, resolution_state, "
                "captured_at, classified_at, evidence_hash FROM adoption_sources "
                "ORDER BY source_family, source_primary, source_secondary"
            )
        ] == stored_first
    finally:
        repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert tuple(
            reopened._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        report = _seed(reopened)
        assert report.inserted_sources == 0
        assert report.unchanged_sources == report.classified_sources
        assert reopened._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        reopened.close()


def test_abandoned_seed_lease_is_orphaned_on_restart_and_cannot_open_admission(
    tmp_path, owner_lock_dir
):
    path = tmp_path / "abandoned-seed.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    repo.close()
    with sqlite3.connect(path) as seed:
        seed.execute(
            "INSERT INTO maintenance_leases "
            "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
            "started_at, owner_task_id, lifecycle, claim_state) "
            "VALUES ('seed-before-crash', 'old-instance', ?, 0, 'adoption_seed', ?, 1, "
            "'live', 'active')",
            ("d" * 64, to_iso(NOW)),
        )

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert tuple(
            reopened._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        orphan = reopened._conn.execute(
            "SELECT lifecycle, orphaned_at FROM maintenance_leases "
            "WHERE operation_id = 'seed-before-crash'"
        ).fetchone()
        assert orphan[0] == "orphaned"
        assert orphan[1] is not None
        assert not asyncio.run(reopened.maintenance.prepare(timeout=0))

        async def ordinary():
            async with reopened.maintenance.operation("ordinary"):
                pass

        with pytest.raises(MaintenanceClosedError):
            asyncio.run(ordinary())
        report = _seed(reopened)
        assert report.inserted_sources > 0
        assert (
            reopened._conn.execute(
                "SELECT lifecycle FROM maintenance_leases WHERE operation_id = 'seed-before-crash'"
            ).fetchone()[0]
            == "orphaned"
        )
        assert (
            reopened._conn.execute(
                "SELECT COUNT(*) FROM maintenance_leases "
                "WHERE operation_kind = 'adoption_seed' AND lifecycle = 'live'"
            ).fetchone()[0]
            == 0
        )
        assert tuple(
            reopened._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
    finally:
        reopened.close()


def test_changed_source_facts_conflict_without_replacing_seeded_decision(tmp_path, owner_lock_dir):
    path = tmp_path / "changed-source.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    adoption_id = None
    try:
        report = _seed(repo)
        adoption_id = report.adoption_id
        before = tuple(
            repo._conn.execute(
                "SELECT reason, resolution_state, resolution_actor, resolution_at, "
                "resolution_reason, evidence_hash FROM adoption_sources "
                "WHERE adoption_id = ? AND source_family = 'reminders' "
                "AND source_primary = 'future'",
                (adoption_id,),
            ).fetchone()
        )
    finally:
        repo.close()

    with sqlite3.connect(path) as seed:
        seed.execute(
            "UPDATE reminders SET fire_at = ? WHERE id = 'future'",
            (to_iso(NOW + timedelta(minutes=2)),),
        )
    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        with pytest.raises(AdoptionConflictError, match="snapshot changed"):
            _seed(reopened)
        after = tuple(
            reopened._conn.execute(
                "SELECT reason, resolution_state, resolution_actor, resolution_at, "
                "resolution_reason, evidence_hash FROM adoption_sources "
                "WHERE adoption_id = ? AND source_family = 'reminders' "
                "AND source_primary = 'future'",
                (adoption_id,),
            ).fetchone()
        )
        assert after == before
        assert reopened._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        reopened.close()


def test_failed_or_cancelled_seed_rolls_back_all_source_rows_and_lease(
    tmp_path, owner_lock_dir, monkeypatch
):
    repo = _pending_repo(tmp_path / "cancelled-seed.sqlite", 13, owner_lock_dir)
    real_persist = adoption_classification.persist_source_snapshot

    def cancel_after_inserts(*args, **kwargs):
        real_persist(*args, **kwargs)
        raise asyncio.CancelledError

    monkeypatch.setattr(adoption_classification, "persist_source_snapshot", cancel_after_inserts)
    try:
        with pytest.raises(asyncio.CancelledError):
            _seed(repo)
        assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
        assert tuple(
            repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
    finally:
        repo.close()


def test_seed_requires_task_and_live_persisted_lease_and_rejects_copied_context(
    tmp_path, owner_lock_dir, monkeypatch
):
    repo = _pending_repo(tmp_path / "lease-seed.sqlite", 13, owner_lock_dir)
    coroutine = repo.maintenance.seed_adoption_sources(TZ, RESET_WEEKDAY, RESET_TIME, NOW)
    try:
        with pytest.raises(MaintenanceStateError, match="asyncio task"):
            coroutine.send(None)
    finally:
        coroutine.close()

    created_children: list[asyncio.Task[None]] = []
    real_classify = adoption_classification.classify_source_snapshot

    def inspect_lease(*args, **kwargs):
        task = asyncio.current_task()
        row = repo._conn.execute(
            "SELECT instance_id, owner_token_hash, owner_task_id, operation_kind, lifecycle "
            "FROM maintenance_leases"
        ).fetchone()
        assert row is not None
        assert row[0] == repo.maintenance.instance_id
        assert row[1] != ""
        assert row[2] == id(task)
        assert tuple(row[3:]) == ("adoption_seed", "live")

        async def copied_child():
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(
                    "INSERT INTO adoption_sources "
                    "(adoption_id, source_family, source_primary, source_secondary, reason, "
                    "resolution_state, captured_at, classified_at, evidence_hash_version, "
                    "evidence_hash) VALUES ('x', 'reminders', 'child', '', 'due_unbound', "
                    "'pending', ?, ?, 1, ?)",
                    (to_iso(NOW), to_iso(NOW), "a" * 64),
                )

        created_children.append(asyncio.create_task(copied_child()))
        with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
            repo._conn.execute("DELETE FROM adoption_sources")
        with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
            repo._conn.execute("UPDATE adoption_sources SET reason = 'due_unbound'")
        with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
            repo._conn.execute("DELETE FROM delivery_attempts")
        return real_classify(*args, **kwargs)

    monkeypatch.setattr(adoption_classification, "classify_source_snapshot", inspect_lease)

    async def scenario():
        with pytest.raises(MaintenanceClosedError):
            async with repo.maintenance.operation("ordinary"):
                pass
        with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
            repo.set_config("ordinary-write", "denied")

        class NeverTransport:
            async def send(self, _plan):
                pytest.fail("BLOCKED/pending adoption must not reach a transport")

        plan = SendPlan(
            "blocked.test",
            Destination.channel("1", "2"),
            SendPayload(content="synthetic blocked send"),
            DedupePolicy.operation(),
        )
        with pytest.raises(MaintenanceClosedError):
            await repo.delivery.execute(plan, NeverTransport())
        await repo.maintenance.seed_adoption_sources(TZ, RESET_WEEKDAY, RESET_TIME, NOW)
        await asyncio.gather(*created_children)

    try:
        asyncio.run(scenario())
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] > 0
        assert repo.get_config("ordinary-write") is None
    finally:
        repo.close()


@pytest.mark.parametrize(
    "timezone, weekday, reset, instant",
    [
        ("Asia/Kuala_Lumpur", RESET_WEEKDAY, RESET_TIME, NOW),
        (TZ, True, RESET_TIME, NOW),
        (TZ, RESET_WEEKDAY, time(0, 0, tzinfo=UTC), NOW),
        (TZ, RESET_WEEKDAY, RESET_TIME, datetime(2026, 9, 23, 12)),
    ],
)
def test_invalid_timezone_reset_or_unaware_instant_is_rejected_before_lease(
    tmp_path, owner_lock_dir, timezone, weekday, reset, instant
):
    repo = _pending_repo(tmp_path / "bad-input.sqlite", 13, owner_lock_dir)
    try:
        with pytest.raises(ValueError):
            asyncio.run(repo.maintenance.seed_adoption_sources(timezone, weekday, reset, instant))
        assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        repo.close()


@pytest.mark.parametrize("version", [13, 14])
def test_synthetic_whole_message_binding_is_grouped_idempotent_and_not_authorship_proof(
    tmp_path, owner_lock_dir, version
):
    repo = _pending_repo(tmp_path / f"adoption-bind-v{version}.sqlite", version, owner_lock_dir)
    report = _seed(repo)
    before = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]

    async def bind(family, message_id, at):
        return await repo.maintenance.bind_adoption_group(
            family,
            "200",
            message_id,
            (_observation(message_id),),
            guild_id="100",
            bot_author_id="900",
            actor="operator-7",
            reason="synthetic offline reconciliation fixture",
            at=at,
        )

    try:

        async def scenario():
            reminder_attempt = await bind("reminders", "300", NOW)
            repeated = await bind("reminders", "300", NOW + timedelta(minutes=1))
            card_attempt = await bind("amendments", "400", NOW)
            digest_attempt = await bind("weekly_digests", "600", NOW)
            decline_attempt = await bind("decline_notices", "700", NOW)
            with pytest.raises(MaintenanceClosedError):
                async with repo.maintenance.operation("ordinary-after-adoption-bind"):
                    pass
            return reminder_attempt, repeated, card_attempt, digest_attempt, decline_attempt

        reminder_attempt, repeated, card_attempt, digest_attempt, decline_attempt = asyncio.run(
            scenario()
        )
        assert repeated == reminder_attempt
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            before + 4
        )
        bound = repo._conn.execute(
            "SELECT origin, state, destination_kind, guild_id, channel_id, message_id, "
            "fingerprint_version, request_fingerprint, observable_fingerprint "
            "FROM delivery_attempts WHERE attempt_id = ?",
            (reminder_attempt,),
        ).fetchone()
        assert tuple(bound[:7]) == ("adoption", "bound", "channel", "100", "200", "300", 1)
        assert bound[7] != bound[8]
        assert all(len(bound[index]) == 64 for index in (7, 8))
        assert [
            tuple(row)
            for row in repo._conn.execute(
                "SELECT target_ordinal, binding_type, key_primary, key_secondary "
                "FROM delivery_attempt_targets WHERE attempt_id = ? ORDER BY target_ordinal",
                (reminder_attempt,),
            )
        ] == [
            (0, "reminder", "adopt-rem-a", ""),
            (1, "reminder", "adopt-rem-b", ""),
        ]
        for family, source_ids, attempt_id in (
            ("reminders", ("adopt-rem-a", "adopt-rem-b"), reminder_attempt),
            ("amendments", ("adopt-card-a", "adopt-card-b"), card_attempt),
        ):
            rows = repo._conn.execute(
                "SELECT source_primary, resolution_state, adopted_attempt_id, resolution_actor, "
                "resolution_at, "
                "resolution_reason FROM adoption_sources WHERE adoption_id = ? "
                "AND source_family = ? ORDER BY source_primary",
                (report.adoption_id, family),
            ).fetchall()
            group_rows = [
                row
                for row in rows
                if row["source_primary"] in source_ids and row["adopted_attempt_id"] == attempt_id
            ]
            assert [row["resolution_state"] for row in group_rows] == [
                "verified_bound",
                "verified_bound",
            ]
            assert all(row["resolution_actor"] == "operator-7" for row in group_rows)
            assert all(
                row["resolution_reason"] == "synthetic offline reconciliation fixture"
                for row in group_rows
            )
            assert all(row["resolution_at"] == to_iso(NOW) for row in group_rows)
        for family, attempt_id, expected_target in (
            (
                "weekly_digests",
                digest_attempt,
                (0, "digest", "2026-10-01T00:00:00+00:00", ""),
            ),
            (
                "decline_notices",
                decline_attempt,
                (0, "decline", "run-canonical", "single-user"),
            ),
        ):
            source = repo._conn.execute(
                "SELECT resolution_state, adopted_attempt_id, resolution_actor "
                "FROM adoption_sources WHERE adoption_id = ? AND source_family = ? "
                "AND adopted_attempt_id = ?",
                (report.adoption_id, family, attempt_id),
            ).fetchall()
            assert len(source) == 1
            assert tuple(source[0]) == ("verified_bound", attempt_id, "operator-7")
            assert (
                tuple(
                    repo._conn.execute(
                        "SELECT target_ordinal, binding_type, key_primary, key_secondary "
                        "FROM delivery_attempt_targets WHERE attempt_id = ? "
                        "ORDER BY target_ordinal",
                        (attempt_id,),
                    ).fetchone()
                )
                == expected_target
            )
        stored = " ".join(
            str(value)
            for row in repo._conn.execute("SELECT * FROM delivery_attempts")
            for value in row
        )
        assert PRIVATE_TEXT not in stored
        assert tuple(
            repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM maintenance_leases "
                "WHERE operation_kind = 'adoption_resolution' "
                "AND lifecycle = 'live'"
            ).fetchone()[0]
            == 0
        )
    finally:
        repo.close()


def test_message_evidence_rejects_zero_multiple_wrong_identity_unsupported_and_partial_input(
    tmp_path, owner_lock_dir
):
    repo = _pending_repo(tmp_path / "adoption-evidence-errors.sqlite", 13, owner_lock_dir)
    _seed(repo)
    valid = _observation()
    cases = (
        ((), {}, AdoptionEvidenceError),
        ((valid, valid), {}, AdoptionEvidenceError),
        ((_observation(author_id="901"),), {}, AdoptionEvidenceError),
        ((_observation(guild_id="101"),), {}, AdoptionEvidenceError),
        ((_observation(channel_id="201"),), {}, AdoptionEvidenceError),
        ((_observation(unsupported=("sticker",)),), {}, AdoptionEvidenceError),
        ((valid,), {"family": "weekly_digests"}, AdoptionResolutionError),
    )
    try:
        for observations, overrides, error in cases:
            arguments = {
                "family": "reminders",
                "channel_id": "200",
                "message_id": "300",
                "observations": observations,
                "guild_id": "100",
                "bot_author_id": "900",
                "actor": "operator-7",
                "reason": "invalid synthetic fixture",
                "at": NOW,
            }
            arguments.update(overrides)
            with pytest.raises(error):
                asyncio.run(repo.maintenance.bind_adoption_group(**arguments))
        with pytest.raises(TypeError):
            CompleteAttachmentEvidence(name="partial.txt", size=3, url=None)
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        repo.close()


def test_adoption_rejects_cross_family_message_identity_and_duplicate_runtime_target_claim(
    tmp_path, owner_lock_dir
):
    mixed = _pending_repo(
        tmp_path / "adoption-mixed-family.sqlite", 13, owner_lock_dir, mixed_group=True
    )
    try:
        _seed(mixed)
        with pytest.raises(AdoptionResolutionError, match="multiple families"):
            asyncio.run(
                mixed.maintenance.bind_adoption_group(
                    "reminders",
                    "200",
                    "300",
                    (_observation(),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="reject mixed group",
                    at=NOW,
                )
            )
        assert mixed._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == 0
    finally:
        mixed.close()

    conflicted = _pending_repo(
        tmp_path / "adoption-runtime-claim.sqlite",
        14,
        owner_lock_dir,
        conflicting_claim=True,
    )
    try:
        _seed(conflicted)
        with pytest.raises(AdoptionResolutionError):
            asyncio.run(
                conflicted.maintenance.bind_adoption_group(
                    "reminders",
                    "200",
                    "300",
                    (_observation(),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="reject preexisting runtime target",
                    at=NOW,
                )
            )
        assert (
            conflicted._conn.execute(
                "SELECT origin FROM delivery_attempts WHERE attempt_id = 'runtime-duplicate'"
            ).fetchone()[0]
            == "runtime"
        )
        assert (
            conflicted._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempts WHERE origin = 'adoption'"
            ).fetchone()[0]
            == 0
        )
    finally:
        conflicted.close()


def test_bind_refuses_source_facts_changed_after_capture(tmp_path, owner_lock_dir):
    path = tmp_path / "stale-adoption-source.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    try:
        _seed(repo)
    finally:
        repo.close()
    with sqlite3.connect(path) as source:
        source.execute(
            "UPDATE reminders SET fire_at = ? WHERE id = 'adopt-rem-a'",
            (to_iso(NOW - timedelta(minutes=2)),),
        )

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        with pytest.raises(AdoptionConflictError, match="source facts or journal claims changed"):
            asyncio.run(
                reopened.maintenance.bind_adoption_group(
                    "reminders",
                    "200",
                    "300",
                    (_observation(),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="stale source evidence",
                    at=NOW,
                )
            )
        assert (
            reopened._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempts WHERE origin = 'adoption'"
            ).fetchone()[0]
            == 0
        )
        assert reopened._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        reopened.close()


@pytest.mark.parametrize("version", [13, 14])
def test_source_only_retirement_records_reason_without_attempt_or_native_mutation(
    tmp_path, owner_lock_dir, version
):
    repo = _pending_repo(tmp_path / f"source-retire-v{version}.sqlite", version, owner_lock_dir)
    report = _seed(repo)
    before_attempts = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]
    try:
        asyncio.run(
            repo.maintenance.retire_adoption_source(
                SourceKey("reminders", "due"),
                actor="operator-8",
                reason="due-null reminder will not be replayed",
                at=NOW + timedelta(minutes=2),
            )
        )
        row = repo._conn.execute(
            "SELECT resolution_state, adopted_attempt_id, resolution_actor, resolution_at, "
            "resolution_reason FROM adoption_sources WHERE adoption_id = ? "
            "AND source_family = 'reminders' AND source_primary = 'due'",
            (report.adoption_id,),
        ).fetchone()
        assert tuple(row) == (
            "reasoned_retired",
            None,
            "operator-8",
            to_iso(NOW + timedelta(minutes=2)),
            "due-null reminder will not be replayed",
        )
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            before_attempts
        )
        assert tuple(
            repo._conn.execute(
                "SELECT sent_at, message_id FROM reminders WHERE id = 'due'"
            ).fetchone()
        ) == (None, None)
        assert PRIVATE_TEXT not in " ".join(str(value) for value in row)
    finally:
        repo.close()


def test_adopted_attempt_retirement_releases_full_claim_group_atomically(tmp_path, owner_lock_dir):
    repo = _pending_repo(tmp_path / "adoption-attempt-retire.sqlite", 13, owner_lock_dir)
    report = _seed(repo)

    async def bind():
        return await repo.maintenance.bind_adoption_group(
            "reminders",
            "200",
            "300",
            (_observation(),),
            guild_id="100",
            bot_author_id="900",
            actor="operator-7",
            reason="synthetic offline reconciliation fixture",
            at=NOW,
        )

    try:
        attempt_id = asyncio.run(bind())
        asyncio.run(
            repo.maintenance.retire_adoption_attempt(
                attempt_id,
                actor="operator-9",
                reason="exact adopted message retired offline",
                at=NOW + timedelta(minutes=3),
            )
        )
        attempt = repo._conn.execute(
            "SELECT origin, state, dedupe_active, resolved_at, resolved_by, resolution_reason "
            "FROM delivery_attempts WHERE attempt_id = ?",
            (attempt_id,),
        ).fetchone()
        assert tuple(attempt) == (
            "adoption",
            "retired",
            0,
            to_iso(NOW + timedelta(minutes=3)),
            "operator-9",
            "exact adopted message retired offline",
        )
        claims = repo._conn.execute(
            "SELECT binding_type, key_primary, released_at, release_actor, release_reason "
            "FROM delivery_attempt_targets WHERE attempt_id = ? ORDER BY target_ordinal",
            (attempt_id,),
        ).fetchall()
        assert [tuple(row) for row in claims] == [
            (
                "reminder",
                "adopt-rem-a",
                to_iso(NOW + timedelta(minutes=3)),
                "operator-9",
                "exact adopted message retired offline",
            ),
            (
                "reminder",
                "adopt-rem-b",
                to_iso(NOW + timedelta(minutes=3)),
                "operator-9",
                "exact adopted message retired offline",
            ),
        ]
        rows = repo._conn.execute(
            "SELECT resolution_state, adopted_attempt_id, resolution_actor, resolution_reason "
            "FROM adoption_sources WHERE adoption_id = ? AND source_family = 'reminders' "
            "AND source_primary IN ('adopt-rem-a', 'adopt-rem-b') ORDER BY source_primary",
            (report.adoption_id,),
        ).fetchall()
        assert [tuple(row) for row in rows] == [
            ("reasoned_retired", None, "operator-9", "exact adopted message retired offline"),
            ("reasoned_retired", None, "operator-9", "exact adopted message retired offline"),
        ]
        assert [
            tuple(row)
            for row in repo._conn.execute(
                "SELECT sent_at, message_id FROM reminders "
                "WHERE id IN ('adopt-rem-a', 'adopt-rem-b') ORDER BY id"
            )
        ] == [(to_iso(NOW - timedelta(hours=1)), "300")] * 2
    finally:
        repo.close()


@pytest.mark.parametrize("failure", [RuntimeError, asyncio.CancelledError])
def test_failed_or_cancelled_second_group_source_update_rolls_back_adoption_attempt_and_claims(
    tmp_path, owner_lock_dir, monkeypatch, failure
):
    repo = _pending_repo(tmp_path / "adoption-bind-rollback.sqlite", 13, owner_lock_dir)
    report = _seed(repo)
    before_attempts = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]
    real_update = adoption_resolution._bind_source_cas
    calls = 0

    def fail_second(*args, **kwargs):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise failure("injected second source CAS failure")
        return real_update(*args, **kwargs)

    monkeypatch.setattr(adoption_resolution, "_bind_source_cas", fail_second)
    try:
        with pytest.raises(failure):
            asyncio.run(
                repo.maintenance.bind_adoption_group(
                    "reminders",
                    "200",
                    "300",
                    (_observation(),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="rollback group",
                    at=NOW,
                )
            )
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            before_attempts
        )
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE key_primary IN "
                "('adopt-rem-a', 'adopt-rem-b')"
            ).fetchone()[0]
            == 0
        )
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM adoption_sources WHERE adoption_id = ? "
                "AND source_family = 'reminders' "
                "AND source_primary IN ('adopt-rem-a', 'adopt-rem-b') "
                "AND resolution_state = 'pending' AND adopted_attempt_id IS NULL",
                (report.adoption_id,),
            ).fetchone()[0]
            == 2
        )
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
        assert tuple(
            repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
    finally:
        repo.close()


def test_failed_second_attempt_source_retirement_rolls_back_all_claims_and_state(
    tmp_path, owner_lock_dir, monkeypatch
):
    repo = _pending_repo(tmp_path / "adoption-retire-rollback.sqlite", 13, owner_lock_dir)
    _seed(repo)
    attempt_id = asyncio.run(
        repo.maintenance.bind_adoption_group(
            "reminders",
            "200",
            "300",
            (_observation(),),
            guild_id="100",
            bot_author_id="900",
            actor="operator-7",
            reason="synthetic offline reconciliation fixture",
            at=NOW,
        )
    )
    real_update = adoption_resolution._retire_bound_source_cas
    calls = 0

    def fail_second(*args, **kwargs):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise RuntimeError("injected second retirement CAS failure")
        return real_update(*args, **kwargs)

    monkeypatch.setattr(adoption_resolution, "_retire_bound_source_cas", fail_second)
    try:
        with pytest.raises(RuntimeError, match="second retirement CAS"):
            asyncio.run(
                repo.maintenance.retire_adoption_attempt(
                    attempt_id,
                    actor="operator-9",
                    reason="rollback exact attempt retirement",
                    at=NOW + timedelta(minutes=1),
                )
            )
        assert tuple(
            repo._conn.execute(
                "SELECT state, dedupe_active FROM delivery_attempts WHERE attempt_id = ?",
                (attempt_id,),
            ).fetchone()
        ) == ("bound", 1)
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE attempt_id = ? "
                "AND released_at IS NULL",
                (attempt_id,),
            ).fetchone()[0]
            == 2
        )
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM adoption_sources WHERE adopted_attempt_id = ? "
                "AND resolution_state = 'verified_bound'",
                (attempt_id,),
            ).fetchone()[0]
            == 2
        )
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
    finally:
        repo.close()


def test_due_null_source_tombstone_stays_blocked_and_never_sends_after_restart(
    tmp_path, owner_lock_dir
):
    path = tmp_path / "due-null-retired.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    report = _seed(repo)
    asyncio.run(
        repo.maintenance.retire_adoption_source(
            SourceKey("reminders", "due"),
            actor="operator-8",
            reason="due-null reminder will not be replayed",
            at=NOW,
        )
    )
    attempts_before = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        row = reopened._conn.execute(
            "SELECT resolution_state, resolution_actor, resolution_reason FROM adoption_sources "
            "WHERE adoption_id = ? AND source_family = 'reminders' AND source_primary = 'due'",
            (report.adoption_id,),
        ).fetchone()
        assert tuple(row) == (
            "reasoned_retired",
            "operator-8",
            "due-null reminder will not be replayed",
        )
        assert tuple(
            reopened._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        assert tuple(
            reopened._conn.execute(
                "SELECT sent_at, message_id FROM reminders WHERE id = 'due'"
            ).fetchone()
        ) == (None, None)

        async def never_send():
            async with reopened.maintenance.operation("must-stay-closed"):
                pass

        with pytest.raises(MaintenanceClosedError):
            asyncio.run(never_send())

        class NeverTransport:
            calls = 0

            async def send(self, _plan):
                self.calls += 1
                pytest.fail("source retirement must never perform a send")

        transport = NeverTransport()
        plan = SendPlan(
            "adoption.no-replay.test",
            Destination.channel("100", "200"),
            SendPayload(content="must remain offline"),
            DedupePolicy.operation(),
        )
        with pytest.raises(MaintenanceClosedError):
            asyncio.run(reopened.delivery.execute(plan, transport))
        assert transport.calls == 0
        assert reopened._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            attempts_before
        )
    finally:
        reopened.close()


def test_resolution_requires_actual_task_live_persisted_authority_and_rejects_copied_stale_use(
    tmp_path, owner_lock_dir, monkeypatch
):
    repo = _pending_repo(tmp_path / "adoption-resolution-authority.sqlite", 13, owner_lock_dir)
    _seed(repo)
    coroutine = repo.maintenance.bind_adoption_group(
        "reminders",
        "200",
        "300",
        (_observation(),),
        guild_id="100",
        bot_author_id="900",
        actor="operator-7",
        reason="taskless denial",
        at=NOW,
    )
    try:
        with pytest.raises(MaintenanceStateError, match="asyncio task"):
            coroutine.send(None)
    finally:
        coroutine.close()

    authorities = []
    real_register = repo._guard._register_adoption_resolution

    def capture_authority(*args, **kwargs):
        authority = real_register(*args, **kwargs)
        authorities.append(authority)
        return authority

    monkeypatch.setattr(repo._guard, "_register_adoption_resolution", capture_authority)
    real_bind_source = adoption_resolution._bind_source_cas
    children = []

    def copy_bound_context(conn, guard, authority, *args):
        if not children:

            async def copied_child():
                with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                    repo._conn.execute(
                        "UPDATE adoption_sources SET resolution_reason = 'copied context' "
                        "WHERE source_family = 'reminders' AND source_primary = 'adopt-rem-a'"
                    )

            with guard._adoption_source_bind_scope(authority):
                with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                    repo._conn.execute(
                        "INSERT INTO delivery_attempts (attempt_id) VALUES ('forged')"
                    )
                with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                    repo._conn.execute(
                        "UPDATE delivery_attempt_targets SET released_at = ?",
                        (to_iso(NOW),),
                    )
                with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                    repo._conn.execute(
                        "UPDATE reminders SET sent_at = ? WHERE id = 'adopt-rem-a'",
                        (to_iso(NOW),),
                    )
                children.append(asyncio.create_task(copied_child()))
        return real_bind_source(conn, guard, authority, *args)

    monkeypatch.setattr(adoption_resolution, "_bind_source_cas", copy_bound_context)

    async def scenario():
        attempt_id = await repo.maintenance.bind_adoption_group(
            "reminders",
            "200",
            "300",
            (_observation(),),
            guild_id="100",
            bot_author_id="900",
            actor="operator-7",
            reason="copied-context regression",
            at=NOW,
        )
        await asyncio.gather(*children)
        with pytest.raises(RuntimeError, match="not live"):
            with repo._guard._adoption_group_insert_scope(authorities[0]):
                pass
        return attempt_id

    try:
        asyncio.run(scenario())

        real_assert = repo.maintenance._assert_adoption_resolution_lease
        removed = False

        def delete_persisted_then_assert(lease):
            nonlocal removed
            if not removed:
                with repo._guard._adoption_resolution_lease_delete_scope():
                    repo._conn.execute(
                        "DELETE FROM maintenance_leases WHERE operation_id = ?",
                        (lease.operation_id,),
                    )
                removed = True
                with pytest.raises(RuntimeError, match="not live"):
                    with repo._guard._adoption_source_retire_scope(lease.authority):
                        pass
            return real_assert(lease)

        monkeypatch.setattr(
            repo.maintenance, "_assert_adoption_resolution_lease", delete_persisted_then_assert
        )
        with pytest.raises(MaintenanceStateError, match="persisted adoption resolution lease"):
            asyncio.run(
                repo.maintenance.retire_adoption_source(
                    SourceKey("reminders", "due"),
                    actor="operator-8",
                    reason="stale lease must not resolve",
                    at=NOW,
                )
            )
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0
        assert (
            repo._conn.execute(
                "SELECT resolution_state FROM adoption_sources WHERE source_family = 'reminders' "
                "AND source_primary = 'due'"
            ).fetchone()[0]
            == "pending"
        )
    finally:
        repo.close()


def test_interrupted_resolution_lease_is_orphaned_on_restart_without_authority(
    tmp_path, owner_lock_dir, monkeypatch
):
    path = tmp_path / "interrupted-adoption-resolution.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir)
    report = _seed(repo)
    captured = []
    real_register = repo._guard._register_adoption_resolution

    def capture_authority(*args, **kwargs):
        authority = real_register(*args, **kwargs)
        captured.append(authority)
        return authority

    monkeypatch.setattr(repo._guard, "_register_adoption_resolution", capture_authority)

    @contextmanager
    def fail_lease_cleanup():
        raise RuntimeError("simulated interruption before lease cleanup")
        yield

    monkeypatch.setattr(
        repo._guard,
        "_adoption_resolution_lease_delete_scope",
        fail_lease_cleanup,
    )
    with pytest.raises(RuntimeError, match="simulated interruption"):
        asyncio.run(
            repo.maintenance.retire_adoption_source(
                SourceKey("reminders", "due"),
                actor="operator-8",
                reason="committed source-only retirement",
                at=NOW,
            )
        )
    assert (
        repo._conn.execute(
            "SELECT resolution_state FROM adoption_sources WHERE adoption_id = ? "
            "AND source_family = 'reminders' AND source_primary = 'due'",
            (report.adoption_id,),
        ).fetchone()[0]
        == "reasoned_retired"
    )
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        lease = reopened._conn.execute(
            "SELECT operation_kind, lifecycle, orphaned_at FROM maintenance_leases"
        ).fetchone()
        assert tuple(lease[:2]) == ("adoption_resolution", "orphaned")
        assert lease[2] is not None
        assert tuple(
            reopened._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == ("BLOCKED", "pending")
        with pytest.raises(RuntimeError, match="not live"):
            with reopened._guard._adoption_source_retire_scope(captured[0]):
                pass
        assert reopened._conn.execute(
            "SELECT resolution_state, resolution_actor FROM adoption_sources "
            "WHERE adoption_id = ? AND source_family = 'reminders' AND source_primary = 'due'",
            (report.adoption_id,),
        ).fetchone()[0:2] == ("reasoned_retired", "operator-8")
    finally:
        reopened.close()


@pytest.mark.parametrize(
    "family",
    [SourceFamily.DECLINE_NOTICES, SourceFamily.WEEKLY_DIGESTS],
)
def test_adoption_rejects_multi_target_decline_and_digest_message_groups(
    tmp_path, owner_lock_dir, family
):
    repo = _pending_repo(
        tmp_path / f"multi-target-{family.value}.sqlite",
        13,
        owner_lock_dir,
        multi_target_family=family.value,
    )
    report = _seed(repo)
    before_attempts = repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0]
    binding_type = "decline" if family is SourceFamily.DECLINE_NOTICES else "digest"
    try:
        source_rows = repo._conn.execute(
            "SELECT source_primary, source_secondary, resolution_state, adopted_attempt_id "
            "FROM adoption_sources WHERE adoption_id = ? AND source_family = ? "
            "AND legacy_channel_id = '200' AND legacy_message_id = '500' "
            "ORDER BY source_primary, source_secondary",
            (report.adoption_id, family.value),
        ).fetchall()
        assert len(source_rows) == 2
        with pytest.raises(AdoptionResolutionError, match="only reminder or card groups"):
            asyncio.run(
                repo.maintenance.bind_adoption_group(
                    family,
                    "200",
                    "500",
                    (_observation("500"),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="reject multi-target single-send family",
                    at=NOW,
                )
            )
        assert repo._conn.execute("SELECT COUNT(*) FROM delivery_attempts").fetchone()[0] == (
            before_attempts
        )
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE binding_type = ? "
                "AND key_primary IN ('run-canonical', '2026-10-08T00:00:00+00:00')",
                (binding_type,),
            ).fetchone()[0]
            == 0
        )
        after = repo._conn.execute(
            "SELECT resolution_state, adopted_attempt_id, resolution_actor FROM adoption_sources "
            "WHERE adoption_id = ? AND source_family = ? AND legacy_channel_id = '200' "
            "AND legacy_message_id = '500' ORDER BY source_primary, source_secondary",
            (report.adoption_id, family.value),
        ).fetchall()
        assert [tuple(row) for row in after] == [("pending", None, None)] * 2
    finally:
        repo.close()


@pytest.mark.parametrize(
    "family",
    [SourceFamily.DECLINE_NOTICES, SourceFamily.WEEKLY_DIGESTS],
)
def test_exact_attempt_retirement_rejects_malformed_multi_target_family_without_releasing(
    tmp_path, owner_lock_dir, family
):
    path = tmp_path / f"malformed-retirement-{family.value}.sqlite"
    repo = _pending_repo(path, 13, owner_lock_dir, multi_target_family=family.value)
    report = _seed(repo)
    repo.close()

    attempt_id = f"malformed-{family.value}"
    stamp = to_iso(NOW)
    with sqlite3.connect(path) as conn:
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys=ON")
        sources = tuple(
            conn.execute(
                "SELECT * FROM adoption_sources WHERE adoption_id = ? AND source_family = ? "
                "AND legacy_channel_id = '200' AND legacy_message_id = '500' "
                "ORDER BY source_primary, source_secondary",
                (report.adoption_id, family.value),
            )
        )
        assert len(sources) == 2
        keys = tuple(
            SourceKey(row["source_family"], row["source_primary"], row["source_secondary"])
            for row in sources
        )
        claims = adoption_resolution._target_claims(family, keys)
        fingerprint = adoption_resolution._group_source_fingerprint(
            report.adoption_id, family, sources
        )
        dedupe_key = adoption_resolution._native_dedupe_key(claims)
        conn.execute(
            "INSERT INTO delivery_attempts "
            "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind, "
            "dedupe_scope, dedupe_key, dedupe_active, state, destination_kind, guild_id, "
            "channel_id, recipient_id, message_id, fingerprint_version, request_fingerprint, "
            "observable_fingerprint, intended_at, resolved_at, resolved_by, resolution_reason) "
            "VALUES (?, ?, 0, 'synthetic', 'adoption', ?, 'native', ?, 1, 'bound', 'channel', "
            "'100', '200', NULL, '500', 1, ?, ?, ?, ?, 'operator-7', "
            "'synthetic malformed fixture')",
            (
                attempt_id,
                f"adoption:{report.adoption_id}",
                f"adoption.{family.value}",
                dedupe_key,
                fingerprint,
                "a" * 64,
                stamp,
                stamp,
            ),
        )
        conn.executemany(
            "INSERT INTO delivery_attempt_targets "
            "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
            "VALUES (?, ?, ?, ?, ?)",
            ((attempt_id, ordinal, *claim) for ordinal, claim in enumerate(claims)),
        )
        for source in sources:
            conn.execute(
                "UPDATE adoption_sources SET resolution_state = 'verified_bound', "
                "adopted_attempt_id = ?, resolution_actor = 'operator-7', resolution_at = ?, "
                "resolution_reason = 'synthetic malformed fixture' WHERE adoption_id = ? "
                "AND source_family = ? AND source_primary = ? AND source_secondary = ?",
                (
                    attempt_id,
                    stamp,
                    report.adoption_id,
                    source["source_family"],
                    source["source_primary"],
                    source["source_secondary"],
                ),
            )

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        # The idempotent re-bind path must enforce cardinality too.
        with pytest.raises(AdoptionResolutionError, match="only reminder or card groups"):
            asyncio.run(
                reopened.maintenance.bind_adoption_group(
                    family,
                    "200",
                    "500",
                    (_observation("500"),),
                    guild_id="100",
                    bot_author_id="900",
                    actor="operator-7",
                    reason="synthetic malformed fixture",
                    at=NOW,
                )
            )
        with pytest.raises(AdoptionResolutionError, match="only reminder or card groups"):
            asyncio.run(
                reopened.maintenance.retire_adoption_attempt(
                    attempt_id,
                    actor="operator-9",
                    reason="reject malformed multi-target retirement",
                    at=NOW + timedelta(minutes=1),
                )
            )
        assert tuple(
            reopened._conn.execute(
                "SELECT state, dedupe_active FROM delivery_attempts WHERE attempt_id = ?",
                (attempt_id,),
            ).fetchone()
        ) == ("bound", 1)
        assert (
            reopened._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets WHERE attempt_id = ? "
                "AND released_at IS NULL",
                (attempt_id,),
            ).fetchone()[0]
            == 2
        )
        assert (
            reopened._conn.execute(
                "SELECT COUNT(*) FROM adoption_sources WHERE adopted_attempt_id = ? "
                "AND resolution_state = 'verified_bound'",
                (attempt_id,),
            ).fetchone()[0]
            == 2
        )
    finally:
        reopened.close()
