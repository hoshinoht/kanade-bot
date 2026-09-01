"""The look of the portal, the grouped nav, and the bot's own identity art.

Things that are only ever seen, never computed, so what is worth testing is the
plumbing under them: a look the server never learns and therefore cannot get
wrong, the two hand-copied dark blocks staying identical, a nav that still has
every page in it, and two image routes that answer honestly when nothing has
been cached -- which is the state a fresh deployment and every test run are in.
"""

from __future__ import annotations

import asyncio
import re
from pathlib import Path

import pytest

from bot import identity
from bot.api.app import STATIC_DIR
from bot.api.templating import COLORWAYS, DEFAULT_COLORWAY, NAV, NAV_GROUPS

from .fake_bot import ADMIN_TOKEN, make_settings

PNG = b"\x89PNG\r\n\x1a\n and then some pixels"

PAGE_JS = (STATIC_DIR / "portal.js").read_text(encoding="utf-8")
PAGE_CSS = (STATIC_DIR / "portal.css").read_text(encoding="utf-8")

DARK_MEDIA = "@media (prefers-color-scheme: dark)"


def rule_body(selector: str) -> str:
    """What one rule declares, by exact selector.

    Token blocks contain no nested braces, so "up to the next `}`" is the whole
    rule. Anchored to the start of a line and matched with its brace attached,
    which is what keeps `th` from matching the tail of `width` and
    `:root:not([data-theme="light"])` from matching its colourway variants.
    """
    match = re.search(
        r"^[ \t]*" + re.escape(selector) + r"\s*\{([^}]*)\}", PAGE_CSS, re.MULTILINE
    )
    assert match is not None, f"no rule for {selector}"
    return match.group(1)


def tokens_of(selector: str) -> dict[str, str]:
    """The custom properties one rule sets, as name -> value."""
    return {
        name: value.strip()
        for name, value in re.findall(r"(--[\w-]+)\s*:\s*([^;]+);", rule_body(selector))
    }


def dark_twins() -> list[tuple[str, str]]:
    """Every pair of "the device says dark" / "the reader said dark" blocks."""
    pairs = [(':root:not([data-theme="light"])', ':root[data-theme="dark"]')]
    for way in COLORWAYS:
        if way["key"] == DEFAULT_COLORWAY:
            continue  # otonose is the unqualified one, already above
        key = way["key"]
        pairs.append(
            (
                f':root:not([data-theme="light"])[data-colorway="{key}"]',
                f':root[data-theme="dark"][data-colorway="{key}"]',
            )
        )
    return pairs


def media_block() -> str:
    start = PAGE_CSS.index(DARK_MEDIA)
    return PAGE_CSS[start : PAGE_CSS.index("\n}\n", start)]


#: The type scale, smallest first. Every size on the page is one of these or a
#: display size named where it is used.
LADDER = ["micro", "mini", "small", "body-sm", "body", "lg", "brand"]


def rem(value: str) -> float:
    """A `1.1rem` or `var(--fs-lg)` value as a number, following the token once."""
    token = re.fullmatch(r"var\((--[\w-]+)\)", value.strip())
    if token:
        value = tokens_of(":root")[token.group(1)]
    match = re.fullmatch(r"([\d.]+)rem", value.strip())
    assert match is not None, f"not a rem size: {value}"
    return float(match.group(1))


def font_size_of(selector: str) -> float:
    """What one rule sets its text to, in rem."""
    declared = re.search(r"font-size:\s*([^;]+);", rule_body(selector))
    assert declared is not None, f"{selector} sets no font-size"
    return rem(declared.group(1))


# --- the look never reaches the server --------------------------------------


def test_no_page_is_rendered_wearing_a_look(auth, seeded):
    """Both are this browser's preference: the bot has no opinion and keeps no note."""
    for path in ("/", "/config", "/extractions/deadbeef"):
        body = auth.get(path).text
        assert '<html lang="en">' in body
        assert "data-colorway=" not in body
        assert "data-theme=" not in body


def test_choosing_one_writes_nothing_back(auth):
    """No cookie, no route -- the whole feature is `portal.js` and localStorage."""
    response = auth.get("/config")
    assert "set-cookie" not in response.headers
    assert auth.cookies.get("colorway") is None
    # The route the cookie version needed is simply not there any more.
    assert auth.post("/theme", data={"colorway": "coral"}).status_code == 404


