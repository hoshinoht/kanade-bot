"""Direct synthetic-connection checks for the unpublished v15 maintenance DDL."""

from __future__ import annotations

import sqlite3

import pytest

import bot.infrastructure.db as db_module


@pytest.fixture
def schema_connection():
    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    db_module._execute_schema(conn)
    try:
        yield conn
    finally:
        conn.close()


def insert_attempt(conn: sqlite3.Connection, **overrides: object) -> None:
    values: dict[str, object] = {
        "attempt_id": "attempt-1",
        "operation_id": "operation-1",
        "effect_ordinal": 0,
        "owner_instance_id": "instance-1",
        "origin": "runtime",
        "effect_kind": "reminder",
        "dedupe_scope": "native",
        "dedupe_key": "a" * 64,
        "dedupe_active": 1,
        "state": "intent",
        "destination_kind": "channel",
        "guild_id": "guild",
        "channel_id": "channel",
        "recipient_id": None,
        "message_id": None,
        "fingerprint_version": 1,
        "request_fingerprint": "b" * 64,
        "observable_fingerprint": None,
        "intended_at": "2026-09-22T00:00:00+00:00",
        "resolved_at": None,
        "resolved_by": None,
        "resolution_reason": "",
    }
    values.update(overrides)
    columns = ", ".join(values)
    placeholders = ", ".join("?" for _ in values)
    conn.execute(
        f"INSERT INTO delivery_attempts ({columns}) VALUES ({placeholders})",
        tuple(values.values()),
    )


def insert_adoption_source(conn: sqlite3.Connection, **overrides: object) -> None:
    values: dict[str, object] = {
        "adoption_id": "adoption-1",
        "source_family": "reminders",
        "source_primary": "malformed historic id",
        "source_secondary": "",
        "reason": "candidate_message",
        "resolution_state": "pending",
        "captured_at": "2026-09-22T00:00:00+00:00",
        "classified_at": "2026-09-22T00:01:00+00:00",
        "evidence_hash_version": 1,
        "evidence_hash": "c" * 64,
        "legacy_channel_id": "historic channel token",
        "legacy_message_id": "historic message token",
        "adopted_attempt_id": None,
        "resolution_actor": None,
        "resolution_at": None,
        "resolution_reason": None,
    }
    values.update(overrides)
    columns = ", ".join(values)
    placeholders = ", ".join("?" for _ in values)
    conn.execute(
        f"INSERT INTO adoption_sources ({columns}) VALUES ({placeholders})",
        tuple(values.values()),
    )


def test_adoption_source_keys_vocabularies_timestamps_and_hash_are_constrained(schema_connection):
    insert_adoption_source(schema_connection)
    stored = schema_connection.execute(
        "SELECT source_primary, source_secondary, legacy_channel_id, legacy_message_id "
        "FROM adoption_sources"
    ).fetchone()
    assert tuple(stored) == (
        "malformed historic id",
        "",
        "historic channel token",
        "historic message token",
    )
    with pytest.raises(sqlite3.IntegrityError):
        insert_adoption_source(schema_connection)
    invalid_rows = (
        {"source_family": "unknown_family"},
        {"reason": "unbounded_reason"},
        {"resolution_state": "unknown_state"},
        {"source_secondary": None},
        {"evidence_hash_version": 2},
        {"evidence_hash": "C" * 64},
        {"evidence_hash": "d" * 63},
        {"captured_at": "2026-09-22T00:00:00"},
        {"classified_at": "2026-09-22T00:00:00"},
    )
    for index, overrides in enumerate(invalid_rows):
        with pytest.raises(sqlite3.IntegrityError):
            insert_adoption_source(
                schema_connection,
                source_primary=f"invalid-{index}",
                **overrides,
            )


