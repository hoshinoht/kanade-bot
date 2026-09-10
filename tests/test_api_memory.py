"""Authenticated, content-free admin contracts for governed chat memory."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta
from types import SimpleNamespace

import pytest

from bot.domain.boss_knowledge import BossKnowledgeBase
from bot.domain.timeutil import to_iso

from .conftest import REPO_ROOT


def _activate(fake_bot, user_id: str = "1001") -> str:
    repo = fake_bot.repo
    guild = fake_bot.settings.guild_id
    assert repo.begin_memory_enrollment(guild, user_id, "admin", "memory-v1")
    assert repo.record_memory_notice_attempt(guild, user_id, "admin")
    assert repo.activate_memory_enrollment(guild, user_id, "admin", "1")
    memory_id = repo.replace_memory(guild, user_id, "answer_format", "bullets", "admin")
    assert memory_id
    return memory_id


def test_memory_detail_is_exact_guild_and_content_free(auth, fake_bot):
    memory_id = _activate(fake_bot)
    fake_bot.repo.upsert_member("4001", "Proposer", None, True)
    fake_bot.repo.upsert_member("5001", "Reviewer", None, True)
    reviewed_at = to_iso(datetime(2026, 1, 2, 3, 4, tzinfo=UTC))
    fake_bot.repo._conn.execute(
        "UPDATE chat_memories SET source_message_id = ?, source_channel_id = ?, proposer_id = ?, "
        "reviewer_id = ?, reviewed_at = ? WHERE id = ?",
        ("3001", "2001", "4001", "5001", reviewed_at, memory_id),
    )
    fake_bot.repo.log_memory_retrieval(
        fake_bot.settings.guild_id, "1001", [memory_id], "matched", latency_ms=7
    )
    foreign_id = _activate(fake_bot, "1002")
    fake_bot.repo._conn.execute(
        "UPDATE chat_memory_enrollments SET guild_id = 'other' WHERE user_id = '1002'"
    )
    fake_bot.repo._conn.execute(
        "UPDATE chat_memories SET guild_id = 'other' WHERE id = ?", (foreign_id,)
    )

    response = auth.get("/api/memory/1001")

    assert response.status_code == 200
    payload = response.json()
    assert payload["user_id"] == "1001"
    memory = payload["memories"][0]
    assert memory["id"] == memory_id
    assert memory["source_message_id"] == "3001"
    assert memory["source_channel_id"] == "2001"
    assert (
        memory["source_message_url"] == "https://discord.com/channels/111111111111111111/2001/3001"
    )
    assert (memory["proposer_name"], memory["proposer_id"]) == ("Proposer", "4001")
    assert (memory["reviewer_name"], memory["reviewer_id"]) == ("Reviewer", "5001")
    assert memory["reviewed_at"] == reviewed_at
    assert not {"source_content", "content", "prompt", "query", "tool_inputs"} & set(memory)
    assert payload["retrievals"][0]["selected_memory_ids"] == [memory_id]
    forbidden = {"value", "prompt", "query", "content", "tool_inputs"}
    for event in payload["events"]:
        assert not forbidden & set(event)
    for retrieval in payload["retrievals"]:
        assert not {"prompt", "query", "content", "tool_inputs"} & set(retrieval)
    assert auth.get("/api/memory/1002").status_code == 404


def test_memory_mutations_require_active_enrollment_and_are_immediate(auth, fake_bot):
    assert (
        auth.put(
            "/api/memory/1001/memories", json={"slot": "answer_format", "value": "bullets"}
        ).status_code
        == 400
    )
    memory_id = _activate(fake_bot)

    revoked = auth.post(f"/api/memory/1001/memories/{memory_id}/revoke")
    assert revoked.status_code == 200
    assert revoked.json()["lifecycle"] == "revoked"
    assert fake_bot.repo.retrieve_memories(fake_bot.settings.guild_id, "1001") == []
    assert auth.delete(f"/api/memory/1001/memories/{memory_id}").json() == {
        "id": memory_id,
        "deleted": True,
    }


def test_memory_enroll_uses_one_member_notification_seam(auth, fake_bot, monkeypatch):
    seen = []

    async def enroll(user_id, actor_id):
        seen.append((user_id, actor_id))
        return SimpleNamespace(state="pending_notice", delivered=False, problem="DM failed")

    monkeypatch.setattr(fake_bot, "enroll_memory_member", enroll, raising=False)
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})

    response = auth.post("/api/memory/1001/enroll")

    assert response.status_code == 200
    assert response.json() == {
        "user_id": "1001",
        "state": "pending_notice",
        "active": False,
        "message": "DM failed",
    }
    assert seen == [("1001", "token")]


@pytest.mark.parametrize("user_id", ["not-a-discord-id", "0", "01", "18446744073709551616"])
def test_memory_subject_ids_are_canonical_before_enrollment_mutates(
    auth, fake_bot, monkeypatch, user_id
):
    async def must_not_enroll(*args):
        raise AssertionError("invalid subject reached the Discord enrollment seam")

    monkeypatch.setattr(fake_bot, "enroll_memory_member", must_not_enroll, raising=False)
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    before = fake_bot.repo._conn.execute("SELECT count(*) FROM chat_memory_enrollments").fetchone()[
        0
    ]
    events = fake_bot.repo._conn.execute("SELECT count(*) FROM chat_memory_events").fetchone()[0]

    response = auth.post(f"/api/memory/{user_id}/enroll")

    assert response.status_code == 400
    assert "snowflake" in response.json()["error"]
    assert (
        fake_bot.repo._conn.execute("SELECT count(*) FROM chat_memory_enrollments").fetchone()[0]
        == before
    )
    assert (
        fake_bot.repo._conn.execute("SELECT count(*) FROM chat_memory_events").fetchone()[0]
        == events
    )


@pytest.mark.parametrize(
    "path,method",
    [
        ("/api/memory", "get"),
        ("/api/memory/1001", "get"),
        ("/api/memory/1001/enroll", "post"),
        ("/api/memory/1001/disable", "post"),
        ("/api/memory/1001/memories", "put"),
        ("/api/memory/1001/memories/id/revoke", "post"),
        ("/api/memory/1001/memories/id/expire", "post"),
        ("/api/memory/1001/memories/id", "delete"),
        ("/api/bosses/hstar/knowledge", "get"),
    ],
)
def test_memory_and_knowledge_routes_require_auth(client, path, method):
    kwargs = {"json": {"slot": "answer_format", "value": "bullets"}} if method == "put" else {}
    response = getattr(client, method)(path, **kwargs)

    assert response.status_code == 401


def test_memory_listing_filters_search_and_pages_deterministically(auth, fake_bot):
    repo, guild = fake_bot.repo, fake_bot.settings.guild_id
    for index in range(1, 23):
        user_id = str(1000 + index)
        repo.upsert_member(user_id, f"Member {index:02d}", None, True)
        assert repo.begin_memory_enrollment(guild, user_id, "admin", "memory-v1")
        assert repo.record_memory_notice_attempt(guild, user_id, "admin")
        assert repo.activate_memory_enrollment(guild, user_id, "admin", "notice")
        slot, value = (
            ("strategy_disclosure", "hints")
            if index == 1
            else (("answer_format", "bullets") if index % 2 else ("answer_detail", "concise"))
        )
        memory_id = repo.replace_memory(guild, user_id, slot, value, "admin")
        assert memory_id
        if index == 1:
            repo._conn.execute(
                "UPDATE chat_memories SET boss_token = ?, expires_at = ? WHERE id = ?",
                ("HMaleficStar", to_iso(datetime.now(UTC) + timedelta(days=1)), memory_id),
            )
        if index == 2:
            assert repo.disable_memory_enrollment(guild, user_id, "admin")

    page = auth.get("/api/memory", params={"page": 2})
    assert page.status_code == 200
    assert page.json()["total"] == 22
    assert page.json()["page"] == 2
    assert len(page.json()["rows"]) == 2
    assert [row["name"] for row in page.json()["rows"]] == ["Member 21", "Member 22"]
    assert not {"source_message_id", "source_channel_id", "proposer_id", "reviewer_id"} & set(
        page.json()["rows"][0]["memories"][0]
    )
    assert auth.get("/api/memory", params={"enrollment": "disabled"}).json()["total"] == 1
    assert (
        auth.get("/api/memory", params={"lifecycle": "active", "slot": "answer_detail"}).json()[
            "total"
        ]
        == 10
    )
    assert auth.get("/api/memory", params={"boss": "hstar"}).json()["total"] == 1
    assert auth.get("/api/memory", params={"expires_before": "2030-01-01"}).json()["total"] == 22
    assert auth.get("/api/memory", params={"q": "Member 01"}).json()["rows"][0]["user_id"] == "1001"


@pytest.mark.parametrize("boss", ["star", "unknown boss", "hkalos"])
def test_memory_boss_filters_require_explicit_supported_difficulty(auth, boss):
    assert auth.get("/api/memory", params={"boss": boss}).status_code == 400


@pytest.mark.parametrize(
    ("slot", "value", "boss"),
    [
        ("answer_detail", "detailed", None),
        ("answer_format", "steps", None),
        ("strategy_disclosure", "hints", "hstar"),
        ("strategy_emphasis", "party_roles", "hstar"),
    ],
)
def test_memory_set_accepts_every_typed_variant(auth, fake_bot, slot, value, boss):
    _activate(fake_bot)
    response = auth.put(
        "/api/memory/1001/memories", json={"slot": slot, "value": value, "boss": boss}
    )

    assert response.status_code == 200
    assert response.json()["boss"] == ("HMaleficStar" if boss else None)


@pytest.mark.parametrize(
    "body",
    [
        {"slot": "answer_format", "value": "detailed"},
        {"slot": "answer_detail", "value": "concise", "boss": "hstar"},
        {"slot": "strategy_disclosure", "value": "free text"},
        {"slot": "answer_format", "value": "bullets", "unexpected": True},
    ],
)
def test_memory_set_rejects_mismatches_and_unknown_fields(auth, body):
    assert auth.put("/api/memory/1001/memories", json=body).status_code == 422


def test_global_switch_only_blocks_enroll_and_edit(auth, fake_bot, monkeypatch):
    memory_id = _activate(fake_bot)
    extra_id = fake_bot.repo.replace_memory(
        fake_bot.settings.guild_id,
        "1001",
        "strategy_disclosure",
        "hints",
        "admin",
        boss_token="HMaleficStar",
        boss_table=fake_bot.bosses,
    )
    assert extra_id
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": False})
    assert auth.post("/api/memory/1002/enroll").status_code == 400
    updated = auth.put(
        "/api/memory/1001/memories", json={"slot": "answer_format", "value": "prose"}
    )
    assert updated.status_code == 200
    assert auth.post(f"/api/memory/1001/memories/{updated.json()['id']}/revoke").status_code == 200
    assert auth.post(f"/api/memory/1001/memories/{extra_id}/expire").status_code == 200
    assert auth.post("/api/memory/1001/disable").status_code == 200
    assert auth.delete(f"/api/memory/1001/memories/{memory_id}").status_code == 200


def test_reenroll_active_reports_current_active_state(auth, fake_bot, monkeypatch):
    async def enroll(user_id, actor_id):
        return SimpleNamespace(state="active", delivered=False, problem="already active")

    monkeypatch.setattr(fake_bot, "enroll_memory_member", enroll, raising=False)
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})

    response = auth.post("/api/memory/1001/enroll")

    assert response.status_code == 200
    assert response.json()["active"] is True


def test_memory_request_is_closed_typed_input(auth):
    response = auth.put("/api/memory/1001/memories", json={"slot": "unknown", "value": "free text"})

    assert response.status_code == 422


def test_boss_knowledge_provenance_preserves_all_validated_urls(auth, fake_bot):
    fake_bot.boss_knowledge = BossKnowledgeBase.load(
        REPO_ROOT / "boss" / "knowledge", fake_bot.bosses
    )

    response = auth.get("/api/bosses/hstar/knowledge")

    assert response.status_code == 200
    payload = response.json()
    assert payload["canonical"] == "HMaleficStar"
    assert payload["sources"]
    assert all(url.startswith("https://") for url in payload["sources"])
    assert auth.get("/api/bosses/not-a-boss/knowledge").status_code == 400


def test_boss_knowledge_missing_base_is_not_found(auth):
    assert auth.get("/api/bosses/hstar/knowledge").status_code == 404
