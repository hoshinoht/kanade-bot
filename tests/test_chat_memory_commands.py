"""Self-service memory slash commands stay subject-scoped and model-free."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

from bot.agent.commands import MEMORY_LIST_LIMIT, MemoryGroup, register_commands

from .chat_support import FakeAuthor

MEMBER_ID = 1002
OTHER_ID = 1003


class Interaction:
    def __init__(self, bot, user_id: int = MEMBER_ID, *, guild=True):
        self.client = bot
        self.user = FakeAuthor(user_id, roles=())
        self.guild = bot.guild if guild else None
        self.sent: list[tuple[str, bool]] = []
        self.response = SimpleNamespace(send_message=self._send)

    async def _send(self, content: str, ephemeral: bool = False) -> None:
        self.sent.append((content, ephemeral))


def active(repo, bot, user_id: int) -> None:
    guild_id = bot.settings.guild_id
    assert repo.begin_memory_enrollment(guild_id, user_id, "admin", "v1")
    assert repo.record_memory_notice_attempt(guild_id, user_id, "admin")
    assert repo.activate_memory_enrollment(guild_id, user_id, "admin", "notice")


def command(group: MemoryGroup, name: str):
    return getattr(group, name).callback


def invoke(group: MemoryGroup, name: str, interaction: Interaction, **kwargs) -> Interaction:
    asyncio.run(command(group, name)(group, interaction, **kwargs))
    return interaction


def test_status_is_available_without_enrollment_or_memory_capability(chat_bot):
    group = MemoryGroup()
    interaction = invoke(group, "status", Interaction(chat_bot))

    said, ephemeral = interaction.sent[0]
    assert "globally **disabled**" in said
    assert "not enrolled" in said
    assert "0 saved preferences" in said
    assert "remember preference:" in said
    assert ephemeral is True


def test_controls_need_a_guild_but_not_a_chatbot_or_bossing_role(chat_bot):
    group = MemoryGroup()

    assert asyncio.run(group.interaction_check(Interaction(chat_bot))) is True
    try:
        asyncio.run(group.interaction_check(Interaction(chat_bot, guild=False)))
    except Exception as exc:
        assert "only available in this guild" in str(exc)
    else:  # pragma: no cover - the check must reject DMs
        raise AssertionError("memory controls accepted a DM")


def test_list_is_subject_only_and_explains_correction(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    active(repo, chat_bot, OTHER_ID)
    mine = repo.replace_memory(
        chat_bot.settings.guild_id, MEMBER_ID, "answer_format", "bullets", "admin"
    )
    other = repo.replace_memory(
        chat_bot.settings.guild_id, OTHER_ID, "answer_detail", "detailed", "admin"
    )
    assert mine and other

    interaction = invoke(MemoryGroup(), "list_", Interaction(chat_bot))

    said, ephemeral = interaction.sent[0]
    assert said.splitlines()[1].startswith("answer_format=bullets")
    assert f"#{mine[:8]}" in said
    assert "active" in said and "expires" in said
    assert "remember preference:" in said
    assert other not in said and "answer_detail=detailed" not in said
    assert ephemeral is True


def test_list_stays_under_discord_message_limit(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    for _ in range(40):
        assert repo.create_memory_proposal(
            chat_bot.settings.guild_id, MEMBER_ID, "answer_detail", "concise"
        )

    interaction = invoke(MemoryGroup(), "list_", Interaction(chat_bot))

    said = interaction.sent[0][0]
    assert "More preferences are available in the portal." in said
    assert len(said) <= 2000
    assert len(said) > MEMORY_LIST_LIMIT


def test_opt_out_is_idempotent_and_revokes_active_and_pending_memory(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    active_id = repo.replace_memory(
        chat_bot.settings.guild_id, MEMBER_ID, "answer_format", "bullets", "admin"
    )
    pending_id = repo.create_memory_proposal(
        chat_bot.settings.guild_id, MEMBER_ID, "answer_detail", "concise"
    )
    assert active_id and pending_id

    first = invoke(MemoryGroup(), "opt_out", Interaction(chat_bot)).sent[0][0]
    second = invoke(MemoryGroup(), "opt_out", Interaction(chat_bot)).sent[0][0]

    assert "were revoked" in first
    assert "already opted out" in second
    assert repo.get_memory(chat_bot.settings.guild_id, MEMBER_ID, active_id)["state"] == "revoked"
    assert repo.get_memory(chat_bot.settings.guild_id, MEMBER_ID, pending_id)["state"] == "revoked"


def test_forget_cannot_delete_another_members_memory_and_is_idempotent(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    active(repo, chat_bot, OTHER_ID)
    mine = repo.replace_memory(
        chat_bot.settings.guild_id, MEMBER_ID, "answer_format", "bullets", "admin"
    )
    other = repo.replace_memory(
        chat_bot.settings.guild_id, OTHER_ID, "answer_format", "steps", "admin"
    )
    assert mine and other
    group = MemoryGroup()

    blocked = invoke(group, "forget", Interaction(chat_bot), id=other).sent[0][0]
    removed = invoke(group, "forget", Interaction(chat_bot), id=mine[:8]).sent[0][0]
    repeated = invoke(group, "forget", Interaction(chat_bot), id=mine[:8]).sent[0][0]

    assert "No matching memory" in blocked
    assert repo.get_memory(chat_bot.settings.guild_id, OTHER_ID, other) is not None
    assert "removed immediately" in removed and "logical deletion" in removed
    assert "No matching memory" in repeated


def test_forget_ambiguous_dash_padded_id_stays_bounded_and_does_not_echo_input(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    for suffix in ("1", "2"):
        memory_id = repo.create_memory_proposal(
            chat_bot.settings.guild_id, MEMBER_ID, "answer_detail", "concise"
        )
        assert memory_id
        repo._conn.execute(
            "UPDATE chat_memories SET id = ? WHERE id = ?",
            (f"dead0000-0000-4000-8000-00000000000{suffix}", memory_id),
        )
    raw = "de-ad" + "-" * 5000

    said = invoke(MemoryGroup(), "forget", Interaction(chat_bot), id=raw).sent[0][0]

    assert "Several memories match" in said
    assert "#dead0000" in said
    assert raw not in said
    assert len(said) <= 2000


def test_forget_all_is_idempotent_and_registered(chat_bot, repo):
    active(repo, chat_bot, MEMBER_ID)
    assert repo.create_memory_proposal(
        chat_bot.settings.guild_id, MEMBER_ID, "answer_detail", "concise"
    )
    group = MemoryGroup()

    first = invoke(group, "forget_all", Interaction(chat_bot)).sent[0][0]
    second = invoke(group, "forget_all", Interaction(chat_bot)).sent[0][0]
    added = []
    chat_bot.tree = SimpleNamespace(
        add_command=added.append, on_error=None, copy_global_to=lambda **_: None
    )
    register_commands(chat_bot)

    assert "Removed 1 saved preference" in first and "logical deletion" in first
    assert "no saved preferences" in second
    assert any(getattr(item, "name", None) == "memory" for item in added)
