"""Supported database creation and synthetic schema upgrade regressions."""

from __future__ import annotations

import sqlite3

import pytest

import bot.infrastructure.db as db_module
from bot.infrastructure.db import SCHEMA_VERSION, Repo


def v9_database(path) -> None:
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (9);
        CREATE TABLE members (
            user_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL DEFAULT '',
            nickname TEXT,
            aliases TEXT NOT NULL DEFAULT '[]',
            has_role INTEGER NOT NULL DEFAULT 0,
            ping_level TEXT NOT NULL DEFAULT 'essential',
            updated_at TEXT NOT NULL
        );
        INSERT INTO members VALUES (
            '7', 'harbour4417', 'MY', '["MY"]', 1, 'off',
            '2026-08-30T00:00:00+00:00'
        );
        CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO config VALUES ('persona', 'persona.md');
        """
    )
    conn.commit()
    conn.close()


def legacy_memory_storage(conn) -> None:
    """Recreate the removed v12-v15 personal-memory tables with one retained row each."""
    conn.executescript(
        """
        CREATE TABLE chat_memory_enrollments (
            guild_id TEXT NOT NULL, user_id TEXT NOT NULL, state TEXT NOT NULL,
            PRIMARY KEY (guild_id, user_id)
        );
        CREATE INDEX chat_memory_enrollments_active
            ON chat_memory_enrollments (guild_id, user_id) WHERE state = 'active';
        CREATE TABLE chat_memories (
            id TEXT PRIMARY KEY, guild_id TEXT NOT NULL, user_id TEXT NOT NULL,
            slot TEXT NOT NULL, value TEXT NOT NULL, state TEXT NOT NULL
        );
        CREATE INDEX chat_memories_retrieval ON chat_memories (guild_id, user_id, state);
        CREATE TABLE chat_memory_events (id TEXT PRIMARY KEY, memory_id TEXT, at TEXT NOT NULL);
        CREATE TABLE chat_memory_retrievals (id TEXT PRIMARY KEY, at TEXT NOT NULL);
        CREATE TRIGGER chat_memories_event AFTER DELETE ON chat_memories
        BEGIN
            INSERT INTO chat_memory_events (id, memory_id, at) VALUES (OLD.id, OLD.id, 'now');
        END;
        INSERT INTO chat_memory_enrollments VALUES ('guild', '7', 'active');
        INSERT INTO chat_memories
            VALUES ('memory-1', 'guild', '7', 'answer_detail', 'concise', 'active');
        INSERT INTO chat_memory_events VALUES ('event-1', 'memory-1', 'then');
        INSERT INTO chat_memory_retrievals VALUES ('retrieval-1', 'then');
        """
    )


LEGACY_MEMORY_OBJECTS = {
    "chat_memory_enrollments",
    "chat_memory_enrollments_active",
    "chat_memories",
    "chat_memories_retrieval",
    "chat_memories_event",
    "chat_memory_events",
    "chat_memory_retrievals",
}


def memory_objects(conn) -> list[str]:
    """Every schema object that belongs to removed personal-memory storage."""
    return [
        row[0]
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE name GLOB 'chat_memor*' "
            "OR tbl_name GLOB 'chat_memor*' ORDER BY name"
        )
    ]


def v13_database_with_retained_rows(path) -> None:
    """Build a realistic pre-maintenance image without using a live Repo."""
    conn = sqlite3.connect(path)
    db_module._execute_schema(conn)
    for table in (
        "adoption_sources",
        "delivery_attempt_targets",
        "delivery_attempts",
        "maintenance_leases",
        "maintenance_state",
    ):
        conn.execute(f"DROP TABLE {table}")
    conn.execute("INSERT INTO schema_version (version) VALUES (13)")
    conn.execute(
        "INSERT INTO members (user_id, display_name, nickname, aliases, has_role, ping_level, "
        "updated_at) "
        "VALUES ('7', 'retained member', 'old', '[\"old\"]', 1, 'all', ?)",
        ("2026-09-20T00:00:00+00:00",),
    )
    legacy_memory_storage(conn)
    conn.commit()
    conn.close()


def memory_journal_claim(conn) -> None:
    """A bound v15 memory-card claim that v16 keeps as journal history."""
    conn.execute(
        "INSERT INTO delivery_attempts "
        "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind, "
        "dedupe_scope, dedupe_key, state, destination_kind, guild_id, channel_id, message_id, "
        "fingerprint_version, request_fingerprint, intended_at) "
        "VALUES ('attempt-memory-1', 'operation-memory-1', 0, 'instance-old', 'runtime', "
        "'memory_proposal', 'native', ?, 'bound', 'channel', 'guild', 'channel-legacy-1', "
        "'message-memory-1', 1, ?, ?)",
        ("d" * 64, "e" * 64, "2026-09-22T00:00:00+00:00"),
    )
    conn.execute(
        "INSERT INTO delivery_attempt_targets "
        "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
        "VALUES ('attempt-memory-1', 0, 'memory_proposal', 'memory-1', '')"
    )


def v14_database_with_retained_rows(
    path,
    *,
    mode: str = "OPEN",
    adoption_state: str = "complete",
    version: int = 14,
    live_lease: bool = True,
    memory_claim: bool = False,
) -> None:
    """Build a v14/v15 source image with schedule, journal, audit, lease and memory rows."""
    conn = sqlite3.connect(path)
    conn.execute("PRAGMA foreign_keys=ON")
    db_module._execute_schema(conn)
    if version == 14:
        conn.execute("DROP TABLE adoption_sources")
    legacy_memory_storage(conn)
    conn.execute("INSERT INTO schema_version (version) VALUES (?)", (version,))
    adoption_id = "existing-pending" if adoption_state == "pending" else "old-complete"
    conn.execute(
        "INSERT INTO maintenance_state "
        "(id, mode, state_revision, adoption_state, adoption_id, started_at, completed_at, "
        "attested_by, attested_at, attestation_reason) "
        "VALUES (1, ?, 4, ?, ?, ?, ?, ?, ?, ?)",
        (
            mode,
            adoption_state,
            adoption_id,
            "2026-09-22T00:00:00+00:00",
            "2026-09-22T00:01:00+00:00" if adoption_state == "complete" else None,
            "operator" if adoption_state == "complete" else None,
            "2026-09-22T00:01:00+00:00" if adoption_state == "complete" else None,
            "previously reconciled" if adoption_state == "complete" else None,
        ),
    )
    conn.execute(
        "INSERT INTO reminders (id, run_id, fire_at, kind, sent_at, message_id) "
        "VALUES ('reminder-legacy-1', 'run-legacy-1', ?, 'countdown:15', ?, ?)",
        (
            "2026-09-24T00:00:00+00:00",
            "2026-09-22T00:00:00+00:00",
            "message-legacy-1",
        ),
    )
    conn.execute(
        "INSERT INTO audit (id, at, surface, actor, action, subject, detail) "
        "VALUES ('audit-legacy-1', ?, 'system', 'operator', 'legacy', 'run-legacy-1', 'kept')",
        ("2026-09-22T00:00:00+00:00",),
    )
    conn.execute(
        "INSERT INTO delivery_attempts "
        "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind, "
        "dedupe_scope, dedupe_key, state, destination_kind, guild_id, channel_id, message_id, "
        "fingerprint_version, request_fingerprint, intended_at) "
        "VALUES ('attempt-legacy-1', 'operation-legacy-1', 0, 'instance-old', 'runtime', "
        "'reminder', 'native', ?, 'bound', 'channel', 'guild', 'channel-legacy-1', "
        "'message-legacy-1', 1, ?, ?)",
        (
            "a" * 64,
            "b" * 64,
            "2026-09-22T00:00:00+00:00",
        ),
    )
    conn.execute(
        "INSERT INTO delivery_attempt_targets "
        "(attempt_id, target_ordinal, binding_type, key_primary, key_secondary) "
        "VALUES ('attempt-legacy-1', 0, 'reminder', 'reminder-legacy-1', '')"
    )
    if memory_claim:
        memory_journal_claim(conn)
    if live_lease:
        conn.execute(
            "INSERT INTO maintenance_leases "
            "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
            "started_at, owner_task_id, lifecycle) "
            "VALUES ('lease-legacy-1', 'instance-old', ?, 0, 'admin', ?, 1, 'live')",
            ("c" * 64, "2026-09-22T00:00:00+00:00"),
        )
    conn.commit()
    conn.close()


def test_a_fresh_database_starts_at_v16_open_without_memory_storage(tmp_path, owner_lock_dir):
    repo = Repo(tmp_path / "fresh.sqlite", owner_lock_dir=owner_lock_dir)
    assert SCHEMA_VERSION == 16
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    assert repo._conn.execute("SELECT COUNT(*) FROM weekly_digests").fetchone()[0] == 0
    assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 0
    assert {
        "adoption_sources_pending",
        "adoption_sources_grouping",
    } <= {row[1] for row in repo._conn.execute("PRAGMA index_list(adoption_sources)")}
    assert tuple(
        repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
    ) == (
        "OPEN",
        "complete",
    )
    assert memory_objects(repo._conn) == []
    repo.upsert_member(7, "harbour4417", "MY", True)
    assert repo.get_reply_style(7) is None
    repo.close()


def test_v9_migrates_to_v16_blocked_without_losing_member_state(tmp_path, owner_lock_dir):
    path = tmp_path / "v9.sqlite"
    v9_database(path)

    repo = Repo(path, owner_lock_dir=owner_lock_dir)

    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    assert memory_objects(repo._conn) == []
    assert repo._conn.execute("SELECT COUNT(*) FROM weekly_digests").fetchone()[0] == 0
    member = repo.get_member(7)
    assert member["display_name"] == "harbour4417"
    assert member["aliases"] == ["MY"]
    assert member["ping_level"] == "off"
    assert member["reply_style"] is None
    assert repo.get_config("persona") == "persona.md"
    assert tuple(
        repo._conn.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
    ) == (
        "BLOCKED",
        "pending",
    )
    repo.close()


def test_v13_migrates_to_v16_blocked_with_one_pending_adoption_and_no_memory(
    tmp_path, owner_lock_dir
):
    path = tmp_path / "v13.sqlite"
    v13_database_with_retained_rows(path)

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 16
        assert tuple(
            repo._conn.execute(
                "SELECT mode, adoption_state FROM maintenance_state WHERE id = 1"
            ).fetchone()
        ) == ("BLOCKED", "pending")
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_state").fetchone()[0] == 1
        assert repo._conn.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 0
        assert memory_objects(repo._conn) == []
        assert (
            repo._conn.execute("SELECT display_name FROM members WHERE user_id = '7'").fetchone()[0]
            == "retained member"
        )
    finally:
        repo.close()


def _upgrade_source(path, version: int) -> None:
    if version == 13:
        v13_database_with_retained_rows(path)
    else:
        v14_database_with_retained_rows(path, version=version)


@pytest.mark.parametrize(
    ("version", "hooks"),
    [(13, ["ddl", "drop"]), (14, ["ddl", "drop"]), (15, ["drop"])],
)
def test_v13_to_v15_snapshots_precede_schema_ddl(
    tmp_path, monkeypatch, version, hooks, owner_lock_dir
):
    path = tmp_path / f"v{version}.sqlite"
    _upgrade_source(path, version)
    events: list[tuple[object, ...]] = []
    real_snapshot = db_module.upgrade_snapshot
    real_execute_schema = db_module._execute_schema
    real_drop = db_module._drop_removed_memory_storage

    def pragma_state(conn):
        return (
            conn.execute("PRAGMA journal_mode").fetchone()[0],
            conn.execute("PRAGMA synchronous").fetchone()[0],
            conn.execute("PRAGMA foreign_keys").fetchone()[0],
        )

    def recorder(name, real):
        def record(conn, *args):
            events.append((name, len(memory_objects(conn)) > 0, *pragma_state(conn)))
            return real(conn, *args)

        return record

    monkeypatch.setattr(db_module, "upgrade_snapshot", recorder("snapshot", real_snapshot))
    monkeypatch.setattr(db_module, "_execute_schema", recorder("ddl", real_execute_schema))
    monkeypatch.setattr(db_module, "_drop_removed_memory_storage", recorder("drop", real_drop))
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert events == [
            ("snapshot", True, "delete", 2, 0),
            *[(hook, True, "wal", 1, 1) for hook in hooks],
        ]
        assert memory_objects(repo._conn) == []
        assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 16
    finally:
        repo.close()


@pytest.mark.parametrize("version", [13, 14, 15])
def test_v13_to_v15_backup_failure_leaves_database_and_existing_backups_untouched(
    tmp_path, monkeypatch, version, owner_lock_dir
):
    path = tmp_path / f"v{version}.sqlite"
    _upgrade_source(path, version)
    backups = tmp_path / "maintenance-upgrades"
    backups.mkdir(mode=0o700)
    previous = backups / "keep.sqlite"
    previous.write_text("preserve")

    def reject_snapshot(*_args):
        raise RuntimeError("snapshot unavailable")

    def reject_ddl(*_args):
        pytest.fail("migration DDL ran after snapshot failure")

    monkeypatch.setattr(db_module, "upgrade_snapshot", reject_snapshot)
    monkeypatch.setattr(db_module, "_execute_schema", reject_ddl)
    monkeypatch.setattr(db_module, "_drop_removed_memory_storage", reject_ddl)
    with pytest.raises(RuntimeError, match="snapshot unavailable"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    check = sqlite3.connect(path)
    try:
        assert check.execute("SELECT version FROM schema_version").fetchone()[0] == version
        assert check.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'adoption_sources'"
        ).fetchone()[0] == (1 if version == 15 else 0)
        assert LEGACY_MEMORY_OBJECTS <= set(memory_objects(check))
        assert check.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
        if version >= 14:
            assert tuple(
                check.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
            ) == ("OPEN", "complete")
    finally:
        check.close()
    assert previous.read_text() == "preserve"
    assert list(backups.iterdir()) == [previous]


@pytest.mark.parametrize(
    ("mode", "adoption_state"),
    [("OPEN", "complete"), ("FROZEN", "complete"), ("BLOCKED", "pending")],
)
def test_v14_upgrade_preserves_rows_and_reuses_pending_adoption_id(
    tmp_path, owner_lock_dir, mode, adoption_state
):
    path = tmp_path / "v14.sqlite"
    v14_database_with_retained_rows(
        path, mode=mode, adoption_state=adoption_state, memory_claim=True
    )
    backups = tmp_path / "maintenance-upgrades"
    backups.mkdir(mode=0o700)
    previous = backups / "keep.sqlite"
    previous.write_text("preserve")

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        state = repo._conn.execute(
            "SELECT mode, adoption_state, adoption_id, started_at, completed_at, attested_by "
            "FROM maintenance_state WHERE id = 1"
        ).fetchone()
        assert tuple(state[:2]) == ("BLOCKED", "pending")
        if adoption_state == "pending":
            assert state[2] == "existing-pending"
        if adoption_state == "complete":
            assert state[2] != "old-complete"
            assert tuple(state[4:]) == (None, None)
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_state").fetchone()[0] == 1
        assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 16
        assert memory_objects(repo._conn) == []
        assert tuple(
            repo._conn.execute(
                "SELECT binding_type, key_primary FROM delivery_attempt_targets "
                "WHERE attempt_id = 'attempt-memory-1'"
            ).fetchone()
        ) == ("memory_proposal", "memory-1")
        assert tuple(repo._conn.execute("SELECT id, message_id FROM reminders").fetchone()) == (
            "reminder-legacy-1",
            "message-legacy-1",
        )
        assert tuple(
            repo._conn.execute(
                "SELECT state, message_id FROM delivery_attempts "
                "WHERE attempt_id = 'attempt-legacy-1'"
            ).fetchone()
        ) == ("bound", "message-legacy-1")
        assert (
            repo._conn.execute(
                "SELECT COUNT(*) FROM delivery_attempt_targets "
                "WHERE attempt_id = 'attempt-legacy-1'"
            ).fetchone()[0]
            == 1
        )
        assert (
            repo._conn.execute("SELECT detail FROM audit WHERE id = 'audit-legacy-1'").fetchone()[0]
            == "kept"
        )
        assert (
            repo._conn.execute(
                "SELECT lifecycle FROM maintenance_leases WHERE operation_id = 'lease-legacy-1'"
            ).fetchone()[0]
            == "orphaned"
        )
    finally:
        repo.close()

    images = [image for image in backups.iterdir() if image.name != previous.name]
    assert len(images) == 1
    assert images[0].name.startswith(f"{path.name}.pre-upgrade-")
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 14
        assert (
            snapshot.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' "
                "AND name = 'adoption_sources'"
            ).fetchone()[0]
            == 0
        )
        assert snapshot.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
        assert tuple(
            snapshot.execute("SELECT mode, adoption_state FROM maintenance_state").fetchone()
        ) == (mode, adoption_state)
        assert (
            snapshot.execute(
                "SELECT lifecycle FROM maintenance_leases WHERE operation_id = 'lease-legacy-1'"
            ).fetchone()[0]
            == "live"
        )
        assert (
            snapshot.execute(
                "SELECT state FROM delivery_attempts WHERE attempt_id = 'attempt-legacy-1'"
            ).fetchone()[0]
            == "bound"
        )
    finally:
        snapshot.close()
    assert previous.read_text() == "preserve"


def test_v14_ddl_fault_rolls_back_singleton_and_keeps_pre_upgrade_image(
    tmp_path, monkeypatch, owner_lock_dir
):
    path = tmp_path / "v14-fault.sqlite"
    v14_database_with_retained_rows(path)
    backups = tmp_path / "maintenance-upgrades"
    backups.mkdir(mode=0o700)
    previous = backups / "keep.sqlite"
    previous.write_text("prior image")
    original_execute_schema = db_module._execute_schema

    def fail_after_ddl(conn):
        original_execute_schema(conn)
        raise RuntimeError("injected v14 DDL fault")

    monkeypatch.setattr(db_module, "_execute_schema", fail_after_ddl)
    with pytest.raises(RuntimeError, match="injected v14 DDL fault"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    check = sqlite3.connect(path)
    try:
        assert check.execute("SELECT version FROM schema_version").fetchone()[0] == 14
        assert (
            check.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' "
                "AND name = 'adoption_sources'"
            ).fetchone()[0]
            == 0
        )
        assert tuple(
            check.execute(
                "SELECT mode, adoption_state, adoption_id, state_revision FROM maintenance_state"
            ).fetchone()
        ) == ("OPEN", "complete", "old-complete", 4)
        assert check.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
        assert (
            check.execute(
                "SELECT state FROM delivery_attempts WHERE attempt_id = 'attempt-legacy-1'"
            ).fetchone()[0]
            == "bound"
        )
        assert (
            check.execute(
                "SELECT lifecycle FROM maintenance_leases WHERE operation_id = 'lease-legacy-1'"
            ).fetchone()[0]
            == "live"
        )
    finally:
        check.close()

    images = [image for image in backups.iterdir() if image.name != previous.name]
    assert len(images) == 1
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 14
        assert (
            snapshot.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' "
                "AND name = 'adoption_sources'"
            ).fetchone()[0]
            == 0
        )
    finally:
        snapshot.close()
    assert previous.read_text() == "prior image"


def test_supported_upgrade_writes_a_private_nonpruning_old_image_before_ddl(
    tmp_path, owner_lock_dir
):
    path = tmp_path / "v9.sqlite"
    v9_database(path)
    old_backups = tmp_path / "maintenance-upgrades"
    old_backups.mkdir(mode=0o700)
    kept = old_backups / "keep.sqlite"
    kept.write_text("do not prune")

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    images = [image for image in old_backups.iterdir() if image.name != kept.name]
    assert len(images) == 1
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 9
        assert "reply_style" not in {
            row[1] for row in snapshot.execute("PRAGMA table_info(members)")
        }
    finally:
        snapshot.close()
    assert kept.read_text() == "do not prune"
    repo.close()


def test_upgrade_snapshot_failure_prevents_any_migration_ddl(tmp_path, monkeypatch, owner_lock_dir):
    path = tmp_path / "v9.sqlite"
    v9_database(path)
    monkeypatch.setattr(
        db_module,
        "upgrade_snapshot",
        lambda *_: (_ for _ in ()).throw(RuntimeError("maintenance upgrade snapshot failed")),
    )

    with pytest.raises(RuntimeError, match="maintenance upgrade snapshot failed"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    conn = sqlite3.connect(path)
    try:
        assert conn.execute("SELECT version FROM schema_version").fetchone()[0] == 9
        assert "reply_style" not in {row[1] for row in conn.execute("PRAGMA table_info(members)")}
    finally:
        conn.close()


def test_intermediate_upgrade_ddl_fault_rolls_back_rows_and_keeps_valid_pre_ddl_image(
    tmp_path, monkeypatch, owner_lock_dir
):
    path = tmp_path / "v13.sqlite"
    v13_database_with_retained_rows(path)
    old_backups = tmp_path / "maintenance-upgrades"
    old_backups.mkdir(mode=0o700)
    kept = old_backups / "keep.sqlite"
    kept.write_text("prior image")
    original_execute_schema = db_module._execute_schema

    def fail_after_intermediate_ddl(conn):
        original_execute_schema(conn)
        raise RuntimeError("injected maintenance DDL fault")

    monkeypatch.setattr(db_module, "_execute_schema", fail_after_intermediate_ddl)
    with pytest.raises(RuntimeError, match="injected maintenance DDL fault"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    check = sqlite3.connect(path)
    try:
        assert check.execute("SELECT version FROM schema_version").fetchone()[0] == 13
        assert (
            check.execute("SELECT display_name FROM members WHERE user_id = '7'").fetchone()[0]
            == "retained member"
        )
        assert check.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
        assert (
            check.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'maintenance_state'"
            ).fetchone()[0]
            == 0
        )
    finally:
        check.close()

    images = [image for image in old_backups.iterdir() if image.name != kept.name]
    assert len(images) == 1
    assert images[0].name.startswith(f"{path.name}.pre-upgrade-")
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 13
        assert (
            snapshot.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' "
                "AND name = 'adoption_sources'"
            ).fetchone()[0]
            == 0
        )
        assert snapshot.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
        assert (
            snapshot.execute("SELECT display_name FROM members WHERE user_id = '7'").fetchone()[0]
            == "retained member"
        )
    finally:
        snapshot.close()
    assert kept.read_text() == "prior image"


@pytest.mark.parametrize("version", [9, 10, 11, 12, 13])
def test_each_supported_old_version_is_snapshotted_before_v16(tmp_path, version, owner_lock_dir):
    path = tmp_path / f"v{version}.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);"
        f"INSERT INTO schema_version VALUES ({version});"
    )
    conn.close()

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    image = next((tmp_path / "maintenance-upgrades").iterdir())
    copied = sqlite3.connect(image)
    try:
        assert copied.execute("SELECT version FROM schema_version").fetchone()[0] == version
    finally:
        copied.close()
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    repo.close()


@pytest.mark.parametrize("version", [9, 10, 11, 12, 13])
def test_each_supported_old_version_stays_unmodified_when_snapshot_fails(
    tmp_path, monkeypatch, version, owner_lock_dir
):
    path = tmp_path / f"v{version}.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);"
        f"INSERT INTO schema_version VALUES ({version});"
    )
    conn.close()
    monkeypatch.setattr(
        db_module,
        "upgrade_snapshot",
        lambda *_: (_ for _ in ()).throw(RuntimeError("maintenance upgrade snapshot failed")),
    )

    with pytest.raises(RuntimeError, match="maintenance upgrade snapshot failed"):
        Repo(path, owner_lock_dir=owner_lock_dir)
    check = sqlite3.connect(path)
    try:
        assert check.execute("SELECT version FROM schema_version").fetchone()[0] == version
        assert (
            check.execute(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'maintenance_state'"
            ).fetchone()[0]
            == 0
        )
    finally:
        check.close()


def test_reply_style_survives_reopening(tmp_path, owner_lock_dir):
    path = tmp_path / "fresh.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    repo.upsert_member(7, "harbour4417", "MY", True)
    repo.set_reply_style(7, "concise")
    repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    assert reopened.get_reply_style(7) == "concise"
    reopened.close()


def test_v9_to_current_is_idempotent(tmp_path, owner_lock_dir):
    path = tmp_path / "v9.sqlite"
    v9_database(path)
    Repo(path, owner_lock_dir=owner_lock_dir).close()
    Repo(path, owner_lock_dir=owner_lock_dir).close()

    conn = sqlite3.connect(path)
    columns = [row[1] for row in conn.execute("PRAGMA table_info(members)")]
    assert columns.count("reply_style") == 1
    conn.close()


def test_pre_v9_database_is_refused_with_upgrade_direction(tmp_path, owner_lock_dir):
    path = tmp_path / "old.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);"
        "INSERT INTO schema_version VALUES (8);"
    )
    conn.close()

    with pytest.raises(RuntimeError, match="supports upgrades from v9 only"):
        Repo(path, owner_lock_dir=owner_lock_dir)


def test_unversioned_existing_database_is_not_mislabeled_v13(tmp_path, owner_lock_dir):
    path = tmp_path / "unversioned.sqlite"
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE members (user_id TEXT PRIMARY KEY)")
    conn.close()

    with pytest.raises(RuntimeError, match="has no schema version"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    conn = sqlite3.connect(path)
    columns = [row[1] for row in conn.execute("PRAGMA table_info(members)")]
    assert columns == ["user_id"]
    assert (
        conn.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'"
        ).fetchone()[0]
        == 0
    )
    conn.close()


def test_a_database_from_a_newer_bot_is_refused(tmp_path, owner_lock_dir):
    path = tmp_path / "future.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    repo._conn.execute("UPDATE schema_version SET version = ?", (SCHEMA_VERSION + 1,))
    repo.close()
    with pytest.raises(RuntimeError, match="newer bot"):
        Repo(path, owner_lock_dir=owner_lock_dir)


def v10_database_with_star_tokens(path) -> None:
    """A v10 database holding the old `NStar`/`HStar` tokens, with full tables."""
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (10);
        CREATE TABLE runs (
            id TEXT PRIMARY KEY,
            fixed_run_id TEXT,
            channel_id TEXT,
            week_start TEXT NOT NULL,
            bosses TEXT NOT NULL,
            datetime TEXT NOT NULL,
            participants TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'planned',
            source TEXT NOT NULL DEFAULT 'fixed',
            created_at TEXT NOT NULL
        );
        INSERT INTO runs VALUES (
            'r1', NULL, '900', '2026-08-27T00:00:00+08:00', '["HStar", "HFA"]',
            '2026-08-31T21:30:00+08:00', '["1"]', 'planned', 'fixed',
            '2026-08-27T00:00:00+08:00'
        );
        INSERT INTO runs VALUES (
            'r2', NULL, '900', '2026-08-27T00:00:00+08:00', '["XKalos"]',
            '2026-09-01T23:00:00+08:00', '["1"]', 'planned', 'fixed',
            '2026-08-27T00:00:00+08:00'
        );
        CREATE TABLE fixed_runs (
            id TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL,
            channel_id TEXT,
            bosses TEXT NOT NULL,
            weekday INTEGER NOT NULL,
            time TEXT NOT NULL,
            participants TEXT NOT NULL,
            note TEXT,
            created_at TEXT NOT NULL
        );
        INSERT INTO fixed_runs VALUES (
            'f1', '1', '900', '["NStar"]', 0, '21:30', '["1"]', NULL,
            '2026-08-27T00:00:00+08:00'
        );
        CREATE TABLE amendments (
            id TEXT PRIMARY KEY,
            week_start TEXT NOT NULL,
            kind TEXT NOT NULL,
            bosses TEXT NOT NULL DEFAULT '[]',
            run_id TEXT,
            new_datetime TEXT,
            participants TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'proposed',
            confidence REAL,
            evidence_msg_ids TEXT NOT NULL DEFAULT '[]',
            proposal_message_id TEXT,
            created_at TEXT NOT NULL,
            channel_id TEXT,
            is_question INTEGER NOT NULL DEFAULT 0,
            rsvp TEXT,
            day_ref TEXT,
            time_ref TEXT,
            summary TEXT,
            payload TEXT NOT NULL DEFAULT '{}'
        );
        INSERT INTO amendments VALUES (
            'a1', '2026-08-27T00:00:00+08:00', 'move', '["HStar"]', 'r1', NULL,
            '[]', 'proposed', 0.9, '[]', NULL, '2026-08-30T00:00:00+08:00',
            '900', 0, NULL, NULL, NULL, NULL, '{"bosses": ["HStar", "HFA"]}'
        );
        INSERT INTO amendments VALUES (
            'a2', '2026-08-27T00:00:00+08:00', 'cancel', '["XKalos"]', 'r2', NULL,
            '[]', 'proposed', 0.9, '[]', NULL, '2026-08-30T00:00:00+08:00',
            '900', 0, NULL, NULL, NULL, NULL, '{}'
        );
        """
    )
    conn.commit()
    conn.close()


