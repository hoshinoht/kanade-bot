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

from bot import behaviour_plugins
from bot.api import service
from bot.domain.ids import short_id
from bot.portal_styles import build_stylesheet

from .conftest import kl
from .fake_bot import FakeGuildMember, FakeGuildRole, add_pilot

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


def test_members_include_chat_staff_saved_and_override_access(fake_bot, seeded):
    chat_member = add_pilot(fake_bot, 2001, "Chat Member")
    saved = FakeGuildMember(2002, "Saved Member")
    saved.guild = fake_bot.guild
    fake_bot.guild.members.append(saved)
    fake_bot.repo.upsert_member(saved.id, saved.display_name, None, False)
    fake_bot.repo.set_reply_style(saved.id, "missing")

    staff = FakeGuildMember(2003, "Staff Member", administrator=True)
    staff.guild = fake_bot.guild
    fake_bot.guild.members.append(staff)

    override_role = FakeGuildRole(9001)
    override = FakeGuildMember(2004, "Override Member")
    override.guild = fake_bot.guild
    override.roles = [override_role]
    fake_bot.guild.members.append(override)
    fake_bot.repo.set_config(
        behaviour_plugins.CONFIG_KEY,
        '[{"role_id":"9001","plugin":"missing"}]',
    )

    names = {row["display_name"] for row in service.relevant_style_members(fake_bot)}
    assert chat_member.display_name in names
    assert {"Saved Member", "Staff Member", "Override Member"} <= names
    assert {"Alvin tan", "Priya"} <= names


def test_members_show_saved_and_next_reply_styles(auth, fake_bot, seeded, tmp_path, monkeypatch):
    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    behaviour_plugins.write("public", "PUBLIC")
    behaviour_plugins.write("override", "OVERRIDE")
    fake_bot.repo.set_config(behaviour_plugins.SELECTABLE_CONFIG_KEY, '["public"]')
    fake_bot.repo.set_config(
        behaviour_plugins.CONFIG_KEY,
        '[{"role_id":"9001","plugin":"override"}]',
    )
    fake_bot.repo.set_reply_style(1001, "public")
    member = FakeGuildMember(1001, "Alvin tan")
    member.guild = fake_bot.guild
    member.roles = [FakeGuildRole(9001)]
    fake_bot.guild.members.append(member)
    fake_bot.guild.roles[9001] = member.roles[0]

    body = auth.get("/members").text
    start = body.index('<dialog class="modal membersheet" id="member-1001"')
    sheet = body[start : body.index("</dialog>", start)]
    assert "public" in sheet
    assert "override" in sheet
    assert "role: 9001" in sheet


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

    assert 'name="boss_tokens" value="HMaleficStar"' in body
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


# --- the frame --------------------------------------------------------------


# --- the two windows that grew tabs -----------------------------------------


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
