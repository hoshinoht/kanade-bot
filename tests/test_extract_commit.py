"""Committing a confirmed amendment: one test per kind, against a real Repo."""

from __future__ import annotations

import pytest

from bot.db import Repo
from bot.extract.commit import (
    PROPOSAL_TTL,
    commit,
    expire_stale,
    may_commit,
    reject,
    supersede,
)
from bot.materialise import DAY_OF, countdown_kind
from bot.timeutil import utcnow

from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl

WEEK = kl(2026, 8, 27)
CHANNEL = "900"
MY, ALVIN, PRIYA, KANON = "1", "2", "3", "4"


@pytest.fixture
def run(repo: Repo) -> dict:
    run_id = repo.create_run(
        WEEK,
        ["HStar", "HFA"],
        kl(2026, 8, 31, 21, 30),
        [MY, ALVIN, PRIYA],
        channel_id=CHANNEL,
    )
    return repo.get_run(run_id)


def make(repo: Repo, kind: str, **kwargs) -> dict:
    kwargs.setdefault("channel_id", CHANNEL)
    kwargs.setdefault("confidence", 0.9)
    amendment_id = repo.create_amendment(WEEK, kind, **kwargs)
    return repo.get_amendment(amendment_id)


def apply(repo: Repo, amendment: dict, actor: str = MY):
    return commit(
        repo,
        amendment,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=actor,
        channel_id=CHANNEL,
    )


def kinds_of(repo: Repo, run_id: str) -> set[str]:
    return {r["kind"] for r in repo.list_reminders(run_id)}


# ---------------------------------------------------------------------------
# who may press ✅
# ---------------------------------------------------------------------------


def test_a_participant_of_the_run_may_confirm(repo: Repo, run):
    amendment = make(repo, "move", run_id=run["id"])
    assert may_commit(amendment, run, ALVIN, has_role=True)


def test_someone_not_on_the_run_may_not(repo: Repo, run):
    amendment = make(repo, "move", run_id=run["id"])
    assert not may_commit(amendment, run, "99", has_role=True)


def test_an_admin_or_the_guild_owner_always_may(repo: Repo, run):
    amendment = make(repo, "move", run_id=run["id"])
    assert may_commit(amendment, run, "99", has_role=False, is_admin=True)
    assert may_commit(amendment, run, "99", has_role=False, is_owner=True)


def test_a_new_run_may_be_confirmed_by_anyone_it_names(repo: Repo):
    amendment = make(repo, "add", participants=[KANON])
    assert may_commit(amendment, None, KANON, has_role=True)
    assert not may_commit(amendment, None, "99", has_role=True)


def test_a_new_run_naming_nobody_may_be_confirmed_by_a_role_member(repo: Repo):
    # The card was posted in the party's own channel, so whoever bosses there counts.
    assert may_commit(make(repo, "add"), None, "99", has_role=True)


@pytest.mark.parametrize(
    ("kind", "fields"),
    [
        ("add", {"bosses": ["NStar"]}),
        ("fix", {"bosses": ["NStar"]}),
    ],
)
def test_a_card_that_names_nobody_still_needs_the_bossing_role(repo: Repo, kind, fields):
    """Without this, any account in the channel could create a weekly ping."""
    assert not may_commit(make(repo, kind, **fields), None, "99", has_role=False)


def test_a_participant_without_the_role_cannot_confirm(repo: Repo, run):
    # They were on the run when it was made, but the role has since been taken
    # off them; the roster is the authority on who may act.
    amendment = make(repo, "move", run_id=run["id"])
    assert not may_commit(amendment, run, ALVIN, has_role=False)


# ---------------------------------------------------------------------------
# move
# ---------------------------------------------------------------------------


def test_move_reschedules_the_run_and_its_reminders(repo: Repo, run):
    new_at = kl(2026, 9, 2, 21, 30)
    amendment = make(repo, "move", run_id=run["id"], new_datetime=new_at, day_ref="wed")
    result = apply(repo, amendment)

    assert result.applied and result.old_datetime == run["datetime"]
    moved = repo.get_run(run["id"])
    assert moved["datetime"] == new_at
    assert repo.get_amendment(amendment["id"])["status"] == "confirmed"
    assert kinds_of(repo, run["id"]) == {DAY_OF, countdown_kind(60), countdown_kind(15)}


def test_move_resets_the_answers_people_gave_about_the_old_slot(repo: Repo, run):
    for uid in run["participants"]:
        repo.set_rsvp(run["id"], uid, "yes")
    repo.set_run_status(run["id"], "confirmed")
    amendment = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))
    apply(repo, amendment)
    assert repo.get_run(run["id"])["status"] == "planned"


