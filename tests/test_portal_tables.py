"""The table pages: one search box, one pager, and a window that scrolls.

Six pages that were "here is everything, scroll" became six pages that ask what
you are looking for. What is worth testing is the seam rather than the markup:
that a search narrows the right rows, that a pager lands somewhere that exists,
that the box being typed into is never the thing a swap replaces, and that with
the CDN blocked all of it is still a form and a pair of links.
"""

from __future__ import annotations

import re

import pytest

from bot.api import service
from bot.api.templating import CHAT_TABS, EXTRACTION_TABS, LIMITS_TABS
from bot.domain.ids import short_id
from bot.portal_styles import build_stylesheet

from .conftest import kl
from .fake_bot import add_pilot

#: Every page that is a list of rows, with the id of the region a search swaps.
TABLES = [
    ("/audit", "audit-rows"),
    ("/extractions", "extraction-rows"),
    ("/chat", "chat-rows"),
    ("/reminders", "reminder-rows"),
    ("/members", "member-rows"),
    ("/fixed", "fixed-rows"),
]

PAGED = ["/audit", "/extractions", "/chat", "/reminders"]

PAGE_CSS = build_stylesheet()

#: The stylesheet with its prose taken out. Every rule in that file is
#: explained above itself, so a test that scans for a declaration has to look at
#: the declarations -- otherwise a comment that *mentions* `100dvh`, saying why
#: the height is not computed that way, reads as one.
CSS_RULES = re.sub(r"/\*.*?\*/", "", PAGE_CSS, flags=re.DOTALL)


def seed_audit(fake_bot, count: int, detail: str = "change") -> None:
    for index in range(count):
        fake_bot.repo.log_audit(
            surface="portal",
            actor="token",
            action="amend",
            subject=f"run-{index:03d}",
            detail=f"{detail} {index:03d}",
            at=kl(2026, 9, 1, 12, index),
        )


# --- searching --------------------------------------------------------------


def test_a_search_narrows_the_rows_to_what_matches(auth, fake_bot):
    seed_audit(fake_bot, 3)
    fake_bot.repo.log_audit(
        surface="cli", actor="hoshino", action="config", detail="turned the extractor off"
    )

    body = auth.get("/audit?q=extractor").text

    assert "turned the extractor off" in body
    assert "change 001" not in body


def test_a_search_does_not_care_about_case(auth, fake_bot):
    seed_audit(fake_bot, 1, detail="Cancelled By Somebody")
    assert "Cancelled By Somebody" in auth.get("/audit?q=cancelled+by").text


def test_a_percent_sign_is_a_character_and_not_every_row(auth, fake_bot):
    """SQLite's LIKE would read it as "anything", and a search box takes anything."""
    seed_audit(fake_bot, 1, detail="ran at 100% capacity")
    seed_audit(fake_bot, 1, detail="ordinary")

    body = auth.get("/audit?q=100%25").text

    assert "100% capacity" in body
    assert "ordinary 000" not in body


def test_a_search_that_matches_nothing_says_so_rather_than_looking_empty(auth, fake_bot):
    """A page with no rows and a page whose search found none are different pages."""
    seed_audit(fake_bot, 2)

    body = auth.get("/audit?q=nothinglikethis").text

    assert "Nothing matches" in body
    assert "Nothing recorded yet" not in body


def test_the_chat_search_finds_a_member_by_the_name_the_page_shows(auth, seeded):
    """The log stores an id; the reader searches "kanon", which is a nickname."""
    body = auth.get("/chat?q=kanon").text
    assert "kanon" in body
    assert "Nothing matches" not in body
    # ...and somebody who asked nothing matches nothing.
    assert "Nothing matches" in auth.get("/chat?q=Priya").text


def test_the_chat_search_finds_a_channel_by_the_name_the_page_shows(auth, seeded):
    """A channel's name is not in the database at all -- it comes from the guild."""
    body = auth.get("/chat?q=hstar-party").text
    assert "#hstar-party" in body
    assert "Nothing matches" not in body


def test_the_chat_search_still_reads_the_question(auth, seeded):
    assert "Nothing matches" not in auth.get("/chat?q=star").text


def test_searching_members_reads_names_nicknames_and_aliases(auth, seeded, fake_bot):
    fake_bot.repo.add_alias(1003, "pri")

    assert "Priya" in auth.get("/members?q=priya").text
    assert "Priya" in auth.get("/members?q=pri").text
    assert "Priya" not in auth.get("/members?q=alvin").text


