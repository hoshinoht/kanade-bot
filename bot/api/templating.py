"""The Jinja environment the portal renders through.

Kept apart from :mod:`bot.api.app` so a template can be rendered in a test
without building a whole application, and so every page shares one set of
filters -- the guild timezone in particular, which has to appear on every screen
(DESIGN.md §5) and must never be re-derived per template.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any

from fastapi.templating import Jinja2Templates

from ..formatting import STATUS_LABEL
from ..ids import short_id
from ..timeutil import from_iso, utcnow
from ..weeks import WEEKDAY_NAMES

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

#: Pinned so the page cannot change under us, with the integrity hash cdnjs
#: publishes for it. The portal reads and submits fine without htmx -- every
#: action is a real form -- so a blocked CDN degrades rather than breaks.
HTMX_SRC = "https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.10/htmx.min.js"
HTMX_SRI = (
    "sha512-5l65kMjGvrm7EATevCK/SFdMUfIUQfAsQLZY57t7tmsPf5"
    "ZtjrzU8wzSBykpRDhqnMwlKISCrxQTURFULsXdpQ=="
)

#: The nav, in three groups: the schedule itself, then what the model did with
#: it, then the levers. Grouped rather than flat because eleven equal-weight
#: links read as a list to search rather than as a place with rooms in it -- and
#: because the phone menu needs somewhere to fold the ones that are not Week.
NAV_GROUPS = [
    (
        "Schedule",
        [
            ("week", "/", "Week"),
            ("fixed", "/fixed", "Fixed"),
            ("bosses", "/bosses", "Bosses"),
        ],
    ),
    (
        "Kanade",
        [
            ("inbox", "/inbox", "Inbox"),
            ("extractions", "/extractions", "Extractions"),
            ("chat", "/chat", "Chat"),
            ("limits", "/limits", "Limits"),
        ],
    ),
    (
        "Operate",
        [
            ("members", "/members", "Members"),
            ("reminders", "/reminders", "Reminders"),
            ("config", "/config", "Config"),
            ("audit", "/audit", "Audit"),
        ],
    ),
]

#: The two the phone keeps in reach: tonight's runs, and anything waiting on an
#: answer. Everything else folds into the disclosure menu beside them.
NAV_PINNED = ("week", "inbox")

#: The same links, flat, for anything that wants "every page there is".
NAV = [item for _group, items in NAV_GROUPS for item in items]

#: The five colourways, each a light face and (in `portal.css`) an after-hours
#: dark one the system picks on its own. ``ground`` and ``accent`` are the two
#: swatch dots on the Config page -- the light face, because that is the one a
#: person is choosing between.
#:
#: The *choice* is not here and never reaches the server: it lives in the
#: browser's ``localStorage`` and is applied by the inline bootstrap in
#: ``partials/theme_boot.html``. This list only says which five exist, so the
#: Config page can offer them; the same five keys are repeated in that snippet
#: and in ``static/portal.js``, which is what stops a cookie, a route and a
#: redirect existing for something no other browser will ever need to know.
#:
#: Named for the colour each one actually is, so the list reads as a palette
#: rather than as a set of references. A browser that stored one of the older
#: names is moved onto its new one by the bootstrap, which is the only reason
#: those names still exist anywhere.
COLORWAYS = [
    {"key": "marigold", "name": "Marigold", "ground": "#eec75f", "accent": "#4d5c9e"},
    {"key": "blossom", "name": "Blossom", "ground": "#f2a8b8", "accent": "#d5537a"},
    {"key": "periwinkle", "name": "Periwinkle", "ground": "#9fb0e4", "accent": "#4a5fae"},
    {"key": "coral", "name": "Coral", "ground": "#e87d85", "accent": "#d95965"},
    {"key": "twilight", "name": "Twilight", "ground": "#8b7ad2", "accent": "#6446ab"},
]

#: What a browser with nothing stored gets: the palette on `:root`, so a page
#: with no ``data-colorway`` at all is already wearing it.
DEFAULT_COLORWAY = "marigold"

#: The six run states as drawings rather than as emoji.
#:
#: :data:`bot.formatting.STATUS_MARK` is Discord's vocabulary and stays Discord's:
#: over there a reaction *is* an emoji, and a reader who has seen "⚠️ unconfirmed"
#: on a card should find the same glyph on the next one. The portal is not a chat
#: client. Its emoji were the one thing on the page a colourway could not tint and
#: the reader's operating system drew for us, so here each state is a name in
#: ``partials/icons.html``, coloured by the ``.status--*`` rule it sits inside:
#: amber for unconfirmed, green for confirmed, red for at risk, blue for own time,
#: and the two finished states in the same grey the row itself fades to.
#:
#: A test holds these keys equal to ``STATUS_LABEL``'s, so a seventh state cannot
#: reach the board with nothing to draw.
STATUS_ICONS: dict[str, str] = {
    "planned": "alert-triangle",
    "confirmed": "check",
    "at_risk": "alert-circle",
    "otot": "clock",
    "done": "check",
    "cancelled": "x",
}

#: The same six labels with the emoji taken off the front, for the places the
#: portal writes the state out in words beside its icon. Derived rather than
#: retyped: the words are Discord's too, and only the picture differs.
STATUS_WORDS: dict[str, str] = {
    status: label.split(" ", 1)[-1] for status, label in STATUS_LABEL.items()
}

#: The Config window's sections, in the order the sidebar lists them: the four
#: things changed often, then how it looks, then the two actions, then the two
#: read-only tables.
#:
#: One list, three readers. The template builds the sidebar and the panels from
#: it; ``portal.css`` enumerates the same keys to mark the open tab (a
#: stylesheet cannot compare an ``href`` to an id, so the pairs are written out
#: and a test holds them to this list); and :func:`read_section` validates the
#: hidden field each form carries so a save lands back where it was made.
CONFIG_SECTIONS = [
    ("pings", "Pings"),
    ("watching", "Chat watching"),
    ("chatbot", "Chatbot"),
    ("notifications", "Notifications"),
    ("theme", "Theme"),
    ("digest", "Weekly digest"),
    ("rescan", "Rescan"),
    ("access", "Channel access"),
    ("env", "Set in .env"),
]

_SECTION_KEYS = frozenset(key for key, _label in CONFIG_SECTIONS)

#: The tabs on the Limits window, in the order the strip lists them: who may
#: ask at all, then what is happening right now, then who is mid-window, then
#: the one thing this page is for changing.
#:
#: Read the same three ways :data:`CONFIG_SECTIONS` is -- the template builds
#: the strip and the panels from it, ``portal.css`` enumerates the same keys to
#: raise the open tab, and a test holds those two to this list. The first key is
#: the panel a reader with no fragment lands on.
LIMITS_TABS = [
    ("who-may-ask", "Who may ask"),
    ("in-flight", "In flight"),
    ("windows", "Windows"),
    # `set-allowance` rather than `allowance`: the Set button on every roster
    # row is a deep link to this panel, and that fragment is in the wild.
    ("set-allowance", "Allowance"),
]

#: The tabs on one chat interaction: what was said, what it looked up, and the
#: facts about the exchange. Same three readers as above.
CHAT_TABS = [
    ("conversation", "Conversation"),
    ("tool-trace", "Tool trace"),
    ("interaction", "This interaction"),
]

#: The tabs on one extraction, shaped like the interaction's: what came out
#: first, then what went in, then the raw exchange for when the answer is wrong
#: and you need to see what the model was actually handed, then the facts.
EXTRACTION_TABS = [
    ("produced", "What it produced"),
    ("chat-read", "The chat it read"),
    ("raw", "Raw exchange"),
    ("extraction", "This extraction"),
]


def read_section(value: str | None) -> str:
    """The section a config form says it came from, or ``""`` for none.

    Goes into a redirect's fragment, so it is checked against the list above
    rather than trusted -- and an unknown one is simply dropped, which lands on
    the first section, which is where a reader with no fragment starts anyway.
    """
    return value if value in _SECTION_KEYS else ""


def local_dt(value: Any, bot: BossBot, fmt: str = "%a %d %b %H:%M") -> str:
    """Render an ISO string or datetime in the guild timezone."""
    if value is None:
        return "—"
    moment = from_iso(value) if isinstance(value, str) else value
    if not isinstance(moment, datetime):  # pragma: no cover - defensive
        return str(value)
    return moment.astimezone(bot.tz).strftime(fmt)


def duration(ms: int | None) -> str:
    """Milliseconds as something readable at a glance.

    Under a second stays in milliseconds, which is the unit a tool call is
    argued about in. Above it, seconds: a chat answer is tens of thousands of
    milliseconds and ``31240 ms`` has to be counted digit by digit before it
    means "half a minute".
    """
    if ms is None:
        return "—"
    return f"{ms} ms" if ms < 1000 else f"{ms / 1000:.1f} s"


def confidence_band(value: float | None) -> str:
    """Low / mid / high, so the inbox can colour a number without a chart."""
    if value is None:
        return "unknown"
    if value >= 0.85:
        return "high"
    if value >= 0.65:
        return "mid"
    return "low"


def build_templates(directory: Path, bot: BossBot) -> Jinja2Templates:
    templates = Jinja2Templates(directory=str(directory))
    env = templates.env
    env.trim_blocks = True
    env.lstrip_blocks = True
    env.filters["local_dt"] = lambda v, fmt="%a %d %b %H:%M": local_dt(v, bot, fmt)
    env.filters["short_id"] = short_id
    env.filters["confidence_band"] = confidence_band
    env.filters["duration"] = duration
    env.globals.update(
        {
            "tz_name": bot.settings.tz,
            "guild_id": bot.settings.guild_id,
            "nav": NAV,
            "nav_groups": NAV_GROUPS,
            "nav_pinned": NAV_PINNED,
            "colorways": COLORWAYS,
            "default_colorway": DEFAULT_COLORWAY,
            "config_sections": CONFIG_SECTIONS,
            "limits_tabs": LIMITS_TABS,
            "chat_tabs": CHAT_TABS,
            "extraction_tabs": EXTRACTION_TABS,
            "status_icons": STATUS_ICONS,
            "status_words": STATUS_WORDS,
            "weekday_names": WEEKDAY_NAMES,
            "htmx_src": HTMX_SRC,
            "htmx_sri": HTMX_SRI,
            "now": utcnow,
        }
    )
    return templates


__all__ = [
    "CHAT_TABS",
    "COLORWAYS",
    "CONFIG_SECTIONS",
    "DEFAULT_COLORWAY",
    "EXTRACTION_TABS",
    "LIMITS_TABS",
    "HTMX_SRC",
    "HTMX_SRI",
    "NAV",
    "NAV_GROUPS",
    "NAV_PINNED",
    "STATUS_ICONS",
    "STATUS_WORDS",
    "build_templates",
    "confidence_band",
    "duration",
    "local_dt",
    "read_section",
]
