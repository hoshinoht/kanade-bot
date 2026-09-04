"""The portal draws its own marks; Discord keeps its emoji.

An emoji is a picture the reader's operating system chooses, which is exactly
right in a chat client -- a reaction *is* an emoji, and the bot's cards have to
go on speaking that -- and exactly wrong on a page with five colourways and a
dark face for each, where the one glyph nobody could restyle was the one
carrying the meaning.

So `templates/partials/icons.html` draws them instead, and what is worth testing
is the seam: that every state has something to draw, that nothing asks for a
drawing the sheet does not have, that the colour still comes from the tokens --
and that the sweep stopped at the water's edge, leaving Discord's vocabulary
alone.
"""

from __future__ import annotations

import re

from bot.agent import formatting
from bot.api.app import TEMPLATE_DIR
from bot.api.templating import STATUS_ICONS, STATUS_WORDS

ICON_SHEET = (TEMPLATE_DIR / "partials" / "icons.html").read_text(encoding="utf-8")

#: Every name the sheet knows how to draw, taken from the branches themselves.
DRAWN = set(re.findall(r'name == "([a-z-]+)"', ICON_SHEET))

#: Every name any template asks for.
ASKED = {
    name
    for path in TEMPLATE_DIR.rglob("*.html")
    if path.name != "icons.html"
    for name in re.findall(r'icon\(\s*"([a-z-]+)"', path.read_text(encoding="utf-8"))
}


# --- the sheet and the pages agree ------------------------------------------


def test_every_icon_the_portal_asks_for_is_one_the_sheet_draws():
    """A name with no branch renders an empty box, which is worse than an emoji
    was: it is a mark that means nothing and says nothing."""
    assert ASKED
    assert ASKED <= DRAWN, sorted(ASKED - DRAWN)


def test_no_icon_is_drawn_that_nobody_asks_for():
    """The set is deliberately small. An icon nobody uses is bytes on every page
    that loads, and a second vocabulary waiting to be picked up by accident."""
    used = ASKED | set(STATUS_ICONS.values())
    assert DRAWN <= used, sorted(DRAWN - used)


def test_every_state_the_bot_knows_has_something_to_draw():
    """The board shows one of six states on every card. A seventh must not be
    able to arrive with nothing to draw for it."""
    assert set(STATUS_ICONS) == set(formatting.STATUS_LABEL)
    assert set(STATUS_ICONS.values()) <= DRAWN


def test_every_state_also_has_its_word():
    """The icon is what a reader sees; the word is what a screen reader says,
    and what the tooltip carries. Both, for all six."""
    assert set(STATUS_WORDS) == set(formatting.STATUS_LABEL)
    for status, word in STATUS_WORDS.items():
        assert word and word in formatting.STATUS_LABEL[status]
        assert word == word.strip()


# --- colour comes from the page, never from the drawing ----------------------


def test_an_icon_is_the_colour_of_the_words_around_it():
    """`currentColor` is the whole trick: a `.status--at_risk` cell makes its
    cross red and a title bar makes its ✕ the chrome's ink, without either
    colour -- or any of the ten colourway faces -- being named in the sheet."""
    assert 'stroke="currentColor"' in ICON_SHEET
    assert not re.search(r"#[0-9a-fA-F]{3,6}\b", ICON_SHEET)
    assert "rgb(" not in ICON_SHEET and "var(--" not in ICON_SHEET


def test_an_icon_is_sized_by_the_text_it_stands_in(client):
    """One em, so there is no icon size to choose and none to keep in step with
    the type ladder."""
    css = client.get("/static/portal.css").text
    rule = css[css.index("\n.icon {") :]
    rule = rule[: rule.index("}")]

    assert "width: 1em" in rule
    assert "height: 1em" in rule


# --- what a reader who cannot see it is told ---------------------------------


def test_an_icon_says_nothing_out_loud_unless_it_is_given_words():
    """A picture of something the page already says in words is noise read
    aloud. Where the icon *is* the content, the words come with it."""
    assert ICON_SHEET.count('aria-hidden="true"') == 1
    assert 'class="vh"' in ICON_SHEET


def test_a_permission_cell_says_granted_or_missing(auth, fake_bot, seeded):
    """The cell is nothing but its mark, so the mark carries its own words."""
    body = auth.get("/config").text
    table = body[body.index('id="access"') : body.index("means the bot")]

    assert 'data-icon="check"' in table
    assert "granted" in table


def test_the_board_still_says_the_state_it_draws(auth, seeded):
    body = auth.get("/").text
    card = body[body.index('class="runcard') :]
    card = card[: card.index("</a>")]

    assert 'data-icon="alert-triangle"' in card
    assert "unconfirmed" in card
    assert 'class="vh"' in card


# --- the sweep stopped at Discord --------------------------------------------


def test_the_week_page_carries_none_of_the_states_emoji(auth, seeded):
    """The six marks were the portal's last emoji, and the reason for all this:
    they were the only thing on the page a colourway could not touch."""
    body = auth.get("/").text
    for mark in formatting.STATUS_MARK.values():
        assert mark not in body, mark


def test_discord_keeps_the_emoji_it_speaks_in():
    """Nothing here may reach across. A reaction *is* an emoji, the quiet marker
    is a character on a real message, and the test prefix is read by people in a
    chat client -- none of them is a picture we get to choose."""
    assert formatting.STATUS_MARK["planned"] == "⚠️"
    assert formatting.QUIET_MARKER == "🔕"
    for word in STATUS_WORDS.values():
        assert not any(mark in word for mark in formatting.STATUS_MARK.values())
