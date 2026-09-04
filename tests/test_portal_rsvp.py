"""The week row as an answer sheet: who is on, what the bot has said, and fixing it.

Three things a run row has to answer at a glance -- who has not answered yet,
who is out, and whether tonight's cards actually went out -- plus the one thing
it has to let you do about it: record somebody's answer for them when they
reacted by accident or told you in-game instead.
"""

from __future__ import annotations

from datetime import time, timedelta

import pytest

from bot.agent.materialise import DAY_OF, countdown_kind, reconcile_day_of
from bot.api import service
from bot.portal_styles import build_stylesheet

from .conftest import TZ, kl
from .fake_bot import WATCHED_CHANNEL

PAGE_CSS = build_stylesheet()


def row_for(body: str, short_id: str) -> str:
    """Just one run's markup out of the week page."""
    start = body.index(f'id="run-{short_id}"')
    start = body.rindex("<article", 0, start)
    return body[start : body.index("</article>", start)]


def star_row(auth, fake_bot, seeded) -> str:
    run = fake_bot.repo.get_run(seeded["run_star"])
    return row_for(auth.get("/").text, service.short_id(run["id"]))


# --- item 1: every participant's answer, not just the ticks -----------------


def test_a_row_says_which_people_have_not_answered(auth, fake_bot, seeded):
    """The seeded run has one yes and one silence, and both must be legible."""
    row = star_row(auth, fake_bot, seeded)

    assert "chip chip--yes" in row
    assert "chip chip--waiting" in row
    assert "no answer yet" in row


def test_each_state_gets_a_shape_as_well_as_a_colour(auth, fake_bot, seeded):
    """Colour alone excludes anyone who cannot separate green from red."""
    repo = fake_bot.repo
    repo.set_rsvp(seeded["run_star"], "1002", "no")
    row = star_row(auth, fake_bot, seeded)

    assert "●" in row and "✕" in row
    assert "chip chip--no" in row


def test_a_maybe_is_neither_a_yes_nor_a_silence(auth, fake_bot, seeded):
    repo = fake_bot.repo
    repo.set_rsvp(seeded["run_star"], "1002", "maybe")
    row = star_row(auth, fake_bot, seeded)

    assert "chip chip--maybe" in row
    assert "1 maybe" in row


def test_the_meta_line_counts_the_silence(auth, fake_bot, seeded):
    row = star_row(auth, fake_bot, seeded)
    assert "1/2 on" in row
    assert "1 waiting" in row


def test_the_view_counts_maybes_of_its_own(fake_bot, seeded):
    fake_bot.repo.set_rsvp(seeded["run_star"], "1002", "maybe")
    run = service.run_view(fake_bot, fake_bot.repo.get_run(seeded["run_star"]))

    assert (run["yes"], run["no"], run["maybe"], run["unanswered"]) == (1, 0, 1, 0)


# --- item 2: the cards a run has produced -----------------------------------


def sent_card(fake_bot, run_id, kind=DAY_OF, message_id=900000000000000321):
    reminder = fake_bot.repo.add_reminder(run_id, kind, kl(2026, 8, 27, 9, 0))
    fake_bot.repo.mark_reminder_sent(reminder, message_id)
    return reminder


def test_a_queued_card_is_one_that_has_not_been_said_yet(fake_bot, seeded):
    cards = service.run_cards(fake_bot, fake_bot.repo.get_run(seeded["run_star"]))

    assert [c["state"] for c in cards] == ["queued"] * len(cards)
    assert {c["label"] for c in cards} == {"morning", "T-1h", "T-15m"}


def test_a_posted_card_links_to_the_message_itself(fake_bot, seeded):
    fake_bot.repo.delete_reminders(seeded["run_star"])
    sent_card(fake_bot, seeded["run_star"])
    (card,) = service.run_cards(fake_bot, fake_bot.repo.get_run(seeded["run_star"]))

    assert card["state"] == "posted"
    assert card["url"] == (
        f"https://discord.com/channels/{fake_bot.settings.guild_id}"
        f"/{WATCHED_CHANNEL}/900000000000000321"
    )
    assert card["local_sent_at"]


def test_a_card_that_fired_into_the_void_says_so(fake_bot, seeded):
    """Retired without a message: the host was asleep, or the run changed. It is
    the one state that explains a ping nobody remembers getting."""
    fake_bot.repo.delete_reminders(seeded["run_star"])
    reminder = fake_bot.repo.add_reminder(seeded["run_star"], DAY_OF, kl(2026, 8, 27, 9, 0))
    fake_bot.repo.mark_reminder_sent(reminder)  # sent, but no message id
    (card,) = service.run_cards(fake_bot, fake_bot.repo.get_run(seeded["run_star"]))

    assert card["state"] == "skipped"
    assert card["url"] is None


