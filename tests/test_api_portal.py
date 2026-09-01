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

from .fake_bot import (
    ADMIN_TOKEN,
    OTHER_CHANNEL,
    UNWATCHED_CHANNEL,
    WATCHED_CHANNEL,
    add_pilot,
)

PAGES = [
    "/",
    "/fixed",
    "/inbox",
    "/extractions",
    "/chat",
    "/limits",
    "/members",
    "/reminders",
    "/config",
]


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


def test_the_chat_page_lists_who_asked_what_it_used_and_how_it_went(auth, seeded):
    body = auth.get("/chat").text
    assert "kanon" in body  # the asker, by roster name
    assert "get_schedule" in body  # what it looked up
    assert "8.4 s" in body  # latency, in a unit a person reads
    assert "answered" in body


def test_the_chat_page_totals_the_models(auth, seeded):
    body = auth.get("/chat").text
    strip = body[body.index('class="stats"') : body.index('class="card"')]
    assert "qwen3:32b" in strip
    assert "answered 1 · failed 0" in strip
    assert "3,120 in · 64 out" in strip


def test_the_chat_detail_shows_the_question_the_reply_and_the_trace(auth, seeded):
    body = auth.get(f"/chat/{short_id(seeded['interaction'])}").text
    assert "when is star this week?" in body
    assert "Star is Monday 21:30" in body
    assert "week=&#39;this&#39;" in body  # the arguments, as the DEBUG log renders them
    assert "3,120" in body  # prompt tokens


def test_a_chat_detail_says_when_the_model_reported_no_tokens(auth, fake_bot):
    interaction = fake_bot.repo.log_chat_interaction(
        model="qwen3:32b", question="hi", reply="hello", outcome="answered", latency_ms=900
    )
    body = auth.get(f"/chat/{short_id(interaction)}").text
    assert "the model reported no usage" in body
    assert "900 ms" in body


def test_a_failed_interaction_says_so_rather_than_looking_answered(auth, fake_bot):
    interaction = fake_bot.repo.log_chat_interaction(
        model="qwen3:32b",
        question="what's on?",
        reply="Sorry — I couldn't get to the schedule just now.",
        outcome="failed",
        error="the model kept calling tools",
        rounds=4,
    )
    body = auth.get(f"/chat/{short_id(interaction)}").text
    assert "The generation failed." in body
    assert "the model kept calling tools" in body


def test_a_chat_detail_links_the_card_the_interaction_raised(auth, fake_bot, seeded):
    interaction = fake_bot.repo.log_chat_interaction(
        model="qwen3:32b",
        question="move star to wed",
        reply="Posted a card.",
        outcome="answered",
        tool_calls=[
            {
                "name": "propose_move",
                "arguments": "run_query='hstar'",
                "ms": 200,
                "outcome": "ok",
                "created": [seeded["amendment"]],
            }
        ],
    )
    body = auth.get(f"/chat/{short_id(interaction)}").text
    assert f"#{short_id(seeded['amendment'])}" in body
    assert "/900000000000000001" in body  # the card, in Discord


def test_an_unknown_chat_interaction_renders_the_error_page_not_a_traceback(auth, seeded):
    response = auth.get("/chat/deadbeef")
    assert response.status_code == 404
    assert "That doesn&#39;t exist" in response.text


def test_an_empty_chat_log_explains_what_turns_it_on(auth):
    assert "Nothing asked yet." in auth.get("/chat").text


def test_the_limits_page_says_the_model_is_free_when_it_is(auth, fake_bot):
    body = auth.get("/limits").text
    assert "The model is free" in body
    assert "Nothing running." in body
    assert "Nobody is mid-window." in body


def test_the_limits_page_names_the_holder_and_the_windows(auth, fake_bot, seeded, model_lock):
    from bot import modellock
    from bot.chat.gate import GLOBAL_KEY

    fake_bot.chat.limiter.allow(1002)
    fake_bot.chat.global_limiter.allow(GLOBAL_KEY)
    fake_bot.chat._busy.add(str(WATCHED_CHANNEL))
    assert auth.portal.call(modellock.acquire_within, 5, modellock.EXTRACTOR) is True
    try:
        body = auth.get("/limits").text
    finally:
        modellock.release()

    assert "The model is busy" in body
    assert "extractor" in body
    assert "#hstar-party" in body
    assert "kanon" in body  # the member mid-window, by roster name
    assert f"of {fake_bot.settings.chat_pilot_global_rate_count} left" in body


