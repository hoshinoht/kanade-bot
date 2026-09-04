"""The JSON API: every route, against an in-memory repo and a stand-in client."""

from __future__ import annotations

import json
import time

import pytest

from bot.api.service import PORTAL_APPLIED, PORTAL_REJECTED
from bot.domain.ids import short_id

from .fake_bot import (
    OTHER_CHANNEL,
    OWNER_ID,
    UNWATCHED_CHANNEL,
    WATCHED_CHANNEL,
    add_pilot,
)


class _AnyHue:
    """The monogram hue is derived from the name; the value itself is arbitrary."""

    def __eq__(self, other: object) -> bool:
        return isinstance(other, int) and 0 <= other < 360


ANY_HUE = _AnyHue()

# --- schedule ---------------------------------------------------------------


def test_schedule_groups_by_day_and_counts_rsvps(auth, fake_bot, seeded):
    body = auth.get("/api/schedule").json()
    assert body["count"] == 2
    assert body["timezone"] == "Asia/Kuala_Lumpur"
    star = next(r for r in body["runs"] if "HStar" in r["bosses"])
    assert star["local_time"] == "21:30"
    assert star["yes"] == 1
    assert star["unanswered"] == 1
    assert star["status_label"] == "⚠️ unconfirmed"
    assert star["channel_name"] == "#hstar-party"
    expected = [
        run["datetime"].astimezone(fake_bot.tz).strftime("%a %d %b")
        for run in fake_bot.repo.list_runs(week_start=seeded["week_start"])
    ]
    assert [day["heading"] for day in body["days"]] == expected


def test_schedule_boss_detail_carries_the_full_in_game_name(auth, seeded):
    body = auth.get("/api/schedule").json()
    star = next(r for r in body["runs"] if "HStar" in r["bosses"])
    assert star["boss_detail"][0] == {
        "token": "HStar",
        "short": "Star",
        "full": "Radiant Malefic Star",
        "level": 280,
        "letter": "H",
        "difficulty": "HARD",
        "label": "Radiant Malefic Star (Hard, Lv280)",
        "portrait": None,
        "monogram": {"text": "RM", "hue": ANY_HUE},
    }


def test_a_boss_no_longer_in_the_table_still_renders(auth, fake_bot, seeded):
    """A run outlives an edit to bosses.yaml; it must not 500 the week view."""
    fake_bot.repo.set_run_bosses(seeded["run_star"], ["HGollux"])
    body = auth.get("/api/schedule").json()
    star = next(r for r in body["runs"] if r["bosses"] == ["HGollux"])
    assert star["boss_detail"][0]["full"] == "HGollux"
    assert star["boss_detail"][0]["difficulty"] == ""


def test_schedule_filters_by_channel_member_and_boss(auth, seeded):
    assert auth.get(f"/api/schedule?channel={OTHER_CHANNEL}").json()["count"] == 1
    assert auth.get("/api/schedule?user=1001").json()["count"] == 1
    assert auth.get("/api/schedule?boss=kalos").json()["count"] == 1
    assert auth.get("/api/schedule?user=1002").json()["count"] == 2


def test_next_week_is_empty_until_it_is_materialised(auth, seeded):
    assert auth.get("/api/schedule?week=next").json()["count"] == 0


def test_an_unknown_week_is_a_422(auth):
    assert auth.get("/api/schedule?week=last").status_code == 422


# --- fixed runs -------------------------------------------------------------


def test_fixed_list_and_get(auth, seeded):
    rows = auth.get("/api/fixed").json()
    assert {r["short_id"] for r in rows} == {
        short_id(seeded["fixed_star"]),
        short_id(seeded["fixed_kalos"]),
    }
    one = auth.get(f"/api/fixed/{short_id(seeded['fixed_star'])}").json()
    assert one["weekday_name"] == "Mon"
    assert one["time"] == "21:30"
    assert one["channel_watched"] is True


def test_creating_a_fixed_run_materialises_it(auth, fake_bot, seeded):
    before = fake_bot.materialised
    response = auth.post(
        "/api/fixed",
        json={
            "bosses": "hcarling, nbaldrix",
            "day": "wed",
            "time": "9:30pm",
            "participants": ["1001", "1003"],
            "channel_id": str(WATCHED_CHANNEL),
        },
    )
    assert response.status_code == 201
    body = response.json()
    assert body["bosses"] == ["HCarling", "NBaldrix"]
    assert body["time"] == "21:30"  # the same parser /fixed add uses
    assert body["weekday_name"] == "Wed"
    assert body["owner_name"]  # defaulted to the portal actor
    assert fake_bot.materialised > before


def test_a_bad_boss_token_is_refused_with_the_valid_forms(auth):
    response = auth.post(
        "/api/fixed",
        json={
            "bosses": "kalos",
            "day": "wed",
            "time": "21:30",
            "participants": ["1001"],
            "channel_id": str(WATCHED_CHANNEL),
        },
    )
    assert response.status_code == 400
    assert "missing a difficulty prefix" in response.json()["error"]
    assert "XKalos" in response.json()["error"]


def test_a_participant_without_the_bossing_role_is_refused(auth, seeded):
    response = auth.post(
        "/api/fixed",
        json={
            "bosses": "hstar",
            "day": "wed",
            "time": "21:30",
            "participants": ["1009"],
            "channel_id": str(WATCHED_CHANNEL),
        },
    )
    assert response.status_code == 400
    assert "not in the bossing role" in response.json()["error"]


def test_an_unwatched_home_channel_is_refused(auth, seeded):
    response = auth.post(
        "/api/fixed",
        json={
            "bosses": "hstar",
            "day": "wed",
            "time": "21:30",
            "participants": ["1001"],
            "channel_id": str(UNWATCHED_CHANNEL),
        },
    )
    assert response.status_code == 400
    assert "isn't watched" in response.json()["error"]


def test_a_run_needs_at_least_one_participant(auth):
    response = auth.post(
        "/api/fixed",
        json={
            "bosses": "hstar",
            "day": "wed",
            "time": "21:30",
            "participants": [],
            "channel_id": str(WATCHED_CHANNEL),
        },
    )
    assert response.status_code == 422


def test_editing_the_time_moves_the_already_materialised_run(auth, fake_bot, seeded):
    response = auth.patch(f"/api/fixed/{short_id(seeded['fixed_star'])}", json={"time": "22:15"})
    assert response.status_code == 200
    assert response.json()["time"] == "22:15"
    run = fake_bot.repo.get_run(seeded["run_star"])
    assert run["datetime"].astimezone(fake_bot.tz).strftime("%H:%M") == "22:15"


