"""Quiet mode: post everything, notify nobody.

For developing against the live guild. The bot must keep saying exactly what it
would have said -- same cards, same wording, `<@id>` still rendering as a
highlighted name -- while every message goes out with an empty mention
allow-list, so a week of testing never puts a red badge on anyone's Discord.

Enforced at the two places the bot builds a non-empty allow-list, and marked on
the message so nobody forgets it is on.
"""

from __future__ import annotations

import asyncio

import discord
import pytest

from bot import formatting
from bot.client import CFG_QUIET, BossBot
from bot.db import Repo

from .fake_bot import WATCHED_CHANNEL

WHO = ["1001", "1002"]


class Sent:
    """One captured `channel.send`, so a test can look at its allow-list."""

    def __init__(self, kwargs):
        self.allowed: discord.AllowedMentions = kwargs["allowed_mentions"]
        self.content: str = kwargs.get("content") or ""
        self.embed = kwargs.get("embed")


class FakeMessage:
    id = 900000000000000321

    async def add_reaction(self, emoji):
        return None


class RecordingChannel:
    id = WATCHED_CHANNEL

    def __init__(self):
        self.sends: list[Sent] = []

    async def send(self, content=None, **kwargs):
        self.sends.append(Sent({"content": content, **kwargs}))
        return FakeMessage()


@pytest.fixture
def bot(repo: Repo):
    """A real client with only what `_post` / `post_plain` reach for."""
    client = BossBot.__new__(BossBot)
    client.repo = repo
    return client


def quiet(repo: Repo, on: bool) -> None:
    repo.set_config(CFG_QUIET, "1" if on else "0")


def a_card() -> formatting.Card:
    return formatting.Card(
        content="HStar + HFA tonight <@1001> <@1002>",
        title="Tonight",
        footer=formatting.REACT_HINT,
        mention_users=list(WHO),
    )


# --- the switch itself ------------------------------------------------------


def test_quiet_mode_is_off_until_it_is_turned_on(bot, repo):
    assert bot.quiet_mode is False
    quiet(repo, True)
    assert bot.quiet_mode is True
    quiet(repo, False)
    assert bot.quiet_mode is False


# --- _post: cards -----------------------------------------------------------


def test_a_card_normally_notifies_the_people_it_names(bot, repo):
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card()))
    (sent,) = channel.sends
    assert [str(u.id) for u in sent.allowed.users] == WHO


def test_quiet_mode_empties_the_allow_list(bot, repo):
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card()))
    (sent,) = channel.sends
    # `none()` rather than `users=[]`: it also clears `replied_user`, which
    # defaults to on and would otherwise notify whoever is being replied to.
    assert sent.allowed.users is False
    assert sent.allowed.replied_user is False
    assert sent.allowed.everyone is False and sent.allowed.roles is False


def test_quiet_mode_leaves_the_words_alone(bot, repo):
    """The card must still read the same -- names included."""
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card()))
    assert channel.sends[0].content == a_card().content


def test_a_quiet_card_says_so_in_its_footer(bot, repo):
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card()))
    footer = channel.sends[0].embed.footer.text
    assert formatting.QUIET_MARKER in footer
    # The footer it already had is kept, not replaced.
    assert formatting.REACT_HINT in footer


def test_a_loud_card_carries_no_marker(bot, repo):
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card()))
    assert formatting.QUIET_MARKER not in channel.sends[0].embed.footer.text


def test_an_explicit_mention_override_cannot_escape_quiet_mode(bot, repo):
    """`_post(..., mention_users=[...])` bypasses the card's own list."""
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, a_card(), mention_users=["1003"]))
    assert channel.sends[0].allowed.users is False


def test_a_plain_string_card_gets_the_marker_in_its_content(bot, repo):
    """No embed to hold a footer, so the note goes on the message itself."""
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot._post(channel, "just a line <@1001>"))
    assert formatting.QUIET_MARKER in channel.sends[0].content
    assert "just a line <@1001>" in channel.sends[0].content


# --- post_plain: decline notices and portal notices --------------------------


def test_a_plain_post_normally_notifies(bot, repo):
    channel = RecordingChannel()
    asyncio.run(bot.post_plain(channel, "can't make it", list(WHO)))
    assert [str(u.id) for u in channel.sends[0].allowed.users] == WHO


def test_quiet_mode_covers_plain_posts_too(bot, repo):
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot.post_plain(channel, "can't make it", list(WHO)))
    (sent,) = channel.sends
    assert sent.allowed.users is False
    assert formatting.QUIET_MARKER in sent.content
    assert "can't make it" in sent.content


def test_a_quiet_reply_does_not_ping_the_person_replied_to(bot, repo):
    """The reason for `none()`: a reply notifies its target by default."""
    quiet(repo, True)
    channel = RecordingChannel()
    asyncio.run(bot.post_plain(channel, "hi", list(WHO), reference_id=123456))
    assert channel.sends[0].allowed.replied_user is False


# --- the formatting helpers -------------------------------------------------


def test_quieting_a_card_also_empties_its_own_allow_list():
    marked = formatting.quieted(a_card())
    assert marked.mention_users == []
    assert a_card().mention_users == WHO, "the original must not be mutated"


def test_quieting_a_footerless_card_does_not_invent_an_embed():
    plain = formatting.Card(content="hello")
    assert formatting.quieted(plain).has_embed is False


# --- toggling it, end to end ------------------------------------------------


def test_the_api_round_trips_the_toggle(auth, fake_bot):
    assert auth.get("/api/config").json()["quiet_mode"] is False
    body = auth.put("/api/config", json={"quiet_mode": True}).json()
    assert body["quiet_mode"] is True
    assert fake_bot.quiet_mode is True
    assert auth.put("/api/config", json={"quiet_mode": False}).json()["quiet_mode"] is False
    assert fake_bot.quiet_mode is False


def test_the_portal_page_toggles_it(auth, fake_bot):
    response = auth.post("/config", data={"quiet_mode": "1"}, follow_redirects=False)
    assert response.status_code in (302, 303)
    assert fake_bot.quiet_mode is True


def test_the_portal_page_shows_which_way_it_is(auth, fake_bot):
    assert "Turn quiet mode on" in auth.get("/config").text
    fake_bot.repo.set_config(CFG_QUIET, "1")
    page = auth.get("/config").text
    assert "Turn quiet mode off" in page
    assert formatting.QUIET_MARKER in page


def test_it_is_a_known_setting(fake_bot):
    from bot.api import service

    assert "quiet_mode" in service.CONFIG_KEYS
    assert service.set_config(fake_bot, "quiet_mode", "on")["quiet_mode"] is True
    assert service.set_config(fake_bot, "quiet_mode", "off")["quiet_mode"] is False


@pytest.mark.parametrize(
    "value,expected",
    [(True, True), ("on", True), ("1", True), (False, False), ("off", False), ("0", False)],
)
def test_the_words_bossctl_sends_all_land_the_right_way(auth, value, expected):
    """`bossctl config set quiet_mode on|off` converts to a bool; the API also
    accepts the raw words, so neither spelling can silently turn it the wrong way."""
    assert auth.put("/api/config", json={"quiet_mode": value}).json()["quiet_mode"] is expected


def test_the_cli_lists_it_among_the_flag_settings():
    """Otherwise `bossctl config set quiet_mode on` would send a bare string."""
    import inspect

    from bot import cli

    assert '"quiet_mode"' in inspect.getsource(cli.config_set)