def test_the_bootstrap_runs_before_the_stylesheet(auth):
    """After first paint would mean the default theme, then a swap to the real one."""
    head = auth.get("/").text
    boot = head.index('localStorage.getItem("colorway")')
    assert boot < head.index('href="/static/portal.css"')
    assert boot > head.index("<head>")


def test_the_bootstrap_only_ever_stamps_one_of_the_five(auth):
    """Straight into a `data-` attribute, so it is checked before it goes there."""
    body = auth.get("/").text
    snippet = body[body.index("<script>") : body.index("</script>")]
    for way in COLORWAYS:
        assert f'"{way["key"]}"' in snippet
    assert "indexOf(c) > -1" in snippet
    assert "document.documentElement.dataset.colorway = c" in snippet
    # localStorage throws rather than answering in some private windows.
    assert "try {" in snippet and "catch" in snippet


def test_the_bootstrap_stamps_the_mode_and_only_the_two(auth):
    """Same rule as the colourway: validated before it becomes an attribute."""
    body = auth.get("/").text
    snippet = body[body.index("<script>") : body.index("</script>")]
    assert 'localStorage.getItem("theme")' in snippet
    assert '["light", "dark"].indexOf(t) > -1' in snippet
    assert "document.documentElement.dataset.theme = t" in snippet
    # "system" is never stored, so it must not be a value the snippet knows.
    assert '"system"' not in snippet


def test_the_sign_in_page_carries_it_too(client):
    """Its own document, so it needs its own copy -- which is why it is a partial."""
    body = client.get("/login").text
    assert 'localStorage.getItem("colorway")' in body
    assert 'localStorage.getItem("theme")' in body
    assert body.index("<script>") < body.index('href="/static/portal.css"')


def test_the_snippet_and_the_script_agree_on_the_five(auth):
    """Three places name them; the two that cannot import each other are checked here."""
    body = auth.get("/").text
    snippet = body[body.index("<script>") : body.index("</script>")]
    listed = 'var COLORWAYS = ["otonose", "nazuna", "sumire", "coral", "hinano"];'

    assert listed in PAGE_JS
    for way in COLORWAYS:
        assert f'"{way["key"]}"' in snippet
    assert [way["key"] for way in COLORWAYS] == [
        "otonose",
        "nazuna",
        "sumire",
        "coral",
        "hinano",
    ]
    assert COLORWAYS[0]["key"] == DEFAULT_COLORWAY


def test_the_script_applies_the_choice_and_then_remembers_it(auth):
    """In that order: a window that refuses to store it still changes colour."""
    applied = PAGE_JS.index("document.documentElement.dataset.colorway = input.value")
    assert applied < PAGE_JS.index("window.localStorage.setItem(COLORWAY_KEY, input.value)")


def test_the_script_and_the_snippet_agree_on_the_two_modes(auth):
    """The stylesheet only knows "light" and "dark"; nothing may store a third."""
    body = auth.get("/").text
    snippet = body[body.index("<script>") : body.index("</script>")]

    assert 'var THEMES = ["light", "dark"];' in PAGE_JS
    assert '["light", "dark"]' in snippet


def test_choosing_system_puts_the_question_back_to_the_device(auth):
    """It is the absence of a stored choice, so both the key and the attribute go."""
    assert "delete document.documentElement.dataset.theme" in PAGE_JS
    assert "window.localStorage.removeItem(THEME_KEY)" in PAGE_JS


# --- the card ---------------------------------------------------------------


def test_the_config_page_offers_all_five_with_the_default_ticked(auth):
    body = auth.get("/config").text
    card = body[body.index("Colourway") : body.index("Weekly digest")]

    for way in COLORWAYS:
        assert f'value="{way["key"]}"' in card
        assert way["name"] in card
        assert way["ground"] in card and way["accent"] in card
    # The server cannot know which one is stored, so it ticks the default and
    # `markColorway` in portal.js moves the tick as soon as it runs.
    flat = " ".join(card.split())
    assert f'value="{DEFAULT_COLORWAY}" checked' in flat
    # One per fieldset: the default colourway, and System.
    assert flat.count("checked") == 2


def test_the_config_page_offers_the_three_modes_with_system_checked(auth):
    """System is what a browser with nothing stored -- and no script -- is on."""
    body = auth.get("/config").text
    card = " ".join(body[body.index("Colourway") : body.index("Weekly digest")].split())

    for value, label in (("system", "System"), ("light", "Light"), ("dark", "Dark")):
        assert f'name="thememode" value="{value}"' in card
        assert f">{label}<" in card
    assert 'value="system" checked' in card
    assert 'value="light" checked' not in card
    assert 'value="dark" checked' not in card


