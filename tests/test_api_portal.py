"""The portal: every page renders empty and populated, and the forms work.

The point of these is that a template typo is a 500 on a phone at 9pm, so each
page is rendered against both an empty database and a seeded one, and each form
is posted the way a browser would post it -- with and without htmx.
"""

from __future__ import annotations

import inspect

import pytest

from bot.api import routes_api, routes_web
from bot.api.templating import HTMX_SRC, confidence_band
from bot.ids import short_id

from .fake_bot import ADMIN_TOKEN, OTHER_CHANNEL, UNWATCHED_CHANNEL, WATCHED_CHANNEL

PAGES = ["/", "/fixed", "/inbox", "/extractions", "/members", "/reminders", "/config"]


# --- the invariant that lets the bot share one SQLite connection ------------


def test_every_route_handler_is_async():
    """A sync handler would run in FastAPI's threadpool.

    The repository is a single ``sqlite3`` connection shared with the bot's own
    loop, and this build of ``sqlite3`` reports ``threadsafety == 1`` (connections
    must not be shared between threads).  Keeping every handler ``async`` keeps
    all database work on the one loop; see :meth:`bot.db.Repo.__init__`.
    """
    offenders = []
    for module in (routes_api, routes_web):
        for route in module.router.routes:
            if not inspect.iscoroutinefunction(route.endpoint):
                offenders.append(f"{module.__name__}.{route.endpoint.__name__}")
    assert offenders == []


# --- pages render -----------------------------------------------------------


@pytest.mark.parametrize("path", PAGES)
def test_every_page_renders_with_an_empty_database(auth, path):
    response = auth.get(path)
    assert response.status_code == 200
    assert "<title>" in response.text


@pytest.mark.parametrize("path", PAGES)
def test_every_page_renders_with_data(auth, seeded, path):
    response = auth.get(path)
    assert response.status_code == 200
    assert HTMX_SRC in response.text
    # The guild timezone appears on every screen (DESIGN.md §5).
    assert "Asia/Kuala_Lumpur" in response.text


def test_the_week_page_shows_the_rail_the_runs_and_the_tally(auth, seeded):
    body = auth.get("/").text
    assert 'class="rail"' in body
    assert "HStar" in body and "XKalos" in body
    assert "1/2 on" in body
    assert "⚠️ unconfirmed" in body


def test_the_rail_starts_on_the_reset_day(auth, seeded):
    body = auth.get("/").text
    start = body.index('class="rail"')
    rail = body[start : body.index("</nav>", start)]
    days = [line for line in rail.splitlines() if 'class="rail__dow"' in line]
    assert ">Thu<" in days[0]  # BOSS_WEEK_RESET_WEEKDAY, not Monday
    assert len(days) == 7
    assert "The boss week starts here" in rail


def test_the_week_page_filters(auth, seeded):
    """The listing narrows; the rail still shows the whole week's shape."""
    body = auth.get(f"/?channel={OTHER_CHANNEL}").text
    listing = body[body.index('<div id="days">') :]
    assert "XKalos" in listing
    assert "HStar" not in listing
    assert "HStar" in body  # still a pip on Monday


def test_next_week_is_reachable_from_the_week_page(auth, seeded):
    assert auth.get("/?week=next").status_code == 200


def test_an_empty_week_explains_what_to_do(auth):
    assert "Set a baseline timing" in auth.get("/").text


def test_the_inbox_shows_the_evidence_and_the_confidence(auth, seeded):
    body = auth.get("/inbox").text
    assert "can change to wed?" in body
    assert "0.82" in body
    assert "kanon" in body


def test_the_nav_counts_what_is_waiting(auth, seeded):
    assert '<span class="pip">1</span>' in auth.get("/").text


def test_the_extraction_page_shows_the_prompt_and_the_raw_response(auth, seeded):
    body = auth.get(f"/extractions/{short_id(seeded['extraction'])}").text
    assert "you are an extractor" in body
    assert "1234 ms" in body


def test_an_unknown_extraction_renders_the_error_page_not_a_traceback(auth, seeded):
    response = auth.get("/extractions/deadbeef")
    assert response.status_code == 404
    assert "That doesn&#39;t exist" in response.text


