"""Proposal / suggestion / 📌 cards (DESIGN.md §2.3 and §2b.3).

The wording matters: a card that reads like a decision when the thread ended on a
question is how a schedule quietly gets rewritten by a 20B model.
"""

from __future__ import annotations

import pytest

from bot.agent.formatting import (
    CONFIRM_HINT,
    TBD,
    card_kind,
    proposal_card,
    proposal_line,
    when_text,
)
from bot.domain.ids import short_id

from .conftest import TZ, kl

MY, ALVIN, PRIYA = "1", "2", "3"
RUN = {
    "id": "a1a1a1a1-0000-0000-0000-000000000001",
    "bosses": ["HMaleficStar", "HFA"],
    "datetime": kl(2026, 8, 31, 21, 30),
    "participants": [MY, ALVIN, PRIYA],
    "status": "planned",
    "channel_id": "900",
}


def amendment(kind="move", **kwargs) -> dict:
    row = {
        "id": "b2b2b2b2-0000-0000-0000-000000000002",
        "kind": kind,
        "bosses": [],
        "run_id": None,
        "new_datetime": None,
        "participants": [],
        "is_question": False,
        "confidence": 0.9,
        "day_ref": None,
        "time_ref": None,
        "summary": None,
        "payload": {},
    }
    row.update(kwargs)
    return row


# ---------------------------------------------------------------------------
# when_text -- the one place a guess could sneak in
# ---------------------------------------------------------------------------


def test_a_resolved_datetime_is_spelled_out():
    text = when_text(amendment(new_datetime=kl(2026, 9, 2, 21, 30)), TZ)
    assert "Wed 02 Sep" in text and "21:30" in text


def test_a_day_with_no_time_says_tbd_rather_than_midnight():
    text = when_text(amendment(day_ref="wed"), TZ)
    assert "wed" in text and TBD in text


def test_a_time_with_no_day_says_tbd():
    assert TBD in when_text(amendment(time_ref="9:30pm"), TZ)


def test_nothing_stated_is_entirely_tbd():
    assert when_text(amendment(), TZ) == TBD


def test_words_that_could_not_be_parsed_are_quoted_back_not_dropped():
    text = when_text(amendment(day_ref="sometime", time_ref="after boss"), TZ)
    assert "sometime" in text and "after boss" in text


# ---------------------------------------------------------------------------
# one line per amendment
# ---------------------------------------------------------------------------


def test_a_move_shows_the_old_time_struck_through():
    name, value = proposal_line(
        amendment(
            "move",
            bosses=["HMaleficStar", "HFA"],
            run_id=RUN["id"],
            new_datetime=kl(2026, 9, 2, 21, 30),
        ),
        RUN,
        TZ,
    )
    assert "Hard MaleficStar + Hard FA" in name and short_id(RUN["id"]) in name
    assert "~~Mon 31 Aug 21:30~~" in value and "Wed 02 Sep 21:30" in value


def test_a_cancel_does_not_pretend_to_have_a_time():
    _name, value = proposal_line(
        amendment("cancel", bosses=["HMaleficStar"], run_id=RUN["id"]), RUN, TZ
    )
    assert "off this week" in value and TBD not in value


def test_otot_says_what_it_does_to_the_pings():
    _name, value = proposal_line(amendment("otot", bosses=["HCarling"]), None, TZ)
    assert "own time" in value and "no countdown" in value


def test_a_sub_names_the_person_dropping_out():
    _name, value = proposal_line(amendment("sub", participants=[MY]), RUN, TZ)
    assert "out this week" in value and f"<@{MY}>" in value


def test_a_new_run_falls_back_to_the_runs_bosses_when_none_were_named():
    name, _value = proposal_line(amendment("add", run_id=RUN["id"]), RUN, TZ)
    assert "Hard MaleficStar + Hard FA" in name


def test_the_model_summary_is_shown_when_there_is_one():
    _name, value = proposal_line(
        amendment("move", bosses=["HMaleficStar"], summary="A proposes Wed"), None, TZ
    )
    assert "_A proposes Wed_" in value


# ---------------------------------------------------------------------------
# which header the card gets
# ---------------------------------------------------------------------------


def test_a_settled_change_is_a_proposal():
    assert card_kind([amendment("move", new_datetime=kl(2026, 9, 2, 21, 30))]) == "proposal"


def test_an_open_question_is_a_suggestion():
    assert (
        card_kind([amendment("move", new_datetime=kl(2026, 9, 2, 21, 30), is_question=True)])
        == "suggestion"
    )


def test_a_missing_time_is_a_suggestion_even_when_nobody_asked_a_question():
    assert card_kind([amendment("move", day_ref="wed")]) == "suggestion"


def test_a_card_of_nothing_but_fixed_timings_gets_the_pin():
    assert card_kind([amendment("fix", bosses=["HLimbo"])]) == "fix"


def test_a_mixed_card_is_not_a_pin():
    assert card_kind([amendment("fix"), amendment("move", day_ref="wed")]) != "fix"


# ---------------------------------------------------------------------------
# the whole card
# ---------------------------------------------------------------------------


def test_a_proposal_card_pings_the_run_and_asks_for_a_reaction():
    card = proposal_card(
        [
            amendment(
                "move",
                bosses=["HMaleficStar", "HFA"],
                run_id=RUN["id"],
                new_datetime=kl(2026, 9, 2, 21, 30),
            )
        ],
        {RUN["id"]: RUN},
        TZ,
        confidence=0.82,
    )
    assert "📋" in card.content
    assert all(f"<@{uid}>" in card.content for uid in RUN["participants"])
    assert card.footer.startswith(CONFIRM_HINT)
    assert "0.82" in card.footer
    assert len(card.fields) == 1


def test_a_suggestion_card_names_who_has_not_answered():
    card = proposal_card(
        [
            amendment(
                "move", bosses=["HMaleficStar"], run_id=RUN["id"], day_ref="wed", participants=[MY]
            )
        ],
        {RUN["id"]: RUN},
        TZ,
        unanswered=[ALVIN, PRIYA],
    )
    assert "💡" in card.content
    assert card.description.startswith("Not yet answered:")
    assert f"<@{ALVIN}>" in card.description


def test_a_fix_card_is_pinned_and_purple():
    from bot.agent.formatting import COLOUR_FIXED

    card = proposal_card(
        [amendment("fix", bosses=["HLimbo", "NBaldrix"], new_datetime=kl(2026, 9, 1, 22, 30))],
        {},
        TZ,
    )
    assert "📌" in card.content and card.colour == COLOUR_FIXED


def test_one_card_carries_every_amendment_in_the_burst():
    card = proposal_card(
        [
            amendment("cancel", bosses=["HMaleficStar", "HFA"], run_id=RUN["id"]),
            amendment("cancel", bosses=["HCarling", "XKalos"]),
        ],
        {RUN["id"]: RUN},
        TZ,
    )
    assert len(card.fields) == 2


def test_the_card_reports_the_lowest_confidence_on_it():
    card = proposal_card(
        [amendment("move", confidence=0.9), amendment("cancel", confidence=0.65)],
        {},
        TZ,
        confidence=0.65,
    )
    assert "0.65" in card.footer


@pytest.mark.parametrize("kind", ["move", "add", "cancel", "otot", "sub", "split", "fix"])
def test_every_kind_renders_without_a_run(kind):
    card = proposal_card([amendment(kind, bosses=["HMaleficStar"])], {}, TZ)
    assert card.has_embed and card.fields