def test_searching_fixed_timings_reads_the_boss_the_day_and_the_party(auth, seeded):
    """Asserted on the rows, by id, rather than on the page.

    Every row carries an editor whose boss picker lists the whole game, so a
    boss's name is on the page whatever the search says -- see below.
    """
    star = f'id="fixed-{short_id(seeded["fixed_star"])}"'
    kalos = f'id="fixed-{short_id(seeded["fixed_kalos"])}"'

    by_boss = auth.get("/fixed?q=kalos", headers={"HX-Request": "true"}).text
    assert kalos in by_boss
    assert star not in by_boss

    by_day = auth.get("/fixed?q=tue", headers={"HX-Request": "true"}).text
    assert kalos in by_day
    assert star not in by_day

    by_party = auth.get("/fixed?q=alvin", headers={"HX-Request": "true"}).text
    assert star in by_party
    assert kalos not in by_party


def test_the_boss_picker_is_never_narrowed_by_the_search(auth, seeded):
    """It is a form control, not a result.

    You add a weekly timing for whichever boss you like, including one no
    current row mentions -- so the picker offers the whole game however the
    rows beside it have been filtered.
    """
    body = auth.get("/fixed?q=kalos").text

    assert 'name="boss_tokens" value="HStar"' in body
    assert "Radiant Malefic Star" in body


def test_searching_reminders_reads_the_bosses_and_the_party(auth, seeded):
    kalos = auth.get("/reminders?q=kalos").text
    assert "Gatekeeper Kalos" in kalos
    assert "Radiant Malefic Star" not in kalos
    # The party is on the row now, and searchable with it.
    assert "Priya" in kalos


@pytest.mark.parametrize("path,target", TABLES)
def test_every_table_page_offers_a_search_box(auth, seeded, path, target):
    body = auth.get(path).text
    assert f'id="{target}-q"' in body
    assert 'type="search"' in body
    assert f'action="{path}"' in body


# --- paging -----------------------------------------------------------------


def test_a_page_holds_twenty_rows_and_says_which_page_it_is(auth, fake_bot):
    seed_audit(fake_bot, 25)

    first = auth.get("/audit").text
    second = auth.get("/audit?page=2").text

    assert first.count("<tr>") == 1 + service.PAGE_SIZE  # the heading row, then twenty
    assert second.count("<tr>") == 1 + 5
    assert "Page 1 of 2" in first
    assert "Page 2 of 2" in second
    assert "25 rows" in first


def test_the_newest_rows_are_on_the_first_page(auth, fake_bot):
    seed_audit(fake_bot, 25)

    assert "change 024" in auth.get("/audit").text
    assert "change 000" in auth.get("/audit?page=2").text


def test_a_page_past_the_end_lands_on_the_last_one_that_exists(auth, fake_bot):
    """A stale bookmark, or a search that narrowed under somebody. Not a 404."""
    seed_audit(fake_bot, 25)

    response = auth.get("/audit?page=99")

    assert response.status_code == 200
    assert "Page 2 of 2" in response.text


@pytest.mark.parametrize("page", ["0", "-3"])
def test_a_page_before_the_first_one_is_the_first_one(auth, fake_bot, page):
    seed_audit(fake_bot, 25)
    response = auth.get(f"/audit?page={page}")
    assert response.status_code == 200
    assert "Page 1 of 2" in response.text


def test_the_pager_carries_the_search_with_it(auth, fake_bot):
    seed_audit(fake_bot, 25, detail="amended")

    body = auth.get("/audit?q=amended").text

    assert "q=amended&amp;page=2" in body or "q=amended&page=2" in body


def test_prev_and_next_are_real_links(auth, fake_bot):
    """With the CDN blocked they are how you turn the page; htmx only swaps."""
    seed_audit(fake_bot, 25)

    second = auth.get("/audit?page=2").text
    pager = second[second.index('class="pager"') :]

    assert 'href="/audit?' in pager
    assert "hx-get=" in pager
    assert "onclick" not in pager


def test_one_page_of_rows_shows_no_pager_at_all(auth, fake_bot):
    seed_audit(fake_bot, 3)
    assert 'class="pager"' not in auth.get("/audit").text


