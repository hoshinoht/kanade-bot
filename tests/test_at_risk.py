"""What `at_risk` does once somebody has said no (DESIGN.md §3).

A run is `at_risk` when a participant declined it: the night is still on, but
the party has a decision to make. That has to stay visible everywhere the run
appears -- and has to *stop* being visible the moment the decline is taken
back, which is where the reaction path was quietly leaving a stale "can't make
it - reschedule?" notice standing in the channel.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

import pytest

from bot import formatting
from bot.client import BossBot
from bot.db import Repo
from bot.materialise import DAY_OF, reminder_specs
from bot.rsvp import EMOJI_NO, EMOJI_YES, apply_reaction

from .conftest import COUNTDOWNS, PING_TIME, TZ, kl
from .fake_bot import WATCHED_CHANNEL

PARTICIPANTS = ["1001", "1002"]
RUN_AT = kl(2026, 8, 31, 21, 30)
WEEK = kl(2026, 8, 27)
CARD_ID = 900000000000000777


@pytest.fixture
def run(repo: Repo) -> dict:
    """A run everyone has ✅'d, with a morning card they reacted on."""
    run_id = repo.create_run(
        WEEK, ["HStar", "HFA"], RUN_AT, PARTICIPANTS, channel_id=WATCHED_CHANNEL
    )
    for user_id in PARTICIPANTS:
        apply_reaction(repo, repo.get_run(run_id), user_id, EMOJI_YES, added=True)
    reminder = repo.add_reminder(run_id, DAY_OF, kl(2026, 8, 31, 9, 0))
    repo.mark_reminder_sent(reminder, CARD_ID)
    return repo.get_run(run_id)


class ReactionBot(BossBot):
    """``Client.user`` is a read-only property fed by the gateway; there is none here."""

    user = SimpleNamespace(id=5555555555555555555)


@pytest.fixture
def bot(repo: Repo):
    """A client with the decline notices stubbed, so a test can see the calls."""
    client = ReactionBot.__new__(ReactionBot)
    client.repo = repo
    client.notified: list[tuple[str, str]] = []
    client.retracted: list[tuple[str, str]] = []

    async def notify_decline(run, user_id, display_name, channel_id=None, reference_id=None):
        client.notified.append((run["id"], str(user_id)))

    async def retract_decline(run, user_id):
        client.retracted.append((run["id"], str(user_id)))

    async def drop_opposite(payload, emoji):
        return None

    client.notify_decline = notify_decline
    client.retract_decline = retract_decline
    client._drop_opposite_reaction = drop_opposite
    return client


def react(bot, emoji: str, added: bool, user_id: str = "1002") -> None:
    payload = SimpleNamespace(
        user_id=int(user_id),
        emoji=emoji,
        message_id=CARD_ID,
        channel_id=WATCHED_CHANNEL,
        member=None,
    )
    asyncio.run(bot._handle_reaction(payload, added=added))


# --- ❌ and back again ------------------------------------------------------


def test_a_cross_puts_the_run_at_risk_and_tells_the_others(bot, repo, run):
    react(bot, EMOJI_NO, added=True)

    assert repo.get_run(run["id"])["status"] == "at_risk"
    assert bot.notified == [(run["id"], "1002")]


def test_taking_the_cross_back_off_takes_the_notice_down_with_it(bot, repo, run):
    """The gap: only a ✅ retracted the notice, so un-reacting ❌ left the party
    re-planning a night around somebody who was available again."""
    react(bot, EMOJI_NO, added=True)
    react(bot, EMOJI_NO, added=False)

    assert bot.retracted == [(run["id"], "1002")]


def test_at_risk_is_not_sticky_the_way_confirmed_is(bot, repo, run):
    """Their answer is gone rather than yes, so the run is back to unanswered --
    `confirmed` survives an incomplete tally, but nothing was confirmed here."""
    react(bot, EMOJI_NO, added=True)
    react(bot, EMOJI_NO, added=False)

    assert repo.get_rsvps(run["id"]) == {"1001": "yes"}
    assert repo.get_run(run["id"])["status"] == "planned"


def test_switching_to_a_tick_confirms_the_run_and_retracts_the_notice(bot, repo, run):
    react(bot, EMOJI_NO, added=True)
    react(bot, EMOJI_YES, added=True)

    assert repo.get_run(run["id"])["status"] == "confirmed"
    assert bot.retracted == [(run["id"], "1002")]


def test_the_stale_remove_discord_sends_after_a_switch_retracts_nothing_twice(bot, repo, run):
    """Discord follows the ✅ with a remove(❌) of its own; the run has already
    recovered, and the notice has already gone."""
    react(bot, EMOJI_NO, added=True)
    react(bot, EMOJI_YES, added=True)
    react(bot, EMOJI_NO, added=False)

    assert bot.retracted == [(run["id"], "1002")]
    assert repo.get_run(run["id"])["status"] == "confirmed"


def test_withdrawing_a_tick_is_not_a_retracted_decline(bot, repo, run):
    """Silence is not "I'm back in": there was no decline to take back."""
    react(bot, EMOJI_YES, added=False)

    assert bot.retracted == []
    assert bot.notified == []


# --- how an at-risk run reads ----------------------------------------------


def at_risk_run(repo: Repo) -> dict:
    run_id = repo.create_run(WEEK, ["HStar", "HFA"], RUN_AT, PARTICIPANTS)
    repo.set_rsvp(run_id, "1002", "no")
    repo.set_run_status(run_id, "at_risk")
    return repo.get_run(run_id)


def test_the_morning_card_says_at_risk_rather_than_unconfirmed(repo, bosses):
    run = at_risk_run(repo)
    card = formatting.day_of_card([run], TZ, {run["id"]: repo.get_rsvps(run["id"])}, table=bosses)

    body = "\n".join(value for _, value in card.fields)
    assert formatting.STATUS_LABEL["at_risk"] in body
    assert formatting.STATUS_LABEL["planned"] not in body


def test_the_countdown_says_it_too(repo, bosses):
    run = at_risk_run(repo)
    card = formatting.countdown_card(run, 60, TZ, repo.get_rsvps(run["id"]), table=bosses)

    assert formatting.STATUS_LABEL["at_risk"] in card.description


def test_the_countdown_does_not_ping_the_person_who_declined(repo):
    """Built exactly as `_send_countdown` builds it: everyone else on the run is
    pinged, and the decliner is named -- the party can see who is out -- but
    their phone stays quiet."""
    from bot.pings import audience

    repo.upsert_member(1001, "Alvin tan", None, True)
    repo.upsert_member(1002, "kanon [AZUR]", "kanon", True)
    run = at_risk_run(repo)
    rsvps = repo.get_rsvps(run["id"])

    who = audience(
        repo, run["participants"], "countdown", candidates=formatting.not_declined(run, rsvps)
    )
    card = formatting.countdown_card(run, 60, TZ, rsvps, who=who)

    assert who.mentioned == ("1001",)
    assert card.mention_users == ["1001"]
    assert "<@1001>" in card.content
    assert "<@1002>" not in card.content
    assert "kanon out" in card.content


def test_an_at_risk_run_keeps_all_of_its_reminders(repo):
    """It is still happening -- only `otot` and `cancelled` lose pings. Whoever
    has not answered still needs asking, which is the point of the countdown."""
    kinds = [spec.kind for spec in reminder_specs(RUN_AT, "at_risk", TZ, PING_TIME, COUNTDOWNS)]
    planned = [spec.kind for spec in reminder_specs(RUN_AT, "planned", TZ, PING_TIME, COUNTDOWNS)]
    assert kinds == planned
