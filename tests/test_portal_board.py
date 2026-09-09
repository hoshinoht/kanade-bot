"""The Week page as a day board, and the strip of numbers above it.

The board is the rail grown up: seven columns, one per day of the boss week,
starting at the reset day. What is worth testing is that it is genuinely the
same runs as the list under it -- one rendering, one set of ids, one htmx target
per run -- because the alternative (a compact copy beside a full one) is two
things that drift apart the first time somebody presses a button.
"""

from __future__ import annotations

import re

from bot.api import service
from bot.portal_styles import build_stylesheet

from .conftest import TZ, kl
from .fake_bot import OTHER_CHANNEL

PAGE_CSS = build_stylesheet()


def board_of(body: str) -> str:
    return body[body.index('class="board"') : body.index('id="days"')]


def card_for(body: str, short_id: str) -> str:
    """One run's article, out of whichever sheet holds it."""
    start = body.rindex("<article", 0, body.index(f'id="run-{short_id}"'))
    return body[start : body.index("</article>", start)]


def board_card_of(body: str, short_id: str) -> str:
    """The same run's compact card off the board.

    Cut to the board first: the now strip's "what's next" tile opens the same
    sheet with the same `data-dialog`, further up the page.
    """
    board = board_of(body)
    start = board.rindex('<a class="runcard', 0, board.index(f'data-dialog="sheet-{short_id}"'))
    return board[start : board.index("</a>", start)]


# --- the shape of the week --------------------------------------------------


def test_the_board_has_a_column_for_every_day_starting_at_the_reset(auth, seeded):
    board = board_of(auth.get("/").text)
    days = re.findall(r'class="board__dow">([A-Za-z]+)<', board)

    assert len(days) == 7
    assert days[0] == "Thu"  # BOSS_WEEK_RESET_WEEKDAY, not Monday
    assert "The boss week starts here" in board


def test_a_day_with_nothing_on_it_is_still_a_column(auth, seeded):
    """An empty Tuesday is a fact about the week, not a row to leave out."""
    board = board_of(auth.get("/").text)
    assert board.count("board__col") >= 7
    assert "board__none" in board


# --- the area principle: width follows content ------------------------------


def tracks_of(body: str) -> list[str]:
    """The board's column list, as the server wrote it onto the element."""
    style = re.search(r'class="board"[^>]*style="grid-template-columns:\s*([^"]+)"', body)
    assert style is not None, "the board carries no track list"
    return style.group(1).split()


def test_a_day_with_nothing_on_it_collapses_to_a_spine(auth, seeded):
    """Six empty days at an equal seventh each spent the page on nothing.

    The seed runs on two nights, so five of the seven days are spines and two
    share everything the spines gave back.
    """
    tracks = tracks_of(auth.get("/").text)

    assert len(tracks) == 7
    assert tracks.count(service.BOARD_SPINE) == 5
    assert tracks.count(service.BOARD_RUN_TRACK) == 2


def test_a_spine_is_a_name_wide_and_a_run_day_is_a_card_wide():
    assert service.BOARD_SPINE == "3.5rem"
    assert service.BOARD_RUN_TRACK.startswith("minmax(230px")


def test_the_track_list_is_counted_from_the_runs(fake_bot, seeded):
    """A stylesheet cannot count runs, so the server does it and says so inline."""
    columns = service.board_columns(fake_bot, "this", service.schedule(fake_bot)["runs"])
    tracks = service.board_tracks(columns).split()

    for column, track in zip(columns, tracks, strict=True):
        wanted = service.BOARD_SPINE if column["empty"] else service.BOARD_RUN_TRACK
        assert track == wanted, column["weekday"]


def test_an_empty_day_says_so_in_its_class(auth, seeded):
    assert "board__col--empty" in board_of(auth.get("/").text)