def test_move_into_next_week_updates_the_runs_week(repo: Repo, run):
    # Thursday 3 Sept is after the Thu 00:00 reset, so it is next boss week.
    amendment = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 3, 21, 30))
    apply(repo, amendment)
    assert repo.get_run(run["id"])["week_start"] == kl(2026, 9, 3)


def test_move_without_a_time_refuses_rather_than_guessing(repo: Repo, run):
    amendment = make(repo, "move", run_id=run["id"], day_ref="wed")
    result = apply(repo, amendment)
    assert not result.applied and "no new time" in result.problem
    assert repo.get_amendment(amendment["id"])["status"] == "proposed"


def test_move_of_a_deleted_run_reports_instead_of_crashing(repo: Repo):
    amendment = make(repo, "move", run_id="gone", new_datetime=kl(2026, 9, 2, 21, 30))
    result = apply(repo, amendment)
    assert not result.applied and "gone" in result.problem


# ---------------------------------------------------------------------------
# add
# ---------------------------------------------------------------------------


def test_add_creates_a_run_with_reminders(repo: Repo):
    amendment = make(
        repo,
        "add",
        bosses=["NStar", "NCarling"],
        new_datetime=kl(2026, 8, 29, 21, 45),
        participants=[KANON, ALVIN, PRIYA],
    )
    result = apply(repo, amendment)

    assert result.applied
    created = repo.get_run(result.run_id)
    assert created["bosses"] == ["NStar", "NCarling"]
    assert created["participants"] == [KANON, ALVIN, PRIYA]
    assert created["source"] == "amend"
    assert created["channel_id"] == CHANNEL
    assert DAY_OF in kinds_of(repo, created["id"])
    # The row now points at what it created, so the card can be linked back.
    assert repo.get_amendment(amendment["id"])["run_id"] == created["id"]


def test_add_falls_back_to_the_person_who_confirmed_it(repo: Repo):
    amendment = make(repo, "add", bosses=["NStar"], new_datetime=kl(2026, 8, 29, 21, 45))
    result = apply(repo, amendment, actor=KANON)
    assert repo.get_run(result.run_id)["participants"] == [KANON]


@pytest.mark.parametrize(
    ("fields", "expected"),
    [
        ({"bosses": ["NStar"]}, "no day and time"),
        ({"new_datetime": kl(2026, 8, 29, 21, 45)}, "no bosses"),
    ],
)
def test_add_refuses_when_something_is_still_tbd(repo: Repo, fields, expected):
    result = apply(repo, make(repo, "add", **fields))
    assert not result.applied and expected in result.problem


# ---------------------------------------------------------------------------
# cancel / otot
# ---------------------------------------------------------------------------


def test_cancel_marks_the_run_and_drops_its_reminders(repo: Repo, run):
    result = apply(repo, make(repo, "cancel", run_id=run["id"], bosses=run["bosses"]))
    assert result.applied
    assert repo.get_run(run["id"])["status"] == "cancelled"
    assert repo.list_reminders(run["id"]) == []


def test_otot_keeps_the_morning_ping_and_drops_the_countdowns(repo: Repo, run):
    result = apply(repo, make(repo, "otot", run_id=run["id"], bosses=["HStar"]))
    assert result.applied
    assert repo.get_run(run["id"])["status"] == "otot"
    assert kinds_of(repo, run["id"]) == {DAY_OF}


# ---------------------------------------------------------------------------
# sub
# ---------------------------------------------------------------------------


def test_sub_drops_the_person_who_is_out(repo: Repo, run):
    repo.set_rsvp(run["id"], MY, "yes")
    amendment = make(repo, "sub", run_id=run["id"], participants=[MY], payload={"remove": [MY]})
    result = apply(repo, amendment)

    assert result.applied
    assert repo.get_run(run["id"])["participants"] == [ALVIN, PRIYA]
    # Their answer goes with them, so the run is not "confirmed" on their behalf.
    assert MY not in repo.get_rsvps(run["id"])


def test_sub_can_bring_someone_in(repo: Repo, run):
    amendment = make(repo, "sub", run_id=run["id"], payload={"remove": [MY], "add": [KANON]})
    apply(repo, amendment)
    assert repo.get_run(run["id"])["participants"] == [ALVIN, PRIYA, KANON]