def test_editing_only_the_note_leaves_the_run_where_it_is(auth, fake_bot, seeded):
    before = fake_bot.repo.get_run(seeded["run_star"])["datetime"]
    auth.patch(f"/api/fixed/{short_id(seeded['fixed_star'])}", json={"note": "ring fee split"})
    assert fake_bot.repo.get_run(seeded["run_star"])["datetime"] == before


def test_an_empty_patch_says_so(auth, seeded):
    response = auth.patch(f"/api/fixed/{short_id(seeded['fixed_star'])}", json={})
    assert response.status_code == 400
    assert "nothing to change" in response.json()["error"]


def test_an_unknown_patch_field_is_rejected(auth, seeded):
    response = auth.patch(f"/api/fixed/{short_id(seeded['fixed_star'])}", json={"weekday": 2})
    assert response.status_code == 422


def test_deleting_a_fixed_run_cancels_its_upcoming_runs(auth, fake_bot, seeded):
    response = auth.delete(f"/api/fixed/{short_id(seeded['fixed_star'])}")
    assert response.status_code == 200
    assert response.json()["cancelled_runs"] == 1
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "cancelled"
    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"]) is None


def test_boss_validation_endpoint_reports_both_outcomes(auth):
    ok = auth.post("/api/validate/bosses", json={"text": "hstar, hfa"}).json()
    assert ok["ok"] is True
    assert [b["token"] for b in ok["bosses"]] == ["HStar", "HFA"]
    bad = auth.post("/api/validate/bosses", json={"text": "hkalos"}).json()
    assert bad["ok"] is False
    assert "no Hard difficulty" in bad["error"]


# --- ids --------------------------------------------------------------------


def test_a_short_prefix_resolves(auth, seeded):
    assert auth.get(f"/api/runs/{seeded['run_star'][:6]}").status_code == 200


def test_a_prefix_that_is_too_short_is_a_404_with_advice(auth, seeded):
    response = auth.get("/api/runs/ab")
    assert response.status_code == 404
    assert "at least 4" in response.json()["error"]


def test_an_unknown_id_is_a_404(auth, seeded):
    response = auth.get("/api/runs/deadbeef")
    assert response.status_code == 404


# --- run actions ------------------------------------------------------------


def test_amend_moves_the_run_and_announces_it(auth, fake_bot, seeded):
    response = auth.post(f"/api/runs/{seeded['run_star']}/amend", json={"to": "2026-09-02 21:45"})
    assert response.status_code == 200
    body = response.json()
    assert body["local_time"] == "21:45"
    assert body["local_day"] == "Wed 02 Sep"
    assert body["status"] == "planned"
    posted = fake_bot.posts[-1]
    assert "moved" in posted.content
    assert posted.channel_id == WATCHED_CHANNEL
    # A move is a receipt, not a summons: the party is named, nobody is pinged
    # (DESIGN.md §3, "Mention policy").
    assert posted.allowed_mentions == []
    assert "Alvin tan" in posted.content and "kanon" in posted.content


def test_amend_rebuilds_the_runs_reminders(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/amend", json={"to": "2026-09-02 21:45"})
    kinds = {r["kind"] for r in fake_bot.repo.list_reminders(seeded["run_star"])}
    assert kinds == {"day_of", "countdown_60", "countdown_15"}


def test_an_unreadable_date_says_what_would_work(auth, seeded):
    response = auth.post(f"/api/runs/{seeded['run_star']}/amend", json={"to": "sometime soon-ish"})
    assert response.status_code == 400
    assert "wed 21:30" in response.json()["error"]


def test_cancel_sets_the_status_and_tells_the_channel(auth, fake_bot, seeded):
    body = auth.post(f"/api/runs/{seeded['run_star']}/cancel").json()
    assert body["status"] == "cancelled"
    assert "cancelled" in fake_bot.posts[-1].content
    assert fake_bot.repo.list_reminders(seeded["run_star"]) == []


def test_otot_keeps_the_morning_ping_and_drops_the_countdowns(auth, fake_bot, seeded):
    body = auth.post(f"/api/runs/{seeded['run_star']}/otot").json()
    assert body["status"] == "otot"
    assert {r["kind"] for r in fake_bot.repo.list_reminders(seeded["run_star"])} == {"day_of"}


def test_rsvp_yes_from_everyone_confirms_the_run(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "yes"})
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "confirmed"
    assert fake_bot.retractions[-1] == (seeded["run_star"], "1002")


def test_rsvp_no_puts_the_run_at_risk_and_notifies(auth, fake_bot, seeded):
    body = auth.post(
        f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "no"}
    ).json()
    assert body["status"] == "at_risk"
    assert fake_bot.declines[-1] == (seeded["run_star"], "1002")


def test_rsvp_from_someone_not_on_the_run_is_refused(auth, seeded):
    response = auth.post(
        f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1003", "answer": "yes"}
    )
    assert response.status_code == 400
    assert "isn't on run" in response.json()["error"]


def test_an_unknown_rsvp_answer_is_refused(auth, seeded):
    response = auth.post(
        f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "sure"}
    )
    assert response.status_code == 422


# --- the inbox --------------------------------------------------------------


def test_pending_inlines_the_chat_it_cited(auth, seeded):
    rows = auth.get("/api/pending").json()
    assert len(rows) == 1
    item = rows[0]
    assert item["kind"] == "move"
    assert item["confidence"] == 0.82
    assert item["evidence"][0]["content"] == "can change to wed?"
    assert item["evidence"][0]["author_name"] == "kanon"
    assert item["card_url"].endswith("/900000000000000001")
    assert item["run"]["short_id"] == short_id(seeded["run_star"])


def test_approving_applies_the_change_and_annotates_the_card(auth, fake_bot, seeded):
    response = auth.post(f"/api/amendments/{seeded['amendment']}/approve")
    assert response.status_code == 200
    body = response.json()
    assert body["applied"] is True
    assert body["actor_id"] == str(OWNER_ID)
    run = fake_bot.repo.get_run(seeded["run_star"])
    assert run["datetime"].astimezone(fake_bot.tz).strftime("%a %H:%M") == "Wed 21:30"
    assert (str(WATCHED_CHANNEL), "900000000000000001", PORTAL_APPLIED) in fake_bot.annotations


def test_approving_credits_an_explicit_actor(auth, fake_bot, seeded):
    body = auth.post(
        f"/api/amendments/{seeded['amendment']}/approve", json={"actor_id": "1002"}
    ).json()
    assert body["actor_id"] == "1002"


