"""``day_ref``/``time_ref`` -> a real datetime, anchored on the evidence message.

The phrasings below are the ones a party channel actually uses.  Anything this
module cannot read must come back empty rather than guess -- a wrong guess
silently moves a run.
"""

from __future__ import annotations

from datetime import date, time

import pytest

from bot.extract.resolve import parse_clock, resolve

from .conftest import TZ, kl

# Sunday 30 August 2026, 13:07 local -- the anchor used by most cases below.
SUN_AFTERNOON = kl(2026, 8, 30, 13, 7)


# ---------------------------------------------------------------------------
# clock times
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        # explicit meridiem
        ("9pm", time(21, 0)),
        ("9 pm", time(21, 0)),
        ("9PM", time(21, 0)),
        ("9:30pm", time(21, 30)),
        ("9:45pm", time(21, 45)),
        ("9.30pm", time(21, 30)),
        ("930pm", time(21, 30)),
        ("1030pm", time(22, 30)),
        ("1130pm", time(23, 30)),
        ("1145pm", time(23, 45)),
        ("12am", time(0, 0)),
        ("12pm", time(12, 0)),
        ("11pm", time(23, 0)),
        ("9+pm", time(21, 0)),
        ("9:30 p.m.", time(21, 30)),
        # 24-hour, left alone
        ("21:30", time(21, 30)),
        ("23:00", time(23, 0)),
        ("2130", time(21, 30)),
        ("00:30", time(0, 30)),
        # bare hours and compact digits default to pm
        ("9:30", time(21, 30)),
        ("9.30", time(21, 30)),
        ("930", time(21, 30)),
        ("1030", time(22, 30)),
        ("1010", time(22, 10)),
        ("10", time(22, 0)),
        ("11", time(23, 0)),
        ("12", time(0, 0)),
        # ranges: the start is the time
        ("1030~11+pm", time(22, 30)),
        ("8~1130", time(20, 0)),
        ("9-10pm", time(21, 0)),
        ("9pm-11pm", time(21, 0)),
        ("11 to 1145pm", time(23, 0)),
        ("930 to 1030", time(21, 30)),
        # trailing/leading noise people actually type
        ("11pm onward", time(23, 0)),
        ("9:30pm onwards", time(21, 30)),
        ("at 11", time(23, 0)),
        ("at 9:30", time(21, 30)),
        ("around 930", time(21, 30)),
        ("10pm ish", time(22, 0)),
        ("1030 sharp", time(22, 30)),
    ],
)
def test_clock_expressions_from_real_chat(text, expected):
    parsed = parse_clock(text)
    assert parsed is not None, f"{text!r} did not parse"
    assert parsed[0] == expected


@pytest.mark.parametrize("text", ["9", "10", "1030", "930", "11"])
def test_bare_hours_are_flagged_as_an_assumption(text):
    assert parse_clock(text)[1] is True


@pytest.mark.parametrize("text", ["9pm", "21:30", "12am", "9:30am"])
def test_explicit_times_are_not_flagged_as_an_assumption(text):
    assert parse_clock(text)[1] is False


@pytest.mark.parametrize(
    "text",
    [
        None,
        "",
        "   ",
        "night",
        "later",
        "after boss",
        "evening",
        "asap",
        "when free",
        "25:00",
        "1090",  # minute 90
        "sometime",
    ],
)
def test_unparseable_times_come_back_none(text):
    assert parse_clock(text) is None


# ---------------------------------------------------------------------------
# days
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("day_ref", "expected"),
    [
        ("today", date(2026, 8, 30)),
        ("tonight", date(2026, 8, 30)),
        ("tmr", date(2026, 8, 31)),
        ("tomorrow", date(2026, 8, 31)),
        ("tmr night", date(2026, 8, 31)),
        ("ytd", date(2026, 8, 29)),
        # Sunday 30 Aug is the anchor
        ("mon", date(2026, 8, 31)),
        ("monday", date(2026, 8, 31)),
        ("tue", date(2026, 9, 1)),
        ("tues", date(2026, 9, 1)),
        ("wed", date(2026, 9, 2)),
        ("weds", date(2026, 9, 2)),
        ("wednesday", date(2026, 9, 2)),
        ("thurs", date(2026, 9, 3)),
        ("fri", date(2026, 9, 4)),
        ("sat", date(2026, 9, 5)),
        ("sun", date(2026, 8, 30)),  # today
        ("this sunday", date(2026, 8, 30)),
        ("next mon", date(2026, 8, 31)),
        ("next sun", date(2026, 9, 6)),  # "next" pushes off today
        ("tue night", date(2026, 9, 1)),
        ("on wed", date(2026, 9, 2)),
        ("2026-09-02", date(2026, 9, 2)),
    ],
)
def test_day_references_from_real_chat(day_ref, expected):
    assert resolve(day_ref, None, SUN_AFTERNOON, TZ).day == expected


