"""Server-rendered governance views keep memory records human-first and content-free."""

from __future__ import annotations

from types import SimpleNamespace


def _active_memory(fake_bot, user_id: str = "1001") -> str:
    repo = fake_bot.repo
    guild = fake_bot.settings.guild_id
    assert repo.begin_memory_enrollment(guild, user_id, "admin", "memory-v1")
    assert repo.record_memory_notice_attempt(guild, user_id, "admin")
    assert repo.activate_memory_enrollment(guild, user_id, "admin", "notice")
    memory_id = repo.replace_memory(guild, user_id, "answer_format", "bullets", "admin")
    assert memory_id
    return memory_id


def test_memory_listing_is_member_first_and_keeps_filter_context(auth, fake_bot):
    fake_bot.repo.upsert_member("1001", "kanon", None, True)
    _active_memory(fake_bot)

    response = auth.get("/memory", params={"q": "kanon", "slot": "answer_format"})

    assert response.status_code == 200
    assert "kanon" in response.text
    assert response.text.index("kanon") < response.text.index("1001")
    assert '<body class="framed memory-frame">' in response.text
    assert 'id="memory-rows"' in response.text
    assert 'name="slot"' in response.text


def test_memory_detail_exposes_typed_records_not_chat_content_and_actions_work(auth, fake_bot):
    memory_id = _active_memory(fake_bot)
    fake_bot.repo.upsert_member("4001", "Proposer <script>alert(1)</script>", None, True)
    fake_bot.repo.upsert_member("5001", "Reviewer", None, True)
    fake_bot.repo._conn.execute(
        "UPDATE chat_memories SET source_message_id = ?, source_channel_id = ?, proposer_id = ?, "
        "reviewer_id = ?, reviewed_at = ? WHERE id = ?",
        ("3001", "2001", "4001", "5001", "2026-01-02T03:04:00+00:00", memory_id),
    )
    fake_bot.repo.log_memory_retrieval(
        fake_bot.settings.guild_id,
        "1001",
        [memory_id],
        "matched",
        latency_ms=7,
    )

    page = auth.get("/memory/1001")
    assert page.status_code == 200
    assert '<body class="framed memory-subject-frame">' in page.text
    assert page.text.count('class="card tabs memory-tabs"') == 1
    assert 'class="memory-subject__scroll"' not in page.text
    assert "memory-detail" not in page.text
    assert 'aria-label="Memory governance panels"' in page.text
    tab_markers = [
        'href="#memory-enrollment"',
        'href="#memory-preference"',
        'href="#memory-records"',
        'href="#memory-activity"',
    ]
    assert all(marker in page.text for marker in tab_markers)
    assert [page.text.index(marker) for marker in tab_markers] == sorted(
        page.text.index(marker) for marker in tab_markers
    )
    assert 'id="memory-enrollment"' in page.text
    assert 'id="memory-preference"' in page.text
    assert 'id="memory-records"' in page.text
    assert 'id="memory-activity"' in page.text
    assert 'tabs__panel tabs__panel--first" id="memory-enrollment"' in page.text
    assert page.text.count('class="tabs__count"') == 2
    assert "answer_format=bullets" in page.text
    assert "Record metadata" in page.text
    assert 'Channel ID <span class="mono">2001</span>' in page.text
    assert 'Message ID <span class="mono">3001</span>' in page.text
    assert "https://discord.com/channels/111111111111111111/2001/3001" in page.text
    assert "Proposer &lt;script&gt;alert(1)&lt;/script&gt;" in page.text
    assert "Proposer <script>" not in page.text
    assert page.text.index("Proposer &lt;script&gt;") < page.text.index(
        'ID <span class="mono">4001'
    )
    assert page.text.index("Reviewer") < page.text.index('ID <span class="mono">5001')
    assert "Retrieval diagnostics" in page.text
    assert "prompt" not in page.text.lower()
    assert f"/memory/1001/memories/{memory_id}/delete" in page.text

    response = auth.post(f"/memory/1001/memories/{memory_id}/revoke", follow_redirects=False)
    assert response.status_code == 303
    assert "Preference+revoked" in response.headers["location"]


def test_memory_detail_marks_missing_provenance_not_recorded(auth, fake_bot):
    memory_id = _active_memory(fake_bot)
    fake_bot.repo._conn.execute(
        "UPDATE chat_memories SET proposer_id = NULL, reviewer_id = NULL, "
        "reviewed_at = NULL WHERE id = ?",
        (memory_id,),
    )

    page = auth.get("/memory/1001")

    assert page.status_code == 200
    assert "Source" in page.text
    assert page.text.count("not recorded") >= 3


def test_memory_page_explains_global_disabled_state(auth, fake_bot):
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": False})

    response = auth.get("/memory")

    assert response.status_code == 200
    assert "Memory is disabled for this server." in response.text


