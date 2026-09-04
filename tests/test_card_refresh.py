"""Posted reminder cards follow the database (the "frozen 2/4 ✅" bug).

A card is built once and then sits in the channel saying whatever was true when
it was posted. Live: everybody ✅'d the morning card, the reactions all
registered, the run went `confirmed` -- and the card still read "✅ confirmed ·
2/4 ✅" hours later, because nothing ever rewrote it.

The fix hooks the *writes* rather than the callers: every path that changes what
a card shows goes through `Repo.set_rsvp` / `clear_rsvp` / `set_run_status` /
`set_run_participants`, so a reaction, `/rsvp`, the portal and a chat-extracted
"Can" all queue the same re-render without any of them knowing it exists.
"""

from __future__ import annotations

import asyncio
from datetime import datetime, timedelta
from types import SimpleNamespace

import pytest

from bot.agent import formatting
from bot.agent.client import BossBot
from bot.agent.debug import TEST_PREFIX
from bot.agent.materialise import DAY_OF, countdown_kind
from bot.agent.rsvp import EMOJI_NO, EMOJI_YES, apply_reaction, recompute_after_roster_change
from bot.domain.timeutil import utcnow
from bot.domain.weeks import week_start
from bot.infrastructure.db import Repo

from .conftest import RESET_TIME, RESET_WEEKDAY, TZ, kl
from .fake_bot import WATCHED_CHANNEL

PARTICIPANTS = ["1001", "1002"]
CARD_ID = 900000000000000123
COUNTDOWN_ID = 900000000000000456
#: A run whose night has been and gone: its cards are a record, not a form.
LAST_YEAR = kl(2026, 1, 1, 21, 30)


def tonight(hours: float = 3) -> datetime:
    """A run time still ahead of the real clock, whenever the suite is run.

    Deliberately relative: `refresh_run_cards` leaves finished runs alone, so a
    hard-coded evening turns these tests into a bomb that goes off at 23:30 on
    the day in question -- which is exactly what happened to the fixtures this
    file was first written beside.
    """
    return utcnow().astimezone(TZ) + timedelta(hours=hours)


# --- fake Discord ----------------------------------------------------------


class FakeMessage:
    def __init__(self, message_id: int):
        self.id = message_id
        self.content = "posted at nine in the morning"
        self.embeds: list[object] = []
        self.edits: list[dict] = []

    async def edit(self, **kwargs):
        self.edits.append(kwargs)
        self.content = kwargs.get("content", self.content)
        return self


class FakeChannel:
    """Hands back messages by id, or raises what Discord would raise."""

    def __init__(self, *message_ids: int):
        self.id = WATCHED_CHANNEL
        self.messages = {mid: FakeMessage(mid) for mid in message_ids}

    async def fetch_message(self, message_id: int):
        import discord

        try:
            return self.messages[int(message_id)]
        except KeyError:
            gone = SimpleNamespace(status=404, reason="Not Found")
            raise discord.NotFound(gone, "unknown message") from None


class RefreshBot(BossBot):
    """A client with just the pieces `refresh_run_cards` reaches for."""

    user = SimpleNamespace(id=5555555555555555555)


@pytest.fixture
def channel() -> FakeChannel:
    return FakeChannel(CARD_ID, COUNTDOWN_ID)


@pytest.fixture
def bot(repo: Repo, bosses, channel: FakeChannel):
    client = RefreshBot.__new__(RefreshBot)
    client.repo = repo
    client.bosses = bosses
    client.tz = TZ
    client._stale_cards = set()
    client._card_refresh = None
    client.get_channel = lambda channel_id: channel
    repo.on_run_changed = client.card_needs_refresh
    return client


def a_run(repo: Repo, participants: list[str] | None = None, at: datetime | None = None) -> dict:
    at = at or tonight()
    run_id = repo.create_run(
        week_start(at, TZ, RESET_WEEKDAY, RESET_TIME),
        ["HStar", "HFA"],
        at,
        participants or PARTICIPANTS,
        channel_id=WATCHED_CHANNEL,
    )
    return repo.get_run(run_id)


def posted(repo: Repo, run: dict, kind: str = DAY_OF, message_id: int = CARD_ID) -> str:
    """A reminder that has already gone out as ``message_id``."""
    reminder = repo.add_reminder(run["id"], kind, run["datetime"] - timedelta(hours=12))
    repo.mark_reminder_sent(reminder, message_id)
    return reminder


def refresh(bot, run_id) -> int:
    return asyncio.run(bot.refresh_run_cards(run_id))


# --- the funnel: every source marks the run stale ---------------------------


def recorder(repo: Repo) -> list[str]:
    seen: list[str] = []
    repo.on_run_changed = seen.append
    return seen