@pytest.mark.parametrize("path", ["/members", "/fixed"])
def test_the_search_only_pages_never_show_a_pager(auth, seeded, path):
    """Both are bounded by the roster; a pager there would never have a page two."""
    assert 'class="pager"' not in auth.get(path).text


def test_only_the_sent_half_of_reminders_is_paged(auth, fake_bot, seeded):
    """Queued is two boss weeks of rows; sent is however long the bot has run."""
    listing = service.reminders_listing(fake_bot)

    assert listing["upcoming"]  # everything the seed materialised, unpaged
    assert listing["rows"] == []  # nothing sent yet
    assert listing["total"] == 0


# --- what a swap replaces, and what it must not ----------------------------


@pytest.mark.parametrize("path,target", TABLES)
def test_an_htmx_request_gets_the_rows_and_not_the_page(auth, seeded, path, target):
    response = auth.get(path, headers={"HX-Request": "true"})

    assert response.status_code == 200
    assert "<html" not in response.text
    assert "<title>" not in response.text
    # The wrapper is the swap *target*, so it must not come back inside itself.
    assert f'id="{target}"' not in response.text


@pytest.mark.parametrize("path,target", TABLES)
def test_the_search_box_is_never_inside_what_it_replaces(auth, seeded, path, target):
    """The Limits page's rule, applied to six more: a swap must not land on the
    element being typed into, or it eats the rest of the word."""
    page = auth.get(path).text
    fragment = auth.get(path, headers={"HX-Request": "true"}).text

    assert 'name="q"' not in fragment
    assert page.index(f'id="{target}-q"') < page.index(f'id="{target}"')


def test_the_swapped_region_carries_its_own_pager(auth, fake_bot):
    """ "Page 2 of 7" has to change when the page does, so it lives in the swap."""
    seed_audit(fake_bot, 25)
    fragment = auth.get("/audit?page=2", headers={"HX-Request": "true"}).text
    assert "Page 2 of 2" in fragment


def test_the_add_dialog_is_not_swapped_away_by_a_search(auth, seeded):
    """It belongs to the page, not to a row -- a keystroke must not empty it."""
    page = auth.get("/fixed").text
    fragment = auth.get("/fixed", headers={"HX-Request": "true"}).text

    assert 'id="add-fixed"' in page
    assert 'id="add-fixed"' not in fragment
    # ...while a row's own editor travels with its row.
    assert 'class="modal" id="edit-' in fragment


# --- the frame --------------------------------------------------------------


@pytest.mark.parametrize("path", [*[p for p, _ in TABLES], "/bosses", "/config", "/limits"])
def test_a_list_page_asks_for_the_no_scroll_frame(auth, seeded, path):
    # /config changed sides when it became the one-window Settings layout: a
    # sidebar and a scrolling detail pane are exactly what the frame is for.
    # /limits followed, for the same reason one page down -- a roster that grows
    # under a row of figures that does not.
    assert '<body class="framed">' in auth.get(path).text


@pytest.mark.parametrize("page", ["chat", "extraction"])
def test_a_detail_page_asks_for_it_too(auth, seeded, page):
    """Not lists, but the same shape: one window whose panels can each run long,
    which is the thing a frame gives its leftover height to."""
    assert '<body class="framed">' in auth.get(tabbed_path(page, seeded)).text


@pytest.mark.parametrize("path", ["/inbox"])
def test_a_page_that_is_not_a_list_scrolls_as_it_always_did(auth, seeded, path):
    """The Inbox is a stack of proposals, each of them its own decision, and no
    one of them is the thing the page is for -- so there is no window to give
    the leftover height to, and it keeps the scrollbar."""
    assert '<body class="">' in auth.get(path).text


def test_the_frame_is_a_desktop_promise_only():
    """A scrolling box inside a scrolling page is the worst of both on a phone."""
    start = CSS_RULES.index("@media (min-width: 900px)")
    frame = CSS_RULES[start:]
    frame = frame[: frame.index("\n}\n")]

    assert "body.framed {" in frame
    assert "100dvh" in frame
    assert "position: sticky" in frame  # the column headings, while rows go past
    # ...and no rule anywhere else pins the page's height, at any width.
    assert "100dvh" not in CSS_RULES[:start]
    assert CSS_RULES.count("100dvh") == frame.count("100dvh")


