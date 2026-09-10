"""Discord-facing typed memory proposals never enter the model loop."""

from __future__ import annotations

from types import SimpleNamespace

import pytest

from bot.agent.client import BossBot
from bot.chat.agent import ChatPilot

from .chat_support import ADMIN_ROLE, FakeAuthor, FakeOllama, FakeRole, build_bot, message


class CardMessage:
    id = 812345678901234567


def activate_memory(repo, bot, user_id=1002):
    assert repo.begin_memory_enrollment(bot.settings.guild_id, user_id, "admin", "v1")
    assert repo.record_memory_notice_attempt(bot.settings.guild_id, user_id, "admin")
    assert repo.activate_memory_enrollment(bot.settings.guild_id, user_id, "admin", "notice")


async def post_card(_channel, _proposal):
    return CardMessage()


@pytest.mark.anyio
async def test_active_member_memory_request_posts_bound_card_without_model(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    cards = []

    async def post_memory_proposal(_channel, proposal):
        cards.append(proposal)
        return CardMessage()

    bot.post_memory_proposal = post_memory_proposal
    model = FakeOllama()
    pilot = ChatPilot(bot, client=model)

    handled = await pilot.offer(message(bot, "remember preference: answer_detail=concise"))

    assert handled.handled
    assert handled.reason == "memory proposal created"
    assert model.calls == []
    assert len(cards) == 1
    assert cards[0]["proposal_message_id"] is None
    bound = repo.memory_proposal_by_message(CardMessage.id)
    assert bound is not None
    assert bound["slot"] == "answer_detail"
    assert bound["value"] == "concise"


@pytest.mark.anyio
async def test_disabled_and_contextual_prefixes_never_create_or_call_model(repo, bosses):
    disabled = build_bot(repo, bosses, chat_memory_enabled=False)
    disabled_model = FakeOllama()
    result = await ChatPilot(disabled, client=disabled_model).offer(
        message(disabled, "remember preference: answer_detail=concise")
    )
    assert result.reason == "memory disabled"
    assert disabled_model.calls == []
    assert repo.list_memories(disabled.settings.guild_id, 1002) == []

    enabled = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, enabled)
    enabled_model = FakeOllama()
    pilot = ChatPilot(enabled, client=enabled_model)
    for content, reference in (
        ("remember preference: answer_detail=concise", object()),
        ("> remember preference: answer_detail=concise", None),
        ("```remember preference: answer_detail=concise", None),
    ):
        handled = await pilot.offer(message(enabled, content, reference=reference))
        assert handled.reason == "malformed memory preference"
    assert enabled_model.calls == []
    assert repo.list_memories(enabled.settings.guild_id, 1002) == []


