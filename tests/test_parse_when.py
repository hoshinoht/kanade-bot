"""Reading the times this guild actually types.

`parse_when` is shared by `/amend`, the portal, `bossctl` and the chatbot, so
the shorthand has to work here or it works nowhere. Live, "schedule Extreme
Kalos tonight at 2300" failed outright, and probing dateparser directly showed
it was not one broken phrase but a family of them: `tonight 23:00`,
`tonight at 11pm`, `tmr 2300`, `tmr 9pm`, `tomorrow night 9pm` and `ltr 9pm` all
returned None, while `today 23:00` was fine.

Worse than the failures was the success: a bare `2300` parsed as the **year**
2300, which is a proposal card three centuries out.

Time is frozen by monkeypatching `service.utcnow`, the convention the extractor
tests already use -- the answers here are all relative to "now".
"""

from __future__ import annotations

from datetime import datetime, timedelta

import pytest

from bot.api import service
from bot.api.errors import BadRequest

from .conftest import TZ, kl

#: A Tuesday evening, well before midnight so "tonight" is still today.
NOW = kl(2026, 9, 1, 20, 0)


@pytest.fixture
def at_eight(monkeypatch):
    monkeypatch.setattr(service, "utcnow", lambda: NOW)
    return NOW


@pytest.fixture
def bot(fake_bot, at_eight):
    return fake_bot


def when(bot, text: str) -> datetime:
    return service.parse_when(bot, text).astimezone(TZ)


# ---------------------------------------------------------------------------
# the phrases that failed live
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        # tonight -> today
        ("tonight 23:00", kl(2026, 9, 1, 23, 0)),
        ("tonight at 11pm", kl(2026, 9, 1, 23, 0)),
        ("tonite 9pm", kl(2026, 9, 1, 21, 0)),
        # tmr -> tomorrow, including the compact clock dateparser drops
        ("tmr 2300", kl(2026, 9, 2, 23, 0)),
        ("tmr 9pm", kl(2026, 9, 2, 21, 0)),
        ("tmrw 21:30", kl(2026, 9, 2, 21, 30)),
        ("tmmr 2300", kl(2026, 9, 2, 23, 0)),
        # the night is already in the 9pm
        ("tomorrow night 9pm", kl(2026, 9, 2, 21, 0)),
        ("tmr night 9pm", kl(2026, 9, 2, 21, 0)),
        # later -> today
        ("ltr 9pm", kl(2026, 9, 1, 21, 0)),
        ("later 10pm", kl(2026, 9, 1, 22, 0)),
    ],
)
def test_the_guilds_shorthand_resolves(bot, text, expected):
    assert when(bot, text) == expected


def test_the_live_message_that_started_it(bot):
    """ "schedule Extreme Kalos tonight at 2300" -- the time half of it."""
    assert when(bot, "tonight at 2300") == kl(2026, 9, 1, 23, 0)


# ---------------------------------------------------------------------------
# a bare clock is a time today, not a year
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("2300", kl(2026, 9, 1, 23, 0)),
        ("23:00", kl(2026, 9, 1, 23, 0)),
        ("930pm", kl(2026, 9, 1, 21, 30)),
        ("9pm", kl(2026, 9, 1, 21, 0)),
        ("9:45pm", kl(2026, 9, 1, 21, 45)),
        ("9+pm", kl(2026, 9, 1, 21, 0)),
        # a range is its start time, as everywhere else in the codebase
        ("1030~11+pm", kl(2026, 9, 1, 22, 30)),
        # the guild's bare-hour convention: evenings, so 10 is 22:00
        ("10", kl(2026, 9, 1, 22, 0)),
    ],
)
def test_a_bare_clock_is_tonight(bot, text, expected):
    assert when(bot, text) == expected


def test_2300_is_not_the_year_2300(bot):
    """The bug that could have put a card three centuries out."""
    assert when(bot, "2300").year == 2026