def test_the_framed_shell_does_not_resize_with_its_content():
    """Measured on the live portal: /audit was 1180px wide and /audit?q=limits
    1023.27px, and the whole frame narrowed as the search was typed.

    `.shell` centres itself with `margin: 0 auto`. As a flex *item* -- which the
    frame makes it -- an auto margin on the cross axis suppresses the stretch
    that would give it the container's width, so it sized to its own content
    instead. A definite width gives those margins something to centre.
    """
    shell = PAGE_CSS[PAGE_CSS.index("body.framed .shell {") :]
    shell = shell[: shell.index("\n  }")]

    assert "width: 100%" in shell

    # ...and the centring still comes from `.shell` itself, unchanged.
    base = PAGE_CSS[PAGE_CSS.index("\n.shell {") :]
    assert "margin: 0 auto" in base[: base.index("}")]


def test_the_gap_under_the_menu_bar_lives_in_exactly_one_place():
    """Config's window sat flush against the masthead. The gap every other page
    had was `.page-head`'s own top margin -- and Config, being one window with a
    title bar of its own, has no page head to carry it.

    So the gap belongs to the shell, which every page has. Which means no page
    head may keep one as well, at any width: the two would add up on the pages
    that do have one, and those are most of them.
    """
    shell = CSS_RULES[CSS_RULES.index("\n.shell {") :]
    padding = re.search(r"padding: ([^;]+);", shell[: shell.index("}")])
    assert padding is not None
    top = padding.group(1).split()[0]
    assert float(top.removesuffix("rem")) > 0, top

    for rule in re.findall(r"\.page-head \{[^}]*\}", CSS_RULES):
        margin = re.search(r"margin: ([^;]+);", rule)
        assert margin is None or margin.group(1).split()[0] == "0", rule


@pytest.mark.parametrize("path,_target", TABLES)
def test_a_framed_page_head_is_one_compact_band(auth, seeded, path, _target):
    """The pane is the page; everything above it is a caption.

    One band between the masthead and the window, carrying at most one line of
    explanation -- the prose that used to sit there is in docs/portal.md, where
    it is read once rather than on every visit.
    """
    body = auth.get(path).text
    above = body[body.index('class="page-head"') : body.index('class="card pane"')]

    assert above.count('<p class="note">') <= 1
    assert '<section class="card">' not in above


def test_the_chat_page_has_one_head_band_and_not_two_cards(auth, seeded):
    """Measured at 1000x690: a hero card over a stat card took two thirds of
    the viewport and left the interactions two rows tall."""
    body = auth.get("/chat").text
    above = body[: body.index('class="card pane"')]

    assert 'class="statline"' in above
    assert '<section class="stat">' not in above
    # ...and every figure the tiles carried is still there, per model.
    for figure in ("qwen3:32b", "calls", "outcome", "latency", "tokens"):
        assert figure in above, figure


def test_a_framed_head_is_caption_weight_at_every_height():
    """Not gated on a height query: the proportion was wrong at all of them,
    and a height query is for trimming further rather than getting it right."""
    head = PAGE_CSS[PAGE_CSS.index("body.framed .page-head {") :]
    head = head[: head.index("\n  }")]
    assert "padding: 0.55rem" in head

    title = PAGE_CSS[PAGE_CSS.index("body.framed .page-head h1 {") :]
    assert "var(--fs-brand)" in title[: title.index("}")]


def test_a_short_screen_gives_the_content_its_height_back():
    """At 1280x720 the chrome above the frame was spending the pane's budget."""
    query = "@media (min-width: 900px) and (max-height: 850px)"
    assert query in PAGE_CSS

    short = PAGE_CSS[PAGE_CSS.index(query) :]
    short = short[: short.index("\n}\n")]

    # Only trimming: the head is already caption weight at every height, so
    # what goes here is the last of the prose and the status bar.
    assert "body.framed .page-head p" in short
    assert "body.framed .footnote" in short
    assert "display: none" in short


def test_a_targeted_dialog_gives_the_scrollbar_back():
    """With no JavaScript the Fixed editor unfolds in place, and it is taller
    than the screen -- inside a frame that cannot scroll it is unreachable."""
    # The bare `dialog:target` is deliberate: it must release the frame for the
    # runsheet dialogs too, not only the Fixed page's .modal editors.
    assert "body.framed:has(dialog:target)" in PAGE_CSS


# --- the two windows that grew tabs -----------------------------------------


def framed_rule(selector: str) -> str:
    """One rule out of the frame's media block, which is indented one level."""
    rule = PAGE_CSS[PAGE_CSS.index(selector) :]
    return rule[: rule.index("\n  }")]