def test_the_card_is_not_a_form_and_says_why(auth):
    """The two documented exceptions to "every control is a real form"."""
    body = auth.get("/config").text
    card = body[body.index("Colourway") : body.index("Weekly digest")]

    assert "<form" not in card
    assert 'action="/theme"' not in card
    assert "<noscript>" in card
    assert "JavaScript" in card


def test_the_script_moves_both_ticks_to_whatever_is_stored(auth):
    assert 'querySelectorAll(\'input[name="colorway"]\')' in PAGE_JS
    assert 'querySelectorAll(\'input[name="thememode"]\')' in PAGE_JS
    assert "onReady(markColorway)" in PAGE_JS
    assert "onReady(markTheme)" in PAGE_JS


# --- the stylesheet's two dark faces ----------------------------------------


@pytest.mark.parametrize("inherited,chosen", dark_twins())
def test_the_two_dark_blocks_say_exactly_the_same_thing(inherited, chosen):
    """The one real risk in this design: one night face, written out twice.

    A dark palette cannot be expressed once, because "the device says dark" is a
    media query and "the reader said dark" is a selector, and CSS has no way to
    or them together. Resolving it in JavaScript instead would leave a browser
    with the script off unable to be dark at all. So they are copies -- and this
    is what notices when somebody edits one of them.
    """
    assert tokens_of(inherited)
    assert tokens_of(inherited) == tokens_of(chosen)


def test_an_explicit_light_beats_a_dark_device():
    """Every block in the media query steps aside for a reader who asked for light."""
    selectors = re.findall(r"^  (\S.*?) \{$", media_block(), re.MULTILINE)

    assert selectors
    for selector in selectors:
        assert selector.startswith(':root:not([data-theme="light"])'), selector


def test_the_media_query_carries_tokens_and_nothing_else():
    """A component styled only in there would lose its colours under `data-theme`.

    Everything else in this file reads its colour from a token, and the tokens
    are declared in both faces; a rule that only exists when the *device* is dark
    would simply not apply to somebody who asked for dark on a light machine.
    """
    body = re.sub(r"/\*.*?\*/", "", media_block(), flags=re.DOTALL)
    ordinary = re.findall(r"^\s+(?!--)([a-z][\w-]*)\s*:", body, re.MULTILINE)

    assert ordinary == []


def test_an_explicit_choice_tells_the_browsers_own_furniture_too():
    """Scrollbars and form controls, which otherwise stay on the device's answer."""
    assert "color-scheme: light dark;" in rule_body(":root")
    assert "color-scheme: light;" in rule_body(':root[data-theme="light"]')
    assert "color-scheme: dark;" in rule_body(':root[data-theme="dark"]')


# --- the nav ----------------------------------------------------------------


def test_the_nav_links_are_big_enough_to_read():
    """From feedback on the live portal: at 0.8rem they were the smallest type in
    the masthead, which is the wrong end of the scale for the only navigation
    there is. A floor rather than a fixed size -- the point is that it never
    goes back to being fine print."""
    assert font_size_of(".nav__link") >= 0.9


# --- the type scale ---------------------------------------------------------


def test_the_ladder_is_a_ladder():
    """Seven steps, each bigger than the last and none of them a duplicate.

    Written as one scale rather than as twenty-three sizes that happened to be
    typed, so "make it all bigger" -- which is what was asked for -- is a change
    to seven numbers rather than a hunt through the file.
    """
    steps = [rem(tokens_of(":root")[f"--fs-{name}"]) for name in LADDER]

    assert steps == sorted(steps)
    assert len(set(steps)) == len(steps)


def test_every_size_on_the_page_is_a_step_or_a_named_display_size():
    """No twenty-fourth value quietly reappearing inside a component.

    The exceptions are deliberate and each is commented where it lives: two
    monograms and an avatar fitted to their squares, the two clocks, the rail's
    date and its corner marker, and the stat tile's count.
    """
    literals = sorted(set(re.findall(r"font-size:\s*([\d.]+rem);", PAGE_CSS)))

    assert literals == [
        "0.62rem",  # the rail's reset marker, below the ladder on purpose
        "0.66rem",  # portrait monogram, small
        "0.82rem",  # the masthead avatar's letter
        "0.84rem",  # portrait monogram, medium
        "1.05rem",  # "own time" -- words, not a clock reading
        "1.15rem",  # the rail's date, on a phone
        "1.25rem",  # a run's clock, on a phone
        "1.4rem",  # the rail's date
        "1.55rem",  # a run's clock: the loudest thing on the row
        "1.75rem",  # a stat tile's count, and the login avatar's letter
    ]