def test_sub_with_nobody_named_refuses(repo: Repo, run):
    result = apply(repo, make(repo, "sub", run_id=run["id"]))
    assert not result.applied and "nobody to swap" in result.problem


# ---------------------------------------------------------------------------
# split
# ---------------------------------------------------------------------------


def test_split_shrinks_the_run_and_creates_a_second_one(repo: Repo, run):
    amendment = make(
        repo,
        "split",
        run_id=run["id"],
        bosses=["HFA"],
        new_datetime=kl(2026, 9, 2, 21, 30),
        payload={"bosses": ["HFA"], "participants": [ALVIN, PRIYA]},
    )
    result = apply(repo, amendment)

    assert result.applied
    assert repo.get_run(run["id"])["bosses"] == ["HStar"]
    (split_id,) = result.created_run_ids
    split = repo.get_run(split_id)
    assert split["bosses"] == ["HFA"]
    assert split["participants"] == [ALVIN, PRIYA]
    assert split["datetime"] == kl(2026, 9, 2, 21, 30)
    assert split["channel_id"] == CHANNEL


def test_split_keeps_the_original_time_when_only_the_people_change(repo: Repo, run):
    amendment = make(
        repo, "split", run_id=run["id"], payload={"bosses": ["HFA"], "participants": [PRIYA]}
    )
    result = apply(repo, amendment)
    assert repo.get_run(result.created_run_ids[0])["datetime"] == run["datetime"]


def test_splitting_off_every_boss_is_really_a_move(repo: Repo, run):
    amendment = make(
        repo,
        "split",
        run_id=run["id"],
        new_datetime=kl(2026, 9, 2, 21, 30),
        payload={"bosses": ["HStar", "HFA"]},
    )
    result = apply(repo, amendment)
    assert result.applied and not result.created_run_ids
    assert repo.get_run(run["id"])["datetime"] == kl(2026, 9, 2, 21, 30)


def test_split_naming_no_boss_of_that_run_refuses(repo: Repo, run):
    result = apply(repo, make(repo, "split", run_id=run["id"], payload={"bosses": ["XKalos"]}))
    assert not result.applied and "no bosses from that run" in result.problem


# ---------------------------------------------------------------------------
# fix
# ---------------------------------------------------------------------------


def test_fix_creates_a_fixed_run_owned_by_whoever_confirmed_it(repo: Repo):
    materialised = []
    amendment = make(
        repo,
        "fix",
        bosses=["HLimbo", "NBaldrix"],
        participants=[KANON, ALVIN],
        payload={"weekday": 1, "time": "22:30"},
    )
    result = commit(
        repo,
        amendment,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=ALVIN,
        channel_id=CHANNEL,
        on_fixed_created=materialised.append,
    )

    assert result.applied
    fixed = repo.get_fixed_run(result.fixed_run_id)
    assert fixed["bosses"] == ["HLimbo", "NBaldrix"]
    assert (fixed["weekday"], fixed["time"]) == (1, "22:30")
    assert fixed["owner_id"] == ALVIN
    assert fixed["participants"] == [KANON, ALVIN]
    assert fixed["channel_id"] == CHANNEL
    # The caller is told to re-materialise, which is what turns it into runs.
    assert materialised == [result.fixed_run_id]


@pytest.mark.parametrize(
    ("payload", "bosses", "expected"),
    [
        ({}, ["HLimbo"], "no recurring day"),
        ({"weekday": 1, "time": "22:30"}, [], "no bosses"),
    ],
)
def test_fix_refuses_a_half_finished_timing(repo: Repo, payload, bosses, expected):
    result = apply(repo, make(repo, "fix", bosses=bosses, payload=payload))
    assert not result.applied and expected in result.problem


# ---------------------------------------------------------------------------
# reject / expire
# ---------------------------------------------------------------------------


def test_rejecting_leaves_the_schedule_alone(repo: Repo, run):
    amendment = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))
    reject(repo, amendment)
    assert repo.get_amendment(amendment["id"])["status"] == "rejected"
    assert repo.get_run(run["id"])["datetime"] == run["datetime"]


def test_unanswered_proposals_expire(repo: Repo, run):
    fresh = make(repo, "move", run_id=run["id"])
    stale = make(repo, "cancel", run_id=run["id"])
    repo._conn.execute(
        "UPDATE amendments SET created_at = ? WHERE id = ?",
        ("2026-08-01T00:00:00+00:00", stale["id"]),
    )

    expired = expire_stale(repo, utcnow())
    assert [a["id"] for a in expired] == [stale["id"]]
    assert repo.get_amendment(stale["id"])["status"] == "expired"
    assert repo.get_amendment(fresh["id"])["status"] == "proposed"