def panel_of(body: str, key: str) -> str:
    """One panel's whole section, opening tag included -- the classes on it are
    half of what is worth asserting."""
    start = body.rindex("<section", 0, body.index(f'id="{key}"'))
    return body[start : body.index("</section>", start)]


def tabbed_path(page: str, seeded: dict) -> str:
    """The URL of each window built on the `.tabs` pattern."""
    return {
        "limits": "/limits",
        "chat": f"/chat/{short_id(seeded['interaction'])}",
        "extraction": f"/extractions/{short_id(seeded['extraction'])}",
    }[page]


@pytest.mark.parametrize(
    "page,tabs",
    [("limits", LIMITS_TABS), ("chat", CHAT_TABS), ("extraction", EXTRACTION_TABS)],
)
def test_every_tab_has_the_panel_it_opens(auth, seeded, page, tabs):
    """One list, three readers -- the same contract `CONFIG_SECTIONS` carries.
    The template builds the strip and the panels from it, the stylesheet
    enumerates the keys to raise the open tab, and this holds both to it."""
    body = auth.get(tabbed_path(page, seeded)).text

    for key, label in tabs:
        assert f'href="#{key}"' in body, key
        assert f'id="{key}"' in body, key
        assert label in body, label
        # ...and the stylesheet knows how to raise this one.
        assert f'.tabs:has(#{key}:target) [href="#{key}"]' in PAGE_CSS, key

    # Exactly one panel opens when the URL says nothing, and it is the first.
    assert body.count("tabs__panel--first") == 1
    assert f'id="{tabs[0][0]}"' in panel_of(body, tabs[0][0])
    assert "tabs__panel--first" in panel_of(body, tabs[0][0])


def test_the_tab_strip_is_the_windows_own_title_bar(auth, seeded):
    """Browser-style: the strip is painted as chrome and the tabs rise out of
    it, so it wears `.card__head` and gets the three dots from it rather than
    growing a second bar under one."""
    body = auth.get("/limits").text

    assert '<div class="card__head tabs__strip">' in body
    strip = PAGE_CSS[PAGE_CSS.index("\n.tabs__strip {") :]
    assert "padding-bottom: 0" in strip[: strip.index("}")]
    # The open tab is painted in the body's own surface, which is what fuses it
    # to the panel below rather than leaving it floating on the bar.
    raised = PAGE_CSS[PAGE_CSS.index(".tabs:has(#who-may-ask:target)") :]
    raised = raised[: raised.index("}")]
    assert "background: var(--surface)" in raised


def test_the_tabs_reach_across_the_swap_boundary(auth, seeded):
    """Three of the four panels are re-read on a timer and the fourth holds the
    form, which must never be inside what a refresh replaces. `display:
    contents` is what lets them be siblings anyway: the wrapper keeps its
    boundary and its polling attributes, and the panels line up in the window."""
    page = auth.get("/limits").text
    live = auth.get("/limits/live").text

    for key in ("who-may-ask", "in-flight", "windows"):
        assert f'id="{key}"' in live, key
    # The one that holds the form is written outside the region, not in it.
    assert 'id="set-allowance"' not in live
    assert 'id="set-allowance"' in page

    assert "display: contents" in framed_rule("body.framed .tabs #limits-live {")


def test_the_limits_tiles_are_never_behind_a_tab(auth, seeded):
    """Four figures read at a glance before anything else is chosen, so they sit
    between the strip and the panels rather than on one of them."""
    body = auth.get("/limits").text
    # From the strip's own nav, not the masthead's, which is further up.
    strip_end = body.index("</nav>", body.index('class="tabs__tabs"'))
    first_panel = body.index('class="tabs__panel')

    assert strip_end < body.index('class="stats"') < first_panel
    # ...and every figure they carried is still on them.
    for figure in ("The model", "Guild budget", "Per person", "Rescans", "Held for"):
        assert figure in body, figure


def test_the_limits_tabs_count_what_their_panels_hold(auth, fake_bot, seeded):
    """The counts are the page's glance, so they are rendered with the tables
    they count -- inside the live region, and re-read with them."""
    fake_bot.chat.limiter.allow(1002)
    add_pilot(fake_bot, 1002, "kanon")

    live = auth.get("/limits/live").text
    strip = live[: live.index("</nav>", live.index('class="tabs__tabs"'))]

    assert 'class="tabs__count">1<' in strip  # one holder of the role
    # The tab with nothing to count carries no figure at all.
    allowance = strip[strip.index('href="#set-allowance"') :]
    assert "tabs__count" not in allowance