def test_a_compact_card_names_a_boss_by_its_token(auth, seeded):
    """The vocabulary the rail pips and the extractor already speak, on the one
    surface too narrow for "Radiant Malefic Star" and a difficulty pill.

    The seed's longest name is the one that forced this, so it is the one to
    check: the card says `HMaleficStar`, and the pill beside it still says HARD --
    redundant with the prefix letter and kept anyway, because the pill is the
    colour a reader learns the tier by.
    """
    card = board_card_of(auth.get("/").text, service.short_id(seeded["run_star"]))

    assert ">HMaleficStar<" in card
    assert ">HFA<" in card
    assert '<span class="pill pill--h">HARD</span>' in card


def test_the_full_name_is_still_there_for_anyone_who_needs_it(auth, seeded):
    """A token is an abbreviation, so the thing it abbreviates has to be within
    reach: on the card's own tooltip, on each boss's, and read out in place of
    the token rather than beside it."""
    body = auth.get("/").text
    card = board_card_of(body, service.short_id(seeded["run_star"]))

    # The card's tooltip names the bosses in full, where it used to list tokens.
    assert "Radiant Malefic Star + The First Adversary" in card
    assert 'title="Radiant Malefic Star (Hard, Lv280)"' in card
    # The token is hidden from the reader who is being read to, and the name is
    # given instead -- not both, which would be "H-Star Radiant Malefic Star".
    assert '<span class="boss__name" aria-hidden="true">HMaleficStar</span>' in card
    assert '<span class="vh">Radiant Malefic Star</span>' in card


def test_only_the_compact_cards_are_abbreviated(auth, seeded):
    """The sheet, which is also the phone list, has room for the real name --
    written plainly, not hidden behind an abbreviation of itself."""
    card = card_for(auth.get("/").text, service.short_id(seeded["run_star"]))

    assert '<span class="boss__name">Radiant Malefic Star</span>' in card
    assert '<span class="vh">Radiant Malefic Star</span>' not in card


def test_today_is_marked(fake_bot, seeded):
    from bot.domain.timeutil import utcnow

    today = utcnow().astimezone(TZ).date().strftime("%Y-%m-%d")
    columns = service.board_columns(fake_bot, "this", [])

    marked = [c for c in columns if c["is_today"]]
    assert marked == [] or (len(marked) == 1 and marked[0]["date"] == today)


def test_the_board_carries_the_runs_the_page_is_showing(auth, seeded):
    """By token, which is what a compact card writes; the full names ride along
    in the tooltips, so it is the tokens that say what is actually on show."""
    board = board_of(auth.get("/").text)
    assert ">HMaleficStar<" in board
    assert ">XKalos<" in board


def test_the_board_narrows_with_the_filter_bar(auth, seeded):
    """It is built from the same filtered runs as the list, not queried again."""
    board = board_of(auth.get(f"/?channel={OTHER_CHANNEL}").text)

    assert "XKalos" in board
    assert "Radiant Malefic Star" not in board  # neither its token nor its name


def test_the_rail_still_shows_the_whole_week_when_the_board_is_filtered(auth, seeded):
    """The rail is the shape of the week; a filter narrows what is on it, not it."""
    body = auth.get(f"/?channel={OTHER_CHANNEL}").text
    rail = body[body.index('class="rail"') : body.index("</nav>", body.index('class="rail"'))]
    assert "HMaleficStar" in rail  # a pip is still there for Monday


def test_a_compact_card_says_the_time_the_bosses_and_the_tally(auth, seeded):
    board = board_of(auth.get("/").text)
    assert "runcard__time" in board
    assert "runcard__tally" in board
    assert "21:30" in board
    # One figure, not a list of names: who the missing person is lives on the sheet.
    assert "1/2" in board


def test_an_own_time_run_says_so_rather_than_showing_a_clock(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "otot")
    board = board_of(auth.get("/").text)
    assert "own time" in board


# --- one rendering, two containers ------------------------------------------


def test_a_swap_still_returns_the_row_and_not_the_sheet(auth, fake_bot, seeded):
    """The htmx contract is unchanged: one article in, one article out."""
    response = auth.post(
        f"/runs/{seeded['run_star']}/cancel", data={"next": "/"}, headers={"HX-Request": "true"}
    )

    assert response.text.strip().startswith('<article class="run run--cancelled"')
    assert "<dialog" not in response.text


