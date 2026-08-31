"""Boss-week arithmetic, including the boundary cases."""

from __future__ import annotations

from datetime import UTC, time, timedelta

import pytest

from bot.weeks import (
    current_week_start,
    next_week_start,
    parse_hhmm,
    parse_weekday,
    slot_in_week,
    week_end,
    week_start,
)

from .conftest import RESET_TIME, RESET_WEEKDAY, TZ, kl


def ws(dt, reset_time=RESET_TIME):
    return week_start(dt, TZ, RESET_WEEKDAY, reset_time)


def test_exactly_on_the_reset_starts_the_new_week():
    reset = kl(2026, 8, 27)  # Thu 00:00
    assert ws(reset) == reset


def test_one_minute_before_the_reset_is_still_the_old_week():
    assert ws(kl(2026, 8, 26, 23, 59)) == kl(2026, 8, 20)


def test_mid_week_resolves_back_to_the_last_thursday():
    assert ws(kl(2026, 8, 30, 12, 0)) == kl(2026, 8, 27)


def test_the_reset_weekday_itself_before_the_reset_time():
    # Reset moved to Thu 12:00: Thursday morning still belongs to the old week.
    assert ws(kl(2026, 8, 27, 9, 0), reset_time=time(12, 0)) == kl(2026, 8, 20, 12, 0)
    assert ws(kl(2026, 8, 27, 12, 0), reset_time=time(12, 0)) == kl(2026, 8, 27, 12, 0)


def test_accepts_utc_input_and_answers_in_guild_time():
    # Wed 2026-08-26 16:30 UTC == Thu 2026-08-27 00:30 in Kuala Lumpur.
    utc = kl(2026, 8, 27, 0, 30).astimezone(UTC)
    assert ws(utc) == kl(2026, 8, 27)


def test_naive_input_is_rejected():
    with pytest.raises(ValueError):
        week_start(kl(2026, 8, 27).replace(tzinfo=None), TZ, RESET_WEEKDAY, RESET_TIME)


def test_week_end_is_exactly_seven_days_on_a_dst_free_zone():
    start = kl(2026, 8, 27)
    assert week_end(start, TZ) == start + timedelta(days=7)


def test_current_and_next_week_are_adjacent():
    now = kl(2026, 8, 30, 12, 0)
    current = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, now)
    following = next_week_start(TZ, RESET_WEEKDAY, RESET_TIME, now)
    assert current == kl(2026, 8, 27)
    assert following == kl(2026, 9, 3)
    assert week_end(current, TZ) == following


@pytest.mark.parametrize(
    ("weekday", "at", "expected"),
    [
        (0, time(21, 30), kl(2026, 8, 31, 21, 30)),  # Mon
        (1, time(23, 0), kl(2026, 9, 1, 23, 0)),  # Tue
        (3, time(0, 0), kl(2026, 8, 27, 0, 0)),  # Thu == the reset instant itself
        (3, time(23, 0), kl(2026, 8, 27, 23, 0)),  # Thu late
        (2, time(21, 30), kl(2026, 9, 2, 21, 30)),  # Wed
    ],
)
def test_slot_in_week_places_fixed_runs_inside_their_week(weekday, at, expected):
    start = kl(2026, 8, 27)
    slot = slot_in_week(start, TZ, weekday, at)
    assert slot == expected
    assert start <= slot < week_end(start, TZ)


def test_slot_before_a_late_reset_is_pushed_into_the_week():
    # Week starts Thu 12:00; a Thursday 09:00 run belongs to the *next* Thursday.
    start = kl(2026, 8, 27, 12, 0)
    slot = slot_in_week(start, TZ, 3, time(9, 0))
    assert slot == kl(2026, 9, 3, 9, 0)
    assert start <= slot < week_end(start, TZ)


@pytest.mark.parametrize(
    ("text", "expected"), [("mon", 0), ("Thu", 3), ("thursday", 3), ("weds", 2), ("6", 6)]
)
def test_parse_weekday(text, expected):
    assert parse_weekday(text) == expected


def test_parse_weekday_rejects_nonsense():
    with pytest.raises(ValueError):
        parse_weekday("caturday")


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("09:00", time(9, 0)),
        ("9:05", time(9, 5)),
        ("9.30", time(9, 30)),
        ("0900", time(9, 0)),
        ("2359", time(23, 59)),
        ("930", time(9, 30)),
        ("2130", time(21, 30)),
        ("9pm", time(21, 0)),
        ("9:30pm", time(21, 30)),
        ("930pm", time(21, 30)),
        ("12am", time(0, 0)),
        ("12pm", time(12, 0)),
        (" 11 PM ", time(23, 0)),
    ],
)
def test_parse_hhmm(text, expected):
    assert parse_hhmm(text) == expected


@pytest.mark.parametrize("text", ["25:00", "09:70", "", "13pm", "0pm", "abc", "12345", "9:3"])
def test_parse_hhmm_rejects_bad_input(text):
    with pytest.raises(ValueError):
        parse_hhmm(text)