@pytest.mark.parametrize("text", ["19:00", "1900", "7pm"])
def test_a_bare_clock_that_has_already_passed_rolls_to_tomorrow(bot, text):
    """It is 20:00; "7pm" means tomorrow, not an hour ago."""
    assert when(bot, text) == kl(2026, 9, 2, 19, 0)


@pytest.mark.parametrize("text", ["22:30", "2230", "1030pm"])
def test_a_bare_clock_still_to_come_stays_today(bot, text):
    assert when(bot, text) == kl(2026, 9, 1, 22, 30)


# ---------------------------------------------------------------------------
# the horizon guard
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("text", ["3000", "jan 2350", "year 2300"])
def test_a_date_centuries_out_is_refused(bot, text):
    with pytest.raises(BadRequest) as exc:
        service.parse_when(bot, text)
    assert "couldn't read" in exc.value.message


def test_the_refusal_says_where_it_landed(bot):
    with pytest.raises(BadRequest) as exc:
        service.parse_when(bot, "3000")
    assert "lands in 3000" in exc.value.message


def test_a_date_inside_the_horizon_is_fine(bot):
    """The guard must not reject next year's boss week."""
    soon = (NOW + timedelta(days=300)).strftime("%Y-%m-%d 21:30")
    assert when(bot, soon).date() == (NOW + timedelta(days=300)).date()


def test_the_horizon_is_a_year_and_a_bit():
    assert service.MAX_HORIZON == timedelta(days=400)


# ---------------------------------------------------------------------------
# nothing that already worked may stop working
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("wed 21:30", kl(2026, 9, 2, 21, 30)),
        ("wed 2300", kl(2026, 9, 2, 23, 0)),
        ("2026-09-02 21:30", kl(2026, 9, 2, 21, 30)),
        ("2026-09-02", kl(2026, 9, 2, 0, 0)),
        ("tomorrow 9:45pm", kl(2026, 9, 2, 21, 45)),
        ("today 23:00", kl(2026, 9, 1, 23, 0)),
        ("saturday 22:00", kl(2026, 9, 5, 22, 0)),
    ],
)
def test_what_already_worked_still_works(bot, text, expected):
    assert when(bot, text) == expected


def test_an_iso_date_is_never_read_as_a_clock(bot):
    """`2026-09-02 21:30` must not have its year spelled out as a time."""
    assert service.normalise_when("2026-09-02 21:30") == "2026-09-02 21:30"


def test_a_year_with_no_day_word_is_left_alone(bot):
    """The compact-time rewrite only fires when a day is already named."""
    assert service.normalise_when("sep 2026") == "sep 2026"
    assert service.normalise_when("tomorrow 2026") == "tomorrow 20:26"


def test_nonsense_is_still_refused(bot):
    for text in ("whenever lah", "", "   ", "next boss night"):
        with pytest.raises(BadRequest):
            service.parse_when(bot, text)


# ---------------------------------------------------------------------------
# the normalisation itself
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("tonight 23:00", "today 23:00"),
        ("TONIGHT 23:00", "today 23:00"),
        ("tmr 9pm", "tomorrow 9pm"),
        ("tomorrow night 9pm", "tomorrow 9pm"),
        ("ltr 9pm", "today 9pm"),
        ("wed 21:30", "wed 21:30"),
    ],
)
def test_normalise_when_rewrites_only_the_day_words(text, expected):
    assert service.normalise_when(text) == expected


def test_a_day_word_inside_another_word_is_not_touched():
    """Word-bounded: `tmr` must not be found inside a name."""
    assert service.normalise_when("meet Tmrock 9pm") == "meet Tmrock 9pm"


def test_the_day_vocabulary_comes_from_the_extractor():
    """One table, so `tmr` means tomorrow in chat and in `/amend` alike."""
    from bot.extract import resolve

    words = {word for word, _ in service._DAY_WORDS}
    assert resolve.TOMORROW_WORDS <= words
    assert resolve.TODAY_WORDS <= words
    assert resolve.SOON_WORDS <= words