def test_approving_credits_portal_actor_id_when_it_is_set(auth, fake_bot, seeded):
    from .fake_bot import make_settings

    fake_bot.settings = make_settings(portal_actor_id=1001)
    body = auth.post(f"/api/amendments/{seeded['amendment']}/approve").json()
    assert body["actor_id"] == "1001"


def test_a_change_can_only_be_answered_once(auth, seeded):
    auth.post(f"/api/amendments/{seeded['amendment']}/approve")
    again = auth.post(f"/api/amendments/{seeded['amendment']}/approve")
    assert again.status_code == 400
    assert "already `confirmed`" in again.json()["error"]


def test_rejecting_marks_it_and_annotates_the_card(auth, fake_bot, seeded):
    body = auth.post(f"/api/amendments/{seeded['amendment']}/reject").json()
    assert body["status"] == "rejected"
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "rejected"
    assert (str(WATCHED_CHANNEL), "900000000000000001", PORTAL_REJECTED) in fake_bot.annotations
    assert fake_bot.repo.get_run(seeded["run_star"])["datetime"].hour  # unchanged


def test_a_move_with_no_agreed_time_cannot_be_applied(auth, fake_bot, seeded):
    amendment = fake_bot.repo.create_amendment(
        week_start=seeded["week_start"],
        kind="move",
        run_id=seeded["run_kalos"],
        channel_id=OTHER_CHANNEL,
        day_ref="wed",
    )
    response = auth.post(f"/api/amendments/{amendment}/approve")
    assert response.status_code == 400
    assert "no new time" in response.json()["error"]


# --- extraction log ---------------------------------------------------------


def test_the_extraction_list_summarises_and_the_detail_shows_its_work(auth, seeded):
    rows = auth.get("/api/extractions").json()
    assert rows[0]["latency_ms"] == 1234
    assert rows[0]["amendment_count"] == 1
    assert "prompt" not in rows[0]

    detail = auth.get(f"/api/extractions/{short_id(seeded['extraction'])}").json()
    assert detail["prompt"].startswith("you are an extractor")
    assert detail["raw_response"] == '{"amendments": []}'
    assert detail["messages"][0]["content"] == "can change to wed?"
    assert detail["amendments"][0]["kind"] == "move"


# --- chat log ---------------------------------------------------------------


def test_the_chat_list_summarises_and_the_detail_shows_its_work(auth, seeded):
    rows = auth.get("/api/chat").json()
    assert rows[0]["author_name"] == "kanon"
    assert rows[0]["model"] == "qwen3:32b"
    assert rows[0]["tool_names"] == ["get_schedule"]
    assert rows[0]["outcome"] == "answered"
    assert "question" not in rows[0]

    detail = auth.get(f"/api/chat/{short_id(seeded['interaction'])}").json()
    assert detail["question"] == "when is star this week?"
    assert detail["reply"].startswith("Star is Monday 21:30")
    assert detail["model_ms"] == 8100
    assert detail["tool_calls"][0]["arguments"] == "week='this'"
    assert detail["tool_calls"][0]["ok"] is True
    # Rows written before diagnostics gained their additive JSON keys stay
    # readable, and do not pretend to know a round or return they never stored.
    assert detail["tool_calls"][0]["round"] == 0
    assert detail["tool_calls"][0]["output"] == ""
    assert detail["model_rounds"] == []


def test_the_chat_detail_exposes_model_rounds_and_tool_returns(auth, fake_bot):
    interaction = fake_bot.repo.log_chat_interaction(
        model="qwen3:32b",
        question="what's on?",
        reply="Two runs.",
        outcome="answered",
        tool_calls=[
            {
                "name": "get_schedule",
                "round": 1,
                "arguments": "week='this'",
                "output": "one\ntwo",
                "ms": 12,
                "outcome": "ok",
                "created": [],
                "posted": [],
            }
        ],
        model_rounds=[
            {
                "round": 1,
                "content": "  checking\n",
                "thinking": "look it up",
                "requested_tools": ["get_schedule"],
            },
            {"round": 2, "content": "Two runs.", "thinking": None, "requested_tools": []},
        ],
    )

    detail = auth.get(f"/api/chat/{short_id(interaction)}").json()
    assert detail["tool_calls"][0]["round"] == 1
    assert detail["tool_calls"][0]["output"] == "one\ntwo"
    assert detail["model_rounds"] == [
        {
            "round": 1,
            "content": "  checking\n",
            "thinking": "look it up",
            "requested_tools": ["get_schedule"],
        },
        {"round": 2, "content": "Two runs.", "thinking": None, "requested_tools": []},
    ]


def test_the_chat_summary_totals_per_model(auth, seeded):
    body = auth.get("/api/chat/summary").json()
    assert body["count"] == 1
    assert body["prompt_tokens"] == 3120
    assert body["models"][0]["model"] == "qwen3:32b"
    assert body["models"][0]["p95_latency_ms"] == 8400


def test_the_chat_summary_is_not_read_as_an_interaction_id(auth, seeded):
    """`/summary` is declared first, so the id route never swallows it."""
    assert auth.get("/api/chat/summary").json()["count"] == 1


def test_an_unknown_chat_interaction_is_a_404(auth, seeded):
    assert auth.get("/api/chat/deadbeef").status_code == 404


def test_the_chat_log_needs_the_token(client, seeded):
    assert client.get("/api/chat").status_code == 401


# --- capacity ---------------------------------------------------------------


def test_limits_reports_an_idle_host(auth, fake_bot):
    body = auth.get("/api/limits").json()
    assert body["model"] == {"busy": False, "holder": None, "held_for_s": 0.0}
    assert body["global_pool"] == {
        "count": fake_bot.settings.chat_pilot_global_rate_count,
        "window_s": fake_bot.settings.chat_pilot_global_rate_window_s,
        "used": 0,
        "remaining": fake_bot.settings.chat_pilot_global_rate_count,
    }
    assert body["per_user"]["windows"] == []
    assert body["jobs"] == {
        "answering": [],
        "extracting": False,
        "rescan": {"worker_running": True, "queued": 0, "job": None, "channel": None},
    }