def test_the_live_region_is_a_wrapper_the_swap_cannot_replace(auth, fake_bot):
    """Everything that refetches hangs off the wrapper, so a swap never rebuilds it."""
    page = auth.get("/limits").text
    assert 'id="limits-live"' in page
    assert 'data-live-src="/limits/events"' in page  # the stream portal.js opens
    assert 'hx-get="/limits/live"' in page
    assert 'hx-swap="innerHTML"' in page

    fragment = auth.get("/limits/live")
    assert fragment.status_code == 200
    # The fragment is content only: it must not carry the wrapper, or a swap
    # would nest a second one and leave two of everything running.
    assert 'id="limits-live"' not in fragment.text
    assert "Windows in use" in fragment.text


def test_the_poll_is_only_a_slow_fallback_behind_the_stream(auth, fake_bot):
    """Updates arrive as events; the timer is for a browser that cannot have them."""
    page = auth.get("/limits").text
    assert "hx-trigger=\"every 60s [document.visibilityState === 'visible']\"" in page
    # ...and the manual link still works with no JavaScript at all.
    assert '<a class="btn btn--ghost" href="/limits">Refresh</a>' in page


def test_the_polled_region_contains_no_inputs(auth, fake_bot, seeded):
    """The invariant that makes refreshing safe: nothing in here can be typed into.

    A ten-second swap landing on a half-filled form would eat it, so the one
    form on the page lives outside the polled region -- and this is the test
    that notices when somebody puts a field back inside it.
    """
    fake_bot.chat.limiter.allow(1002)
    add_pilot(fake_bot, 1002, "kanon")

    live = auth.get("/limits/live").text

    assert "<input" not in live
    assert "<textarea" not in live
    assert "<select" not in live
    # ...while the page as a whole does have the form.
    assert "<input" in auth.get("/limits").text


def test_the_form_survives_a_refresh_of_the_live_panel(auth, fake_bot, seeded):
    """Structural: the form is not in what a poll replaces."""
    page = auth.get("/limits").text
    live_start = page.index('id="limits-live"')
    form_start = page.index('id="set-allowance"')
    assert form_start > live_start
    assert '<input name="user_id"' not in page[live_start:form_start]


def test_resetting_a_window_from_the_page_removes_its_row(auth, fake_bot, seeded):
    """The refreshed panel is the confirmation: the row is simply not in it."""
    fake_bot.chat.limiter.allow(1002)
    assert "kanon" in auth.get("/limits").text

    response = auth.post("/limits/windows/1002/reset", headers={"HX-Request": "true"})

    assert response.status_code == 200
    assert "Nobody is mid-window." in response.text  # the live panel came back
    assert fake_bot.chat.limiter.remaining(1002) == fake_bot.settings.chat_pilot_rate_count


def test_the_reset_button_is_a_real_form_too(auth, fake_bot, seeded):
    """htmx blocked: the same POST still lands and the page says what happened."""
    fake_bot.chat.limiter.allow(1002)
    assert 'action="/limits/windows/1002/reset"' in auth.get("/limits").text

    response = auth.post("/limits/windows/1002/reset", follow_redirects=False)

    assert response.status_code == 303
    assert response.headers["location"].startswith("/limits?")
    assert "kanon" in response.headers["location"]


def test_giving_a_member_their_own_allowance_from_the_page(auth, fake_bot, seeded):
    response = auth.post(
        "/limits/overrides",
        data={"user_id": "1002", "count": "10", "window_s": "60"},
        headers={"HX-Request": "true"},
    )

    assert response.status_code == 200
    assert "own allowance" not in response.text or "kanon" in response.text
    assert "10 per 60s" in response.text  # the overrides table
    assert fake_bot.chat.limiter.limit_for("1002") == (10, 60.0)


def test_an_override_can_be_set_for_somebody_with_no_open_window(auth, fake_bot, seeded):
    """The usual reason to raise it is that they are about to need it."""
    assert fake_bot.chat.limiter.snapshot() == {}

    auth.post("/limits/overrides", data={"user_id": "1003", "count": "8", "window_s": "120"})

    assert fake_bot.chat.limiter.limit_for("1003") == (8, 120.0)
    assert "8 per 120s" in auth.get("/limits").text


def test_a_bad_allowance_comes_back_as_a_flash_not_a_traceback(auth, fake_bot, seeded):
    response = auth.post(
        "/limits/overrides",
        data={"user_id": "1002", "count": "0", "window_s": "60"},
        follow_redirects=False,
    )

    assert response.status_code == 303
    assert "kind=error" in response.headers["location"]
    assert fake_bot.repo.list_rate_limits() == []


