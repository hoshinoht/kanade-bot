"""Portal state marks stay independent from Discord's emoji vocabulary."""

from __future__ import annotations

from bot.agent import formatting


def test_the_week_page_carries_none_of_the_states_emoji(auth, seeded):
    body = auth.get("/").text
    for mark in formatting.STATUS_MARK.values():
        assert mark not in body, mark