def test_a_proposal_survives_right_up_to_the_ttl(repo: Repo, run):
    make(repo, "move", run_id=run["id"])
    assert expire_stale(repo, utcnow() + PROPOSAL_TTL / 2) == []


# ---------------------------------------------------------------------------
# superseding
# ---------------------------------------------------------------------------


def test_committing_one_amendment_retires_the_other_cards_for_that_run(repo: Repo, run):
    stale = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 4, 21, 30))
    repo.set_amendment_proposal_message(stale["id"], 7001)
    winner = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))

    result = apply(repo, winner)

    assert result.applied
    assert repo.get_amendment(stale["id"])["status"] == "superseded"
    # The caller is handed the card to annotate.
    assert [a["id"] for a in result.superseded] == [stale["id"]]
    assert result.superseded[0]["proposal_message_id"] == "7001"


def test_a_committed_add_retires_the_other_proposals_for_the_same_new_run(repo: Repo):
    stale = make(repo, "add", bosses=["NStar"], new_datetime=kl(2026, 8, 29, 21, 0))
    winner = make(repo, "add", bosses=["NStar"], new_datetime=kl(2026, 8, 29, 21, 45))

    assert apply(repo, winner).applied
    assert repo.get_amendment(stale["id"])["status"] == "superseded"


def test_superseding_never_touches_another_run(repo: Repo, run):
    other = repo.create_run(WEEK, ["XKalos"], kl(2026, 9, 1, 23, 0), [MY], channel_id=CHANNEL)
    untouched = make(repo, "move", run_id=other, new_datetime=kl(2026, 9, 3, 23, 0))
    winner = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))

    apply(repo, winner)
    assert repo.get_amendment(untouched["id"])["status"] == "proposed"


def test_superseding_leaves_settled_amendments_alone(repo: Repo, run):
    already = make(repo, "cancel", run_id=run["id"])
    reject(repo, already)
    winner = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))

    apply(repo, winner)
    assert repo.get_amendment(already["id"])["status"] == "rejected"


def test_a_refused_commit_supersedes_nothing(repo: Repo, run):
    other = make(repo, "cancel", run_id=run["id"])
    # No new time, so the move cannot apply.
    assert not apply(repo, make(repo, "move", run_id=run["id"])).applied
    assert repo.get_amendment(other["id"])["status"] == "proposed"


def test_supersede_can_be_called_directly_for_a_run(repo: Repo, run):
    first = make(repo, "move", run_id=run["id"])
    second = make(repo, "cancel", run_id=run["id"])

    retired = supersede(repo, run_id=run["id"], keep_id=second["id"])

    assert [a["id"] for a in retired] == [first["id"]]
    assert repo.get_amendment(second["id"])["status"] == "proposed"


def test_supersede_matches_a_new_run_on_its_exact_boss_set(repo: Repo):
    nstar = make(repo, "add", bosses=["NStar"], channel_id=CHANNEL)
    pair = make(repo, "add", bosses=["NStar", "NCarling"], channel_id=CHANNEL)
    elsewhere = make(repo, "add", bosses=["NStar"], channel_id="901")

    retired = supersede(repo, channel_id=CHANNEL, bosses=["NStar"])

    assert [a["id"] for a in retired] == [nstar["id"]]
    assert repo.get_amendment(pair["id"])["status"] == "proposed"
    assert repo.get_amendment(elsewhere["id"])["status"] == "proposed"


def test_a_card_in_another_channel_cannot_retire_a_partys_own_cards(repo: Repo, run):
    """The retire-across-channels a chat card would otherwise perform.

    The run lives in ``CHANNEL`` and so does the party's live card. A row raised
    somewhere else -- the chatbot posts its card in the channel the question came
    from, whichever channel the run lives in -- must not take that card out from
    under them, in a channel that never saw either the request or the card.
    """
    theirs = make(repo, "cancel", run_id=run["id"], channel_id=CHANNEL)
    intruder = make(
        repo, "move", run_id=run["id"], channel_id="901", new_datetime=kl(2026, 9, 2, 21, 30)
    )

    result = commit(
        repo,
        intruder,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=MY,
        channel_id="901",
    )

    assert result.applied
    assert result.superseded == []
    assert repo.get_amendment(theirs["id"])["status"] == "proposed"


