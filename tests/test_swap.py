"""Swapping members in and out for one week (item 11).

The point of the whole feature is that it does **not** touch the fixed timing:
a stand-in for one night must not change who gets pinged next week.
"""

from __future__ import annotations

from bot import formatting
from bot.extract.pipeline import volunteers_for
from bot.extract.schema import Amendment

from .fake_bot import WATCHED_CHANNEL


def test_taking_someone_off_leaves_the_weekly_timing_alone(auth, fake_bot, seeded):
    before = fake_bot.repo.get_fixed_run(seeded["fixed_star"])["participants"]
    body = auth.patch(
        f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1002"]}
    ).json()
    assert [p["id"] for p in body["participants"]] == ["1001"]
    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"])["participants"] == before


def test_bringing_someone_in(auth, fake_bot, seeded):
    body = auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"add": ["1003"]}).json()
    assert "1003" in [p["id"] for p in body["participants"]]


def test_a_swap_is_one_out_and_one_in(auth, fake_bot, seeded):
    body = auth.patch(
        f"/api/runs/{seeded['run_star']}/participants",
        json={"remove": ["1002"], "add": ["1003"]},
    ).json()
    assert [p["id"] for p in body["participants"]] == ["1001", "1003"]


def test_the_person_who_left_has_their_answer_cleared(auth, fake_bot, seeded):
    """Their yes was about a run they are no longer on."""
    fake_bot.repo.set_rsvp(seeded["run_star"], 1002, "no")
    auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1002"]})
    assert "1002" not in fake_bot.repo.get_rsvps(seeded["run_star"])


def test_losing_the_person_who_said_no_can_settle_the_run(auth, fake_bot, seeded):
    fake_bot.repo.set_rsvp(seeded["run_star"], 1002, "no")
    fake_bot.repo.set_run_status(seeded["run_star"], "at_risk")
    body = auth.patch(
        f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1002"]}
    ).json()
    assert body["status"] == "confirmed"  # 1001 already said yes, and is now the whole party


def test_a_run_cannot_be_emptied(auth, seeded):
    response = auth.patch(
        f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1001", "1002"]}
    )
    assert response.status_code == 400
    assert "cancel it instead" in response.json()["error"]


def test_removing_someone_who_is_not_on_the_run_is_refused(auth, seeded):
    response = auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1003"]})
    assert response.status_code == 400
    assert "not on this run" in response.json()["error"]


def test_someone_without_the_bossing_role_cannot_be_added(auth, seeded):
    response = auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"add": ["1009"]})
    assert response.status_code == 400
    assert "not in the bossing role" in response.json()["error"]


def test_a_no_op_swap_changes_nothing_and_says_nothing(auth, fake_bot, seeded):
    before = len(fake_bot.posts)
    auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"add": ["1001"]})
    assert len(fake_bot.posts) == before


def test_the_channel_is_told_who_is_in_and_out(auth, fake_bot, seeded):
    auth.patch(
        f"/api/runs/{seeded['run_star']}/participants",
        json={"remove": ["1002"], "add": ["1003"]},
    )
    posted = fake_bot.posts[-1]
    assert "<@1002> out" in posted.content
    assert "<@1003> in" in posted.content
    assert "the weekly timing is unchanged" in posted.content
    assert formatting.VIA_PORTAL in posted.content
    # The person who left is told too, or they turn up anyway.
    assert "1002" in posted.mentions


def test_the_run_reports_how_it_differs_from_its_timing(auth, fake_bot, seeded):
    auth.patch(
        f"/api/runs/{seeded['run_star']}/participants",
        json={"remove": ["1002"], "add": ["1003"]},
    )
    body = auth.get(f"/api/runs/{seeded['run_star']}").json()
    change = body["roster_change"]
    assert change["changed"] is True
    assert [p["id"] for p in change["out"]] == ["1002"]
    assert [p["id"] for p in change["in"]] == ["1003"]


def test_an_unchanged_party_reports_no_difference(auth, seeded):
    assert auth.get(f"/api/runs/{seeded['run_star']}").json()["roster_change"]["changed"] is False


def test_a_run_with_no_fixed_timing_has_nothing_to_differ_from(auth, fake_bot, seeded):
    run_id = fake_bot.repo.create_run(
        seeded["week_start"], ["HFA"], seeded["week_start"], ["1001"], source="amend"
    )
    assert auth.get(f"/api/runs/{run_id}").json()["roster_change"]["changed"] is False