def test_a_reaction_marks_the_run_stale(repo):
    run = a_run(repo)
    seen = recorder(repo)
    apply_reaction(repo, run, "1001", EMOJI_YES, added=True)
    assert run["id"] in seen


def test_taking_a_reaction_back_marks_it_too(repo):
    run = a_run(repo)
    apply_reaction(repo, run, "1001", EMOJI_YES, added=True)
    seen = recorder(repo)
    apply_reaction(repo, repo.get_run(run["id"]), "1001", EMOJI_YES, added=False)
    assert run["id"] in seen


def test_the_portal_and_slash_rsvp_path_marks_it(repo):
    """`/rsvp`, `PATCH /api/runs/../rsvp` and a chat "Can" all land on `set_rsvp`."""
    run = a_run(repo)
    seen = recorder(repo)
    repo.set_rsvp(run["id"], "1002", "yes", source="chat")
    assert seen == [run["id"]]


def test_a_status_change_marks_it(repo):
    """Cancelled, own-time, back on: the card shows the status too."""
    run = a_run(repo)
    seen = recorder(repo)
    repo.set_run_status(run["id"], "cancelled")
    assert seen == [run["id"]]


def test_a_line_up_change_marks_it(repo):
    """A `/swap` changes who the card names, and can change the status with it."""
    run = a_run(repo)
    seen = recorder(repo)
    repo.set_run_participants(run["id"], ["1001", "1002", "1003"])
    recompute_after_roster_change(repo, run["id"])
    assert seen[0] == run["id"]


def test_a_hook_that_throws_does_not_break_the_write(repo):
    """A stale card is cosmetic; losing the ✅ that caused it would not be."""
    run = a_run(repo)

    def explode(_run_id):
        raise RuntimeError("no loop, no channel, no idea")

    repo.on_run_changed = explode
    repo.set_rsvp(run["id"], "1001", "yes")
    assert repo.get_rsvps(run["id"]) == {"1001": "yes"}


def test_writing_from_a_cli_or_a_test_queues_nothing(bot, repo):
    """No event loop is running, so there is nothing posted to keep in step."""
    run = a_run(repo)
    repo.set_rsvp(run["id"], "1001", "yes")
    assert bot._stale_cards == set()


# --- re-rendering ----------------------------------------------------------


def test_the_morning_card_is_rewritten_with_the_current_tally(bot, repo, channel):
    run = a_run(repo)
    posted(repo, run)
    for user_id in PARTICIPANTS:
        apply_reaction(repo, repo.get_run(run["id"]), user_id, EMOJI_YES, added=True)

    assert refresh(bot, run["id"]) == 1
    embed = channel.messages[CARD_ID].edits[-1]["embed"]
    body = "\n".join(field.value for field in embed.fields)
    assert "2/2 ✅" in body
    assert formatting.STATUS_LABEL["confirmed"] in body


def test_the_countdown_is_rewritten_too(bot, repo, channel):
    run = a_run(repo)
    posted(repo, run, kind=countdown_kind(60), message_id=COUNTDOWN_ID)
    apply_reaction(repo, run, "1002", EMOJI_NO, added=True)

    assert refresh(bot, run["id"]) == 1
    content = channel.messages[COUNTDOWN_ID].edits[-1]["content"]
    assert "<@1002> out" in content


def test_a_grouped_morning_card_keeps_every_run_on_it(bot, repo, channel):
    """One message covers the whole channel's night. Re-rendering it from the
    one run that changed would silently drop the others off the card."""
    star = a_run(repo)
    kalos = a_run(repo, participants=["1002"], at=tonight(4))
    repo.set_run_bosses(kalos["id"], ["XKalos"])
    for run in (star, repo.get_run(kalos["id"])):
        reminder = repo.add_reminder(run["id"], DAY_OF, run["datetime"] - timedelta(hours=12))
        repo.mark_reminder_sent(reminder, CARD_ID)

    apply_reaction(repo, star, "1001", EMOJI_YES, added=True)
    refresh(bot, star["id"])

    embed = channel.messages[CARD_ID].edits[-1]["embed"]
    names = "\n".join(field.name for field in embed.fields)
    assert "HStar" in names and "XKalos" in names


def test_both_of_a_runs_cards_are_refreshed(bot, repo, channel):
    run = a_run(repo)
    posted(repo, run)
    posted(repo, run, kind=countdown_kind(60), message_id=COUNTDOWN_ID)

    assert refresh(bot, run["id"]) == 2


def test_a_reminder_that_never_went_out_is_left_alone(bot, repo, channel):
    run = a_run(repo)
    repo.add_reminder(run["id"], DAY_OF, kl(2026, 8, 31, 9, 0))  # queued, not sent

    assert refresh(bot, run["id"]) == 0