def test_v10_rewrites_stored_star_tokens_to_maleficstar(tmp_path, owner_lock_dir):
    """Stored `NStar`/`HStar` become `NMaleficStar`/`HMaleficStar`; the rest is untouched."""
    import json

    path = tmp_path / "v10.sqlite"
    v10_database_with_star_tokens(path)

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION

    def bosses(table: str, row_id: str) -> list:
        return json.loads(
            repo._conn.execute(f"SELECT bosses FROM {table} WHERE id = ?", (row_id,)).fetchone()[0]
        )

    assert bosses("runs", "r1") == ["HMaleficStar", "HFA"]
    assert bosses("runs", "r2") == ["XKalos"]
    assert bosses("fixed_runs", "f1") == ["NMaleficStar"]
    assert bosses("amendments", "a1") == ["HMaleficStar"]
    assert json.loads(
        repo._conn.execute("SELECT payload FROM amendments WHERE id = 'a1'").fetchone()[0]
    ) == {"bosses": ["HMaleficStar", "HFA"]}
    assert bosses("amendments", "a2") == ["XKalos"]
    repo.close()


def test_v10_star_rewrite_is_idempotent(tmp_path, owner_lock_dir):
    path = tmp_path / "v10.sqlite"
    v10_database_with_star_tokens(path)
    Repo(path, owner_lock_dir=owner_lock_dir).close()
    Repo(path, owner_lock_dir=owner_lock_dir).close()

    conn = sqlite3.connect(path)
    assert conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    assert conn.execute("SELECT bosses FROM runs WHERE id = 'r1'").fetchone()[0] == (
        '["HMaleficStar", "HFA"]'
    )
    conn.close()