def test_the_page_title_still_outranks_the_loudest_row():
    """h1 is fluid, so the floor is what has to beat the clock -- at every width."""
    floor = re.search(r"font-size:\s*clamp\(([\d.]+)rem", rule_body("h1"))

    assert floor is not None
    assert float(floor.group(1)) > font_size_of(".run__time")


def test_the_bump_did_not_flatten_the_hierarchy():
    """Everything came up; what outranked what still does."""
    assert font_size_of(".run__time") > font_size_of(".run__bosses")
    assert font_size_of(".run__bosses") > font_size_of(".run__meta")
    assert font_size_of(".brand") > font_size_of(".nav__link")
    assert font_size_of(".nav__link") > font_size_of(".nav__eyebrow")
    assert font_size_of(".card__title") > font_size_of("table")
    assert font_size_of("table") > font_size_of("th")
    assert font_size_of(".note") > font_size_of(".chip")
    # The eyebrows stay the smallest thing anybody is expected to read.
    smallest = rem(tokens_of(":root")["--fs-micro"])
    for selector in (".eyebrow", "th", ".stat__name", ".label", ".pill"):
        assert font_size_of(selector) == smallest, selector


def test_the_body_is_the_scales_own_middle():
    """`rem` is the root's size, not this rule's, so the two have to be said to agree."""
    assert font_size_of("body") == 1.0
    assert rem(tokens_of(":root")["--fs-body"]) == 1.0


# --- the nav ----------------------------------------------------------------


def test_every_page_is_still_in_the_nav_after_the_grouping():
    """The groups are the source; the flat list is derived, so they cannot drift."""
    assert NAV == [item for _label, items in NAV_GROUPS for item in items]
    assert len(NAV) == len({key for key, _href, _label in NAV})
    assert [label for label, _items in NAV_GROUPS] == ["Schedule", "Kanade", "Operate"]


def test_the_nav_is_grouped_and_every_link_is_there(auth, seeded):
    body = auth.get("/").text
    for label, _items in NAV_GROUPS:
        assert f'class="nav__eyebrow">{label}<' in body
    for _key, href, label in NAV:
        assert f'class="nav__link" href="{href}"' in body, href
        assert label in body, label


def test_the_page_you_are_on_is_marked(auth, seeded):
    body = auth.get("/members").text
    assert '<a class="nav__link" href="/members" aria-current="page">' in body


def test_the_pending_pip_survives_the_regrouping(auth, seeded):
    assert '<span class="pip">1</span>' in auth.get("/").text


def test_the_phone_menu_is_a_real_disclosure(auth):
    """No script: <details> opens on its own, and the two links beside it are links."""
    body = auth.get("/").text
    phone = body[body.index('class="nav__phone"') : body.index("</nav>")]
    assert '<details class="nav__more">' in phone
    assert "<summary" in phone
    assert 'href="/inbox"' in phone  # the pinned pair stay outside the menu
    assert "onclick" not in phone


# --- the identity cache -----------------------------------------------------


def identity_client(fake_bot, tmp_path: Path):
    """The app, with a data directory a test can drop files into."""
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.settings = make_settings(db_path=str(tmp_path / "bot.sqlite"))
    return TestClient(create_app(fake_bot))


def test_nothing_cached_is_a_404_not_a_broken_page(fake_bot, tmp_path):
    with identity_client(fake_bot, tmp_path) as client:
        client.headers["Authorization"] = f"Bearer {ADMIN_TOKEN}"
        assert client.get("/identity/avatar").status_code == 404
        assert client.get("/identity/banner").status_code == 404


def test_a_cached_avatar_is_served_as_what_it_actually_is(fake_bot, tmp_path):
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(PNG)

    with identity_client(fake_bot, tmp_path) as client:
        response = client.get("/identity/avatar")

    assert response.status_code == 200
    assert response.content == PNG
    assert response.headers["content-type"] == "image/png"
    assert "max-age" in response.headers["cache-control"]


def test_an_animated_avatar_keeps_its_own_type_under_the_png_name(fake_bot, tmp_path):
    """Discord hands back a GIF for an animated avatar; the cache filename is fixed."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(b"GIF89a still not a real gif")

    with identity_client(fake_bot, tmp_path) as client:
        assert client.get("/identity/avatar").headers["content-type"] == "image/gif"


def test_the_art_does_not_need_a_session(fake_bot, tmp_path):
    """Like the stylesheet and the boss portraits: the sign-in page is wearing it."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "banner.png").write_bytes(PNG)

    with identity_client(fake_bot, tmp_path) as client:
        assert client.get("/identity/banner").status_code == 200