def test_a_skipped_morning_reopened_by_a_later_ping_time_is_queued(fake_bot, seeded):
    run_id = seeded["run_star"]
    repo = fake_bot.repo
    repo.delete_reminders(run_id)
    repo.delete_reminders(seeded["run_kalos"])
    repo.set_run_datetime(run_id, kl(2026, 9, 7, 21, 30), seeded["week_start"])
    reminder = repo.add_reminder(run_id, DAY_OF, kl(2026, 9, 7, 9, 0))
    now = kl(2026, 9, 7, 9, 30)
    repo.mark_reminder_sent(reminder, at=now)

    assert reconcile_day_of(repo, fake_bot.tz, time(10, 0), now=now) == 1

    (card,) = service.run_cards(fake_bot, repo.get_run(run_id))
    assert card["state"] == "queued"
    assert card["message_id"] is None


def test_the_countdowns_are_named_the_way_the_countdown_itself_reads(fake_bot, seeded):
    assert service.card_label(DAY_OF) == "morning"
    assert service.card_label(countdown_kind(60)) == "T-1h"
    assert service.card_label(countdown_kind(90)) == "T-1h30m"
    assert service.card_label(countdown_kind(15)) == "T-15m"


def test_the_row_shows_the_cards_with_a_link_out(auth, fake_bot, seeded):
    fake_bot.repo.delete_reminders(seeded["run_star"])
    sent_card(fake_bot, seeded["run_star"])
    row = star_row(auth, fake_bot, seeded)

    assert "cardlink--posted" in row
    assert "discord.com/channels/" in row
    assert 'rel="noopener"' in row


def test_the_row_marks_what_has_not_gone_out_yet(auth, fake_bot, seeded):
    row = star_row(auth, fake_bot, seeded)
    assert "cardlink--queued" in row
    assert "morning" in row and "T-1h" in row


def test_a_run_with_no_reminders_shows_no_card_strip(auth, fake_bot, seeded):
    fake_bot.repo.delete_reminders(seeded["run_star"])
    row = star_row(auth, fake_bot, seeded)

    assert "run__cards" not in row


def test_the_api_carries_the_cards_too(auth, fake_bot, seeded):
    """So `bossctl` can grow the same view without a second source of truth."""
    fake_bot.repo.delete_reminders(seeded["run_star"])
    sent_card(fake_bot, seeded["run_star"])
    body = auth.get(f"/api/runs/{seeded['run_star']}").json()

    (card,) = body["cards"]
    assert card["label"] == "morning"
    assert card["state"] == "posted"
    assert card["url"].endswith("/900000000000000321")


# --- item 3: recording an answer from the row -------------------------------


def answer(auth, run_id, user_id="1002", value="yes", **extra):
    return auth.post(
        f"/runs/{run_id}/rsvp",
        data={"user_id": user_id, "answer": value, "next": "/", **extra},
        headers={"HX-Request": "true"},
    )


def test_recording_an_answer_updates_the_row_in_place(auth, fake_bot, seeded):
    response = answer(auth, seeded["run_star"])

    assert response.status_code == 200
    assert response.text.strip().startswith('<article class="run run--confirmed"')
    assert fake_bot.repo.get_rsvps(seeded["run_star"])["1002"] == "yes"


def test_marking_someone_out_puts_the_run_at_risk_and_tells_the_others(auth, fake_bot, seeded):
    answer(auth, seeded["run_star"], value="no")

    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "at_risk"
    assert fake_bot.declines[-1] == (seeded["run_star"], "1002")


def test_clearing_takes_the_answer_off_rather_than_recording_a_maybe(auth, fake_bot, seeded):
    """The fix for a ✅ left by accident: they go back to not having answered."""
    answer(auth, seeded["run_star"], value="yes")
    answer(auth, seeded["run_star"], value="clear")

    assert "1002" not in fake_bot.repo.get_rsvps(seeded["run_star"])
    run = fake_bot.repo.get_run(seeded["run_star"])
    # Still `confirmed`: withdrawing an answer is silence, and silence does not
    # argue a run down that somebody decided was on. Only a "no" does.
    assert run["status"] == "confirmed"