def test_the_reminders_page_separates_queued_from_sent(auth, fake_bot, seeded):
    reminder = fake_bot.repo.list_reminders(seeded["run_star"])[0]
    fake_bot.repo.mark_reminder_sent(reminder["id"], 900000000000000009)
    body = auth.get("/reminders").text
    assert "open in Discord" in body
    assert "/900000000000000009" in body


def test_the_config_page_shows_the_read_only_deployment_values(auth, seeded):
    body = auth.get("/config").text
    assert "gpt-oss:20b" in body
    assert "Thu 00:00" in body
    assert "Pause watching" in body


@pytest.mark.parametrize(
    "value,expected",
    [(0.95, "high"), (0.7, "mid"), (0.3, "low"), (None, "unknown")],
)
def test_confidence_bands(value, expected):
    assert confidence_band(value) == expected


# --- forms: with htmx, and without ------------------------------------------


def test_cancelling_from_a_form_post_redirects_back_with_a_message(auth, fake_bot, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/cancel", data={"next": "/"}, follow_redirects=False
    )
    assert response.status_code == 303
    assert response.headers["location"].startswith("/?msg=")
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "cancelled"


def test_cancelling_over_htmx_returns_just_the_updated_row(auth, fake_bot, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/cancel",
        data={"next": "/"},
        headers={"HX-Request": "true"},
    )
    assert response.status_code == 200
    assert response.text.strip().startswith('<article class="run run--cancelled"')
    assert "<html" not in response.text


def test_moving_a_run_from_the_row_form(auth, fake_bot, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/amend",
        data={"to": "2026-09-02 21:45", "next": "/"},
        headers={"HX-Request": "true"},
    )
    assert response.status_code == 200
    assert "21:45" in response.text
    assert "Wed 02 Sep" in response.text


def test_a_bad_date_over_htmx_comes_back_as_a_flash_not_a_page(auth, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/amend",
        data={"to": "whenever", "next": "/"},
        headers={"HX-Request": "true"},
    )
    assert response.status_code == 400
    assert 'class="flash flash--error"' in response.text
    assert "<html" not in response.text


def test_own_time_and_preview_ping_from_the_row(auth, fake_bot, seeded):
    auth.post(f"/runs/{seeded['run_star']}/otot", data={"next": "/"}, follow_redirects=False)
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "otot"
    auth.post(
        f"/runs/{seeded['run_star']}/ping",
        data={"kind": "day_of", "next": "/"},
        follow_redirects=False,
    )
    assert fake_bot.posts[-1].content.startswith("🧪 TEST — ")


def test_adding_a_fixed_timing_from_the_page(auth, fake_bot, seeded):
    response = auth.post(
        "/fixed/new",
        data={
            "bosses": "hcarling",
            "day": "2",
            "time": "23:00",
            "channel_id": str(WATCHED_CHANNEL),
            "participants": ["1001", "1003"],
            "note": "",
        },
        follow_redirects=False,
    )
    assert response.status_code == 303
    assert "Added" in response.headers["location"]
    assert any(f["bosses"] == ["HCarling"] for f in fake_bot.repo.list_fixed_runs())


def test_a_bad_boss_token_comes_back_as_a_flash_on_the_fixed_page(auth, seeded):
    response = auth.post(
        "/fixed/new",
        data={
            "bosses": "kalos",
            "day": "2",
            "time": "23:00",
            "channel_id": str(WATCHED_CHANNEL),
            "participants": ["1001"],
        },
        follow_redirects=True,
    )
    assert response.status_code == 200
    assert "missing a difficulty prefix" in response.text
    assert "flash--error" in response.text


def test_editing_and_removing_a_fixed_timing_from_the_page(auth, fake_bot, seeded):
    auth.post(
        f"/fixed/{seeded['fixed_star']}/edit",
        data={"bosses": "hstar, hfa", "day": "0", "time": "22:00", "participants": ["1001"]},
        follow_redirects=False,
    )
    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"])["time"] == "22:00"
    auth.post(f"/fixed/{seeded['fixed_star']}/delete", follow_redirects=False)
    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"]) is None


def test_live_boss_validation_returns_chips_or_the_reason(auth):
    good = auth.post("/validate/bosses", data={"bosses": "hstar hfa"})
    assert 'class="boss boss--h"' in good.text
    bad = auth.post("/validate/bosses", data={"bosses": "hkalos"})
    assert "no Hard difficulty" in bad.text
    assert auth.post("/validate/bosses", data={"bosses": "  "}).text == ""


