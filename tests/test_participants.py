"""Resolving a typed `participants:` field against the roster.

Discord's string options don't open the member picker, so people type names.
This is the pure fallback that turns "MY, alvin" into user ids.
"""

from __future__ import annotations

import pytest

from bot.util import match_roster, resolve_participant_text

ROSTER = [
    {"user_id": "1", "display_name": "harbour4417", "nickname": None, "aliases": ["MY"]},
    {"user_id": "2", "display_name": "Alvin tan", "nickname": None, "aliases": []},
    {"user_id": "3", "display_name": "Priya", "nickname": None, "aliases": []},
    {"user_id": "4", "display_name": "kanon [AZUR]", "nickname": "kanon", "aliases": []},
    {"user_id": "5", "display_name": "ZedRSXD [Nagi]", "nickname": None, "aliases": ["ZedRS"]},
    {"user_id": "6", "display_name": "priya2", "nickname": None, "aliases": []},
]


def ids(text):
    return resolve_participant_text(text, ROSTER).ids


# -- mentions and ids --------------------------------------------------------


def test_mentions_from_the_picker_still_work():
    assert ids("<@2> <@!3>") == ["2", "3"]


def test_bare_snowflakes_work():
    assert ids("100000000000000010") == ["100000000000000010"]


def test_mentions_and_names_can_be_mixed():
    assert ids("<@2> MY") == ["2", "1"]


def test_order_is_preserved_and_duplicates_dropped():
    assert ids("priya, MY, priya") == ["3", "1"]


# -- names -------------------------------------------------------------------


def test_a_nick_alias_resolves():
    assert ids("MY") == ["1"]


def test_alias_matching_is_case_insensitive():
    assert ids("my") == ["1"]
    assert ids("zedrs") == ["5"]


def test_a_leading_at_sign_is_stripped():
    assert ids("@kanon") == ["4"]


def test_a_prefix_of_a_decorated_display_name_resolves():
    # "Alvin tan" and "kanon [AZUR]" are what Discord actually reports.
    assert ids("alvin") == ["2"]
    assert ids("kanon") == ["4"]


def test_commas_and_whitespace_both_separate_names():
    assert ids("MY, alvin priya") == ["1", "2", "3"]


def test_an_exact_match_beats_a_longer_prefix_match():
    # "priya" must not be ambiguous just because "priya2" also starts with it.
    assert ids("priya") == ["3"]


def test_a_substring_match_is_the_last_resort():
    assert ids("AZUR") == ["4"]


# -- failures ----------------------------------------------------------------


def test_an_unmatched_name_is_reported_not_guessed():
    result = resolve_participant_text("xyzzy", ROSTER)
    assert result.ids == []
    assert result.unknown == ["xyzzy"]
    assert not result.ok


def test_every_unmatched_name_is_listed_at_once():
    assert resolve_participant_text("xyzzy, plugh", ROSTER).unknown == ["xyzzy", "plugh"]


def test_an_ambiguous_name_names_both_candidates():
    result = resolve_participant_text("priy", ROSTER)
    assert result.ids == []
    assert set(result.ambiguous["priy"]) == {"Priya", "priya2"}
    assert not result.ok


def test_good_names_still_resolve_alongside_a_bad_one():
    result = resolve_participant_text("MY, xyzzy", ROSTER)
    assert result.ids == ["1"]
    assert result.unknown == ["xyzzy"]


@pytest.mark.parametrize("text", [None, "", "   ", ","])
def test_empty_input_resolves_to_nothing(text):
    result = resolve_participant_text(text, ROSTER)
    assert result.ids == [] and result.ok


def test_an_empty_roster_matches_nothing():
    assert resolve_participant_text("MY", []).unknown == ["MY"]


# -- match_roster ------------------------------------------------------------


def test_match_roster_returns_every_candidate():
    assert [m["user_id"] for m in match_roster("priy", ROSTER)] == ["3", "6"]


def test_match_roster_on_a_blank_token():
    assert match_roster("   ", ROSTER) == []