# --- more than one boss on a run --------------------------------------------


# --- the phone keeps what it had --------------------------------------------


def test_the_past_toggle_still_works_on_both(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")

    hidden = auth.get("/").text
    shown = auth.get("/?show_past=1").text

    assert "1 past or cancelled run hidden" in hidden
    assert "HMaleficStar" not in board_of(hidden)
    # Shown, it is greyed in its column rather than moved somewhere else.
    assert "HMaleficStar" in board_of(shown)
    assert "runcard--done" in board_of(shown)


# --- the now strip ----------------------------------------------------------


def test_the_strip_says_what_is_next_and_how_long(auth, seeded):
    body = auth.get("/").text
    strip = body[body.index('class="now"') : body.index('class="rail"')]

    assert "Next" in strip
    assert "Unanswered" in strip
    assert "Inbox" in strip
    assert "Model" in strip
    assert 'href="/inbox"' in strip
    assert 'href="/limits"' in strip


def test_the_next_tile_opens_the_run_it_names(auth, seeded, fake_bot):
    strip = auth.get("/").text
    now = service.week_now(fake_bot, service.schedule(fake_bot)["runs"])
    if now["next"]:
        assert f'href="#sheet-{now["next"]["short_id"]}"' in strip


def test_a_week_with_nothing_ahead_says_so(auth, fake_bot):
    now = service.week_now(fake_bot, [])
    assert now["next"] is None
    assert "nothing ahead" in auth.get("/").text


def synthetic_run(short: str, hours: float, status: str = "planned", unanswered: int = 1) -> dict:
    """A run view with only the keys the strip reads, at a known distance from now."""
    from datetime import timedelta

    from bot.domain.timeutil import to_iso, utcnow

    return {
        "id": f"id-{short}",
        "short_id": short,
        "status": status,
        "datetime": to_iso(utcnow() + timedelta(hours=hours)),
        "local_day": "Mon",
        "local_time": "21:30",
        "bosses": ["HMaleficStar"],
        "yes": 1,
        "participants": [{"id": "1"}, {"id": "2"}],
        "unanswered": unanswered,
    }


def test_only_the_runs_still_ahead_owe_an_answer(fake_bot):
    """Nobody can answer for a night that has already been."""
    strip = service.week_now(
        fake_bot,
        [synthetic_run("gone", -3, unanswered=5), synthetic_run("soon", +3, unanswered=2)],
    )

    assert strip["unanswered"] == 2
    assert strip["next"]["short_id"] == "soon"


def test_a_cancelled_run_is_not_what_is_next(fake_bot):
    strip = service.week_now(
        fake_bot,
        [synthetic_run("off", +1, status="cancelled"), synthetic_run("on", +5)],
    )

    assert strip["next"]["short_id"] == "on"
    assert strip["unanswered"] == 1


def test_the_inbox_count_is_the_one_the_nav_shows(auth, fake_bot, seeded):
    now = service.week_now(fake_bot, [])
    assert now["pending"] == len(fake_bot.repo.list_amendments(status="proposed"))
    assert now["pending"] == 1


def test_the_countdown_is_coarse_on_purpose():
    """Read once, at a glance, by somebody deciding whether to eat first."""
    now = kl(2026, 9, 1, 12, 0)

    assert service.countdown(kl(2026, 9, 1, 12, 0), now) == "now"
    assert service.countdown(kl(2026, 9, 1, 11, 0), now) == "now"
    assert service.countdown(kl(2026, 9, 1, 12, 45), now) == "in 45 min"
    assert service.countdown(kl(2026, 9, 1, 15, 5), now) == "in 3h 05m"
    assert service.countdown(kl(2026, 9, 3, 18, 0), now) == "in 2d 6h"


def test_the_week_page_is_framed(auth, seeded):
    assert '<body class="framed">' in auth.get("/").text