def test_v11_migrates_to_v16_without_memory_storage(tmp_path, owner_lock_dir):
    path = tmp_path / "v11.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (11);
        CREATE TABLE members (user_id TEXT PRIMARY KEY, display_name TEXT NOT NULL);
        INSERT INTO members VALUES ('7', 'legacy member');
        CREATE TABLE messages (id TEXT PRIMARY KEY, content TEXT NOT NULL);
        INSERT INTO messages VALUES ('message-1', 'remember everything');
        """
    )
    conn.close()

    repo = Repo(path, owner_lock_dir=owner_lock_dir)

    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    assert memory_objects(repo._conn) == []
    assert repo._conn.execute("SELECT content FROM messages WHERE id = 'message-1'").fetchone()[
        0
    ] == ("remember everything")
    repo.close()


def test_pre_card_v12_memory_table_is_dropped_on_upgrade(tmp_path, owner_lock_dir):
    path = tmp_path / "pre-card-v12.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (12);
        CREATE TABLE chat_memories (
            id TEXT PRIMARY KEY, guild_id TEXT NOT NULL, user_id TEXT NOT NULL,
            slot TEXT NOT NULL, value TEXT NOT NULL, boss_token TEXT, state TEXT NOT NULL,
            source_message_id TEXT, source_channel_id TEXT, proposer_id TEXT, reviewer_id TEXT,
            created_at TEXT NOT NULL, reviewed_at TEXT, expires_at TEXT NOT NULL,
            state_at TEXT NOT NULL
        );
        INSERT INTO chat_memories VALUES
            ('kept', 'g', 'u', 'answer_detail', 'concise', NULL, 'proposed', NULL, NULL,
             NULL, NULL, 'now', NULL, 'later', 'now');
        """
    )
    conn.close()

    repo = Repo(path, owner_lock_dir=owner_lock_dir)

    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == SCHEMA_VERSION
    assert repo._conn.execute("SELECT COUNT(*) FROM weekly_digests").fetchone()[0] == 0
    assert memory_objects(repo._conn) == []
    repo.close()


