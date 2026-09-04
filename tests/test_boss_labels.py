"""Bosses spelled out for the people reading a card.

A stored token is for parsing: ``XKalos`` is what a member types and what the
tools accept. It is not what anybody says out loud, and a proposal card is the
last thing read before somebody commits their evening to a run -- so the card
says "Extreme Kalos".

:data:`bot.formatting.DIFFICULTY_WORDS` is the one place that mapping lives, and
it duplicates ``difficulties:`` in ``config/bosses.yaml``. The last test here is
what makes that duplication safe.
"""

from __future__ import annotations

import yaml

from bot.agent.formatting import DIFFICULTY_WORDS, boss_label, boss_labels

from .conftest import REPO_ROOT


def test_a_token_becomes_the_difficulty_and_the_boss():
    assert boss_label("XKalos") == "Extreme Kalos"
    assert boss_label("HBellona") == "Hard Bellona"
    assert boss_label("NCarling") == "Normal Carling"


def test_a_short_name_is_kept_as_it_is_written():
    """`HFA` is "Hard FA", not "Hard Fa" -- the short name is a proper noun."""
    assert boss_label("HFA") == "Hard FA"


def test_a_list_reads_as_one_night():
    assert boss_labels(["XKalos", "HBellona"]) == "Extreme Kalos + Hard Bellona"
    assert boss_labels([]) == "(no bosses)"


def test_anything_that_is_not_a_token_comes_back_unchanged():
    """Better an unhelpful label than a mangled one."""
    for text in ("", "Q", "ZZKalos", "Kalos"):
        assert boss_label(text) == text


def test_the_words_match_the_boss_table():
    """The guard on duplicating ``difficulties:`` outside ``bosses.yaml``.

    If a prefix is ever added or renamed in the config, this fails rather than
    letting cards quietly render the raw token again.
    """
    raw = yaml.safe_load((REPO_ROOT / "config" / "bosses.yaml").read_text(encoding="utf-8"))
    assert {k.lower(): v for k, v in raw["difficulties"].items()} == DIFFICULTY_WORDS