def test_clearing_an_override_from_the_page_removes_its_row(auth, fake_bot, seeded):
    auth.post("/limits/overrides", data={"user_id": "1002", "count": "10", "window_s": "60"})
    assert "10 per 60s" in auth.get("/limits").text

    response = auth.post("/limits/overrides/1002/clear", headers={"HX-Request": "true"})

    assert response.status_code == 200
    assert "10 per 60s" not in response.text
    assert fake_bot.repo.list_rate_limits() == []


def test_an_overridden_window_is_marked_and_counted_against_its_own_allowance(
    auth, fake_bot, seeded
):
    auth.post("/limits/overrides", data={"user_id": "1002", "count": "10", "window_s": "60"})
    fake_bot.chat.limiter.allow(1002)

    body = auth.get("/limits").text

    assert "own allowance" in body
    assert "1 of 10" in body


def test_the_limits_page_lists_who_may_ask(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    add_pilot(fake_bot, 1001, "Alvin tan", staff=True)

    body = auth.get("/limits").text

    assert "Who may ask" in body
    assert "kanon" in body and "Alvin tan" in body
    # Staff are marked and given no controls, because every one would change a
    # number nothing consults for them.
    assert "staff" in body
    assert "exempt" in body
    assert 'href="/limits?user=1002#set-allowance"' in body
    assert 'href="/limits?user=1001#set-allowance"' not in body


def test_the_page_says_when_it_cannot_read_the_role(auth, fake_bot):
    body = auth.get("/limits").text
    assert "No holders to show." in body
    assert "not connected to read it" in body


def test_a_pilots_set_button_prefills_the_form_with_their_own_numbers(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})

    body = auth.get("/limits?user=1002").text

    form = body[body.index('id="set-allowance"') :]
    assert 'value="1002"' in form
    assert 'value="10"' in form
    assert 'value="60"' in form


def test_the_prefill_falls_back_to_the_guild_default(auth, fake_bot, seeded):
    """No `?user=`, or a nonsense one, offers the numbers everybody else is on."""
    for path in ("/limits", "/limits?user=notanid"):
        form = auth.get(path).text
        form = form[form.index('id="set-allowance"') :]
        assert 'name="user_id" required inputmode="numeric" size="20" class="mono"\n' in form
        assert f'value="{fake_bot.settings.chat_pilot_rate_count}"' in form


def test_a_pilots_row_offers_the_actions_that_apply_to_them(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    plain = auth.get("/limits").text
    # Nothing to clear and no window to reset yet.
    assert "/limits/overrides/1002/clear" not in plain
    assert "/limits/windows/1002/reset" not in plain

    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})
    fake_bot.chat.limiter.allow(1002)
    body = auth.get("/limits").text

    assert "/limits/overrides/1002/clear" in body
    assert "/limits/windows/1002/reset" in body


def test_the_capacity_numbers_are_editable_from_the_config_page(auth, fake_bot):
    response = auth.post(
        "/config",
        data={"chat_pilot_rate_count": "9", "chat_pilot_rate_window_s": "600"},
        follow_redirects=False,
    )

    assert response.status_code == 303
    assert fake_bot.chat.limiter.count == 9
    assert fake_bot.chat.limiter.window == 600.0
    assert 'name="chat_pilot_rate_count"' in auth.get("/config").text


