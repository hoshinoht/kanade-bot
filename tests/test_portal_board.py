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
from bot.api.app import STATIC_DIR

from .conftest import TZ, kl
from .fake_bot import OTHER_CHANNEL

PAGE_CSS = (STATIC_DIR / "portal.css").read_text(encoding="utf-8")


def board_of(body: str) -> str:
    return body[body.index('class="board"') : body.index('id="days"')]


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


def test_an_empty_today_is_slim_but_still_today(auth, fake_bot):
    """"Today, nothing on" is worth the same glance as a night with four runs."""
    board = board_of(auth.get("/").text)  # an empty database: every day a spine

    assert tracks_of(auth.get("/").text) == [service.BOARD_SPINE] * 7
    assert "board__col--empty board__col--today" in board
    # Today's head wash is written after the empty rules, so it survives them.
    assert PAGE_CSS.index(".board__col--empty .board__head") < PAGE_CSS.index(
        ".board__col--today .board__head"
    )


def test_a_busy_day_packs_its_runs_across_as_well_as_down():
    """A column that took most of the board should not stretch four cards to it.

    Two spaces of indent, so this finds the rule itself rather than the
    `.board__col--empty .board__runs` override that precedes it.
    """
    runs = PAGE_CSS[PAGE_CSS.index("  .board__runs {") :]
    runs = runs[: runs.index("}")]

    assert "grid-template-columns: repeat(auto-fill" in runs
    assert "align-content: start" in runs


def test_a_difficulty_pill_never_falls_off_a_card():
    """Measured at 153px columns: EXTREME's right edge overflowed the card by
    21px and was clipped, because a boss is a name and a pill in one
    inline-flex box and that box does not wrap. Now the pill drops under the
    name, and a name too long for the column breaks instead of pushing out."""
    rule = PAGE_CSS[PAGE_CSS.index(".runcard__bosses .boss {") :]
    rule = rule[: rule.index("}")]

    assert "flex-wrap: wrap" in rule
    assert "min-width: 0" in rule


def test_a_column_with_more_below_the_fold_says_so():
    """Measured at 1280x720: the busiest column showed 113px of 631px of runs,
    with the bottom card sliced and nothing to say the list went on."""
    rule = PAGE_CSS[PAGE_CSS.index(".board__col:not(.board__col--empty) .board__runs {") :]
    rule = rule[: rule.index("}")]

    # Covers scroll with the content and hide the shadows at each end; the
    # shadows are fixed to the box. So the hint is there only when it is true.
    assert "local no-repeat" in rule
    assert "scroll no-repeat" in rule
    assert "scrollbar-gutter: stable" in rule


def test_seven_busy_days_scroll_rather_than_shrink_below_legible():
    board = PAGE_CSS[PAGE_CSS.index("  .board {") :]
    assert "overflow-x: auto" in board[: board.index("}")]


def test_today_is_marked(fake_bot, seeded):
    from bot.timeutil import utcnow

    today = utcnow().astimezone(TZ).date().strftime("%Y-%m-%d")
    columns = service.board_columns(fake_bot, "this", [])

    marked = [c for c in columns if c["is_today"]]
    assert marked == [] or (len(marked) == 1 and marked[0]["date"] == today)


def test_the_board_carries_the_runs_the_page_is_showing(auth, seeded):
    board = board_of(auth.get("/").text)
    assert "Radiant Malefic Star" in board
    assert "Gatekeeper Kalos" in board


def test_the_board_narrows_with_the_filter_bar(auth, seeded):
    """It is built from the same filtered runs as the list, not queried again."""
    board = board_of(auth.get(f"/?channel={OTHER_CHANNEL}").text)

    assert "Gatekeeper Kalos" in board
    assert "Radiant Malefic Star" not in board


def test_the_rail_still_shows_the_whole_week_when_the_board_is_filtered(auth, seeded):
    """The rail is the shape of the week; a filter narrows what is on it, not it."""
    body = auth.get(f"/?channel={OTHER_CHANNEL}").text
    rail = body[body.index('class="rail"') : body.index("</nav>", body.index('class="rail"'))]
    assert "HStar" in rail  # a pip is still there for Monday


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


def test_the_state_and_the_tally_share_one_line(auth, seeded):
    """"⚠️ unconfirmed" is an emoji and a word; at a card's width the word
    wrapped and pushed the tally onto a third line -- three lines, two facts."""
    board = board_of(auth.get("/").text)
    card = board[board.index('class="runcard') :]
    card = card[: card.index("</a>")]

    assert "runcard__top" in card
    assert "⚠️" in card
    assert "runcard__tally" in card
    # The word is not laid out, but it is still read out and still in the tooltip.
    assert "unconfirmed" in card
    assert 'class="vh"' in card
    assert 'aria-hidden="true"' in card


