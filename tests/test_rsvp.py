"""Reaction -> RSVP -> run status transitions (the pure logic behind the events)."""

from __future__ import annotations

import pytest

from bot.db import Repo
from bot.rsvp import EMOJI_NO, EMOJI_YES, apply_reaction, compute_status, state_for_emoji

from .conftest import kl

PARTICIPANTS = ["1", "2", "3"]
RUN_AT = kl(2026, 8, 31, 21, 30)
WEEK = kl(2026, 8, 27)


@pytest.fixture
def run(repo: Repo) -> dict:
    run_id = repo.create_run(WEEK, ["HStar", "HFA"], RUN_AT, PARTICIPANTS)
    return repo.get_run(run_id)


def test_emoji_mapping():
    assert state_for_emoji(EMOJI_YES) == "yes"
    assert state_for_emoji(EMOJI_NO) == "no"
    assert state_for_emoji("🍕") is None


# -- compute_status ----------------------------------------------------------


def test_a_run_stays_planned_until_everyone_answers():
    assert compute_status("planned", PARTICIPANTS, {}) == "planned"
    assert compute_status("planned", PARTICIPANTS, {"1": "yes"}) == "planned"


def test_everyone_yes_confirms_the_run():
    tally = {"1": "yes", "2": "yes", "3": "yes"}
    assert compute_status("planned", PARTICIPANTS, tally) == "confirmed"


def test_one_no_puts_the_run_at_risk():
    tally = {"1": "yes", "2": "no", "3": "yes"}
    assert compute_status("planned", PARTICIPANTS, tally) == "at_risk"
    assert compute_status("confirmed", PARTICIPANTS, tally) == "at_risk"


def test_rsvps_from_outsiders_are_ignored():
    tally = {"1": "yes", "2": "yes", "3": "yes", "99": "no"}
    assert compute_status("planned", PARTICIPANTS, tally) == "confirmed"


@pytest.mark.parametrize("status", ["cancelled", "otot", "done"])
def test_deliberate_statuses_are_not_overwritten_by_rsvps(status):
    tally = {"1": "yes", "2": "yes", "3": "yes"}
    assert compute_status(status, PARTICIPANTS, tally) == status


def test_a_run_with_no_participants_is_never_confirmed():
    assert compute_status("planned", [], {}) == "planned"


# -- apply_reaction ----------------------------------------------------------


def test_a_tick_records_a_yes(repo: Repo, run: dict):
    result = apply_reaction(repo, run, 1, EMOJI_YES, added=True)
    assert result.applied and result.state == "yes"
    assert repo.get_rsvps(run["id"]) == {"1": "yes"}
    assert repo.get_run(run["id"])["status"] == "planned"


def test_the_last_tick_confirms_the_run(repo: Repo, run: dict):
    for user_id in PARTICIPANTS[:-1]:
        apply_reaction(repo, repo.get_run(run["id"]), user_id, EMOJI_YES, added=True)
    assert repo.get_run(run["id"])["status"] == "planned"

    result = apply_reaction(repo, repo.get_run(run["id"]), "3", EMOJI_YES, added=True)
    assert result.new_status == "confirmed"
    assert result.status_changed
    assert repo.get_run(run["id"])["status"] == "confirmed"


def test_a_cross_puts_the_run_at_risk_and_flags_a_decline(repo: Repo, run: dict):
    result = apply_reaction(repo, run, 2, EMOJI_NO, added=True)
    assert result.declined
    assert result.new_status == "at_risk"
    assert repo.get_run(run["id"])["status"] == "at_risk"


def test_switching_from_no_to_yes_recovers_the_run(repo: Repo):
    run_id = repo.create_run(WEEK, ["HStar"], RUN_AT, ["1", "2"])
    for user_id in ("1", "2"):
        apply_reaction(repo, repo.get_run(run_id), user_id, EMOJI_YES, added=True)
    assert repo.get_run(run_id)["status"] == "confirmed"

    apply_reaction(repo, repo.get_run(run_id), "2", EMOJI_NO, added=True)
    assert repo.get_run(run_id)["status"] == "at_risk"

    apply_reaction(repo, repo.get_run(run_id), "2", EMOJI_YES, added=True)
    assert repo.get_run(run_id)["status"] == "confirmed"


def test_non_participants_are_ignored(repo: Repo, run: dict):
    result = apply_reaction(repo, run, 99, EMOJI_YES, added=True)
    assert not result.applied
    assert repo.get_rsvps(run["id"]) == {}


def test_unrelated_emoji_does_nothing(repo: Repo, run: dict):
    result = apply_reaction(repo, run, 1, "🍕", added=True)
    assert not result.applied
    assert repo.get_rsvps(run["id"]) == {}


def test_removing_a_reaction_clears_that_rsvp(repo: Repo):
    run_id = repo.create_run(WEEK, ["HStar"], RUN_AT, ["1", "2"])
    for user_id in ("1", "2"):
        apply_reaction(repo, repo.get_run(run_id), user_id, EMOJI_YES, added=True)
    assert repo.get_run(run_id)["status"] == "confirmed"

    apply_reaction(repo, repo.get_run(run_id), "2", EMOJI_YES, added=False)
    assert repo.get_rsvps(run_id) == {"1": "yes"}
    assert repo.get_run(run_id)["status"] == "planned"


def test_removing_the_old_reaction_after_switching_does_not_wipe_the_new_one(repo: Repo, run):
    # Discord sends reaction_add(✅) then reaction_remove(❌) when a user switches.
    apply_reaction(repo, repo.get_run(run["id"]), "1", EMOJI_NO, added=True)
    apply_reaction(repo, repo.get_run(run["id"]), "1", EMOJI_YES, added=True)
    result = apply_reaction(repo, repo.get_run(run["id"]), "1", EMOJI_NO, added=False)

    assert not result.applied
    assert repo.get_rsvps(run["id"]) == {"1": "yes"}


def test_reacting_on_an_otot_run_records_the_rsvp_without_changing_status(repo: Repo):
    run_id = repo.create_run(WEEK, ["HCarling"], RUN_AT, ["1"], status="otot")
    result = apply_reaction(repo, repo.get_run(run_id), "1", EMOJI_YES, added=True)
    assert result.applied
    assert repo.get_rsvps(run_id) == {"1": "yes"}
    assert repo.get_run(run_id)["status"] == "otot"