def test_a_card_somebody_deleted_is_skipped_quietly(bot, repo):
    run = a_run(repo)
    posted(repo, run, message_id=424242)  # not in the fake channel

    assert refresh(bot, run["id"]) == 0


def test_last_nights_cards_are_not_touched(bot, repo, channel):
    """The tally on a finished run's card is a record of the night, not a form."""
    run = a_run(repo, at=LAST_YEAR)
    posted(repo, run)

    assert refresh(bot, run["id"]) == 0
    assert channel.messages[CARD_ID].edits == []


def test_a_run_that_has_gone_away_refreshes_nothing(bot, repo):
    assert refresh(bot, "no-such-run") == 0


# --- /debug ping cards go stale in exactly the same way ---------------------


def test_a_test_card_is_refreshed_too(bot, repo, channel):
    """The card the "frozen 2/4 ✅" report was actually about: the live message
    was a `/debug ping`, which is deliberately not a reminder row, so walking
    the reminders table alone would have left the reported bug in place."""
    run = a_run(repo)
    repo.add_debug_message(CARD_ID, run["id"], WATCHED_CHANNEL, countdown_kind(15))
    for user_id in PARTICIPANTS:
        apply_reaction(repo, repo.get_run(run["id"]), user_id, EMOJI_YES, added=True)

    assert refresh(bot, run["id"]) == 1
    content = channel.messages[CARD_ID].edits[-1]["content"]
    assert "everyone's confirmed" in content


def test_a_refreshed_test_card_still_says_it_is_a_test(bot, repo, channel):
    run = a_run(repo)
    repo.add_debug_message(CARD_ID, run["id"], WATCHED_CHANNEL, DAY_OF)
    apply_reaction(repo, run, "1001", EMOJI_YES, added=True)

    refresh(bot, run["id"])
    assert channel.messages[CARD_ID].edits[-1]["content"].startswith(TEST_PREFIX)


def test_a_rehearsal_does_not_start_notifying_people_when_it_is_refreshed(bot, repo, channel):
    """`/debug ping` names the party without summoning it. An edit that
    reached for the day-of audience would quietly change that."""
    run = a_run(repo)
    repo.add_debug_message(CARD_ID, run["id"], WATCHED_CHANNEL, DAY_OF)
    apply_reaction(repo, run, "1001", EMOJI_YES, added=True)

    refresh(bot, run["id"])
    assert channel.messages[CARD_ID].edits[-1]["allowed_mentions"].users == []


def test_a_test_notice_with_no_tally_is_left_alone(bot, repo, channel):
    """An amend or decline rehearsal is a sentence, not a scoreboard."""
    run = a_run(repo)
    repo.add_debug_message(CARD_ID, run["id"], WATCHED_CHANNEL, "amend")

    assert refresh(bot, run["id"]) == 0


# --- edits must not become a second way to ping -----------------------------


def test_an_edit_carries_the_same_allow_list_as_the_send(bot, repo, channel):
    run = a_run(repo)
    repo.upsert_member(1001, "Alvin tan", None, True)
    repo.upsert_member(1002, "kanon [AZUR]", "kanon", True)
    posted(repo, run)

    refresh(bot, run["id"])
    allowed = channel.messages[CARD_ID].edits[-1]["allowed_mentions"]
    assert [str(u.id) for u in allowed.users] == PARTICIPANTS
    assert allowed.everyone is False and allowed.roles is False


def test_quiet_mode_covers_the_edit_too(bot, repo, channel):
    from bot.agent.client import CFG_QUIET

    run = a_run(repo)
    posted(repo, run)
    repo.set_config(CFG_QUIET, "1")

    refresh(bot, run["id"])
    edit = channel.messages[CARD_ID].edits[-1]
    assert edit["allowed_mentions"].users is False
    assert formatting.QUIET_MARKER in edit["embed"].footer.text


# --- the queue --------------------------------------------------------------


def test_one_edit_for_a_burst_of_writes(bot, repo, channel):
    """A reaction writes the rsvp row and then the status; the drain task only
    starts once the handler yields, so both collapse into one edit."""
    run = a_run(repo)
    posted(repo, run)

    async def react_then_settle():
        for user_id in PARTICIPANTS:
            apply_reaction(repo, repo.get_run(run["id"]), user_id, EMOJI_YES, added=True)
        await asyncio.sleep(0)  # let the drain task run
        await asyncio.sleep(0)

    asyncio.run(react_then_settle())
    (edit,) = channel.messages[CARD_ID].edits
    assert "2/2 ✅" in "\n".join(field.value for field in edit["embed"].fields)