def test_approving_from_the_inbox(auth, fake_bot, seeded):
    response = auth.post(f"/inbox/{seeded['amendment']}/approve", follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"].startswith("/inbox?msg=")
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "confirmed"


def test_edit_then_approve_uses_the_time_the_reader_typed(auth, fake_bot, seeded):
    auth.post(
        f"/inbox/{seeded['amendment']}/approve",
        data={"to": "2026-09-03 22:15"},
        follow_redirects=False,
    )
    run = fake_bot.repo.get_run(seeded["run_star"])
    assert run["datetime"].astimezone(fake_bot.tz).strftime("%a %H:%M") == "Thu 22:15"


def test_rejecting_from_the_inbox(auth, fake_bot, seeded):
    auth.post(f"/inbox/{seeded['amendment']}/reject", follow_redirects=False)
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "rejected"


def test_adding_an_alias_from_the_members_page(auth, fake_bot, seeded):
    auth.post("/members/1002/nick", data={"alias": "MY"}, follow_redirects=False)
    assert fake_bot.repo.get_member(1002)["aliases"] == ["MY"]


def test_saving_config_from_the_page(auth, fake_bot, seeded):
    auth.post(
        "/config",
        data={"day_of_ping_time": "08:15", "countdown_minutes": "45"},
        follow_redirects=False,
    )
    assert fake_bot.ping_time.strftime("%H:%M") == "08:15"
    assert fake_bot.countdowns == [45]


def test_a_bad_config_value_comes_back_as_an_error_flash(auth, seeded):
    response = auth.post("/config", data={"day_of_ping_time": "25:99"}, follow_redirects=True)
    assert "flash--error" in response.text


def test_posting_the_digest_from_the_config_page(auth, fake_bot, seeded):
    response = auth.post("/digest", data={"week": "this", "channel_id": ""}, follow_redirects=False)
    assert response.status_code == 303
    assert "digest" in response.headers["location"]


def test_rescanning_from_the_config_page(auth, fake_bot, seeded):
    response = auth.post(
        "/rescan",
        data={"channel_id": str(WATCHED_CHANNEL), "hours": "6"},
        follow_redirects=False,
    )
    assert response.status_code == 303
    assert fake_bot.extractor.calls == [(str(WATCHED_CHANNEL), 6, True)]


def test_rescanning_an_unwatched_channel_comes_back_as_an_error(auth, seeded):
    response = auth.post(
        "/rescan", data={"channel_id": str(UNWATCHED_CHANNEL)}, follow_redirects=True
    )
    assert "flash--error" in response.text
    assert "isn&#39;t watched" in response.text or "isn't watched" in response.text


# --- degradation without htmx ----------------------------------------------


def test_every_action_in_a_row_is_a_real_form(auth, seeded):
    """With the CDN blocked the page must still work, so nothing is js-only."""
    body = auth.get("/").text
    row = body[body.index('<article class="run') : body.index("</article>")]
    assert row.count('<form method="post"') == 4
    assert row.count("hx-post=") == 4
    assert "onclick" not in row


def test_the_htmx_script_is_pinned_and_has_an_integrity_hash(auth):
    body = auth.get("/").text
    assert "cdnjs.cloudflare.com/ajax/libs/htmx/2.0.10/htmx.min.js" in body
    assert 'integrity="sha512-' in body


def test_the_portal_asks_not_to_be_indexed(auth):
    assert 'name="robots" content="noindex, nofollow"' in auth.get("/").text


def test_the_stylesheet_is_served(client):
    response = client.get("/static/portal.css")
    assert response.status_code == 200
    assert "--ground" in response.text


def test_a_signed_in_browser_sees_who_it_is(client, seeded):
    client.post("/login", data={"token": ADMIN_TOKEN}, follow_redirects=False)
    assert "sign out" in client.get("/").text


def test_the_api_reference_is_behind_the_same_auth(client, auth):
    """FastAPI's own /docs carries no dependency, so it is re-declared with one."""
    from fastapi.testclient import TestClient

    anonymous = TestClient(client.app)
    assert anonymous.get("/api/openapi.json").status_code == 401
    assert anonymous.get("/api/docs").status_code == 401
    assert auth.get("/api/docs").status_code == 200
    schema = auth.get("/api/openapi.json").json()
    assert "/api/schedule" in schema["paths"]