def test_the_event_stream_needs_signing_in(client):
    response = client.get("/limits/events", follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"] == "/login?next=/limits/events"


def test_the_limits_page_needs_signing_in(client):
    """A portal page sends you to the sign-in form; the JSON behind it 401s."""
    for path in ("/limits", "/limits/live"):
        response = client.get(path, follow_redirects=False)
        assert response.status_code == 303
        assert response.headers["location"] == f"/login?next={path}"


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
    assert 'class="pill pill--h"' in good.text
    assert "Radiant Malefic Star" in good.text
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


def finish_rescan(auth, response) -> str:
    """Wait for a started rescan job and return its finished fragment.

    The job runs as a task on the app's loop, so it is not done the instant the
    POST returns; polling the fragment is exactly what the page does.
    """
    job_id = response.headers["location"].split("job=")[1]
    for _ in range(50):
        body = auth.get(f"/rescan/{job_id}").text
        if "Re-reading" not in body:
            return body
    raise AssertionError("the rescan job never finished")


def test_rescanning_from_the_config_page_starts_a_job(auth, fake_bot, seeded):
    response = auth.post("/rescan", data={"window": "2weeks"}, follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"].startswith("/config?job=")


def test_the_job_reports_every_channel_when_it_finishes(auth, fake_bot, seeded):
    body = finish_rescan(
        auth, auth.post("/rescan", data={"window": "week"}, follow_redirects=False)
    )
    assert "#hstar-party" in body
    assert "#xkalos-party" in body
    assert "card(s) posted" in body


def test_an_htmx_rescan_gets_a_fragment_that_polls_itself(auth, fake_bot, seeded):
    response = auth.post("/rescan", data={"window": "week"}, headers={"HX-Request": "true"})
    assert response.status_code == 200
    assert 'id="rescan-job"' in response.text
    assert "<html" not in response.text


def test_only_the_ticked_channels_are_read(auth, fake_bot, seeded):
    finish_rescan(
        auth,
        auth.post(
            "/rescan",
            data={"window": "week", "channels": [str(OTHER_CHANNEL)]},
            follow_redirects=False,
        ),
    )
    assert [c for c, _, _ in fake_bot.extractor.calls] == [str(OTHER_CHANNEL)]


def test_rescanning_an_unwatched_channel_comes_back_as_an_error(auth, seeded):
    response = auth.post(
        "/rescan", data={"channels": [str(UNWATCHED_CHANNEL)]}, follow_redirects=True
    )
    assert "flash--error" in response.text
    assert "isn&#39;t watched" in response.text or "isn't watched" in response.text


def test_an_unknown_job_is_a_404(auth, seeded):
    assert auth.get("/rescan/nope").status_code == 404


def test_the_config_page_lists_the_channels_to_pick_from(auth, seeded):
    body = auth.get("/config").text
    assert "Re-read the party channels" in body
    assert 'name="channels"' in body
    assert "leave all unticked for every one" in body


def test_the_week_view_can_re_read_one_channel(auth, fake_bot, seeded):
    body = auth.get("/").text
    assert f'id="reread-{WATCHED_CHANNEL}"' in body
    assert 'form="reread-' in body
    finish_rescan(
        auth,
        auth.post(
            "/rescan",
            data={"channels": [str(WATCHED_CHANNEL)], "window": "week"},
            follow_redirects=False,
        ),
    )
    assert [c for c, _, _ in fake_bot.extractor.calls] == [str(WATCHED_CHANNEL)]


# --- degradation without htmx ----------------------------------------------


def test_every_action_in_a_row_is_a_real_form(auth, seeded):
    """With the CDN blocked the page must still work, so nothing is js-only."""
    body = auth.get("/").text
    row = body[body.index('<article class="run') : body.index("</article>")]
    # Move, preview ping, the party swap, the status control and one answer
    # form per participant -- each a real POST with an htmx upgrade, so the
    # count follows the party size rather than being a number to keep in step.
    for action in ("/amend", "/ping", "/participants", "/status", "/rsvp"):
        assert f'action="/runs/{seeded["run_star"]}{action}"' in row, action
    # Every one of them is a real POST *and* htmx-upgraded -- neither a form
    # that only works with the CDN, nor one that reloads the page for nothing.
    assert row.count('method="post"') == row.count("hx-post=") > 0
    assert "onclick" not in row
    assert row.count('type="submit" name="status"') == 5


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


def test_the_week_page_says_how_many_past_runs_it_hid(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")
    body = auth.get("/").text
    assert "1 past or cancelled run hidden" in body
    assert "show_past=1" in body


def test_showing_the_past_offers_the_way_back(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")
    body = auth.get("/?show_past=1").text
    assert "Hide the past" in body
    assert "🏁 done" in body


# --- difficulty pills and the boss grid (item 4) ----------------------------


def test_the_week_row_names_the_boss_and_its_difficulty(auth, seeded):
    body = auth.get("/").text
    assert "Radiant Malefic Star" in body
    assert '<span class="pill pill--h">HARD</span>' in body
    assert "Lv. 280" in body


def test_extreme_and_normal_get_their_own_pill(auth, seeded):
    body = auth.get("/").text
    assert '<span class="pill pill--x">EXTREME</span>' in body
    assert "Gatekeeper Kalos" in body


def test_the_bosses_page_lists_the_table_in_level_order(auth, seeded):
    body = auth.get("/bosses").text
    order = [body.index(name) for name in ("Chosen Seren", "Radiant Malefic Star", "Jupiter")]
    assert order == sorted(order)
    assert "Lv. 260" in body and "Lv. 295" in body


def test_the_bosses_page_ticks_what_the_guild_runs(auth, seeded):
    body = auth.get("/bosses").text
    assert "pill-toggle--on" in body  # HStar, HFA and XKalos have timings
    assert 'class="grid-bosses"' in body
    assert "10 bosses, 28 difficulties" in body
    assert "<strong>3</strong> ticked" in body


def test_the_bosses_page_is_read_only(auth, seeded):
    body = auth.get("/bosses").text
    assert "<input" not in body
    assert "<form" not in body


def test_a_boss_with_no_hard_difficulty_offers_no_hard_pill(auth):
    """Kalos has E/N/C/X in game; the grid must not offer a run nobody can enter."""
    body = auth.get("/bosses").text
    row = body[body.index("Gatekeeper Kalos") :]
    row = row[: row.index("</div>\n  </div>")]
    assert "EXTREME" in row and "CHAOS" in row
    assert ">HARD<" not in row


def test_the_fixed_page_offers_the_grid_as_a_picker(auth, seeded):
    body = auth.get("/fixed").text
    assert 'name="boss_tokens" value="HStar"' in body
    assert "tap the difficulties this party runs" in body


def test_the_editor_pre_ticks_the_bosses_a_timing_already_has(auth, seeded):
    body = " ".join(auth.get("/fixed").text.split())
    assert 'value="HStar" checked' in body
    assert 'value="XSeren" checked' not in body


def test_adding_a_timing_from_the_grid(auth, fake_bot, seeded):
    response = auth.post(
        "/fixed/new",
        data={
            "boss_tokens": ["NCarling", "HFA"],
            "day": "2",
            "time": "23:00",
            "channel_id": str(WATCHED_CHANNEL),
            "participants": ["1001"],
        },
        follow_redirects=False,
    )
    assert response.status_code == 303
    added = next(f for f in fake_bot.repo.list_fixed_runs() if f["weekday"] == 2)
    assert added["bosses"] == ["NCarling", "HFA"]


def test_the_grid_and_the_text_box_are_merged(auth, fake_bot, seeded):
    auth.post(
        "/fixed/new",
        data={
            "boss_tokens": ["NCarling"],
            "bosses": "hlimbo",
            "day": "3",
            "time": "23:00",
            "channel_id": str(WATCHED_CHANNEL),
            "participants": ["1001"],
        },
        follow_redirects=False,
    )
    added = next(f for f in fake_bot.repo.list_fixed_runs() if f["weekday"] == 3)
    assert added["bosses"] == ["NCarling", "HLimbo"]


def test_editing_a_timing_from_the_grid(auth, fake_bot, seeded):
    auth.post(
        f"/fixed/{seeded['fixed_star']}/edit",
        data={"boss_tokens": ["XKalos"], "day": "0", "time": "21:30", "participants": ["1001"]},
        follow_redirects=False,
    )
    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"])["bosses"] == ["XKalos"]


def test_the_inbox_names_the_boss_in_full(auth, seeded):
    assert "Radiant Malefic Star" in auth.get("/inbox").text


def test_the_pills_are_defined_for_both_themes(client):
    css = client.get("/static/portal.css").text
    for token in ("--pill-e-", "--pill-n-", "--pill-h-", "--pill-c-", "--pill-x-"):
        assert css.count(token) >= 4  # light bg+fg and dark bg+fg
    assert "prefers-color-scheme: dark" in css


def test_the_pill_toggle_is_big_enough_to_tap(client):
    assert "min-height: 36px" in client.get("/static/portal.css").text


# --- channel access (item 10) -----------------------------------------------


def test_the_config_page_shows_what_the_bot_may_do(auth, fake_bot, seeded):
    body = auth.get("/config").text
    assert "Channel access" in body
    assert "#hstar-party" in body
    table = body[body.index("Channel access") : body.index("A ❌ means")]
    assert "❌" not in table


def test_a_missing_permission_is_visible_at_a_glance(auth, fake_bot, seeded):
    fake_bot.channels[WATCHED_CHANNEL].permissions.send_messages = False
    body = auth.get("/config").text
    table = body[body.index("Channel access") : body.index("A ❌ means")]
    assert "❌" in table
    assert "Edit Channel" in body


def test_the_access_panel_can_be_refreshed_on_its_own(auth, seeded):
    fragment = auth.get("/access").text
    assert "<html" not in fragment
    assert "#hstar-party" in fragment


def test_rechecking_without_javascript_reloads_config(auth, seeded):
    response = auth.post("/access", follow_redirects=False)
    assert response.status_code == 303
    assert response.headers["location"].startswith("/config")


# --- inbox: mentions, and the "old → new" that only a move has --------------


def test_the_inbox_reads_a_mention_as_a_name_not_a_snowflake(auth, fake_bot, seeded):
    """A chat line quoted on a card must not print raw ``<@id>`` markup.

    Discord resolves mentions itself, so what is stored is the markup; the
    portal has to do that resolution on the way out or the evidence -- the one
    thing an approver actually reads -- is a wall of snowflakes.
    """
    from .conftest import kl
    from .fake_bot import WATCHED_CHANNEL

    fake_bot.repo.record_message(
        800000000000000009,
        WATCHED_CHANNEL,
        1002,
        kl(2026, 8, 30, 11, 55),
        "<@1001> <@!1003> <@4040404040404040404> we doing our nstar tonight?",
    )
    amendment = fake_bot.repo.create_amendment(
        week_start=seeded["week_start"],
        kind="move",
        bosses=["HStar"],
        run_id=seeded["run_star"],
        new_datetime=kl(2026, 9, 2, 22, 0),
        participants=["1002"],
        confidence=0.7,
        evidence_msg_ids=["800000000000000009"],
        channel_id=WATCHED_CHANNEL,
        summary="<@1001> asked to move it",
    )
    assert amendment
    body = auth.get("/inbox").text
    assert "<@1001>" not in body and "&lt;@1001&gt;" not in body
    assert "@Alvin tan" in body  # a mention resolves to the roster name
    assert "@Priya" in body  # ...and so does the <@!id> spelling
    assert "@member" in body  # ...while an id nobody matches stays anonymous
    assert "4040404040404040404" not in body


def test_the_extraction_page_reads_mentions_too(auth, fake_bot, seeded):
    from .conftest import kl
    from .fake_bot import WATCHED_CHANNEL

    fake_bot.repo.record_message(
        800000000000000010, WATCHED_CHANNEL, 1002, kl(2026, 8, 30, 11, 56), "<@1001> ok"
    )
    extraction = fake_bot.repo.log_extraction(
        model="gpt-oss:20b",
        prompt="you are an extractor...",
        raw_response="{}",
        latency_ms=10,
        message_ids=["800000000000000010"],
        amendment_ids=[],
    )
    body = auth.get(f"/extractions/{short_id(extraction)}").text
    assert "@Alvin tan" in body
    assert "&lt;@1001&gt;" not in body


def test_a_new_run_has_no_old_time_to_move_away_from(fake_bot, seeded):
    """`add`/`fix` cards used to render "<run's time> → TBD".

    The old time belongs to the run the amendment *matched*, which for a new
    run is only context, not the left half of a transition -- so it is only
    set for a move.
    """
    from bot.api import service

    added = fake_bot.repo.create_amendment(
        week_start=seeded["week_start"],
        kind="add",
        bosses=["HStar"],
        run_id=seeded["run_star"],
        participants=["1002"],
        confidence=0.5,
        evidence_msg_ids=[],
        channel_id=None,
    )
    view = service.amendment_view(fake_bot, fake_bot.repo.get_amendment(added))
    assert view["run"] is not None  # it did match an existing run
    assert view["from_when"] is None
    assert view["when"] == "TBD"

    moved = service.amendment_view(fake_bot, fake_bot.repo.get_amendment(seeded["amendment"]))
    assert moved["from_when"] == f"{moved['run']['local_day']} {moved['run']['local_time']}"


def test_the_inbox_draws_the_arrow_only_for_a_move(auth, fake_bot, seeded):
    added = fake_bot.repo.create_amendment(
        week_start=seeded["week_start"],
        kind="add",
        bosses=["HStar"],
        run_id=seeded["run_star"],
        participants=["1002"],
        confidence=0.5,
        evidence_msg_ids=[],
        channel_id=None,
    )
    cards = auth.get("/inbox").text.split('<article class="card"')
    new_run = next(c for c in cards if short_id(added) in c)
    moved = next(c for c in cards if short_id(seeded["amendment"]) in c)
    # The struck-out "was" time and its arrow are the transition; a new run
    # shows its proposed time (here TBD) on its own.
    assert 'class="mono was"' not in new_run and 'class="arrow"' not in new_run
    assert "TBD" in new_run
    assert 'class="mono was"' in moved and 'class="arrow"' in moved