def test_memory_uses_exact_state_enums_and_rejects_crafted_filters(auth, fake_bot):
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    repo, guild = fake_bot.repo, fake_bot.settings.guild_id
    assert repo.begin_memory_enrollment(guild, "1001", "admin", "memory-v1")

    pending = auth.get("/memory", params={"enrollment": "pending_notice"})
    invalid = auth.get("/memory", params={"enrollment": "pending"})
    invalid_lifecycle = auth.get("/memory", params={"lifecycle": "invented"})

    assert pending.status_code == 200
    assert "pending notice" in pending.text
    assert "Retry individual notice" in auth.get("/memory/1001").text
    assert invalid.status_code == 400
    assert invalid_lifecycle.status_code == 400


def test_memory_preference_form_is_allowlisted_and_safe_without_javascript(auth, fake_bot):
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    memory_id = _active_memory(fake_bot)

    page = auth.get("/memory/1001")
    assert 'name="preference"' in page.text
    assert 'name="value"' not in page.text
    assert "Strategy-only boss scope" in page.text

    saved = auth.post(
        "/memory/1001/memories",
        data={"preference": "answer_format:steps"},
        follow_redirects=False,
    )
    invalid = auth.post(
        "/memory/1001/memories", data={"preference": "free:text"}, follow_redirects=False
    )
    htmx_invalid = auth.post(
        "/memory/1001/memories",
        data={"preference": "free:text"},
        headers={"HX-Request": "true"},
    )

    assert saved.status_code == 303 and "Typed+preference+saved" in saved.headers["location"]
    assert invalid.status_code == 303 and "Choose+one+of+the+listed" in invalid.headers["location"]
    assert htmx_invalid.status_code == 200
    assert "Choose one of the listed typed preferences." in htmx_invalid.text

    deleted = auth.post(f"/memory/1001/memories/{memory_id}/delete", follow_redirects=False)
    assert "permanently+removed+from+memory+and+retrieval" in deleted.headers["location"]


def test_memory_rejects_non_strategy_boss_scope_before_mutating(auth, fake_bot):
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    _active_memory(fake_bot)
    before = len(fake_bot.repo.list_memories(fake_bot.settings.guild_id, "1001"))
    form = {"preference": "answer_format:steps", "boss": "hstar"}

    plain = auth.post("/memory/1001/memories", data=form, follow_redirects=False)
    htmx = auth.post("/memory/1001/memories", data=form, headers={"HX-Request": "true"})

    assert plain.status_code == 303
    assert "Boss+scope+is+available+only+for+strategy+preferences" in plain.headers["location"]
    assert htmx.status_code == 200
    assert "Boss scope is available only for strategy preferences." in htmx.text
    assert len(fake_bot.repo.list_memories(fake_bot.settings.guild_id, "1001")) == before

    strategy = auth.post(
        "/memory/1001/memories",
        data={"preference": "strategy_disclosure:hints", "boss": "hstar"},
        follow_redirects=False,
    )
    assert strategy.status_code == 303
    assert len(fake_bot.repo.list_memories(fake_bot.settings.guild_id, "1001")) == before + 1


def test_memory_global_disabled_hides_edit_but_keeps_record_governance(auth, fake_bot):
    memory_id = _active_memory(fake_bot)
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": False})

    page = auth.get("/memory/1001")

    assert 'name="preference"' not in page.text
    assert "Typed preferences cannot be created or replaced until it is enabled." in page.text
    assert f"/memory/1001/memories/{memory_id}/delete" in page.text


def test_memory_pending_enrollment_keeps_actions_and_explains_preference_lock(auth, fake_bot):
    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    assert fake_bot.repo.begin_memory_enrollment(
        fake_bot.settings.guild_id, "1001", "admin", "memory-v1"
    )

    page = auth.get("/memory/1001")

    assert "Retry individual notice" in page.text
    assert 'name="preference"' not in page.text
    assert (
        "Typed preferences can be created or replaced only while enrollment is active." in page.text
    )


def test_memory_enroll_collection_route_precedes_subject_route(auth, fake_bot, monkeypatch):
    called = []

    async def enroll(user_id, actor_id):
        called.append(user_id)
        return SimpleNamespace(state="pending_notice", delivered=False, problem="DM pending")

    fake_bot.settings = fake_bot.settings.model_copy(update={"chat_memory_enabled": True})
    monkeypatch.setattr(fake_bot, "enroll_memory_member", enroll, raising=False)

    response = auth.post("/memory/enroll", data={"user_id": "1001"}, follow_redirects=False)

    assert response.status_code == 303
    assert called == ["1001"]
    assert response.headers["location"].startswith("/memory/1001")