def test_adoption_source_verified_bound_requires_a_bound_adoption_attempt(schema_connection):
    with pytest.raises(sqlite3.IntegrityError):
        insert_adoption_source(
            schema_connection,
            source_primary="pending-with-resolution",
            resolution_actor="operator",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_adoption_source(
            schema_connection,
            source_primary="resolved-without-evidence",
            resolution_state="proven_unsent",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_adoption_source(
            schema_connection,
            source_primary="unsent-with-attempt",
            reason="future_unsent",
            resolution_state="proven_unsent",
            adopted_attempt_id="missing-attempt",
            resolution_actor="operator",
            resolution_at="2026-09-22T00:02:00+00:00",
            resolution_reason="verified not sent",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_adoption_source(
            schema_connection,
            source_primary="bound-without-attempt",
            resolution_state="verified_bound",
            resolution_actor="operator",
            resolution_at="2026-09-22T00:02:00+00:00",
            resolution_reason="actual message verified",
        )

    invalid_attempts = (
        ("runtime-bound", "runtime", "bound", {"message_id": "runtime-message"}),
        ("adoption-intent", "adoption", "intent", {}),
        ("adoption-indeterminate", "adoption", "indeterminate", {}),
        (
            "adoption-retired",
            "adoption",
            "retired",
            {
                "dedupe_active": 0,
                "resolved_at": "2026-09-22T00:02:00+00:00",
                "resolved_by": "operator",
                "resolution_reason": "source was retired",
            },
        ),
    )
    for index, (attempt_id, origin, state, extra) in enumerate(invalid_attempts):
        insert_attempt(
            schema_connection,
            attempt_id=attempt_id,
            operation_id=f"operation-{attempt_id}",
            dedupe_key="def0"[index] * 64,
            origin=origin,
            state=state,
            **extra,
        )
        resolution = {
            "resolution_state": "verified_bound",
            "adopted_attempt_id": attempt_id,
            "resolution_actor": "operator",
            "resolution_at": "2026-09-22T00:02:00+00:00",
            "resolution_reason": "actual message verified",
        }
        with pytest.raises(sqlite3.IntegrityError):
            insert_adoption_source(
                schema_connection,
                source_primary=f"insert-{attempt_id}",
                **resolution,
            )
        source_primary = f"update-{attempt_id}"
        insert_adoption_source(schema_connection, source_primary=source_primary)
        with pytest.raises(sqlite3.IntegrityError):
            schema_connection.execute(
                "UPDATE adoption_sources SET resolution_state = 'verified_bound', "
                "adopted_attempt_id = ?, resolution_actor = ?, resolution_at = ?, "
                "resolution_reason = ? WHERE source_primary = ?",
                (
                    attempt_id,
                    "operator",
                    "2026-09-22T00:02:00+00:00",
                    "actual message verified",
                    source_primary,
                ),
            )
        assert (
            schema_connection.execute(
                "SELECT resolution_state FROM adoption_sources WHERE source_primary = ?",
                (source_primary,),
            ).fetchone()[0]
            == "pending"
        )

    insert_attempt(
        schema_connection,
        attempt_id="adopted-attempt",
        operation_id="adopted-operation",
        dedupe_key="9" * 64,
        origin="adoption",
        state="bound",
        message_id="actual-message",
    )
    insert_adoption_source(schema_connection, source_primary="verified-source")
    schema_connection.execute(
        "UPDATE adoption_sources SET resolution_state = 'verified_bound', "
        "adopted_attempt_id = 'adopted-attempt', resolution_actor = 'operator', "
        "resolution_at = '2026-09-22T00:02:00+00:00', "
        "resolution_reason = 'actual message verified' WHERE source_primary = 'verified-source'"
    )
    for index, state in enumerate(("proven_unsent", "terminal_no_replay", "reasoned_retired")):
        insert_adoption_source(
            schema_connection,
            source_primary=f"resolved-{index}",
            reason="future_unsent" if state == "proven_unsent" else "terminal_unbound",
            resolution_state=state,
            resolution_actor="operator",
            resolution_at="2026-09-22T00:02:00+00:00",
            resolution_reason="source evidence reviewed",
        )
    assert schema_connection.execute("SELECT COUNT(*) FROM adoption_sources").fetchone()[0] == 8


def test_adoption_source_table_and_indexes_are_guarded_while_open():
    from bot.infrastructure.db import Repo

    repo = Repo(":memory:")
    try:
        for sql in (
            "INSERT INTO adoption_sources "
            "(adoption_id, source_family, source_primary, source_secondary, reason, "
            "resolution_state, captured_at, classified_at, evidence_hash_version, evidence_hash) "
            "VALUES ('a', 'reminders', 'r', '', 'candidate_message', 'pending', ?, ?, 1, ?)",
            "CREATE INDEX untrusted_adoption_index ON adoption_sources(source_primary)",
            "DROP INDEX adoption_sources_pending",
            "DROP TRIGGER adoption_sources_verified_attempt_insert",
        ):
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(
                    sql,
                    ("2026-09-22T00:00:00+00:00", "2026-09-22T00:01:00+00:00", "c" * 64)
                    if sql.startswith("INSERT")
                    else (),
                )
    finally:
        repo.close()


def test_attempt_hashes_are_exact_lowercase_sha256_and_state_controls_dedupe(
    schema_connection,
):
    insert_attempt(schema_connection)
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="bad-dedupe",
            operation_id="bad-dedupe",
            dedupe_key="a" * 63 + "G",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="bad-request",
            operation_id="bad-request",
            dedupe_key="c" * 64,
            request_fingerprint="b" * 63 + "z",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="bad-observable",
            operation_id="bad-observable",
            dedupe_key="d" * 64,
            observable_fingerprint="e" * 63 + "Z",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="inactive-intent",
            operation_id="inactive-intent",
            dedupe_key="f" * 64,
            dedupe_active=0,
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="active-retired",
            operation_id="active-retired",
            dedupe_key="0" * 64,
            state="retired",
            dedupe_active=1,
            resolved_at="2026-09-22T00:00:00+00:00",
            resolved_by="operator",
            resolution_reason="retired after review",
        )
    insert_attempt(
        schema_connection,
        attempt_id="retired",
        operation_id="retired",
        dedupe_key="1" * 64,
        state="retired",
        dedupe_active=0,
        resolved_at="2026-09-22T00:00:00+00:00",
        resolved_by="operator",
        resolution_reason="retired after review",
    )


def test_bound_attempt_requires_native_channel_or_dm_identity(schema_connection):
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="bound-no-message",
            operation_id="bound-no-message",
            dedupe_key="2" * 64,
            state="bound",
            channel_id="channel",
        )
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="bound-dm-no-recipient",
            operation_id="bound-dm-no-recipient",
            dedupe_key="3" * 64,
            state="bound",
            destination_kind="dm",
            channel_id=None,
            recipient_id=None,
            message_id="message",
        )
    insert_attempt(
        schema_connection,
        attempt_id="bound-channel",
        operation_id="bound-channel",
        dedupe_key="4" * 64,
        state="bound",
        message_id="message",
    )
    insert_attempt(
        schema_connection,
        attempt_id="bound-dm",
        operation_id="bound-dm",
        dedupe_key="5" * 64,
        state="bound",
        destination_kind="dm",
        channel_id=None,
        recipient_id="recipient",
        message_id="message-dm",
    )


