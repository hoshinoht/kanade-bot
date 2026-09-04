"""`/say` and `/debug`: admins only, and the one place the bot really pings.

Two gates, because either alone is a hole. `default_permissions` keeps both out
of everyone else's picker but is only a *default* -- a server can hand it back
out under Server Settings -> Integrations -- so the rule is checked again when
the command runs, and it is there that `ADMIN_ROLE_ID` grants access.

`/say` is also the one thing the bot writes that may notify people: its
allow-list is built from the mentions in the text, so it can reach exactly who
it names and nobody else.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

import pytest
from discord import app_commands

from bot.agent import formatting
from bot.agent.client import CFG_QUIET, BossBot
from bot.agent.commands import SAY_LIMIT, NotAnAdmin, register_commands, say
from bot.agent.debug import DebugGroup, DebugNotAllowed, may_debug
from bot.agent.util import is_bot_admin, mentions_in

from .fake_bot import OTHER_CHANNEL, OWNER_ID, WATCHED_CHANNEL, make_settings

ADMIN_ROLE = 777777777777777777
MEMBER_ID = 1001
BOSSING_ROLE = 555555555555555555


class FakeResponse:
    def __init__(self):
        self.sent: list[tuple[str, bool]] = []

    def is_done(self) -> bool:
        return bool(self.sent)

    async def send_message(self, content, ephemeral=False):
        self.sent.append((content, ephemeral))


class FakeUser:
    def __init__(self, user_id=MEMBER_ID, administrator=False, role_ids=()):
        self.id = user_id
        self.guild_permissions = SimpleNamespace(administrator=administrator)
        self.roles = [SimpleNamespace(id=r) for r in role_ids]

    def __str__(self) -> str:
        return f"user-{self.id}"


class FakeInteraction:
    """Only what `/say` reaches for."""

    def __init__(self, bot, user=None, channel=None):
        self.client = bot
        self.user = user or FakeUser(administrator=True)
        self.guild = bot.guild
        self.channel = channel if channel is not None else bot.channels[WATCHED_CHANNEL]
        self.response = FakeResponse()


class RecordingChannel:
    """Captures what the *real* `post_plain` hands to Discord."""

    id = WATCHED_CHANNEL

    def __init__(self):
        self.sends: list[tuple[str, object]] = []

    async def send(self, content=None, **kwargs):
        self.sends.append((content, kwargs["allowed_mentions"]))
        return SimpleNamespace(id=1)


def real_client(repo):
    """A client with only what `post_plain` reaches for -- the allow-list is
    built in there, so the fake cannot answer these questions."""
    client = BossBot.__new__(BossBot)
    client.repo = repo
    return client


def run_say(bot, message="hello party", channel=None, user=None, invoked_in=None):
    interaction = FakeInteraction(bot, user=user, channel=invoked_in)
    asyncio.run(say.callback(interaction, message=message, channel=channel))
    return interaction


# --- who may use it ---------------------------------------------------------


def test_the_configured_admin_role_may():
    """The point of the setting: a role the guild hands out, not a permission."""
    assert is_bot_admin(False, False, [BOSSING_ROLE, ADMIN_ROLE], ADMIN_ROLE)


def test_a_server_admin_without_that_role_may():
    """Administrator is the fallback, so a wrong role id cannot lock anyone out."""
    assert is_bot_admin(True, False, [], ADMIN_ROLE)


def test_the_guild_owner_may():
    assert is_bot_admin(False, True, [], None)


def test_neither_may_not():
    assert not is_bot_admin(False, False, [BOSSING_ROLE], ADMIN_ROLE)


def test_with_no_admin_role_configured_it_is_administrators_only():
    """`ADMIN_ROLE_ID` unset: nobody gets in on a role, admins still do."""
    assert not is_bot_admin(False, False, [BOSSING_ROLE], None)
    assert is_bot_admin(True, False, [BOSSING_ROLE], None)


def test_the_role_id_is_compared_as_a_number():
    """Settings hands over an int; Discord hands over ints. A string id in
    either place would silently match nobody."""
    assert is_bot_admin(False, False, [str(ADMIN_ROLE)], str(ADMIN_ROLE))


def test_the_command_is_hidden_from_non_admins():
    """`default_permissions(administrator=True)`: it does not appear in their picker."""
    assert say.default_permissions is not None
    assert say.default_permissions.administrator is True


def test_it_is_also_checked_at_run_time(fake_bot):
    """The default can be overridden per server, so hiding it is not the gate."""
    assert say.checks, "the runtime check is what actually stops anyone"
    interaction = FakeInteraction(fake_bot, user=FakeUser())
    with pytest.raises(NotAnAdmin):
        asyncio.run(say.checks[0](interaction))


def test_the_admin_role_passes_the_run_time_check(repo, bosses):
    from .fake_bot import FakeBot

    bot = FakeBot(repo, bosses, make_settings(admin_role_id=ADMIN_ROLE))
    interaction = FakeInteraction(bot, user=FakeUser(role_ids=[ADMIN_ROLE]))
    assert asyncio.run(say.checks[0](interaction)) is True


def test_the_guild_owner_passes_the_run_time_check(fake_bot):
    interaction = FakeInteraction(fake_bot, user=FakeUser(user_id=OWNER_ID))
    assert asyncio.run(say.checks[0](interaction)) is True


def test_it_is_registered(fake_bot):
    added = []
    fake_bot.tree = SimpleNamespace(
        add_command=added.append, on_error=None, copy_global_to=lambda **_: None
    )
    register_commands(fake_bot)
    assert any(getattr(c, "name", None) == "say" for c in added)


def test_it_is_guild_only():
    assert say.guild_only is True


# --- what it posts ----------------------------------------------------------


def test_it_posts_in_the_channel_it_was_invoked_in(fake_bot):
    interaction = run_say(fake_bot, "raid at nine")

    (posted,) = fake_bot.posts
    assert posted.channel_id == WATCHED_CHANNEL
    assert posted.content == "raid at nine"
    assert interaction.response.sent[0][1] is True  # ephemeral


def test_it_can_be_pointed_at_another_channel(fake_bot):
    run_say(fake_bot, "over here", channel=fake_bot.channels[OTHER_CHANNEL])
    assert fake_bot.posts[0].channel_id == OTHER_CHANNEL


def test_the_people_it_names_really_are_notified(fake_bot):
    """Unlike everything else the bot writes: an admin who types `@kanon` meant it."""
    run_say(fake_bot, "<@1001> <@1002> raid moved to nine")
    assert fake_bot.posts[0].allowed_mentions == ["1001", "1002"]


def test_a_role_mention_is_carried_too(fake_bot):
    run_say(fake_bot, "<@&555555555555555555> reset tonight")
    assert fake_bot.posts[0].roles == ["555555555555555555"]


def test_it_can_only_notify_the_people_the_message_names(fake_bot):
    """The allow-list is built from the text, so there is nothing else it could ping."""
    run_say(fake_bot, "nobody in particular")
    assert fake_bot.posts[0].allowed_mentions == []
    assert fake_bot.posts[0].roles == []


def test_a_bare_snowflake_is_not_a_mention(fake_bot):
    """Discord only notifies what it renders as a mention; a number is a number."""
    run_say(fake_bot, "run 900000000000000123 was the one")
    assert fake_bot.posts[0].allowed_mentions == []


def test_everyone_stays_blocked(repo):
    """The one mention nobody can opt out of. The allow-list is built inside the
    real client, so this drives `post_plain` itself rather than the fake."""
    channel = RecordingChannel()
    users, roles = mentions_in("@everyone @here <@1001> <@&777>")
    asyncio.run(real_client(repo).post_plain(channel, "x", users, mention_roles=roles))

    (_content, allowed) = channel.sends[0]
    assert allowed.everyone is False
    assert [str(u.id) for u in allowed.users] == ["1001"]
    assert [str(r.id) for r in allowed.roles] == ["777"]


def test_every_other_caller_still_pings_no_roles_at_all(repo):
    """`mention_roles` defaults to nothing, so a decline notice cannot start
    tagging a role because `/say` needed the parameter."""
    channel = RecordingChannel()
    asyncio.run(real_client(repo).post_plain(channel, "can't make it", ["1001"]))

    assert channel.sends[0][1].roles is False


def test_a_channel_the_bot_cannot_post_in_is_refused(fake_bot):
    """And the refusal says what to grant, rather than "couldn't post"."""
    channel = fake_bot.channels[OTHER_CHANNEL]
    channel.permissions.send_messages = False

    interaction = run_say(fake_bot, "hello", channel=channel)
    assert fake_bot.posts == []
    assert "no access" in interaction.response.sent[0][0]
    assert "Send Messages" in interaction.response.sent[0][0]


def test_it_does_not_silently_fall_back_to_the_digest_channel(fake_bot):
    """`find_channel` falls back to POST_CHANNEL_ID, which is right for a
    reminder that must land somewhere and wrong for "post this in #here"."""
    unreachable = fake_bot.channels[OTHER_CHANNEL]
    unreachable.permissions.send_messages = False

    run_say(fake_bot, "hello", channel=unreachable)
    assert [p.channel_id for p in fake_bot.posts] == []
    assert fake_bot.settings.post_channel_id == WATCHED_CHANNEL, "the fallback was available"


def test_an_empty_message_is_refused(fake_bot):
    interaction = run_say(fake_bot, "   ")
    assert fake_bot.posts == []
    assert "Nothing to say" in interaction.response.sent[0][0]


def test_an_overlong_message_is_refused_before_discord_rejects_it(fake_bot):
    interaction = run_say(fake_bot, "x" * (SAY_LIMIT + 1))
    assert fake_bot.posts == []
    assert str(SAY_LIMIT) in interaction.response.sent[0][0]


def test_quiet_mode_silences_it_like_everything_else(repo):
    """Even the one command that is allowed to ping: quiet mode is the switch
    that means "say it all, notify nobody", and it has no exceptions."""
    repo.set_config(CFG_QUIET, "1")
    channel = RecordingChannel()
    said = "<@1001> <@&777> tonight"
    users, roles = mentions_in(said)
    asyncio.run(real_client(repo).post_plain(channel, said, users, mention_roles=roles))

    content, allowed = channel.sends[0]
    assert allowed.users is False and allowed.roles is False
    assert "<@1001>" in content, "it still says the same words"
    assert formatting.QUIET_MARKER in content


# --- /debug is gated the same way -------------------------------------------


def debug_check(bot, user) -> bool:
    return asyncio.run(DebugGroup().interaction_check(FakeInteraction(bot, user=user)))


def test_debug_is_hidden_from_non_admins():
    """It used to be listed for everyone and merely refuse them."""
    group = DebugGroup()
    assert group.default_permissions is not None
    assert group.default_permissions.administrator is True
    assert group.guild_only is True


def test_the_admin_role_may_debug(repo, bosses):
    from .fake_bot import FakeBot

    bot = FakeBot(repo, bosses, make_settings(admin_role_id=ADMIN_ROLE))
    assert debug_check(bot, FakeUser(role_ids=[ADMIN_ROLE])) is True


def test_a_server_admin_may_now_debug(fake_bot):
    """New: the Administrator permission was not a way in before this."""
    assert debug_check(fake_bot, FakeUser(administrator=True)) is True


def test_an_ordinary_bosser_may_not_debug(fake_bot):
    with pytest.raises(DebugNotAllowed):
        debug_check(fake_bot, FakeUser(role_ids=[BOSSING_ROLE]))


def test_a_listed_tester_keeps_the_access_they_were_given(repo, bosses):
    """`DEBUG_USER_IDS` is an operator's deliberate exception, not a leak."""
    from .fake_bot import FakeBot

    bot = FakeBot(repo, bosses, make_settings(debug_user_ids=str(MEMBER_ID)))
    assert debug_check(bot, FakeUser()) is True


def test_the_old_five_argument_rule_still_reads_the_same(fake_bot):
    """`is_guild_admin` was added last with a default, so nothing that called
    `may_debug` positionally changed meaning."""
    assert may_debug(MEMBER_ID, [], MEMBER_ID, None, []) is True
    assert may_debug(MEMBER_ID, [], None, None, []) is False


def test_the_signature_is_the_two_options_a_person_would_expect():
    parameters = {p.name for p in say.parameters}
    assert parameters == {"message", "channel"}
    assert not next(p for p in say.parameters if p.name == "channel").required
    assert isinstance(say, app_commands.Command)