def test_limits_names_who_is_holding_the_model(auth, fake_bot, model_lock):
    """The point of the label: "busy" is half an answer and "busy with what" is the rest."""
    from bot.infrastructure import modellock

    assert auth.portal.call(modellock.acquire_within, 5, modellock.EXTRACTOR) is True
    try:
        body = auth.get("/api/limits").json()
    finally:
        modellock.release()

    assert body["model"]["busy"] is True
    assert body["model"]["holder"] == "extractor"
    assert body["model"]["held_for_s"] >= 0
    # An extraction is simply whatever has the model, so the jobs view agrees.
    assert body["jobs"]["extracting"] is True
    assert auth.get("/api/limits").json()["model"]["busy"] is False


def test_limits_reports_a_lock_taken_without_a_label_as_busy(auth, fake_bot, model_lock):
    """Reporting "idle" for a lock that is plainly held is the one wrong answer."""
    auth.portal.call(model_lock.acquire)
    try:
        body = auth.get("/api/limits").json()
    finally:
        model_lock.release()

    assert body["model"]["busy"] is True
    assert body["model"]["holder"] is None
    assert body["jobs"]["extracting"] is False


def test_limits_counts_down_both_budgets(auth, fake_bot, seeded):
    from bot.chat.gate import GLOBAL_KEY

    pilot = fake_bot.chat
    pilot.global_limiter.allow(GLOBAL_KEY)
    pilot.global_limiter.allow(GLOBAL_KEY)
    pilot.limiter.allow(1002)

    body = auth.get("/api/limits").json()

    assert body["global_pool"]["used"] == 2
    assert body["global_pool"]["remaining"] == body["global_pool"]["count"] - 2
    # Named from the roster, exactly as the chat log names an asker, and
    # carrying the allowance they are actually on.
    default = fake_bot.settings.chat_pilot_rate_count
    assert body["per_user"]["windows"] == [
        {
            "user_id": "1002",
            "name": "kanon",
            "used": 1,
            "remaining": default - 1,
            "count": default,
            "window_s": fake_bot.settings.chat_pilot_rate_window_s,
            "overridden": False,
        }
    ]


def test_asking_for_the_limits_never_spends_one(auth, fake_bot):
    """A page that polls this must not quietly drink the pool it is reporting on."""
    from bot.chat.gate import GLOBAL_KEY

    for _ in range(5):
        auth.get("/api/limits")
    assert fake_bot.chat.global_limiter.remaining(GLOBAL_KEY) == (
        fake_bot.settings.chat_pilot_global_rate_count
    )
    assert fake_bot.chat.limiter.snapshot() == {}


def test_limits_lists_the_channels_currently_answering(auth, fake_bot):
    fake_bot.chat._busy.add(str(WATCHED_CHANNEL))

    body = auth.get("/api/limits").json()

    assert body["jobs"]["answering"] == [
        {"channel_id": str(WATCHED_CHANNEL), "channel_name": "#hstar-party"}
    ]


def test_limits_needs_the_token(client):
    assert client.get("/api/limits").status_code == 401


def test_resetting_a_window_gives_a_member_their_answers_back(auth, fake_bot, seeded):
    pilot = fake_bot.chat
    pilot.limiter.allow(1002)
    pilot.limiter.allow(1002)
    assert auth.get("/api/limits").json()["per_user"]["windows"] != []

    body = auth.delete("/api/limits/windows/1002").json()

    assert body == {"user_id": "1002", "name": "kanon"}
    # Gone from the live view, which is the only place the window ever existed.
    assert auth.get("/api/limits").json()["per_user"]["windows"] == []
    assert pilot.limiter.remaining(1002) == fake_bot.settings.chat_pilot_rate_count


def test_resetting_a_window_is_written_to_the_audit_trail(auth, fake_bot, seeded):
    auth.delete("/api/limits/windows/1002")

    row = fake_bot.repo.list_audit(limit=1)[0]
    assert row["action"] == "limits"
    assert row["subject"] == "1002"
    assert "kanon" in row["detail"]


def test_resetting_a_window_clears_the_notice_that_went_with_it(auth, fake_bot, seeded):
    """Otherwise their next refusal would be silently the second of an episode."""
    fake_bot.chat._told_until["1002"] = time.monotonic() + 300

    auth.delete("/api/limits/windows/1002")

    assert fake_bot.chat._told_until == {}


def test_resetting_a_window_nobody_has_is_harmless(auth, fake_bot, seeded):
    """The roster is not consulted: a chat-role holder need not be a bosser."""
    body = auth.delete("/api/limits/windows/4242").json()
    assert body == {"user_id": "4242", "name": "user 4242"}


def test_only_one_members_window_is_cleared(auth, fake_bot, seeded):
    """Individual resets only -- there is no route that empties everybody's."""
    pilot = fake_bot.chat
    pilot.limiter.allow(1001)
    pilot.limiter.allow(1002)

    auth.delete("/api/limits/windows/1002")

    assert pilot.limiter.snapshot() == {"1001": 1}


def test_the_guilds_pool_is_not_resettable(auth, fake_bot, seeded):
    """Scope guard: the pool measures the host, not a person's deserts."""
    from bot.chat.gate import GLOBAL_KEY

    pilot = fake_bot.chat
    pilot.global_limiter.allow(GLOBAL_KEY)
    spent = auth.get("/api/limits").json()["global_pool"]["used"]

    auth.delete("/api/limits/windows/1002")

    assert auth.get("/api/limits").json()["global_pool"]["used"] == spent
    assert auth.delete("/api/limits/windows").status_code in (404, 405)


def test_resetting_a_window_needs_the_token(client, seeded):
    assert client.delete("/api/limits/windows/1002").status_code == 401


# --- per-member allowances --------------------------------------------------


def test_an_override_is_stored_applied_and_reported(auth, fake_bot, seeded):
    body = auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60}).json()

    assert body == {"user_id": "1002", "name": "kanon", "count": 10, "window_s": 60.0}
    # Stored, so it survives a restart...
    assert fake_bot.repo.list_rate_limits() == [
        {
            "user_id": "1002",
            "count": 10,
            "window_s": 60.0,
            "updated_at": fake_bot.repo.list_rate_limits()[0]["updated_at"],
        }
    ]
    # ...and live, so the very next question is judged by it.
    assert fake_bot.chat.limiter.limit_for("1002") == (10, 60.0)
    listed = auth.get("/api/limits").json()["per_user"]["overrides"]
    assert listed == [{"user_id": "1002", "name": "kanon", "count": 10, "window_s": 60.0}]


def test_an_override_shows_on_the_window_it_governs(auth, fake_bot, seeded):
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})
    fake_bot.chat.limiter.allow(1002)

    (row,) = auth.get("/api/limits").json()["per_user"]["windows"]

    assert (row["count"], row["window_s"], row["overridden"]) == (10, 60.0, True)
    assert row["remaining"] == 9