# --- the portal -------------------------------------------------------------


def test_the_week_row_offers_a_cross_on_each_chip_and_an_add_picker(auth, seeded):
    body = auth.get("/").text
    assert 'name="remove" value="1002"' in body
    assert 'class="chip__add"' in body
    assert 'action="/runs/' in body and "/participants" in body


def test_the_week_row_marks_a_changed_party(auth, fake_bot, seeded):
    auth.patch(f"/api/runs/{seeded['run_star']}/participants", json={"remove": ["1002"]})
    body = auth.get("/").text
    assert "this week:" in body
    assert "−kanon" in body


def test_swapping_from_the_portal(auth, fake_bot, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/participants",
        data={"remove": ["1002"], "add": ["1003"], "next": "/"},
        headers={"HX-Request": "true"},
    )
    assert response.status_code == 200
    assert fake_bot.repo.get_run(seeded["run_star"])["participants"] == ["1001", "1003"]


def test_a_swap_form_with_nobody_picked_says_so(auth, seeded):
    response = auth.post(
        f"/runs/{seeded['run_star']}/participants", data={"next": "/"}, follow_redirects=True
    )
    assert "Nobody was picked" in response.text


# --- what the chat extractor makes of it ------------------------------------


def test_the_author_of_a_later_message_is_the_volunteer():
    """ "find temp for me" then "I can take" is two messages and two people."""
    amendment = Amendment(
        kind="sub",
        participants=["1001"],
        confidence=0.9,
        evidence_message_ids=["10", "11"],
    )
    assert volunteers_for(amendment, {"10": "1001", "11": "1002"}) == ["1002"]


def test_the_person_asking_is_never_their_own_volunteer():
    amendment = Amendment(
        kind="sub", participants=["1001"], confidence=0.9, evidence_message_ids=["10"]
    )
    assert volunteers_for(amendment, {"10": "1001"}) == []


def test_a_sub_card_names_both_halves():
    amendment = {
        "kind": "sub",
        "bosses": ["HStar"],
        "participants": ["1001"],
        "payload": {"remove": ["1001"], "add": ["1002"]},
        "summary": None,
        "new_datetime": None,
        "day_ref": None,
        "time_ref": None,
    }
    _name, value = formatting.proposal_line(amendment, None, formatting.ZoneInfo("UTC"))
    assert "<@1001> out" in value
    assert "<@1002> in" in value


def test_a_sub_card_with_nobody_offering_asks_for_one():
    amendment = {
        "kind": "sub",
        "bosses": ["HStar"],
        "participants": ["1001"],
        "payload": {"remove": ["1001"], "add": []},
        "summary": None,
        "new_datetime": None,
        "day_ref": None,
        "time_ref": None,
    }
    _name, value = formatting.proposal_line(amendment, None, formatting.ZoneInfo("UTC"))
    assert "temp needed" in value


def test_approving_a_sub_applies_both_halves(repo, bosses):
    from datetime import time as clock

    from bot.extract.commit import commit

    from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl

    week = kl(2026, 8, 27, 0, 0)
    run_id = repo.create_run(week, ["HStar"], kl(2026, 8, 31, 21, 30), ["1001", "1002"])
    amendment_id = repo.create_amendment(
        week_start=week,
        kind="sub",
        run_id=run_id,
        payload={"remove": ["1002"], "add": ["1003"]},
        channel_id=WATCHED_CHANNEL,
    )
    result = commit(
        repo,
        repo.get_amendment(amendment_id),
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id="1001",
    )
    assert result.applied
    assert repo.get_run(run_id)["participants"] == ["1001", "1003"]
    assert clock  # keeps the import honest


def test_a_sub_that_would_empty_the_run_is_refused(repo):
    from bot.extract.commit import commit

    from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl

    week = kl(2026, 8, 27, 0, 0)
    run_id = repo.create_run(week, ["HStar"], kl(2026, 8, 31, 21, 30), ["1001"])
    amendment_id = repo.create_amendment(
        week_start=week, kind="sub", run_id=run_id, payload={"remove": ["1001"]}
    )
    result = commit(
        repo,
        repo.get_amendment(amendment_id),
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id="1001",
    )
    assert not result.applied
    assert "nobody on it" in result.problem


# --- the CLI ----------------------------------------------------------------


def test_the_schedule_line_shows_the_delta():
    assert formatting.roster_delta(["MY"], ["kanon"]) == "this week: −MY +kanon"
    assert formatting.roster_delta([], []) == ""