def test_a_card_may_still_retire_the_ones_beside_it_in_its_own_channel(repo: Repo, run):
    """Scoped, not disabled: the deduplication this exists for is same-channel."""
    stale = make(repo, "cancel", run_id=run["id"], channel_id="901")
    winner = make(
        repo, "move", run_id=run["id"], channel_id="901", new_datetime=kl(2026, 9, 2, 21, 30)
    )

    result = commit(
        repo,
        winner,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=MY,
        channel_id="901",
    )

    assert [a["id"] for a in result.superseded] == [stale["id"]]


def test_supersede_names_no_channel_and_retires_everything_for_the_run(repo: Repo, run):
    """The old, unscoped behaviour is what a caller with no channel still gets."""
    first = make(repo, "cancel", run_id=run["id"], channel_id="901")
    second = make(repo, "cancel", run_id=run["id"], channel_id=CHANNEL)

    retired = supersede(repo, run_id=run["id"])
    assert {a["id"] for a in retired} == {first["id"], second["id"]}


def test_supersede_with_nothing_to_key_on_does_nothing(repo: Repo):
    make(repo, "add", bosses=["NStar"], channel_id=CHANNEL)
    assert supersede(repo) == []


def test_an_unknown_kind_is_refused_not_applied(repo: Repo, run):
    amendment = make(repo, "teleport", run_id=run["id"])
    result = apply(repo, amendment)
    assert not result.applied and "don't know how" in result.problem


def test_a_day_only_move_now_commits_instead_of_refusing(repo: Repo, run):
    """The payoff of reading the unsaid half off the run (DESIGN §2b.1).

    "can change to wed?" used to reach `_record` with no `new_datetime`, so the
    row was written, the card said "-> TBD", and ✅ answered "no new time was
    agreed - use /amend". The pipeline now fills the time in from the run, and
    the same press moves the night.
    """
    amendment = make(repo, "move", run_id=run["id"], new_datetime=kl(2026, 9, 2, 21, 30))
    result = apply(repo, amendment)
    assert result.applied and result.problem is None
    assert repo.get_run(run["id"])["datetime"] == kl(2026, 9, 2, 21, 30)


def test_a_move_with_no_datetime_still_refuses(repo: Repo, run):
    """Unchanged for the cases that genuinely have nothing to apply."""
    result = apply(repo, make(repo, "move", run_id=run["id"]))
    assert not result.applied
    assert "no new time was agreed" in result.problem


def test_a_sub_with_nobody_joining_just_removes_the_person(repo: Repo, run):
    """The default proposal is the weekly "-1", the same as `/swap out:`."""
    amendment = make(
        repo, "sub", run_id=run["id"], participants=[ALVIN], payload={"remove": [ALVIN], "add": []}
    )
    result = apply(repo, amendment)
    assert result.applied and result.problem is None
    assert repo.get_run(run["id"])["participants"] == [MY, PRIYA]


def test_removing_someone_clears_the_rsvp_they_left(repo: Repo, run):
    from bot.rsvp import EMOJI_YES, apply_reaction

    apply_reaction(repo, run, ALVIN, EMOJI_YES, added=True)
    assert ALVIN in repo.get_rsvps(run["id"])
    apply(
        repo,
        make(
            repo,
            "sub",
            run_id=run["id"],
            participants=[ALVIN],
            payload={"remove": [ALVIN], "add": []},
        ),
    )
    assert ALVIN not in repo.get_rsvps(run["id"])


def test_removing_the_last_unanswered_person_settles_the_run(repo: Repo, run):
    """The roster-change recompute runs on a plain removal too."""
    from bot.rsvp import EMOJI_YES, apply_reaction

    for uid in (MY, PRIYA):
        apply_reaction(repo, repo.get_run(run["id"]), uid, EMOJI_YES, added=True)
    assert repo.get_run(run["id"])["status"] == "planned"
    apply(
        repo,
        make(
            repo,
            "sub",
            run_id=run["id"],
            participants=[ALVIN],
            payload={"remove": [ALVIN], "add": []},
        ),
    )
    assert repo.get_run(run["id"])["status"] == "confirmed"


def test_a_sub_that_removes_everybody_is_refused(repo: Repo):
    run_id = repo.create_run(WEEK, ["HStar"], kl(2026, 8, 31, 21, 30), [MY], channel_id=CHANNEL)
    amendment = make(
        repo, "sub", run_id=run_id, participants=[MY], payload={"remove": [MY], "add": []}
    )
    result = apply(repo, amendment)
    assert not result.applied
    assert "nobody on it" in result.problem