def test_setting_an_override_replaces_the_previous_one(auth, fake_bot, seeded):
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})
    auth.put("/api/limits/overrides/1002", json={"count": 2, "window_s": 30})

    assert fake_bot.chat.limiter.limit_for("1002") == (2, 30.0)
    assert len(fake_bot.repo.list_rate_limits()) == 1


def test_clearing_an_override_puts_them_back_on_the_default(auth, fake_bot, seeded):
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})

    body = auth.delete("/api/limits/overrides/1002").json()

    assert body == {"user_id": "1002", "name": "kanon"}
    assert fake_bot.repo.list_rate_limits() == []
    assert fake_bot.chat.limiter.limit_for("1002") == (
        fake_bot.settings.chat_pilot_rate_count,
        fake_bot.settings.chat_pilot_rate_window_s,
    )


def test_clearing_an_override_nobody_had_is_not_an_error(auth, fake_bot, seeded):
    """The caller asked for "this member is on the default", and that is the result."""
    assert auth.delete("/api/limits/overrides/1002").status_code == 200


def test_both_override_changes_are_written_to_the_audit_trail(auth, fake_bot, seeded):
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})
    set_row = fake_bot.repo.list_audit(limit=1)[0]
    assert set_row["action"] == "limits"
    assert set_row["subject"] == "1002"
    assert "10 answer(s) per 60s" in set_row["detail"]

    auth.delete("/api/limits/overrides/1002")
    cleared = fake_bot.repo.list_audit(limit=1)[0]
    assert "back on the guild's default" in cleared["detail"]


def test_an_unclearable_override_writes_no_audit_row(auth, fake_bot, seeded):
    """Nothing changed, so the trail must not claim something did."""
    before = len(fake_bot.repo.list_audit(limit=50))
    auth.delete("/api/limits/overrides/4242")
    assert len(fake_bot.repo.list_audit(limit=50)) == before


def test_a_nonsense_allowance_is_refused(auth, fake_bot, seeded):
    assert auth.put("/api/limits/overrides/1002", json={"count": 0, "window_s": 60}).status_code
    for body in ({"count": 0, "window_s": 60}, {"count": 4, "window_s": 0}, {"count": "lots"}):
        assert auth.put("/api/limits/overrides/1002", json=body).status_code == 422
    assert fake_bot.repo.list_rate_limits() == []


def test_an_override_can_be_given_to_somebody_off_the_roster(auth, fake_bot, seeded):
    """The chat role and the bossing role are different things."""
    body = auth.put("/api/limits/overrides/4242", json={"count": 3, "window_s": 60}).json()
    assert body["name"] == "user 4242"
    assert fake_bot.chat.limiter.limit_for("4242") == (3, 60.0)


def test_the_override_routes_need_the_token(client, seeded):
    assert (
        client.put("/api/limits/overrides/1002", json={"count": 4, "window_s": 60}).status_code
        == 401
    )
    assert client.delete("/api/limits/overrides/1002").status_code == 401


# --- the pilot roster -------------------------------------------------------


