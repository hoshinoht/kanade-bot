"""Watched-channel resolution and the roster's bot filter."""

from __future__ import annotations

from types import SimpleNamespace

import pytest

from bot.agent.util import roster_rows
from bot.infrastructure.watch import is_watched

CHANNELS = [100]
CATEGORIES = [200]


def channel(cid, category_id=None, parent=None):
    return SimpleNamespace(id=cid, category_id=category_id, parent=parent)


def watched(ch):
    return is_watched(ch, CHANNELS, CATEGORIES)


def test_an_explicitly_listed_channel_is_watched():
    assert watched(channel(100))


def test_any_channel_under_a_watched_category_is_watched():
    assert watched(channel(999, category_id=200))


def test_a_channel_created_later_under_the_category_needs_no_restart():
    # Nothing is cached: resolution is by the channel object handed to us.
    assert watched(channel(123456789, category_id=200))


def test_an_unrelated_channel_is_not_watched():
    assert not watched(channel(999, category_id=888))


def test_a_thread_inherits_from_its_parent_channel():
    assert watched(channel(555, parent=channel(100)))


def test_a_thread_inherits_from_its_parents_category():
    assert watched(channel(555, parent=channel(999, category_id=200)))


def test_a_thread_under_an_unwatched_parent_is_not_watched():
    assert not watched(channel(555, parent=channel(999, category_id=888)))


def test_none_is_never_watched():
    assert not watched(None)


def test_nothing_configured_watches_nothing():
    assert not is_watched(channel(100), [], [])


def test_ids_given_as_strings_still_match():
    assert is_watched(channel(100), ["100"], [])


def test_a_channel_without_a_category_does_not_match_the_category_list():
    assert not watched(channel(999, category_id=None))


# -- roster -----------------------------------------------------------------


def member(uid, name, nick=None, bot=False):
    return SimpleNamespace(id=uid, display_name=name, nick=nick, bot=bot)


def test_roster_rows_maps_members():
    rows = roster_rows([member(1, "harbour4417", nick="MY")])
    assert rows == [("1", "harbour4417", "MY", True)]


def test_bot_accounts_are_never_added_to_the_roster():
    rows = roster_rows(
        [member(1, "harbour4417"), member(2, "YuukiSakuna", bot=True), member(3, "Alvin")]
    )
    assert [r[0] for r in rows] == ["1", "3"]


def test_a_bot_holding_the_bossing_role_is_still_skipped():
    assert roster_rows([member(9, "SomeBot", bot=True)]) == []


@pytest.mark.parametrize("members", [[], [member(1, "a", bot=True)]])
def test_an_all_bot_role_yields_an_empty_roster(members):
    assert roster_rows(members) == []
