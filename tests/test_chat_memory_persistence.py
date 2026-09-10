"""Governed-memory persistence is typed, scoped, and transactionally lifecycle-bound."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta

import pytest

from bot.infrastructure.db import Repo


@pytest.fixture
def repo():
    value = Repo(":memory:")
    yield value
    value.close()


def now(day: int = 1) -> datetime:
    return datetime(2026, 9, day, tzinfo=UTC)


def activate(repo: Repo, guild: str = "g", user: str = "u") -> None:
    assert repo.begin_memory_enrollment(guild, user, "admin", "v1", now=now())
    assert repo.record_memory_notice_attempt(guild, user, "admin", now=now())
    assert repo.activate_memory_enrollment(guild, user, "admin", "notice", now=now())


def test_pending_notice_is_ineligible_and_notice_activation_is_cas(repo):
    assert repo.begin_memory_enrollment("g", "u", "admin", "v1", now=now())
    assert repo.create_memory_proposal("g", "u", "answer_detail", "concise", now=now()) is None
    assert repo.record_memory_notice_attempt("g", "u", "admin", now=now())
    assert repo.activate_memory_enrollment("g", "u", "admin", "notice", now=now())
    assert not repo.activate_memory_enrollment("g", "u", "admin", "notice", now=now())


def test_approval_supersedes_one_scope_and_opt_out_never_reactivates_old_rows(repo):
    activate(repo)
    first = repo.create_memory_proposal(
        "g", "u", "answer_detail", "concise", proposer_id="u", now=now()
    )
    assert first and repo.approve_memory_proposal("g", "u", first, "u", now=now())
    second = repo.create_memory_proposal(
        "g", "u", "answer_detail", "detailed", proposer_id="u", now=now()
    )
    assert second and repo.approve_memory_proposal("g", "u", second, "u", now=now())
    assert repo.get_memory("g", "u", first)["state"] == "superseded"
    assert [row["id"] for row in repo.retrieve_memories("g", "u", now=now())] == [second]
    repo.opt_out_memory("g", "u", "u", now=now())
    assert repo.retrieve_memories("g", "u", now=now()) == []
    assert repo.begin_memory_enrollment("g", "u", "admin", "v2", now=now())
    assert repo.record_memory_notice_attempt("g", "u", "admin", now=now())
    assert repo.activate_memory_enrollment("g", "u", "admin", "notice-2", now=now())
    assert repo.retrieve_memories("g", "u", now=now()) == []


def test_deletion_is_available_without_enrollment_and_events_never_hold_values(repo):
    activate(repo)
    memory_id = repo.replace_memory("g", "u", "answer_format", "bullets", "admin", now=now())
    assert memory_id and repo.delete_memory("g", "u", memory_id, "u", now=now())
    assert repo.get_memory("g", "u", memory_id) is None
    assert repo.forget_all_memories("g", "u", "u", now=now()) == 0
    columns = {row[1] for row in repo._conn.execute("PRAGMA table_info(chat_memory_events)")}
    assert {"slot", "value", "detail", "content", "reason_text"}.isdisjoint(columns)


def test_retrieval_is_exactly_scoped_and_expiry_cleanup_is_bounded(repo):
    activate(repo)
    memory_id = repo.replace_memory("g", "u", "answer_format", "bullets", "admin", now=now())
    assert memory_id
    assert repo.retrieve_memories("g", "other", now=now()) == []
    assert repo.retrieve_memories("other", "u", now=now()) == []
    assert repo.retrieve_memories("g", "u", now=datetime(2026, 9, 1, tzinfo=UTC))
    result = repo.cleanup_memories(now=now() + timedelta(days=181))
    assert result["expired"] == 1
    assert repo.retrieve_memories("g", "u", now=now() + timedelta(days=181)) == []
    assert repo.cleanup_memories(now=now() + timedelta(days=212))["purged"] == 1


def test_naive_memory_times_fail_closed(repo):
    with pytest.raises(ValueError, match="timezone-aware"):
        repo.begin_memory_enrollment("g", "u", "admin", "v1", now=datetime(2026, 9, 1))


def test_retrieval_diagnostics_and_events_follow_their_retention_contracts(repo):
    for index in range(501):
        repo.log_memory_retrieval("g", "u", [], "no_match", now=now() + timedelta(seconds=index))
    assert len(repo.list_memory_retrievals("g", "u")) == 500
    repo.begin_memory_enrollment("g", "u", "admin", "v1", now=now())
    assert repo.list_memory_events("g", "u")
    repo.cleanup_memories(now=now() + timedelta(days=366))
    assert repo.list_memory_events("g", "u") == []
    assert repo.list_memory_retrievals("g", "u") == []


@pytest.mark.parametrize("bad_expiry", ["not-a-time", "2026-09-08T00:00:00"])
def test_malformed_or_naive_expiry_is_ineligible_for_retrieval_and_review(repo, bad_expiry):
    activate(repo)
    active = repo.replace_memory("g", "u", "answer_format", "bullets", "admin", now=now())
    proposal = repo.create_memory_proposal("g", "u", "answer_detail", "concise", now=now())
    assert active and proposal
    repo._conn.execute("UPDATE chat_memories SET expires_at = ? WHERE id = ?", (bad_expiry, active))
    repo._conn.execute(
        "UPDATE chat_memories SET expires_at = ? WHERE id = ?", (bad_expiry, proposal)
    )
    assert repo.retrieve_memories("g", "u", now=now()) == []
    assert not repo.approve_memory_proposal("g", "u", proposal, "u", now=now())
    assert not repo.reject_memory_proposal("g", "u", proposal, "u", now=now())
    assert repo.cleanup_memories(now=now())["expired"] == 2


def test_nested_memory_transactions_commit_or_rollback_at_their_boundary(repo):
    with repo._memory_transaction():
        assert repo.begin_memory_enrollment("g", "outer", "admin", "v1", now=now())
        assert repo.begin_memory_enrollment("g", "inner", "admin", "v1", now=now())
    assert repo.get_memory_enrollment("g", "outer")
    assert repo.get_memory_enrollment("g", "inner")

    with repo._memory_transaction():
        assert repo.begin_memory_enrollment("g", "kept", "admin", "v1", now=now())
        with pytest.raises(RuntimeError):
            with repo._memory_transaction():
                assert repo.begin_memory_enrollment("g", "discarded", "admin", "v1", now=now())
                raise RuntimeError("inner failure")
    assert repo.get_memory_enrollment("g", "kept")
    assert repo.get_memory_enrollment("g", "discarded") is None

    with pytest.raises(RuntimeError):
        with repo._memory_transaction():
            assert repo.begin_memory_enrollment("g", "rolled-back", "admin", "v1", now=now())
            assert repo.begin_memory_enrollment("g", "also-rolled-back", "admin", "v1", now=now())
            raise RuntimeError("outer failure")
    assert repo.get_memory_enrollment("g", "rolled-back") is None
    assert repo.get_memory_enrollment("g", "also-rolled-back") is None


def test_boss_scopes_require_a_catalog_and_store_its_canonical_token(repo, bosses):
    activate(repo)
    with pytest.raises(ValueError, match="BossTable"):
        repo.create_memory_proposal(
            "g", "u", "strategy_emphasis", "survival", boss_token="hstar", now=now()
        )
    memory_id = repo.create_memory_proposal(
        "g",
        "u",
        "strategy_emphasis",
        "survival",
        boss_token="hstar",
        boss_table=bosses,
        now=now(),
    )
    assert memory_id
    assert repo.get_memory("g", "u", memory_id)["boss_token"] == "HMaleficStar"
    for bad in ("hdoesnotexist", "h\N{SNOWMAN}"):
        with pytest.raises(ValueError, match="boss"):
            repo.replace_memory(
                "g",
                "u",
                "strategy_emphasis",
                "survival",
                "admin",
                boss_token=bad,
                boss_table=bosses,
                now=now(),
            )


def test_retrieval_diagnostics_accept_only_exact_member_memory_uuids(repo):
    activate(repo)
    memory_id = repo.replace_memory("g", "u", "answer_format", "bullets", "admin", now=now())
    activate(repo, user="other")
    other_id = repo.replace_memory("g", "other", "answer_format", "prose", "admin", now=now())
    assert memory_id and other_id
    repo.log_memory_retrieval("g", "u", [memory_id], "matched", now=now())
    for selected in (
        [memory_id, memory_id],
        [other_id],
        ["not prose"],
        ["12345678-1234-4234-8234-123456789abc"],
    ):
        with pytest.raises(ValueError):
            repo.log_memory_retrieval("g", "u", selected, "matched", now=now())
    with pytest.raises(ValueError):
        repo.log_memory_retrieval("g", "u", [memory_id] * 5, "matched", now=now())