def test_the_limits_member_table_scrolls_inside_its_panel(auth, fake_bot, seeded):
    """The five-column table must not widen a phone's page past its frame."""
    add_pilot(fake_bot, 1002, "kanon")

    panel = panel_of(auth.get("/limits/live").text, "who-may-ask")
    wrapper = panel.index('<div class="table-wrap">')
    assert wrapper < panel.index("<table>") < panel.index("</div>", wrapper)


def test_a_failed_generation_is_never_behind_a_tab(auth, fake_bot):
    """The first fact about the interaction, and a tab is somewhere a fact can
    be missed -- so it sits above the window rather than inside it."""
    interaction = fake_bot.repo.log_chat_interaction(
        model="qwen3:32b",
        question="what's on?",
        reply="Sorry — I couldn't get to the schedule just now.",
        outcome="failed",
        error="the model kept calling tools",
    )
    body = auth.get(f"/chat/{short_id(interaction)}").text

    assert body.index("The generation failed.") < body.index('class="card tabs"')


def test_a_panel_scrolls_rather_than_the_window(auth, seeded):
    """The window takes the frame's leftover height; the open panel takes what
    the strip and the tiles leave, and its headings stay while its rows go."""
    window = framed_rule("body.framed .tabs {")
    assert "overflow: hidden" in window
    assert "flex-direction: column" in window

    panel = framed_rule("body.framed .tabs__panel {")
    assert "overflow-y: auto" in panel
    assert "min-height: 0" in panel

    assert "position: sticky" in framed_rule("body.framed .tabs__panel thead th {")


def test_a_phone_gets_every_panel_and_pills_to_jump_by():
    """A window that answers with a blank panel because a fragment went missing
    is worse than one you scroll -- the settings window's rule, for its reason."""
    blocks, pos = [], 0
    while (start := PAGE_CSS.find("@media (max-width: 899px)", pos)) != -1:
        blocks.append(PAGE_CSS[start : PAGE_CSS.index("\n}\n", start)])
        pos = start + 1
    narrow = [b for b in blocks if ".tabs__panel {" in b]

    assert len(narrow) == 1
    assert "display: block" in narrow[0]
    assert "border-radius: 999px" in narrow[0]  # the tabs become jump pills


# --- the arithmetic ---------------------------------------------------------


def test_a_page_of_nothing_is_still_page_one_of_one():
    meta = service._page_meta(0, 1)
    assert (meta["page"], meta["pages"], meta["offset"]) == (1, 1, 0)
    assert meta["prev"] is None and meta["next"] is None


@pytest.mark.parametrize(
    "total,page,expected_offset,expected_pages",
    [(25, 1, 0, 2), (25, 2, 20, 2), (40, 2, 20, 2), (41, 3, 40, 3), (25, 99, 20, 2)],
)
def test_where_a_page_starts(total, page, expected_offset, expected_pages):
    meta = service._page_meta(total, page)
    assert meta["offset"] == expected_offset
    assert meta["pages"] == expected_pages


def test_the_count_in_the_heading_is_of_the_search_not_of_the_table(auth, fake_bot):
    seed_audit(fake_bot, 25, detail="ordinary")
    seed_audit(fake_bot, 3, detail="special")

    assert "28 changes" in auth.get("/audit").text
    assert "3 changes" in auth.get("/audit?q=special").text


def test_the_search_is_answered_in_sql_for_the_logs(fake_bot):
    """The three capped logs page in the database rather than in Python: the
    alternative is building two thousand views to show twenty of them."""
    seed_audit(fake_bot, 25)

    rows = fake_bot.repo.list_audit(limit=5, offset=10, q="change")

    assert len(rows) == 5
    assert fake_bot.repo.count_audit(q="change") == 25
    assert fake_bot.repo.count_audit(q="nothinglikethis") == 0


def test_the_json_api_is_untouched_by_any_of_this(auth, fake_bot):
    """It asked for `?limit=` before and it asks for `?limit=` now."""
    seed_audit(fake_bot, 25)

    payload = auth.get("/api/audit?limit=5").json()

    assert len(payload) == 5
    assert re.match(r"change \d{3}", payload[0]["detail"])
