"""What the party sees when a change is made outside Discord (items 6 and 8).

The owner moved four runs from the portal and the channel said nothing, so
every schedule change now posts the same notice a slash command would, marked
``(via portal)`` so it is clear nobody in the channel decided it.
"""

from __future__ import annotations

from bot import formatting
from bot.api import service
from bot.ids import short_id
from bot.materialise import LIVE_STATUSES

from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

PORTAL = formatting.VIA_PORTAL


def last(fake_bot):
    assert fake_bot.posts, "nothing was posted"
    return fake_bot.posts[-1]


# --- run changes ------------------------------------------------------------


def test_moving_a_run_tells_the_home_channel(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/amend", json={"to": "2026-09-02 21:45"})
    posted = last(fake_bot)
    assert posted.channel_id == WATCHED_CHANNEL
    assert "moved" in posted.content
    assert PORTAL in posted.content
    assert set(posted.mentions) == {"1001", "1002"}


def test_cancelling_tells_the_home_channel(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/cancel")
    assert "cancelled" in last(fake_bot).content
    assert PORTAL in last(fake_bot).content


def test_own_time_now_says_so_too(auth, fake_bot, seeded):
    """It used to change the schedule silently."""
    auth.post(f"/api/runs/{seeded['run_star']}/otot")
    posted = last(fake_bot)
    assert "own-time this week" in posted.content
    assert "no countdowns" in posted.content
    assert PORTAL in posted.content


def test_restoring_says_it_is_back_on(auth, fake_bot, seeded):
    auth.post(f"/api/runs/{seeded['run_star']}/cancel")
    auth.post(f"/api/runs/{seeded['run_star']}/restore")
    assert "back on the schedule" in last(fake_bot).content


def test_confirming_says_who_it_is_for(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "confirmed"})
    assert "is confirmed for" in last(fake_bot).content


def test_marking_done_says_cleared(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "done"})
    assert "cleared" in last(fake_bot).content


def test_setting_the_status_it_already_has_posts_nothing(auth, fake_bot, seeded):
    before = len(fake_bot.posts)
    body = auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "planned"}).json()
    assert body["status"] == "planned"
    assert len(fake_bot.posts) == before


def test_an_rsvp_does_not_announce_a_status_change(auth, fake_bot, seeded):
    """Answering is not a decision about the run; the decline notice covers it."""
    auth.post(f"/api/runs/{seeded['run_star']}/rsvp", json={"user_id": "1002", "answer": "yes"})
    assert all("confirmed for" not in p.content for p in fake_bot.posts)


# --- fixed timings ----------------------------------------------------------


def test_adding_a_weekly_timing_is_announced(auth, fake_bot, seeded):
    auth.post(
        "/api/fixed",
        json={
            "bosses": "hcarling",
            "day": "wed",
            "time": "23:00",
            "participants": ["1001", "1003"],
            "channel_id": str(OTHER_CHANNEL),
        },
    )
    posted = last(fake_bot)
    assert posted.channel_id == OTHER_CHANNEL
    assert "Weekly timing added" in posted.content
    assert "HCarling" in posted.content
    assert "Wed 23:00" in posted.content
    assert PORTAL in posted.content


def test_editing_a_weekly_timing_is_announced(auth, fake_bot, seeded):
    auth.patch(f"/api/fixed/{short_id(seeded['fixed_star'])}", json={"time": "22:15"})
    assert "Weekly timing changed" in last(fake_bot).content
    assert "22:15" in last(fake_bot).content


def test_removing_a_weekly_timing_is_announced(auth, fake_bot, seeded):
    auth.delete(f"/api/fixed/{short_id(seeded['fixed_star'])}")
    posted = fake_bot.posts[-1]
    assert "Weekly timing removed" in posted.content
    assert PORTAL in posted.content


# --- the status machine -----------------------------------------------------


def reminder_kinds(fake_bot, run_id):
    return {r["kind"] for r in fake_bot.repo.list_reminders(run_id)}


def test_planned_and_confirmed_keep_both_kinds_of_ping(auth, fake_bot, seeded):
    for target in ("confirmed", "planned"):
        auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": target})
        assert reminder_kinds(fake_bot, seeded["run_star"]) == {
            "day_of",
            "countdown_60",
            "countdown_15",
        }


def test_own_time_keeps_only_the_morning_ping(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "otot"})
    assert reminder_kinds(fake_bot, seeded["run_star"]) == {"day_of"}


def test_cancelled_and_done_keep_none(auth, fake_bot, seeded):
    for target in ("cancelled", "done"):
        auth.patch(f"/api/runs/{seeded['run_kalos']}/status", json={"status": target})
        assert reminder_kinds(fake_bot, seeded["run_kalos"]) == set()


def test_coming_back_from_cancelled_rebuilds_the_pings(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "cancelled"})
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "planned"})
    assert reminder_kinds(fake_bot, seeded["run_star"]) == {
        "day_of",
        "countdown_60",
        "countdown_15",
    }


def test_confirming_keeps_the_answers_people_gave(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "confirmed"})
    assert fake_bot.repo.get_rsvps(seeded["run_star"]) == {"1001": "yes"}


def test_coming_back_from_cancelled_clears_them(auth, fake_bot, seeded):
    """Those answers were about a run that was off; ask again."""
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "cancelled"})
    auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "planned"})
    assert fake_bot.repo.get_rsvps(seeded["run_star"]) == {}


def test_at_risk_cannot_be_set_by_hand(auth, seeded):
    response = auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "at_risk"})
    assert response.status_code == 422


def test_an_invented_status_is_refused(auth, seeded):
    assert (
        auth.patch(f"/api/runs/{seeded['run_star']}/status", json={"status": "maybe"}).status_code
        == 422
    )


def test_at_risk_is_the_only_status_a_person_cannot_choose():
    """It means somebody said no; setting it by hand would invent an answer."""
    from bot.db import RUN_STATUSES

    assert set(service.SETTABLE_STATUSES) == set(RUN_STATUSES) - {"at_risk"}
    assert set(LIVE_STATUSES) - {"at_risk"} <= set(service.SETTABLE_STATUSES)


# --- the portal control -----------------------------------------------------


def test_the_week_row_offers_every_status(auth, seeded):
    row = auth.get("/").text
    for value in ("planned", "confirmed", "otot", "done", "cancelled"):
        assert f'name="status" value="{value}"' in row


def test_the_current_status_is_marked_as_current(auth, seeded):
    assert 'value="planned"\naria-current="true"' in auth.get("/").text.replace("  ", "")


def test_cancelling_from_the_control_asks_first(auth, seeded):
    assert "hx-confirm=" in auth.get("/").text


def test_setting_a_status_from_the_portal(auth, fake_bot, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/status",
        data={"status": "otot", "next": "/"},
        headers={"HX-Request": "true"},
    )
    assert response.status_code == 200
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "otot"
    assert "own time" in response.text


def test_restoring_from_the_portal(auth, fake_bot, seeded):
    fake_bot.repo.set_run_status(seeded["run_star"], "cancelled")
    auth.post(f"/runs/{seeded['run_star']}/restore", data={"next": "/"}, follow_redirects=False)
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "planned"


def test_a_hostile_next_on_a_run_action_is_ignored(auth, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/otot",
        data={"next": "https://evil.example"},
        follow_redirects=False,
    )
    assert response.headers["location"].startswith("/?")