@pytest.mark.anyio
async def test_boss_scope_requires_valid_difficulty_and_stores_canonical_token(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    bot.post_memory_proposal = post_card
    model = FakeOllama()
    pilot = ChatPilot(bot, client=model)
    for content in (
        "remember preference: strategy_emphasis=survival; boss=Black Mage",
        "remember preference: strategy_emphasis=survival; boss=Unknown Boss",
        "remember preference: strategy_emphasis=survival; boss=Normal Black Mage",
    ):
        result = await pilot.offer(message(bot, content))
        assert result.reason in {"memory boss needs difficulty", "memory boss unresolved"}
    assert repo.list_memories(bot.settings.guild_id, 1002) == []
    valid = await pilot.offer(
        message(bot, "remember preference: strategy_emphasis=survival; boss=Hard Black Mage")
    )
    assert valid.reason == "memory proposal created"
    assert repo.memory_proposal_by_message(CardMessage.id)["boss_token"] == "HBM"
    assert model.calls == []


@pytest.mark.anyio
async def test_inactive_or_malformed_memory_request_does_not_call_model(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    model = FakeOllama()
    pilot = ChatPilot(bot, client=model)

    inactive = await pilot.offer(message(bot, "remember preference: answer_detail=concise"))
    malformed = await pilot.offer(message(bot, "remember preference: answer_detail=verbose"))

    assert inactive.reason == "memory enrollment inactive"
    assert malformed.reason == "malformed memory preference"
    assert model.calls == []


@pytest.mark.anyio
async def test_role_mention_prefix_and_card_failure_never_call_model_or_leave_proposal(
    repo, bosses
):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    role = FakeRole(717171717171717171)
    bot.guild.self_role = role

    async def unavailable(*_args):
        return None

    bot.post_memory_proposal = unavailable
    model = FakeOllama()
    pilot = ChatPilot(bot, client=model)
    incoming = message(
        bot, f"<@&{role.id}> remember preference: answer_detail=concise", mentions=()
    )
    incoming.role_mentions = [role]

    result = await pilot.offer(incoming)

    assert result.reason == "memory card unavailable"
    assert model.calls == []
    assert repo.list_memories(bot.settings.guild_id, 1002) == []


@pytest.mark.anyio
async def test_bind_cas_failure_discards_posted_unbound_proposal(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    bot.post_memory_proposal = post_card
    original = repo.bind_memory_proposal_message
    repo.bind_memory_proposal_message = lambda *_args: False
    try:
        result = await ChatPilot(bot, client=FakeOllama()).offer(
            message(bot, "remember preference: answer_detail=concise")
        )
    finally:
        repo.bind_memory_proposal_message = original
    assert result.reason == "memory card unavailable"
    assert repo.list_memories(bot.settings.guild_id, 1002) == []
    assert repo.memory_proposal_by_message(CardMessage.id) is None


@pytest.mark.anyio
async def test_memory_reaction_authority_transitions_and_stale_card_state(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    updates = []

    class Service:
        def __init__(self):
            self.settings = bot.settings
            self.repo = repo

        def get_guild(self, _guild_id):
            return bot.guild

        async def edit_card(self, _channel_id, _message_id, card):
            updates.append(card.footer)
            return True

    service = Service()

    def proposal(message_id):
        memory_id = repo.create_memory_proposal(
            bot.settings.guild_id, 1002, "answer_detail", "concise", proposer_id=1002
        )
        assert memory_id
        assert repo.bind_memory_proposal_message(bot.settings.guild_id, 1002, memory_id, message_id)
        return repo.get_memory(bot.settings.guild_id, 1002, memory_id)

    def payload(user_id, message_id, member=None):
        return SimpleNamespace(
            user_id=user_id, message_id=message_id, channel_id=777, member=member
        )

    first = proposal(1)
    await BossBot._handle_memory_proposal_reaction(service, payload(1002, 1), first, "✅")
    assert repo.get_memory(bot.settings.guild_id, 1002, first["id"])["state"] == "active"
    await BossBot._handle_memory_proposal_reaction(service, payload(1002, 1), first, "✅")
    assert updates[-1] == "Approved"

    admin = FakeAuthor(2000, roles=(ADMIN_ROLE,))
    second = proposal(2)
    await BossBot._handle_memory_proposal_reaction(service, payload(2000, 2, admin), second, "✅")
    assert repo.get_memory(bot.settings.guild_id, 1002, second["id"])["state"] == "proposed"
    await BossBot._handle_memory_proposal_reaction(service, payload(3000, 2), second, "❌")
    assert repo.get_memory(bot.settings.guild_id, 1002, second["id"])["state"] == "proposed"
    await BossBot._handle_memory_proposal_reaction(service, payload(2000, 2, admin), second, "❌")
    assert repo.get_memory(bot.settings.guild_id, 1002, second["id"])["state"] == "rejected"
    await BossBot._handle_memory_proposal_reaction(service, payload(1002, 2), second, "❌")
    assert updates[-1] == "Rejected"


@pytest.mark.anyio
async def test_memory_reaction_without_guild_is_ignored_safely(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    activate_memory(repo, bot)
    memory_id = repo.create_memory_proposal(
        bot.settings.guild_id, 1002, "answer_detail", "concise", proposer_id=1002
    )
    assert memory_id

    service = SimpleNamespace(
        settings=bot.settings,
        repo=repo,
        get_guild=lambda _guild_id: None,
    )
    payload = SimpleNamespace(user_id=3000, message_id=9, channel_id=777, member=None)

    await BossBot._handle_memory_proposal_reaction(
        service, payload, repo.get_memory(bot.settings.guild_id, 1002, memory_id), "❌"
    )

    assert repo.get_memory(bot.settings.guild_id, 1002, memory_id)["state"] == "proposed"


@pytest.mark.anyio
async def test_enrollment_failures_remain_pending_until_explicit_successful_retry(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)

    class Service:
        def __init__(self):
            self.settings = bot.settings
            self.repo = repo
            self.member = None

        def get_guild(self, _guild_id):
            return SimpleNamespace(members=[] if self.member is None else [self.member])

    service = Service()
    missing = await BossBot.enroll_memory_member(service, 1002, "admin")
    assert not missing.delivered
    assert repo.get_memory_enrollment(bot.settings.guild_id, 1002)["state"] == "pending_notice"
    assert (
        repo.create_memory_proposal(bot.settings.guild_id, 1002, "answer_detail", "concise") is None
    )

    class FailingMember:
        id = 1002

        async def send(self, _notice):
            raise RuntimeError("DMs closed")

    service.member = FailingMember()
    failed = await BossBot.enroll_memory_member(service, 1002, "admin")
    assert not failed.delivered
    assert repo.get_memory_enrollment(bot.settings.guild_id, 1002)["state"] == "pending_notice"

    class Sent:
        id = 9001

    class Member:
        id = 1002

        async def send(self, _notice):
            return Sent()

    service.member = Member()
    retried = await BossBot.enroll_memory_member(service, 1002, "admin")
    enrollment = repo.get_memory_enrollment(bot.settings.guild_id, 1002)
    assert retried.delivered and enrollment["state"] == "active"
    assert enrollment["notice_message_id"] == "9001"

    disabled = build_bot(repo, bosses, chat_memory_enabled=False)
    disabled_service = SimpleNamespace(settings=disabled.settings, repo=repo)
    blocked = await BossBot.enroll_memory_member(disabled_service, 4004, "admin")
    assert not blocked.delivered
    assert repo.get_memory_enrollment(disabled.settings.guild_id, 4004) is None


@pytest.mark.anyio
async def test_notification_first_enrollment_activates_only_after_a_dm(repo, bosses):
    bot = build_bot(repo, bosses, chat_memory_enabled=True)
    notices = []

    class Sent:
        id = 812345678901234568

    class Member:
        id = 1002

        async def send(self, notice):
            assert repo.get_memory_enrollment(bot.settings.guild_id, self.id)["state"] == (
                "pending_notice"
            )
            notices.append(notice)
            return Sent()

    class Guild:
        owner_id = 9999
        members = [Member()]

    class Service:
        def __init__(self):
            self.settings = bot.settings
            self.repo = repo

        def get_guild(self, guild_id):
            return Guild() if guild_id == self.settings.guild_id else None

    result = await BossBot.enroll_memory_member(Service(), 1002, "admin")

    assert result.delivered
    enrollment = repo.get_memory_enrollment(bot.settings.guild_id, 1002)
    assert enrollment["state"] == "active"
    assert enrollment["notice_message_id"] == str(Sent.id)
    expected_notice = (
        "Kanade memory notice: stored fields are limited to "
        "answer_detail=concise|standard|detailed; "
        "answer_format=prose|bullets|steps; "
        "strategy_disclosure=none|hints|full; "
        "strategy_emphasis=mechanics|survival|party_roles. "
        "Portal administrators can view and edit them. "
        "Active preferences expire after 180 days. "
        "You can opt out or delete memory anytime without a policy prompt. "
        "Deletion removes the live memory row; Discord messages and historical backups may "
        "retain residual copies."
    )
    assert notices == [expected_notice]
    for field in (
        "answer_detail=concise|standard|detailed",
        "answer_format=prose|bullets|steps",
        "strategy_disclosure=none|hints|full",
        "strategy_emphasis=mechanics|survival|party_roles",
    ):
        assert field in notices[0]
    for governance_point in (
        "Portal administrators can view and edit them.",
        "Active preferences expire after 180 days.",
        "You can opt out or delete memory anytime without a policy prompt.",
        "Deletion removes the live memory row; Discord messages and historical backups may "
        "retain residual copies.",
    ):
        assert governance_point in notices[0]