def test_attempt_retirement_and_target_release_require_bounded_evidence(schema_connection):
    with pytest.raises(sqlite3.IntegrityError):
        insert_attempt(
            schema_connection,
            attempt_id="retired-incomplete",
            operation_id="retired-incomplete",
            dedupe_key="6" * 64,
            state="retired",
            dedupe_active=0,
        )
    insert_attempt(schema_connection)
    with pytest.raises(sqlite3.IntegrityError):
        schema_connection.execute(
            "INSERT INTO delivery_attempt_targets "
            "(attempt_id, target_ordinal, binding_type, key_primary, released_at, release_actor, "
            "release_reason) "
            "VALUES ('attempt-1', 0, 'reminder', 'run-1', ?, NULL, 'reason')",
            ("2026-09-22T00:00:00+00:00",),
        )
    with pytest.raises(sqlite3.IntegrityError):
        schema_connection.execute(
            "INSERT INTO delivery_attempt_targets "
            "(attempt_id, target_ordinal, binding_type, key_primary, released_at, release_actor, "
            "release_reason) "
            "VALUES ('attempt-1', 0, 'reminder', 'run-1', ?, 'operator', '')",
            ("2026-09-22T00:00:00+00:00",),
        )
    schema_connection.execute(
        "INSERT INTO delivery_attempt_targets "
        "(attempt_id, target_ordinal, binding_type, key_primary, released_at, release_actor, "
        "release_reason) "
        "VALUES ('attempt-1', 0, 'reminder', 'run-1', ?, 'operator', 'released after review')",
        ("2026-09-22T00:00:00+00:00",),
    )


