"""UUID ids: the short display form and prefix resolution."""

from __future__ import annotations

import uuid

import pytest

from bot.ids import (
    MIN_PREFIX,
    IdAmbiguous,
    IdNotFound,
    IdTooShort,
    canonical,
    new_id,
    resolve_id,
    short_id,
    tag,
)

A = "a1b2c3d4-1111-4222-8333-444444444444"
B = "a1b2ffff-5555-4666-8777-888888888888"
C = "99999999-0000-4000-8000-000000000000"
ALL = [A, B, C]


# -- generation --------------------------------------------------------------


def test_new_id_is_a_uuid4():
    parsed = uuid.UUID(new_id())
    assert parsed.version == 4


def test_new_ids_are_distinct():
    assert len({new_id() for _ in range(200)}) == 200


# -- display -----------------------------------------------------------------


def test_short_id_is_the_first_eight_hex_characters():
    assert short_id(A) == "a1b2c3d4"


def test_tag_adds_the_hash():
    assert tag(A) == "#a1b2c3d4"


def test_short_ids_of_different_uuids_usually_differ():
    assert short_id(A) != short_id(C)


@pytest.mark.parametrize("value", [A, A.upper(), f"#{A}", f"  #{A}  "])
def test_canonical_strips_decoration(value):
    assert canonical(value) == A.replace("-", "")


# -- resolution --------------------------------------------------------------


def test_a_full_uuid_resolves_to_itself():
    assert resolve_id(A, ALL) == A


def test_a_unique_prefix_resolves():
    assert resolve_id("a1b2c3", ALL) == A
    assert resolve_id("9999", ALL) == C


def test_resolution_is_case_insensitive():
    assert resolve_id("A1B2C3D4", ALL) == A


def test_a_leading_hash_is_tolerated():
    assert resolve_id("#a1b2c3d4", ALL) == A


def test_the_printed_short_form_can_be_pasted_straight_back():
    assert resolve_id(tag(A), ALL) == A


def test_dashes_are_ignored():
    assert resolve_id("a1b2c3d4-1111", ALL) == A


def test_an_exact_full_match_wins_over_prefix_scanning():
    # A full id must resolve even if it is a prefix of nothing else.
    assert resolve_id(B, ALL) == B


def test_an_ambiguous_prefix_lists_its_candidates():
    with pytest.raises(IdAmbiguous) as exc:
        resolve_id("a1b2", ALL)
    assert set(exc.value.candidates) == {A, B}


def test_an_unknown_prefix_is_rejected():
    with pytest.raises(IdNotFound):
        resolve_id("dead", ALL)


@pytest.mark.parametrize("text", ["", "a", "ab", "abc", "#abc"])
def test_a_prefix_shorter_than_the_minimum_is_rejected(text):
    with pytest.raises(IdTooShort):
        resolve_id(text, ALL)


def test_the_minimum_prefix_length_is_four():
    assert MIN_PREFIX == 4


def test_resolving_against_nothing_is_a_not_found():
    with pytest.raises(IdNotFound):
        resolve_id("a1b2c3d4", [])


def test_a_prefix_that_is_unique_at_four_chars_resolves():
    assert resolve_id("a1b2", [A, C]) == A