def test_the_pilots_are_read_live_from_the_role(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    add_pilot(fake_bot, 1003, "Priya")

    pilots = auth.get("/api/limits").json()["pilots"]

    assert [p["user_id"] for p in pilots] == ["1002", "1003"]
    # The roster's name when it knows them, so one person reads the same in
    # both tables on the page.
    assert pilots[0]["name"] == "kanon"
    assert pilots[1]["name"] == "Priya"
    default = fake_bot.settings.chat_pilot_rate_count
    assert (pilots[0]["count"], pilots[0]["overridden"], pilots[0]["staff"]) == (
        default,
        False,
        False,
    )


def test_a_pilot_carries_their_own_allowance_and_open_window(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    auth.put("/api/limits/overrides/1002", json={"count": 10, "window_s": 60})
    fake_bot.chat.limiter.allow(1002)

    (pilot,) = auth.get("/api/limits").json()["pilots"]

    assert (pilot["count"], pilot["window_s"], pilot["overridden"]) == (10, 60.0, True)
    assert (pilot["used"], pilot["remaining"], pilot["has_window"]) == (1, 9, True)


def test_staff_holding_the_pilot_role_are_marked_exempt(auth, fake_bot, seeded):
    add_pilot(fake_bot, 1001, "Alvin tan", staff=True)
    add_pilot(fake_bot, 1002, "kanon [AZUR]")

    pilots = auth.get("/api/limits").json()["pilots"]

    by_id = {p["user_id"]: p for p in pilots}
    assert by_id["1001"]["staff"] is True
    assert by_id["1002"]["staff"] is False
    # Staff sort last: the rows somebody can act on come first.
    assert pilots[-1]["user_id"] == "1001"


def test_a_bot_holding_the_pilot_role_is_not_listed(auth, fake_bot, seeded):
    """The same rule the bossing roster applies: another bot is not a member."""
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    add_pilot(fake_bot, 4040, "SomeWebhook", is_bot=True)

    assert [p["user_id"] for p in auth.get("/api/limits").json()["pilots"]] == ["1002"]


def test_an_empty_pilot_role_lists_nobody(auth, fake_bot, seeded):
    from .fake_bot import PILOT_ROLE, FakeGuildRole

    fake_bot.settings.chat_pilot_role_id = PILOT_ROLE
    fake_bot.guild.roles[PILOT_ROLE] = FakeGuildRole(PILOT_ROLE)

    assert auth.get("/api/limits").json()["pilots"] == []


def test_an_unresolvable_role_or_guild_lists_nobody_rather_than_failing(auth, fake_bot, seeded):
    """A bot that is not connected cannot read the role, and must not 500 saying so."""
    # No role configured at all.
    assert auth.get("/api/limits").json()["pilots"] == []

    # Configured, but the guild does not have it.
    fake_bot.settings.chat_pilot_role_id = 999000000000000009
    assert auth.get("/api/limits").json()["pilots"] == []

    # Configured, but there is no guild to ask.
    add_pilot(fake_bot, 1002, "kanon [AZUR]")
    fake_bot.get_guild = lambda _guild_id: None
    assert auth.get("/api/limits").json()["pilots"] == []


# --- the capacity settings --------------------------------------------------


def test_the_capacity_numbers_are_readable_and_writable_over_the_api(auth, fake_bot):
    before = auth.get("/api/config").json()
    assert before["chat_pilot_rate_count"] == 4

    body = auth.put("/api/config", json={"chat_pilot_rate_count": 9}).json()

    assert body["chat_pilot_rate_count"] == 9
    assert fake_bot.chat.limiter.count == 9


def test_the_api_refuses_a_capacity_number_that_makes_no_sense(auth, fake_bot):
    for body in (
        {"chat_pilot_rate_count": 0},
        {"chat_pilot_rate_window_s": 0},
        {"chat_pilot_global_rate_count": -1},
    ):
        assert auth.put("/api/config", json=body).status_code == 422
    assert fake_bot.chat.limiter.count == 4


# --- members ----------------------------------------------------------------


def test_members_lists_the_roster_with_this_weeks_load(auth, seeded):
    rows = auth.get("/api/members").json()
    by_id = {m["user_id"]: m for m in rows}
    assert set(by_id) == {"1001", "1002", "1003"}  # 1009 has no role
    assert by_id["1002"]["runs_this_week"] == 2
    assert by_id["1002"]["nickname"] == "kanon"


def test_member_api_exposes_saved_style_without_override_metadata(auth, fake_bot, seeded):
    fake_bot.repo.set_reply_style(1002, "brief")
    member = next(row for row in auth.get("/api/members").json() if row["user_id"] == "1002")
    assert member["reply_style"] == "brief"
    assert "effective_style" not in member
    assert "style_source" not in member


def test_adding_an_alias(auth, fake_bot, seeded):
    body = auth.post("/api/members/1002/nick", json={"alias": "MY"}).json()
    assert body["aliases"] == ["MY"]
    assert fake_bot.repo.get_member(1002)["aliases"] == ["MY"]


def test_an_alias_for_an_unknown_member_is_a_404(auth, seeded):
    assert auth.post("/api/members/424242/nick", json={"alias": "x"}).status_code == 404


def test_an_empty_alias_is_refused(auth, seeded):
    assert auth.post("/api/members/1002/nick", json={"alias": "  "}).status_code == 400


# --- reminders --------------------------------------------------------------


def test_reminders_can_be_scoped_to_one_run(auth, seeded):
    everything = auth.get("/api/reminders").json()
    one = auth.get(f"/api/reminders?run_id={seeded['run_star']}").json()
    assert len(one) < len(everything)
    assert {r["run_short_id"] for r in one} == {short_id(seeded["run_star"])}
    assert {r["kind"] for r in one} == {"day_of", "countdown_60", "countdown_15"}


# --- config -----------------------------------------------------------------


def test_config_reports_runtime_and_deployment_values(auth, seeded):
    body = auth.get("/api/config").json()
    assert body["day_of_ping_time"] == "09:00"
    assert body["countdown_minutes"] == "60,15"
    assert body["paused"] is False
    assert body["extract_enabled"] is True
    assert body["timezone"] == "Asia/Kuala_Lumpur"
    assert body["reset"] == "Thu 00:00"
    assert body["model"] == "gpt-oss:20b"


def test_setting_the_ping_time_re_places_the_pending_morning_pings(auth, fake_bot, seeded):
    body = auth.put("/api/config", json={"day_of_ping_time": "8:30am"}).json()
    assert body["day_of_ping_time"] == "08:30"
    day_of = [r for r in fake_bot.repo.list_reminders(seeded["run_star"]) if r["kind"] == "day_of"]
    assert day_of[0]["fire_at"].astimezone(fake_bot.tz).strftime("%H:%M") == "08:30"


def test_setting_the_ping_time_preserves_a_posted_morning_mapping(auth, fake_bot, seeded):
    morning = next(
        reminder
        for reminder in fake_bot.repo.list_reminders(seeded["run_star"])
        if reminder["kind"] == "day_of"
    )
    fake_bot.repo.mark_reminder_sent(morning["id"], 900000000000000013)

    auth.put("/api/config", json={"day_of_ping_time": "08:30"})

    preserved = fake_bot.repo.get_reminder(morning["id"])
    assert preserved["message_id"] == "900000000000000013"
    assert [row["id"] for row in fake_bot.repo.reminders_by_message(900000000000000013)] == [
        morning["id"]
    ]


def test_setting_the_countdowns_preserves_posted_mornings_and_replaces_pending_kinds(
    auth, fake_bot, seeded
):
    morning = next(
        reminder
        for reminder in fake_bot.repo.list_reminders(seeded["run_star"])
        if reminder["kind"] == "day_of"
    )
    fake_bot.repo.mark_reminder_sent(morning["id"], 900000000000000011)

    auth.put("/api/config", json={"countdown_minutes": "30, 5"})

    kinds = {r["kind"] for r in fake_bot.repo.list_reminders(seeded["run_star"])}
    assert kinds == {"day_of", "countdown_30", "countdown_5"}
    assert [row["id"] for row in fake_bot.repo.reminders_by_message(900000000000000011)] == [
        morning["id"]
    ]


def test_pausing_and_turning_the_extractor_off(auth, fake_bot, seeded):
    auth.put("/api/config", json={"paused": True, "extract_enabled": False})
    assert fake_bot.paused is True
    assert fake_bot.extract_enabled is False


@pytest.mark.parametrize("value", ["25:00", "half past nine", ""])
def test_a_bad_ping_time_is_refused(auth, value):
    assert auth.put("/api/config", json={"day_of_ping_time": value}).status_code == 400


@pytest.mark.parametrize("value", ["-5", "0", "soon", ""])
def test_bad_countdowns_are_refused(auth, value):
    assert auth.put("/api/config", json={"countdown_minutes": value}).status_code == 400


def test_config_refuses_a_key_it_does_not_own(auth):
    assert auth.put("/api/config", json={"discord_token": "x"}).status_code == 422


def test_an_empty_config_put_says_so(auth):
    response = auth.put("/api/config", json={})
    assert response.status_code == 400
    assert "nothing to change" in response.json()["error"]


def test_role_plugins_are_readable_and_writable_over_the_api(auth, tmp_path, monkeypatch):
    from bot import behaviour_plugins

    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    behaviour_plugins.write("playful", "Use playful, smug banter.")
    behaviour_plugins.write("concise", "Always answer in short lines.")
    assignments = [
        {"role_id": "1540491480936751205", "plugin": "playful"},
        {"role_id": "1540491480936751206", "plugin": "concise"},
    ]

    body = auth.put("/api/config", json={"chat_role_plugins": assignments}).json()

    assert body["chat_role_plugins"] == assignments
    assert auth.get("/api/config").json()["chat_role_plugins"] == assignments


def test_public_profile_catalog_is_explicit_and_ordered(auth, tmp_path, monkeypatch):
    from bot import behaviour_plugins

    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    behaviour_plugins.write("brief", "BRIEF")
    behaviour_plugins.write("playful", "PLAYFUL")

    body = auth.put(
        "/api/config", json={"chat_selectable_plugins": ["playful", "brief"]}
    ).json()
    assert body["chat_selectable_plugins"] == ["playful", "brief"]


@pytest.mark.parametrize(
    "prompts",
    [
        [{"role_id": "not-an-id", "plugin": "playful"}],
        [{"role_id": "1540491480936751205", "plugin": ""}],
        [
            {"role_id": "1540491480936751205", "plugin": "playful"},
            {"role_id": "1540491480936751205", "plugin": "concise"},
        ],
    ],
)
def test_invalid_role_plugins_are_refused(auth, prompts):
    response = auth.put("/api/config", json={"chat_role_plugins": prompts})

    assert response.status_code == 400


# --- digest, rescan, ping ---------------------------------------------------


def test_digest_posts_and_returns_a_link(auth, fake_bot, seeded):
    body = auth.post("/api/digest", json={"channel_id": str(OTHER_CHANNEL)}).json()
    assert body["posted"] is True
    assert fake_bot.digest_channel == str(OTHER_CHANNEL)
    assert body["url"].startswith("https://discord.com/channels/")


def test_digest_says_which_channel_and_why(auth, fake_bot, seeded):
    """ "couldn't post" sends the owner hunting; the reason does not."""
    from .fake_bot import make_settings

    fake_bot.settings = make_settings(post_channel_id=None)
    response = auth.post("/api/digest")
    assert response.status_code == 400
    assert "POST_CHANNEL_ID is not set" in response.json()["error"]


def test_digest_says_when_the_bot_cannot_post_there(auth, fake_bot, seeded):
    fake_bot.channels[WATCHED_CHANNEL].permissions.send_messages = False
    response = auth.post("/api/digest", json={"channel_id": str(WATCHED_CHANNEL)})
    assert response.status_code == 400
    error = response.json()["error"]
    assert "#hstar-party" in error
    assert "View Channel + Send Messages" in error


def test_digest_says_when_the_channel_does_not_exist(auth, fake_bot, seeded):
    from .fake_bot import make_settings

    fake_bot.settings = make_settings(post_channel_id=None)
    response = auth.post("/api/digest", json={"channel_id": "424242"})
    assert response.status_code == 400
    assert "does not exist" in response.json()["error"]


def test_a_rejected_message_is_reported_separately(auth, fake_bot, seeded):
    fake_bot.digest_fails = True
    response = auth.post("/api/digest")
    assert response.status_code == 400
    assert "Discord rejected the message" in response.json()["error"]


# --- what the bot may actually do in each channel ---------------------------


def test_the_access_report_covers_every_watched_channel(auth, fake_bot, seeded):
    rows = auth.get("/api/access").json()
    assert {r["name"] for r in rows} == {"#hstar-party", "#xkalos-party"}
    assert all(r["watched"] for r in rows)
    assert all(r["view"] and r["send"] for r in rows)


def test_the_access_report_flags_a_missing_permission(auth, fake_bot, seeded):
    fake_bot.channels[OTHER_CHANNEL].permissions.send_messages = False
    rows = auth.get("/api/access").json()
    broken = next(r for r in rows if r["name"] == "#xkalos-party")
    assert broken["send"] is False
    assert broken["view"] is True


def test_the_digest_channel_is_included_and_marked(auth, fake_bot, seeded):
    rows = auth.get("/api/access").json()
    assert [r["is_digest_channel"] for r in rows if r["name"] == "#hstar-party"] == [True]


def wait_for_job(auth, job_id, tries: int = 60) -> dict:
    """Poll a queued rescan the way the portal and the CLI do."""
    for _ in range(tries):
        body = auth.get(f"/api/rescan/{job_id}").json()
        if not body["running"]:
            return body
    raise AssertionError("the rescan never finished")


def test_a_rescan_is_queued_not_awaited(auth, seeded):
    """It is minutes of model time; the request must come straight back."""
    response = auth.post("/api/rescan", json={})
    assert response.status_code == 202
    body = response.json()
    assert body["status"] in ("queued", "running")
    assert body["total"] == 2


def test_rescan_refuses_an_unwatched_channel(auth, seeded):
    response = auth.post("/api/rescan", json={"channels": [str(UNWATCHED_CHANNEL)]})
    assert response.status_code == 400
    assert "isn't watched" in response.json()["error"]


def test_rescan_refuses_while_paused(auth, fake_bot, seeded):
    fake_bot.repo.set_config("paused", "1")
    response = auth.post("/api/rescan", json={})
    assert response.status_code == 400
    assert "paused" in response.json()["error"]


def test_rescan_refuses_while_the_extractor_is_off(auth, fake_bot, seeded):
    fake_bot.repo.set_config("extract_enabled", "0")
    response = auth.post("/api/rescan", json={})
    assert response.status_code == 400
    assert "switched off" in response.json()["error"]


def test_no_channels_means_every_watched_one(auth, fake_bot, seeded):
    job = auth.post("/api/rescan", json={}).json()
    body = wait_for_job(auth, job["job_id"])
    assert {r["channel_name"] for r in body["results"]} == {"#hstar-party", "#xkalos-party"}
    assert [c for c, _, _ in fake_bot.extractor.calls] == [
        str(WATCHED_CHANNEL),
        str(OTHER_CHANNEL),
    ]


def test_the_channels_are_read_one_at_a_time(auth, fake_bot, seeded):
    """The model lock serialises them anyway; in order keeps each party's cards together."""
    order: list[str] = []
    original = fake_bot.extractor.rescan_window

    async def record(channel_id, window="week", post=True, automated=False, should_stop=None):
        order.append(f"start {channel_id}")
        result = await original(
            channel_id, window=window, post=post, automated=automated, should_stop=should_stop
        )
        order.append(f"end {channel_id}")
        return result

    fake_bot.extractor.rescan_window = record
    job = auth.post("/api/rescan", json={}).json()
    wait_for_job(auth, job["job_id"])
    assert order == [
        f"start {WATCHED_CHANNEL}",
        f"end {WATCHED_CHANNEL}",
        f"start {OTHER_CHANNEL}",
        f"end {OTHER_CHANNEL}",
    ]


def test_one_named_channel_is_the_only_one_read(auth, fake_bot, seeded):
    job = auth.post("/api/rescan", json={"channels": [str(OTHER_CHANNEL)]}).json()
    body = wait_for_job(auth, job["job_id"])
    assert [r["channel_name"] for r in body["results"]] == ["#xkalos-party"]


def test_a_rescan_takes_a_window(auth, fake_bot, seeded):
    job = auth.post("/api/rescan", json={"window": "2weeks"}).json()
    assert job["window"] == "2weeks"
    wait_for_job(auth, job["job_id"])
    assert {w for _, w, _ in fake_bot.extractor.calls} == {"2weeks"}


def test_an_unknown_rescan_window_is_refused(auth, seeded):
    response = auth.post("/api/rescan", json={"window": "since forever"})
    assert response.status_code == 422


def test_the_totals_add_the_channels_up(auth, fake_bot, seeded):
    fake_bot.backfill_count = 5
    job = auth.post("/api/rescan", json={}).json()
    body = wait_for_job(auth, job["job_id"])
    assert body["totals"]["backfilled"] == sum(r["backfilled"] for r in body["results"])


def test_a_rescan_can_be_stopped(auth, seeded):
    job = auth.post("/api/rescan", json={}).json()
    response = auth.delete(f"/api/rescan/{job['job_id']}")
    assert response.status_code in (200, 400)  # it may already have finished


def test_stopping_a_finished_rescan_says_so(auth, seeded):
    job = auth.post("/api/rescan", json={}).json()
    wait_for_job(auth, job["job_id"])
    response = auth.delete(f"/api/rescan/{job['job_id']}")
    assert response.status_code == 400
    assert "already" in response.json()["error"]


def test_an_unknown_job_is_a_404(auth, seeded):
    assert auth.get("/api/rescan/nope").status_code == 404


def test_recent_rescans_are_listed(auth, seeded):
    job = auth.post("/api/rescan", json={}).json()
    wait_for_job(auth, job["job_id"])
    listed = auth.get("/api/rescan").json()
    assert listed[0]["job_id"] == job["job_id"]


def test_a_job_is_recorded_in_the_database(auth, fake_bot, seeded):
    """So the portal can still show it after a restart."""
    job = auth.post("/api/rescan", json={}).json()
    wait_for_job(auth, job["job_id"])
    row = fake_bot.repo.get_rescan_job(job["job_id"])
    assert row["status"] == "done"
    assert row["window"] == "week"
    assert len(row["results"]) == 2


def test_the_rescan_targets_put_party_channels_first(auth, fake_bot, seeded):
    rows = auth.get("/api/rescan/targets").json()
    assert [r["name"] for r in rows] == ["#hstar-party", "#xkalos-party"]
    assert all(r["has_runs"] for r in rows)


def test_a_channel_with_no_timings_is_listed_last(auth, fake_bot, seeded):
    """The general channel is the noisy one; a rescan is usually about a party's."""
    fake_bot.repo.delete_fixed_run(seeded["fixed_kalos"])
    rows = auth.get("/api/rescan/targets").json()
    assert rows[-1]["name"] == "#xkalos-party"
    assert rows[-1]["has_runs"] is False


def test_debug_ping_posts_a_test_message_without_touching_the_reminders(auth, fake_bot, seeded):
    before = fake_bot.repo.list_reminders(seeded["run_star"])
    body = auth.post(
        "/api/debug/ping", json={"run_id": seeded["run_star"], "kind": "day_of"}
    ).json()
    assert body["kind"] == "day_of"
    assert fake_bot.posts[-1].content.startswith("🧪 TEST — ")
    assert fake_bot.repo.list_reminders(seeded["run_star"]) == before
    assert fake_bot.repo.debug_messages_for(body["message_id"])


def test_an_unknown_ping_kind_is_refused(auth, seeded):
    response = auth.post("/api/debug/ping", json={"run_id": seeded["run_star"], "kind": "nope"})
    assert response.status_code == 400
    assert "day_of" in response.json()["error"]


# --- message export ---------------------------------------------------------


def test_export_streams_stored_messages_as_jsonl(auth, seeded):
    response = auth.get(f"/api/messages?channel={WATCHED_CHANNEL}&since=2026-08-01")
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("application/x-ndjson")
    rows = [json.loads(line) for line in response.text.splitlines() if line.strip()]
    assert rows[0]["content"] == "can change to wed?"
    assert rows[0]["author_name"] == "kanon"
    assert rows[0]["source"] == "stored"


def test_export_refuses_an_unwatched_channel(auth, seeded):
    response = auth.get(f"/api/messages?channel={UNWATCHED_CHANNEL}&since=2026-08-01")
    assert response.status_code == 400
    assert "only watched channels" in response.json()["error"]


def test_export_needs_a_readable_since(auth, seeded):
    response = auth.get(f"/api/messages?channel={WATCHED_CHANNEL}&since=last%20tuesday")
    assert response.status_code == 400
    assert "YYYY-MM-DD" in response.json()["error"]


def test_export_refuses_a_window_that_runs_backwards(auth, seeded):
    response = auth.get(
        f"/api/messages?channel={WATCHED_CHANNEL}&since=2026-08-30&until=2026-08-01"
    )
    assert response.status_code == 400
    assert "after since" in response.json()["error"]


# --- past runs and "mine" ---------------------------------------------------


def test_the_schedule_hides_runs_that_have_already_happened(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")
    body = auth.get("/api/schedule").json()
    assert body["count"] == 1
    assert body["hidden"] == 1
    assert body["show_past"] is False
    assert all("HStar" not in r["bosses"] for r in body["runs"])


def test_show_past_brings_them_back(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "done")
    body = auth.get("/api/schedule?show_past=true").json()
    assert body["count"] == 2
    assert body["hidden"] == 0
    assert body["show_past"] is True


def test_cancelled_runs_are_hidden_by_the_same_rule(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_kalos"], "cancelled")
    assert auth.get("/api/schedule").json()["hidden"] == 1


def test_user_filter_counts_runs_you_own_as_well_as_ones_you_are_on(auth, fake_bot, seeded):
    """1001 owns the HStar timing and is on it; 1002 owns XKalos but is also on it."""
    fake_bot.repo.update_fixed_run(seeded["fixed_star"], participants=["1002"])
    fake_bot.repo.set_run_participants(seeded["run_star"], ["1002"])
    body = auth.get("/api/schedule?user=1001").json()
    assert [r["short_id"] for r in body["runs"]] == [short_id(seeded["run_star"])]
