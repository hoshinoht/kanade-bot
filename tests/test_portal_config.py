"""How the Config page is laid out, and the twelve pixels that started it.

The settings cards used to sit in a grid whose rows stretched every card to its
tallest sibling. Two things came out of that: half-empty cards, and -- because
`.card + .card` drew a stacking gap that the grid then stretched back out of
sight -- a first card whose top edge floated above its neighbours' while their
bottoms stayed level. Both are layout, so both are tested here as the rules
that cause them rather than as pixels, which is all that can be checked without
a browser; the pixels are for the screenshot pass.
"""

from __future__ import annotations

import re

import pytest

from bot.api.app import STATIC_DIR

PAGE_CSS = (STATIC_DIR / "portal.css").read_text(encoding="utf-8")


def rule_body(selector: str) -> str:
    match = re.search(
        r"^[ \t]*" + re.escape(selector) + r"\s*\{([^}]*)\}", PAGE_CSS, re.MULTILINE
    )
    assert match is not None, f"no rule for {selector}"
    return match.group(1)


# --- the bug ----------------------------------------------------------------


def test_a_stacking_gap_is_not_drawn_where_a_container_already_draws_one():
    """The twelve pixels.

    `.card + .card` is how cards stacked in normal flow get their gap, and it
    still is -- but in a container with its own gap it lands on every card but
    the first and offsets them all. Under `align-items: stretch` the offset was
    invisible on the bottom edge and plain on the top, which is why it read as
    "the first card is too high" rather than "the rest are too low".
    """
    assert "margin-top: 0.75rem;" in rule_body(".card + .card")
    assert "margin-top: 0;" in rule_body(".cardcols > .card + .card")


def test_cards_stacked_in_normal_flow_still_get_their_gap(auth, seeded):
    """The two full-width cards under the wall are stacked, not packed."""
    body = auth.get("/config").text
    wall = body[body.index('class="cardcols"') : body.index('class="rule"')]

    assert body.count('<section class="card">') > wall.count('<section class="card">')
    assert "Channel access" not in wall
    assert "Set in <code>.env</code>" not in wall


# --- hollow cards, and the space they left -----------------------------------


def test_a_card_is_its_own_height_rather_than_its_tallest_sibling(auth, seeded):
    """Nothing stretches any more: the container packs instead of ruling rows."""
    packed = rule_body(".cardcols")

    assert "columns:" in packed
    assert "display: grid" not in packed
    assert "align-items: stretch" not in packed


def test_a_card_is_never_cut_in_half_by_a_column_break():
    """Both halves would draw a border and neither would look like a window."""
    assert "break-inside: avoid;" in rule_body(".cardcols > .card")


def test_the_stretching_grid_is_gone_rather_than_left_behind(auth, seeded):
    """A rule nothing uses is a rule somebody re-uses by accident later."""
    assert "grid-2" not in PAGE_CSS
    assert "grid-2" not in auth.get("/config").text


def test_the_wall_keeps_the_width_it_had(auth, seeded):
    """Three columns at the width this was reported at, as before -- the
    complaint was the space inside the cards, not how many there were."""
    packed = rule_body(".cardcols")
    width = re.search(r"columns:\s*([\d.]+)rem", packed)

    assert width is not None
    assert float(width.group(1)) * 16 == pytest.approx(280, abs=1)


# --- the frame, and why not ---------------------------------------------------


def test_config_keeps_its_scrollbar(auth, seeded):
    """Judged, not overlooked.

    Config is forms whose height changes in place -- the rescan disclosure, the
    access table redrawing itself, a <noscript> that appears only for some
    readers. A fixed frame that clipped an expanding disclosure would be worse
    than a scrollbar, and packing the cards already took most of the height the
    page was wasting.
    """
    assert '<body class="">' in auth.get("/config").text


# --- and it still says everything it said ------------------------------------


def test_every_setting_is_still_on_the_page(auth, seeded):
    body = auth.get("/config").text

    for title in (
        "Pings",
        "Chat watching",
        "Chatbot",
        "Notifications",
        "Theme",
        "Weekly digest",
        "Re-read the party channels",
        "Channel access",
    ):
        assert title in body, title


def test_the_order_the_cards_are_read_in_is_unchanged(auth, seeded):
    """Packing is a container's job. Reordering would have been churn -- and
    would have broken every test that slices this page between two headings."""
    body = auth.get("/config").text
    seen = [
        body.index(title)
        for title in ("Pings", "Chat watching", "Chatbot", "Notifications", "Theme")
    ]

    assert seen == sorted(seen)