@pytest.mark.parametrize(
    ("mode", "adoption_state"),
    [("OPEN", "complete"), ("FROZEN", "complete"), ("BLOCKED", "pending")],
)
def test_v15_upgrade_drops_memory_and_preserves_maintenance_and_journal(
    tmp_path, owner_lock_dir, mode, adoption_state
):
    path = tmp_path / "v15.sqlite"
    v14_database_with_retained_rows(
        path,
        mode=mode,
        adoption_state=adoption_state,
        version=15,
        live_lease=False,
        memory_claim=True,
    )
    backups = tmp_path / "maintenance-upgrades"
    backups.mkdir(mode=0o700)
    previous = backups / "keep.sqlite"
    previous.write_text("preserve")

    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 16
        assert memory_objects(repo._conn) == []
        assert tuple(
            repo._conn.execute(
                "SELECT mode, adoption_state, adoption_id, state_revision FROM maintenance_state"
            ).fetchone()
        ) == (
            mode,
            adoption_state,
            "existing-pending" if adoption_state == "pending" else "old-complete",
            # Only the existing pending-adoption restart rule advances the revision.
            5 if adoption_state == "pending" else 4,
        )
        assert repo._conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
        assert tuple(
            repo._conn.execute(
                "SELECT a.state, t.binding_type, t.released_at FROM delivery_attempts a "
                "JOIN delivery_attempt_targets t USING (attempt_id) "
                "WHERE attempt_id = 'attempt-memory-1'"
            ).fetchone()
        ) == ("bound", "memory_proposal", None)
        assert tuple(repo._conn.execute("SELECT id, message_id FROM reminders").fetchone()) == (
            "reminder-legacy-1",
            "message-legacy-1",
        )
    finally:
        repo.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert reopened._conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
        assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == mode
    finally:
        reopened.close()

    images = [image for image in backups.iterdir() if image.name != previous.name]
    assert len(images) == 1
    assert images[0].name.startswith(f"{path.name}.pre-upgrade-")
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 15
        assert LEGACY_MEMORY_OBJECTS <= set(memory_objects(snapshot))
        assert snapshot.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
    finally:
        snapshot.close()
    assert previous.read_text() == "preserve"