def test_the_sign_in_page_needs_no_image_to_look_right(client):
    """It asks for both and gets neither, and there is no <img> to break."""
    body = client.get("/login").text
    assert "gate__hero" in body
    assert "gate__avatar" in body
    assert "<img" not in body


# --- writing the cache ------------------------------------------------------


class FakeAsset:
    """Stands in for ``discord.Asset``: the one method this code calls."""

    def __init__(self, data: bytes | None = None, error: Exception | None = None):
        self.data = data
        self.error = error

    async def read(self) -> bytes:
        if self.error is not None:
            raise self.error
        return self.data or b""


class FakeUser:
    def __init__(self, avatar=None, banner=None):
        self.id = 5555555555555555555
        self.display_avatar = avatar
        self.banner = banner


class FakeClient:
    """Only what :func:`bot.identity.refresh` reaches for."""

    def __init__(self, tmp_path: Path, avatar=None, banner=None, fetch_error=None):
        self.settings = make_settings(db_path=str(tmp_path / "bot.sqlite"))
        self.user = FakeUser(avatar)
        self.banner = banner
        self.fetch_error = fetch_error
        self.fetched: list[int] = []

    async def fetch_user(self, user_id):
        self.fetched.append(user_id)
        if self.fetch_error is not None:
            raise self.fetch_error
        return FakeUser(banner=self.banner)


def test_both_pictures_are_written_beside_the_database(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG), banner=FakeAsset(b"GIF89a banner"))

    written = asyncio.run(identity.refresh(client))

    assert written == ["avatar.png", "banner.png"]
    assert (tmp_path / "identity" / "avatar.png").read_bytes() == PNG
    assert (tmp_path / "identity" / "banner.png").read_bytes() == b"GIF89a banner"
    # The banner is not on the gateway payload, so it costs one REST fetch.
    assert client.fetched == [client.user.id]


def test_a_bot_with_no_banner_still_gets_its_avatar(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG))

    assert asyncio.run(identity.refresh(client)) == ["avatar.png"]
    assert not (tmp_path / "identity" / "banner.png").exists()


def test_discord_being_unreachable_leaves_the_last_copy_alone(tmp_path):
    """Identity art is cosmetic: a refresh that fails must not empty the cache."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(PNG)
    client = FakeClient(
        tmp_path,
        avatar=FakeAsset(error=OSError("no route to host")),
        fetch_error=OSError("no route to host"),
    )

    assert asyncio.run(identity.refresh(client)) == []
    assert (tmp_path / "identity" / "avatar.png").read_bytes() == PNG


def test_an_in_memory_database_has_nowhere_to_cache(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG))
    client.settings = make_settings(db_path=":memory:")

    assert asyncio.run(identity.refresh(client)) == []
    assert identity.identity_dir(":memory:") is None
    assert identity.cached(":memory:", identity.AVATAR_NAME) is None


def test_a_half_written_file_is_never_what_a_browser_sees(tmp_path):
    """Written to a temp name in the same directory, then renamed over."""
    target = tmp_path / "identity" / "avatar.png"
    identity.write_atomic(target, PNG)
    identity.write_atomic(target, b"\x89PNG\r\n\x1a\n newer")

    assert target.read_bytes() == b"\x89PNG\r\n\x1a\n newer"
    assert list(target.parent.iterdir()) == [target]


@pytest.mark.parametrize(
    "head,expected",
    [
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"GIF89a", "image/gif"),
        (b"\xff\xd8\xff\xe0", "image/jpeg"),
        (b"RIFF\x00\x00\x00\x00WEBPVP8 ", "image/webp"),
        (b"not a picture at all", "image/png"),
    ],
)
def test_the_type_is_read_off_the_bytes_not_the_name(tmp_path, head, expected):
    path = tmp_path / "avatar.png"
    path.write_bytes(head)
    assert identity.media_type(path) == expected


def test_caching_identity_never_breaks_a_start(tmp_path, monkeypatch):
    """`on_ready` calls this; nothing cosmetic may take the bot down with it."""
    from bot.client import BossBot

    async def explode(_client):
        raise RuntimeError("discord.py changed under us")

    monkeypatch.setattr(identity, "refresh", explode)
    asyncio.run(BossBot.cache_identity(FakeClient(tmp_path)))