def test_clearing_a_decline_takes_the_notice_down_too(auth, fake_bot, seeded):
    answer(auth, seeded["run_star"], value="no")
    answer(auth, seeded["run_star"], value="clear")

    assert fake_bot.retractions[-1] == (seeded["run_star"], "1002")
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "planned"


def test_the_panel_stays_open_between_two_answers(auth, fake_bot, seeded):
    """The row is replaced wholesale, so the server has to re-open it or the
    panel snaps shut after every click."""
    response = answer(auth, seeded["run_star"], answers_open="1")
    assert '<details class="answers" open>' in response.text

    closed = answer(auth, seeded["run_star"], value="clear")
    assert '<details class="answers" open>' not in closed.text


def test_the_answers_panel_says_that_it_opens(auth, fake_bot, seeded):
    """The bare word read as a stray label: nothing about it said there was
    anything behind it. It is a control now, with a caret that turns when the
    panel is open and a line saying what opening it is for."""
    row = star_row(auth, fake_bot, seeded)

    assert 'class="btn answers__summary"' in row
    assert "set who" in row  # "Answers — set who's in or out"
    assert 'data-icon="chevron-right"' in row  # the caret is one of the drawings

    turned = PAGE_CSS[PAGE_CSS.index(".answers[open] > .answers__summary .icon {") :]
    assert "transform: rotate(90deg)" in turned[: turned.index("}")]


def test_the_caret_turns_only_as_fast_as_the_reader_allows():
    """One reduced-motion block for the whole stylesheet, so a transition added
    here is already covered by it rather than needing its own query."""
    reduced = PAGE_CSS[PAGE_CSS.index("@media (prefers-reduced-motion: reduce) {") :]

    assert "*::before" in reduced[: reduced.index("\n}\n")]
    assert "transition-duration: 0.001ms !important" in reduced[: reduced.index("\n}\n")]


def test_the_clear_button_is_dead_until_there_is_something_to_clear(auth, fake_bot, seeded):
    row = star_row(auth, fake_bot, seeded)
    # 1001 answered yes in the fixture; 1002 has not answered at all.
    assert row.count('value="clear"') == 2
    assert row.count("disabled>Clear") == 1


def test_a_non_participant_cannot_be_answered_for(auth, fake_bot, seeded):
    response = answer(auth, seeded["run_star"], user_id="1003")

    assert response.status_code == 400
    assert "on run" in response.text  # "Priya isn&#39;t on run a82f29cb"
    assert 'class="flash flash--error"' in response.text


def test_recording_an_answer_refreshes_the_discord_card(fake_bot, seeded):
    """It goes through the same `set_rsvp` the CLI uses, which is what fires the
    card-refresh hook -- the portal and the posted card cannot disagree."""
    seen: list[str] = []
    fake_bot.repo.on_run_changed = seen.append

    import asyncio

    asyncio.run(service.set_rsvp(fake_bot, seeded["run_star"], "1002", "yes"))
    assert seeded["run_star"] in seen


def test_the_api_takes_a_clear_as_well(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "yes"})
    body = auth.post(
        f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "clear"}
    ).json()

    assert [p for p in body["participants"] if p["id"] == "1002"][0]["rsvp"] is None


def test_an_answer_that_is_not_a_word_we_know_is_refused(auth, seeded):
    response = auth.post(
        f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "perhaps"}
    )
    assert response.status_code == 422


@pytest.mark.parametrize("value", ["yes", "no", "maybe", "clear"])
def test_every_answer_the_portal_offers_is_one_the_service_takes(value):
    assert value in service.RSVP_ANSWERS


def test_a_finished_run_still_shows_its_cards(auth, fake_bot, seeded):
    """The row is a record once the night is over, and the link still works."""
    run_id = seeded["run_star"]
    fake_bot.repo.set_run_datetime(
        run_id, kl(2026, 8, 27, 21, 30) - timedelta(days=1), seeded["week_start"]
    )
    fake_bot.repo.delete_reminders(run_id)
    sent_card(fake_bot, run_id)
    cards = service.run_cards(fake_bot, fake_bot.repo.get_run(run_id))

    assert cards[0]["state"] == "posted"


def test_the_local_times_are_the_guilds(fake_bot, seeded):
    fake_bot.repo.delete_reminders(seeded["run_star"])
    sent_card(fake_bot, seeded["run_star"])
    (card,) = service.run_cards(fake_bot, fake_bot.repo.get_run(seeded["run_star"]))

    assert card["local_fire_at"] == kl(2026, 8, 27, 9, 0).astimezone(TZ).strftime("%H:%M")
