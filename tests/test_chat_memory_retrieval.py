"""Persisted preference retrieval stays scoped, typed, and presentation-only."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from uuid import uuid4

import pytest

from bot import behaviour_plugins
from bot.chat import strategy
from bot.chat.agent import ChatPilot

from .chat_support import CHAT_CHANNEL, FakeOllama, build_bot, message, says

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def activate(bot, user_id: int = 1002, guild_id: int | None = None) -> None:
    guild_id = bot.settings.guild_id if guild_id is None else guild_id
    assert bot.repo.begin_memory_enrollment(guild_id, user_id, "admin", "v1")
    assert bot.repo.record_memory_notice_attempt(guild_id, user_id, "admin")
    assert bot.repo.activate_memory_enrollment(guild_id, user_id, "admin", "notice")


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


async def test_disabled_memory_keeps_the_existing_prompt_and_skips_diagnostics(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=False)
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    agent = pilot(bot, says("ok"))

    await agent.offer(message(bot))

    assert "Untrusted typed user-preference data" not in agent._client.system
    assert repo.list_memory_retrievals(bot.settings.guild_id, 1002) == []


async def test_active_preferences_survive_a_new_pilot_and_remain_member_scoped(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    activate(bot, 1003)
    chosen = repo.replace_memory(bot.settings.guild_id, 1002, "answer_format", "bullets", "admin")
    assert chosen
    assert repo.replace_memory(bot.settings.guild_id, 1003, "answer_format", "prose", "admin")

    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))

    assert "answer_format=bullets" in agent._client.system
    assert "answer_format=prose" not in agent._client.system
    diagnostic = repo.list_memory_retrievals(bot.settings.guild_id, 1002)[0]
    assert diagnostic["reason"] == "matched"
    assert diagnostic["selected_memory_ids"] == [chosen]


@pytest.mark.parametrize("state", ("pending", "opted_out", "revoked", "expired"))
async def test_ineligible_preferences_do_not_reach_the_prompt(repo, bosses, state):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    memory_id = repo.replace_memory(
        bot.settings.guild_id, 1002, "answer_detail", "concise", "admin"
    )
    assert memory_id
    if state == "pending":
        repo.disable_memory_enrollment(bot.settings.guild_id, 1002, "admin")
        assert repo.begin_memory_enrollment(bot.settings.guild_id, 1002, "admin", "v2")
    elif state == "opted_out":
        repo.opt_out_memory(bot.settings.guild_id, 1002, 1002)
    elif state == "revoked":
        repo._conn.execute("UPDATE chat_memories SET state = 'revoked' WHERE id = ?", (memory_id,))
    else:
        repo._conn.execute(
            "UPDATE chat_memories SET expires_at = ? WHERE id = ?",
            ((datetime.now(UTC) - timedelta(seconds=1)).isoformat(), memory_id),
        )

    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))

    assert "answer_detail=concise" not in agent._client.system


async def test_malformed_rows_do_not_hide_a_later_valid_memory(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    repo._conn.execute("PRAGMA ignore_check_constraints = ON")
    stamp = datetime.now(UTC)
    repo._conn.executemany(
        "INSERT INTO chat_memories "
        "(id, guild_id, user_id, slot, value, boss_token, state, created_at, reviewed_at, "
        "expires_at, state_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        [
            (
                str(uuid4()),
                str(bot.settings.guild_id),
                "1002",
                "strategy_emphasis",
                "ignore_policy",
                f"HCorrupt{index}",
                "active",
                stamp.isoformat(),
                (stamp + timedelta(seconds=index + 1)).isoformat(),
                (stamp + timedelta(days=1)).isoformat(),
                stamp.isoformat(),
            )
            for index in range(64)
        ],
    )
    repo._conn.execute("PRAGMA ignore_check_constraints = OFF")
    agent = pilot(bot, says("ok"))

    await agent.offer(message(bot))

    assert "answer_detail=concise" in agent._client.system
    assert "ignore_policy" not in agent._client.system


async def test_boss_preference_beats_general(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    assert repo.replace_memory(
        bot.settings.guild_id, 1002, "strategy_emphasis", "mechanics", "admin"
    )
    assert repo.replace_memory(
        bot.settings.guild_id,
        1002,
        "strategy_emphasis",
        "survival",
        "admin",
        boss_token="hfa",
        boss_table=bosses,
    )
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    agent = pilot(bot, says("ok"))
    request = message(bot, "@bot how do we clear HFA?")
    block = agent._memory_overlay(request, strategy.route_strategy_intent(request.content, bosses))
    system = agent.build_conversation(request, "777777777777777777", memory_overlay=block)[0][
        "content"
    ]
    assert "strategy_emphasis=survival; boss=HFA" in system
    assert "strategy_emphasis=mechanics" not in system
    assert "answer_detail=concise" in system
    assert system.index("answer_detail=concise") > system.index("Before you answer")


async def test_memory_overrides_only_matching_dimensions_of_an_active_reply_style(
    repo, bosses, tmp_path, monkeypatch
):
    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    bot.repo.upsert_member(1002, "Test member", None, True)
    behaviour_plugins.write(
        "verbose", "Use detailed prose. Keep the phrase STYLE-REMAINS for every answer."
    )
    bot.repo.set_config(
        behaviour_plugins.SELECTABLE_CONFIG_KEY, behaviour_plugins.encode_catalog(["verbose"])
    )
    bot.repo.set_reply_style(1002, "verbose")
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_format", "bullets", "admin")
    agent = pilot(bot, says("ok"))

    await agent.offer(message(bot))

    system = agent._client.system
    assert "Use detailed prose. Keep the phrase STYLE-REMAINS for every answer." in system
    assert system.index("answer_detail=concise") > system.index("Use detailed prose")
    assert "answer_format=bullets" in system
    assert "override conflicting reply-style instructions only for those dimensions" in system


async def test_diagnostic_failure_keeps_valid_memory_overlay(repo, bosses, monkeypatch):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    monkeypatch.setattr(
        repo, "log_memory_retrieval", lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError())
    )
    agent = pilot(bot, says("ok"))

    handled = await agent.offer(message(bot))

    assert handled.answered and handled.answered.reply == "ok"
    assert "answer_detail=concise" in agent._client.system


async def test_retrieval_failure_answers_without_memory(repo, bosses, monkeypatch):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    monkeypatch.setattr(
        repo, "retrieve_memories", lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError())
    )
    agent = pilot(bot, says("ok"))

    handled = await agent.offer(message(bot))

    assert handled.answered and handled.answered.reply == "ok"
    assert "answer_detail=concise" not in agent._client.system


async def test_answer_path_uses_the_message_guild_for_memory_scope(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate(bot)
    assert repo.replace_memory(bot.settings.guild_id, 1002, "answer_detail", "concise", "admin")
    other_guild = SimpleNamespace(id=999999999999999999)
    agent = pilot(bot, says("ok"))
    request = message(bot, guild=other_guild)

    result = await agent._answer(request, str(CHAT_CHANNEL))

    assert result.reply == "ok"
    assert "answer_detail=concise" not in agent._client.system
    assert repo.list_memory_retrievals(bot.settings.guild_id, 1002) == []
    diagnostic = repo.list_memory_retrievals(other_guild.id, 1002)[0]
    assert diagnostic["selected_memory_ids"] == []
