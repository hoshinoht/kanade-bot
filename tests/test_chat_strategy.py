"""Pure routing boundaries for checked-in boss strategy requests."""

from __future__ import annotations

import pytest

from bot.chat.strategy import (
    STRATEGY_CLARIFICATION_REPLY,
    STRATEGY_NARROW_REPLY,
    route_strategy_intent,
)
from bot.domain.bosses import BossReference


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("how to beat fa", (BossReference("FA", None),)),
        ("how do we clear HFA?", (BossReference("FA", "h"),)),
        ("how do we handle HFA?", (BossReference("FA", "h"),)),
        ("how should I approach FA?", (BossReference("FA", None),)),
        ("how to beat FA, please?", (BossReference("FA", None),)),
        ("guide for The First Adversary", (BossReference("FA", None),)),
        ("Kalos mechanics", (BossReference("Kalos", None),)),
        ("what attacks does FA have?!", (BossReference("FA", None),)),
        ("what moves should I watch for in FA?", (BossReference("FA", None),)),
        ("what should I watch out for in FA?", (BossReference("FA", None),)),
        ("what mechanics and requirements does FA have?", (BossReference("FA", None),)),
        ("tips for FA, HFA", (BossReference("FA", "h"),)),
        (
            "strategy for HFA and Extreme Kalos",
            (BossReference("FA", "h"), BossReference("Kalos", "x")),
        ),
        (
            "strategy for HFA + Extreme Kalos",
            (BossReference("FA", "h"), BossReference("Kalos", "x")),
        ),
    ],
)
def test_route_strategy_resolves_aliases_and_preserves_difficulty(bosses, text, expected):
    result = route_strategy_intent(text, bosses)

    assert result.kind == "resolved"
    assert result.references == expected


def test_schedule_wording_without_a_strong_cue_is_not_strategy(bosses):
    assert route_strategy_intent("when are we clearing HFA?", bosses).kind == "not_strategy"
    assert route_strategy_intent("how do I schedule HFA?", bosses).kind == "not_strategy"
    assert route_strategy_intent("move HFA to Tuesday", bosses).kind == "not_strategy"


@pytest.mark.parametrize(
    "text",
    ["strategy", "tips for Zakum", "tips for Lotus, Damien", "tips for Lotus and Damien"],
)
def test_route_strategy_refuses_zero_or_partial_unknown_targets(bosses, text):
    result = route_strategy_intent(text, bosses)

    assert result.kind == "unresolved"
    assert result.references == ()
    assert result.reply == STRATEGY_CLARIFICATION_REPLY


def test_route_strategy_requires_a_narrower_list_after_three_bosses(bosses):
    result = route_strategy_intent("guide for FA, Kalos, Seren, and Lotus", bosses)

    assert result.kind == "unresolved"
    assert result.reply == STRATEGY_NARROW_REPLY