def test_the_mark_on_a_card_is_the_labels_own_emoji():
    """One vocabulary. Somebody who has read "⚠️ unconfirmed" on a Discord card
    should not have to learn a second glyph for it on the board."""
    from bot import formatting

    assert set(formatting.STATUS_MARK) == set(formatting.STATUS_LABEL)
    for status, label in formatting.STATUS_LABEL.items():
        assert label.startswith(formatting.STATUS_MARK[status]), status
        assert " " not in formatting.STATUS_MARK[status]


# --- one rendering, two containers ------------------------------------------


def test_every_run_is_rendered_exactly_once(auth, seeded):
    """A compact copy beside a full one is two things that drift apart."""
    body = auth.get("/").text
    assert body.count('<article class="run ') == 2  # the two runs the seed makes
    for short in re.findall(r'id="run-([0-9a-f]+)"', body):
        assert body.count(f'id="run-{short}"') == 1


def test_a_card_opens_the_sheet_that_holds_the_real_run_card(auth, seeded):
    body = auth.get("/").text
    (short,) = re.findall(r'id="run-([0-9a-f]+)"', body)[:1]

    assert f'href="#sheet-{short}"' in body
    assert f'data-dialog="sheet-{short}"' in body
    assert f'<dialog class="runsheet" id="sheet-{short}"' in body


def test_the_sheet_is_a_real_link_and_a_real_dialog(auth, seeded):
    """With JavaScript it opens modal; without, `dialog:target` unfolds it."""
    board = board_of(auth.get("/").text)

    assert 'href="#sheet-' in board
    assert "onclick" not in board
    assert "dialog.runsheet:target" in PAGE_CSS


def test_every_action_still_works_inside_the_sheet(auth, seeded):
    """run.html goes in unchanged, so its forms and swap targets are untouched."""
    body = auth.get("/").text
    sheet = body[body.index('<dialog class="runsheet"') :]
    row = sheet[: sheet.index("</article>")]
    (run_id,) = re.findall(r'action="/runs/([0-9a-f-]+)/amend"', row)

    for action in ("amend", "ping", "participants", "status", "rsvp"):
        assert f'action="/runs/{run_id}/{action}"' in row, action
    assert 'hx-target="closest .run"' in row
    assert row.count('method="post"') == row.count("hx-post=") > 0


def test_a_swap_still_returns_the_row_and_not_the_sheet(auth, fake_bot, seeded):
    """The htmx contract is unchanged: one article in, one article out."""
    response = auth.post(
        f"/runs/{seeded['run_star']}/cancel", data={"next": "/"}, headers={"HX-Request": "true"}
    )

    assert response.text.strip().startswith('<article class="run run--cancelled"')
    assert "<dialog" not in response.text


# --- the phone keeps what it had --------------------------------------------


def test_the_phone_still_gets_the_rail_and_the_list(auth, seeded):
    body = auth.get("/").text
    assert 'class="rail"' in body
    assert 'class="day__head"' in body
    assert 'id="days"' in body


def test_the_board_and_the_rail_never_show_at_once():
    """One says the shape of the week; two would say it twice."""
    wide = PAGE_CSS[PAGE_CSS.index("@media (min-width: 900px)") :]
    assert ".board {\n  display: none;\n}" in PAGE_CSS  # the phone default
    assert ".rail {\n    display: none;\n  }" in wide


def test_the_sheets_are_the_list_on_a_phone():
    """No board to open them, so each one simply is the row it holds."""
    narrow = PAGE_CSS[PAGE_CSS.index("@media (max-width: 899px)") :]
    narrow = narrow[: narrow.index("\n}\n")]
    assert "dialog.runsheet" in narrow
    assert "display: block" in narrow
    assert "position: static" in narrow


def test_the_past_toggle_still_works_on_both(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")

    hidden = auth.get("/").text
    shown = auth.get("/?show_past=1").text

    assert "1 past or cancelled run hidden" in hidden
    assert "Radiant Malefic Star" not in board_of(hidden)
    # Shown, it is greyed in its column rather than moved somewhere else.
    assert "Radiant Malefic Star" in board_of(shown)
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

    from bot.timeutil import to_iso, utcnow

    return {
        "id": f"id-{short}",
        "short_id": short,
        "status": status,
        "datetime": to_iso(utcnow() + timedelta(hours=hours)),
        "local_day": "Mon",
        "local_time": "21:30",
        "bosses": ["HStar"],
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