def test_v15_drop_fault_rolls_back_and_keeps_pre_upgrade_image(
    tmp_path, monkeypatch, owner_lock_dir
):
    path = tmp_path / "v15-fault.sqlite"
    v14_database_with_retained_rows(path, version=15, live_lease=False)
    real_drop = db_module._drop_removed_memory_storage

    def fail_after_drop(conn):
        real_drop(conn)
        raise RuntimeError("injected v16 drop fault")

    monkeypatch.setattr(db_module, "_drop_removed_memory_storage", fail_after_drop)
    with pytest.raises(RuntimeError, match="injected v16 drop fault"):
        Repo(path, owner_lock_dir=owner_lock_dir)

    check = sqlite3.connect(path)
    try:
        assert check.execute("SELECT version FROM schema_version").fetchone()[0] == 15
        assert LEGACY_MEMORY_OBJECTS <= set(memory_objects(check))
        assert check.execute("SELECT COUNT(*) FROM chat_memory_events").fetchone()[0] == 1
        assert tuple(
            check.execute(
                "SELECT mode, adoption_state, state_revision FROM maintenance_state"
            ).fetchone()
        ) == ("OPEN", "complete", 4)
    finally:
        check.close()
    images = list((tmp_path / "maintenance-upgrades").iterdir())
    assert len(images) == 1
    snapshot = sqlite3.connect(images[0])
    try:
        assert snapshot.execute("SELECT version FROM schema_version").fetchone()[0] == 15
        assert snapshot.execute("SELECT COUNT(*) FROM chat_memories").fetchone()[0] == 1
    finally:
        snapshot.close()