@pytest.mark.parametrize("day_ref", ["someday", "whenever", "next week sometime", "", None, "idk"])
def test_unparseable_days_come_back_empty(day_ref):
    result = resolve(day_ref, None, SUN_AFTERNOON, TZ)
    assert result.day is None and result.at is None


def test_a_day_with_no_time_stays_tbd():
    result = resolve("wed", None, SUN_AFTERNOON, TZ)
    assert result.day == date(2026, 9, 2)
    assert result.clock is None and result.at is None


def test_neither_day_nor_time_resolves_to_nothing():
    assert not resolve(None, None, SUN_AFTERNOON, TZ).known


# ---------------------------------------------------------------------------
# combining the two
# ---------------------------------------------------------------------------


def test_the_worked_example_amend_to_945pm():
    # The `add-tonight` fixture: "we doing our nstar and ncarl tonight?" (29/08
    # 11:54) -> "9pm i reach kk early" -> "amend to 9:45pm" (29/08 14:52)
    anchor = kl(2026, 8, 29, 14, 52)
    assert resolve("tonight", "9:45pm", anchor, TZ).at == kl(2026, 8, 29, 21, 45)


def test_the_worked_example_move_to_weds_930pm():
    # The `move-with-time` fixture: "Wed i done with boss so 9:30pm onwards"
    # (30/08 13:13)
    anchor = kl(2026, 8, 30, 13, 13)
    assert resolve("wed", "9:30pm onwards", anchor, TZ).at == kl(2026, 9, 2, 21, 30)


def test_the_worked_example_tmr_1030():
    # export: kanon "@here pls note tmr 1030~11+pm hlimbo+baldrix" (01/06 11:52 UTC)
    anchor = kl(2026, 6, 1, 19, 52)
    assert resolve("tmr", "1030~11+pm", anchor, TZ).at == kl(2026, 6, 2, 22, 30)


def test_a_time_with_no_day_means_today():
    assert resolve(None, "9:30pm", SUN_AFTERNOON, TZ).at == kl(2026, 8, 30, 21, 30)


def test_a_time_already_past_today_rolls_into_tomorrow():
    late = kl(2026, 8, 30, 23, 30)
    assert resolve(None, "9:30pm", late, TZ).at == kl(2026, 8, 31, 21, 30)


def test_later_rolls_forward_but_tonight_does_not():
    late = kl(2026, 8, 30, 23, 30)
    # "later at 11" past 11pm is tomorrow; "tonight 9:30pm" is still tonight,
    # because the word names the day even when the time has gone.
    assert resolve("later", "at 11", late, TZ).at == kl(2026, 8, 31, 23, 0)
    assert resolve("tonight", "9:30pm", late, TZ).at == kl(2026, 8, 30, 21, 30)


def test_the_same_weekday_late_at_night_rolls_a_whole_week():
    late_wed = kl(2026, 9, 2, 23, 30)
    assert resolve("wed", "9:30pm", late_wed, TZ).at == kl(2026, 9, 9, 21, 30)


def test_the_same_weekday_earlier_in_the_day_stays_today():
    wed_morning = kl(2026, 9, 2, 9, 0)
    assert resolve("wed", "9:30pm", wed_morning, TZ).at == kl(2026, 9, 2, 21, 30)


def test_an_explicit_date_is_never_rolled_forward():
    anchor = kl(2026, 9, 5, 12, 0)
    assert resolve("2026-09-02", "9pm", anchor, TZ).at == kl(2026, 9, 2, 21, 0)


def test_the_anchor_is_converted_into_the_guild_timezone_first():
    from datetime import UTC, datetime

    # 30 Aug 17:30 UTC is 31 Aug 01:30 in Kuala Lumpur, so "today" is the 31st.
    anchor = datetime(2026, 8, 30, 17, 30, tzinfo=UTC)
    assert resolve("today", "9pm", anchor, TZ).at == kl(2026, 8, 31, 21, 0)


def test_a_naive_anchor_is_refused():
    from datetime import datetime

    with pytest.raises(ValueError, match="aware"):
        resolve("wed", "9pm", datetime(2026, 8, 30, 13, 7), TZ)