def test_lease_orphan_and_retirement_timestamps_are_required(schema_connection):
    with pytest.raises(sqlite3.IntegrityError):
        schema_connection.execute(
            "INSERT INTO maintenance_leases "
            "(operation_id, instance_id, owner_token_hash, generation, operation_kind, started_at, "
            "owner_task_id, lifecycle) "
            "VALUES ('orphan', 'instance', ?, 0, 'work', ?, 1, 'orphaned')",
            ("a" * 64, "2026-09-22T00:00:00+00:00"),
        )
    with pytest.raises(sqlite3.IntegrityError):
        schema_connection.execute(
            "INSERT INTO maintenance_leases "
            "(operation_id, instance_id, owner_token_hash, generation, operation_kind, started_at, "
            "owner_task_id, lifecycle, retired_at, retired_by, retirement_reason) "
            "VALUES ('retired', 'instance', ?, 0, 'work', ?, 1, 'retired', ?, ' ', ' ') ",
            ("a" * 64, "2026-09-22T00:00:00+00:00", "2026-09-22T00:00:00+00:00"),
        )
    schema_connection.execute(
        "INSERT INTO maintenance_leases "
        "(operation_id, instance_id, owner_token_hash, generation, operation_kind, started_at, "
        "owner_task_id, lifecycle, orphaned_at, retired_at, retired_by, retirement_reason) "
        "VALUES ('retired', 'instance', ?, 0, 'work', ?, 1, 'retired', ?, ?, "
        "'operator', 'reviewed')",
        (
            "a" * 64,
            "2026-09-22T00:00:00+00:00",
            "2026-09-22T00:00:00+00:00",
            "2026-09-22T00:00:01+00:00",
        ),
    )


def test_nullable_maintenance_hashes_validate_when_present(schema_connection):
    with pytest.raises(sqlite3.IntegrityError):
        schema_connection.execute(
            "INSERT INTO maintenance_state (id, mode, adoption_state, week_fingerprint) "
            "VALUES (1, 'OPEN', 'complete', 'not-a-sha256')"
        )
    schema_connection.execute(
        "INSERT INTO maintenance_state "
        "(id, mode, adoption_state, week_fingerprint, reset_fingerprint, roster_hash, "
        "reactions_hash) "
        "VALUES (1, 'OPEN', 'complete', ?, ?, ?, ?)",
        ("a" * 64, "b" * 64, "c" * 64, "d" * 64),
    )


@pytest.mark.parametrize("mode", ["OPEN", "FROZEN"])
def test_reopened_v16_enforces_foreign_keys_without_reapplying_schema(
    tmp_path, monkeypatch, owner_lock_dir, mode
):
    from bot.infrastructure.db import Repo

    path = tmp_path / "reopened.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    repo.close()
    if mode == "FROZEN":
        with sqlite3.connect(path) as seed:
            seed.execute("UPDATE maintenance_state SET mode = 'FROZEN'")

    monkeypatch.setattr(
        db_module,
        "_execute_schema",
        lambda *_: pytest.fail("schema DDL reran on v16 reopen"),
    )
    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        assert reopened._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 16
        assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == mode
        assert reopened._conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
        assert any(
            row[2] == "delivery_attempts"
            for row in reopened._conn.execute("PRAGMA foreign_key_list(delivery_attempt_targets)")
        )
        assert any(
            row[2] == "delivery_attempts"
            for row in reopened._conn.execute("PRAGMA foreign_key_list(adoption_sources)")
        )
    finally:
        reopened.close()
