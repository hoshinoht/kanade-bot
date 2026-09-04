"""The card bugs from the live rescan of #hstar-hfa-xkalos-alvin-priya-my.

Each test here is one thing the owner saw on a real card that should never have
been on it.
"""

from __future__ import annotations

from datetime import date, time, timedelta

import pytest

from bot.agent import formatting
from bot.extract.pipeline import (
    Planned,
    consolidate,
    is_no_op,
    one_per_run,
    plan_burst,
)
from bot.extract.resolve import Resolved
from bot.extract.schema import Amendment, Extraction

from .conftest import TZ, kl

NOW = kl(2026, 8, 31, 12, 0)


def run_row(run_id, bosses, at, status="planned", participants=("1001", "1002")):
    return {
        "id": run_id,
        "bosses": list(bosses),
        "datetime": at,
        "status": status,
        "participants": list(participants),
        "channel_id": "900",
        "fixed_run_id": None,
    }


def entry(kind, bosses=(), run=None, day=None, at=None):
    return Planned(
        amendment=Amendment(kind=kind, bosses=list(bosses), confidence=0.9),
        resolved=Resolved(day=day, at=at),
        run=run,
    )


# --- 1. one amendment per run per card --------------------------------------


def test_a_move_with_a_day_beats_own_time_for_the_same_run():
    """Live: the card carried both `move HCarling -> Tue` and `own time HCarling`."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date())
    kept = one_per_run([move, entry("otot", ["HCarling"], run)])
    assert [e.kind for e in kept] == ["move"]


def test_the_loser_is_named_on_the_winning_line():
    """Nothing the thread said disappears silently."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date())
    kept = one_per_run([move, entry("otot", ["HCarling"], run)])
    assert kept[0].also_mentioned == ["otot"]


def test_a_cancel_beats_everything():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    kept = one_per_run(
        [
            entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date()),
            entry("cancel", ["HCarling"], run),
        ]
    )
    assert [e.kind for e in kept] == ["cancel"]


def test_own_time_beats_a_move_that_never_named_a_day():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    kept = one_per_run([entry("move", ["HCarling"], run), entry("otot", ["HCarling"], run)])
    assert [e.kind for e in kept] == ["otot"]


def test_changes_to_different_runs_all_survive():
    one = run_row("r1", ["HStar"], NOW + timedelta(days=1))
    two = run_row("r2", ["XKalos"], NOW + timedelta(days=2))
    kept = one_per_run([entry("otot", ["HStar"], one), entry("otot", ["XKalos"], two)])
    assert len(kept) == 2


def test_a_split_is_never_collapsed_away():
    """It changes a run *and* creates another; it is not competing with them."""
    run = run_row("r1", ["HStar", "HFA"], NOW + timedelta(days=1))
    kept = one_per_run([entry("split", ["HFA"], run), entry("otot", ["HStar"], run)])
    assert {e.kind for e in kept} == {"split", "otot"}


def test_a_new_run_is_never_collapsed_against_an_existing_one():
    kept = one_per_run([entry("add", ["NCarling"]), entry("add", ["NStar"])])
    assert len(kept) == 2


def test_the_rescan_consolidation_applies_the_same_rule():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date())
    kept = consolidate([move, entry("otot", ["HCarling"], run)])
    assert [e.kind for e in kept] == ["move"]


def test_the_card_names_what_else_was_mentioned():
    amendment = {
        "kind": "move",
        "bosses": ["HCarling"],
        "participants": [],
        "summary": None,
        "new_datetime": kl(2026, 9, 1, 23, 0),
        "day_ref": "tue",
        "time_ref": "11pm",
        "also_mentioned": ["otot"],
    }
    run = run_row("r1", ["HCarling"], NOW)
    _name, value = formatting.proposal_line(amendment, run, TZ)
    assert "(also mentioned: own time)" in value


# --- 2. never match across bosses -------------------------------------------


def test_a_move_about_bosses_nobody_runs_here_is_not_forced_onto_a_run():
    """Live: `move · NBaldrix` matched next week's HStar run on participants alone."""
    hstar = run_row("17d1e8be", ["HStar"], NOW + timedelta(days=7))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["NBaldrix"],
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar],
        author_ids={"1": "1001"},
        now=NOW,
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason.startswith("no run matched")


def test_with_a_day_and_time_it_becomes_a_new_run_instead():
    hstar = run_row("17d1e8be", ["HStar"], NOW + timedelta(days=7))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["NBaldrix"],
                    day_ref="wed",
                    time_ref="9:30pm",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar],
        author_ids={"1": "1001"},
        now=NOW,
    )
    (only,) = plan.planned
    assert only.kind == "add"
    assert only.run is None
    assert only.amendment.bosses == ["NBaldrix"]


def test_a_bare_time_change_is_never_promoted_to_a_new_run():
    """ "amend to 9:45" is settling something, not proposing a fresh night."""
    hstar = run_row("r1", ["HStar"], NOW + timedelta(days=1))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["NBaldrix"],
                    time_ref="9:45pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar],
        now=NOW,
    )
    assert plan.planned == []


# --- 3. no-op changes --------------------------------------------------------


def test_a_move_to_the_time_the_run_already_has_is_dropped():
    """Live: `HStar + HFA: Wed 02 Sep 21:30 -> Wed 02 Sep 21:30`."""
    at = kl(2026, 9, 2, 21, 30)
    run = run_row("r1", ["HStar", "HFA"], at)
    assert is_no_op(entry("move", ["HStar"], run, at=at)) is True


def test_a_move_to_a_different_time_is_not_a_no_op():
    run = run_row("r1", ["HStar"], kl(2026, 9, 2, 21, 30))
    assert is_no_op(entry("move", ["HStar"], run, at=kl(2026, 9, 2, 22, 0))) is False


@pytest.mark.parametrize("kind,status", [("otot", "otot"), ("cancel", "cancelled")])
def test_setting_the_status_a_run_already_has_is_dropped(kind, status):
    run = run_row("r1", ["HStar"], NOW + timedelta(days=1), status=status)
    assert is_no_op(entry(kind, ["HStar"], run)) is True


def test_a_day_only_move_to_the_day_the_run_is_already_on_is_dropped():
    """Live: `HCarling: Tue -> Tue`, from a rescan re-reading a settled thread."""
    run = run_row("r1", ["HCarling"], kl(2026, 9, 1, 23, 0))
    day_only = entry("move", ["HCarling"], run, day=date(2026, 9, 1))
    assert is_no_op(day_only, TZ) is True


def test_a_day_only_move_to_a_different_day_is_not_a_no_op():
    run = run_row("r1", ["HCarling"], kl(2026, 9, 1, 23, 0))
    assert is_no_op(entry("move", ["HCarling"], run, day=date(2026, 9, 2)), TZ) is False


def test_the_day_is_judged_in_the_guild_timezone():
    """A 00:20 run is 16:20 the previous day in UTC; "wed" means the local Wed."""
    run = run_row("r1", ["HCarling"], kl(2026, 9, 3, 0, 20))
    assert is_no_op(entry("move", ["HCarling"], run, day=date(2026, 9, 3)), TZ) is True
    assert is_no_op(entry("move", ["HCarling"], run, day=date(2026, 9, 2)), TZ) is False


def test_adding_a_run_the_channel_already_has_is_a_no_op():
    run = run_row("r1", ["NStar"], NOW + timedelta(days=1))
    assert is_no_op(entry("add", ["NStar"], run), TZ) is True


def test_an_add_for_a_boss_the_channel_already_runs_is_dropped(bosses):
    """The week already has that night; proposing it again is a card about nothing."""
    existing = run_row("r1", ["HStar", "HFA"], kl(2026, 9, 2, 21, 30))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="add",
                    bosses=["HStar"],
                    day_ref="thu",
                    time_ref="10pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[existing],
        now=NOW,
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason == "already scheduled"


def test_an_add_never_carries_a_run(bosses):
    """`commit` creates a run for an `add`; a run_id on one points it at a night
    it never meant to touch, and makes it supersede that night's other cards."""
    existing = run_row("r1", ["HStar", "HFA"], kl(2026, 9, 2, 21, 30))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="add",
                    bosses=["NStar"],
                    day_ref="thu",
                    time_ref="10pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                    participants=["1001"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[existing],
        author_ids={"1": "1001"},
        now=NOW,
    )
    assert [e.kind for e in plan.planned] == ["add"]
    assert plan.planned[0].run is None


def test_a_stated_add_with_neither_a_day_nor_a_time_is_dropped(bosses):
    """Live: an `HLimbo` line with nothing to schedule it by and nobody asking."""
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="add",
                    bosses=["HLimbo"],
                    confidence=0.9,
                    is_question=False,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[],
        now=NOW,
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason == "a stated add with no day or time"


def test_an_asked_add_with_no_day_or_time_still_becomes_a_question(bosses):
    """DESIGN.md §8 row 4: "wanna try trio ncarling?" is a card asking "when?".

    The same shape as the dropped one above, and the only difference that
    matters: somebody asked. The card carries it with the day and time TBD.
    """
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="add",
                    bosses=["NCarling"],
                    confidence=0.9,
                    is_question=True,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[],
        now=NOW,
    )
    assert [e.kind for e in plan.planned] == ["add"]
    assert plan.planned[0].needs_answer is True
    assert plan.planned[0].resolved.known is False


def test_an_add_with_a_day_but_no_time_still_asks(bosses):
    """A day is a schedulable anchor; the time is what the card asks for."""
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="add",
                    bosses=["HLimbo"],
                    day_ref="thu",
                    confidence=0.9,
                    is_question=True,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[],
        now=NOW,
    )
    assert [e.kind for e in plan.planned] == ["add"]
    assert plan.planned[0].needs_answer is True


def test_a_no_op_never_reaches_the_card(bosses):
    at = kl(2026, 9, 2, 21, 30)
    run = run_row("r1", ["HStar"], at)
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    day_ref="wed",
                    time_ref="9:30pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=kl(2026, 9, 1, 20, 0),
        tz=TZ,
        channel_runs=[run],
        now=kl(2026, 9, 1, 20, 0),
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason == "already scheduled"


# --- 4. the #id label belongs to the run it names ---------------------------


def test_the_heading_uses_the_matched_runs_bosses_not_the_amendments():
    """A foreign id beside another run's bosses is how you ✅ the wrong night."""
    amendment = {
        "kind": "move",
        "bosses": ["NBaldrix"],
        "participants": [],
        "summary": None,
        "new_datetime": None,
        "day_ref": "wed",
        "time_ref": None,
    }
    run = run_row("17d1e8be", ["HStar"], NOW)
    name, _value = formatting.proposal_line(amendment, run, TZ)
    assert "Hard Star" in name
    assert "NBaldrix" not in name
    assert "#17d1e8be" in name


def test_with_no_run_the_heading_uses_the_amendments_bosses():
    amendment = {
        "kind": "add",
        "bosses": ["NCarling"],
        "participants": [],
        "summary": None,
        "new_datetime": None,
        "day_ref": None,
        "time_ref": None,
    }
    name, _value = formatting.proposal_line(amendment, None, TZ)
    assert "Normal Carling" in name
    assert "#" not in name


def test_a_matched_run_always_shares_a_boss_with_its_amendment():
    """The invariant that makes the heading safe -- assert it, do not assume it."""
    hstar = run_row("r1", ["HStar", "HFA"], NOW + timedelta(days=1))
    kalos = run_row("r2", ["XKalos"], NOW + timedelta(days=2))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HFA"],
                    day_ref="wed",
                    time_ref="10pm",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar, kalos],
        author_ids={"1": "1001"},
        now=NOW,
    )
    for planned in plan.planned:
        if planned.run is not None and planned.amendment.bosses:
            assert set(planned.amendment.bosses) & set(planned.run["bosses"])


# --- 5. a deleted card retires its proposals --------------------------------


def test_deleting_a_card_withdraws_the_proposals_it_carried(fake_bot, seeded):
    from bot.agent.client import BossBot

    amendment = seeded["amendment"]
    assert fake_bot.repo.get_amendment(amendment)["status"] == "proposed"
    withdrawn = BossBot.withdraw_card(fake_bot, 900000000000000001)
    assert [a["id"] for a in withdrawn] == [amendment]
    assert fake_bot.repo.get_amendment(amendment)["status"] == "withdrawn"


def test_deleting_an_unrelated_message_does_nothing(fake_bot, seeded):
    from bot.agent.client import BossBot

    assert BossBot.withdraw_card(fake_bot, 424242) == []
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "proposed"


def test_an_already_answered_card_is_left_alone(fake_bot, seeded):
    """Deleting the card afterwards must not rewrite what was decided."""
    from bot.agent.client import BossBot

    fake_bot.repo.set_amendment_status(seeded["amendment"], "confirmed")
    assert BossBot.withdraw_card(fake_bot, 900000000000000001) == []
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "confirmed"


def test_a_withdrawn_proposal_leaves_the_inbox(auth, fake_bot, seeded):
    from bot.agent.client import BossBot

    assert len(auth.get("/api/pending").json()) == 1
    BossBot.withdraw_card(fake_bot, 900000000000000001)
    assert auth.get("/api/pending").json() == []


def test_withdrawn_is_a_real_status():
    from bot.infrastructure.db import AMENDMENT_STATUSES

    assert "withdrawn" in AMENDMENT_STATUSES


def test_a_tie_between_two_changes_goes_to_the_later_one():
    """ "wed" then "no, thu" is the group changing its mind, not repeating itself.

    Entries reach `one_per_run` in evidence order, so on a tie the second one
    said is the decision and the first is only "(also mentioned)".
    """
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    wed = entry("move", ["HCarling"], run, day=date(2026, 9, 2))
    thu = entry("move", ["HCarling"], run, day=date(2026, 9, 3))
    kept = one_per_run([wed, thu])
    assert [e.resolved.day for e in kept] == [date(2026, 9, 3)]


def test_a_tie_keeps_only_one_line_for_the_run():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    kept = one_per_run([entry("otot", ["HCarling"], run), entry("otot", ["HCarling"], run)])
    assert len(kept) == 1


def test_a_stand_in_survives_beside_a_move_for_the_same_run():
    """ "ZedRS's out, X fills, and we do Wed" is one decision: both lines stay."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date())
    sub = entry("sub", ["HCarling"], run)
    kept = one_per_run([sub, move, entry("otot", ["HCarling"], run)])
    assert [e.kind for e in kept] == ["sub", "move"]
    assert kept[1].also_mentioned == ["otot"]
    assert kept[0].also_mentioned == []


def test_a_bare_answer_is_still_recorded_when_two_runs_tie():
    """ "Can" never names a boss, so in a two-run channel it always ties.

    Dropping it broke chat RSVPs outright. It is safe to guess: an rsvp is an
    opinion, and `apply_reaction` ignores one from somebody who is not on the
    run it landed on.
    """
    hstar = run_row("r1", ["HStar", "HFA"], NOW + timedelta(days=1))
    kalos = run_row("r2", ["HCarling", "XKalos"], NOW + timedelta(days=2))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="rsvp",
                    bosses=[],
                    rsvp="yes",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar, kalos],
        author_ids={"1": "1001"},
        now=NOW,
    )
    assert [e.kind for e in plan.planned] == ["rsvp"]
    assert plan.planned[0].ambiguous is True


def test_a_coin_toss_move_is_still_dropped():
    """The kinds that change a night do not get to guess."""
    hstar = run_row("r1", ["HStar", "HFA"], NOW + timedelta(days=1))
    kalos = run_row("r2", ["HCarling", "XKalos"], NOW + timedelta(days=2))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=[],
                    day_ref="wed",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[hstar, kalos],
        author_ids={"1": "1001"},
        now=NOW,
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason.startswith("ambiguous")


# --- 6. a half-stated move keeps the run's other half -----------------------


def test_a_move_that_names_only_a_day_keeps_the_runs_time(bosses):
    """Live: three `move` rows with day_ref=wed rendered "→ TBD" on the card."""
    run = run_row("r1", ["HStar", "HFA"], kl(2026, 8, 31, 21, 30))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    day_ref="wed",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[run],
        now=NOW,
    )
    (only,) = plan.planned
    assert only.resolved.at == kl(2026, 9, 2, 21, 30)
    assert only.resolved.clock == time(21, 30)
    assert only.resolved.day == date(2026, 9, 2)


def test_a_move_that_names_only_a_time_keeps_the_runs_day(bosses):
    """ "amend to 9:45pm" about Monday's run is about Monday, not about today."""
    run = run_row("r1", ["HStar", "HFA"], kl(2026, 9, 7, 21, 30))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    time_ref="9:45pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[run],
        now=NOW,
    )
    (only,) = plan.planned
    # NOW is 31 Aug; without the run it would have resolved to 31 Aug 21:45.
    assert only.resolved.at == kl(2026, 9, 7, 21, 45)


def test_a_move_stating_both_halves_is_left_alone(bosses):
    run = run_row("r1", ["HStar", "HFA"], kl(2026, 8, 31, 21, 30))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    day_ref="wed",
                    time_ref="10pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[run],
        now=NOW,
    )
    assert plan.planned[0].resolved.at == kl(2026, 9, 2, 22, 0)


def test_only_a_move_inherits(bosses):
    """An `add` has no run to read from and keeps the question-card "when?" path."""
    from bot.extract.pipeline import inherit_from_run

    run = run_row("r1", ["HStar"], kl(2026, 8, 31, 21, 30))
    for kind in ("add", "otot", "cancel", "sub"):
        untouched = entry(kind, ["HStar"], run, day=date(2026, 9, 2))
        assert inherit_from_run(untouched, TZ).at is None, kind


def test_a_day_only_move_onto_the_runs_own_day_collapses_to_a_no_op(bosses):
    """Inheriting the time makes "move it to Tuesday" on a Tuesday run exact."""
    run = run_row("r1", ["HCarling"], kl(2026, 9, 1, 23, 0))
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HCarling"],
                    day_ref="tue",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[run],
        now=NOW,
    )
    assert plan.planned == []
    assert plan.dropped[0].match_reason == "already scheduled"


def test_a_question_move_now_names_the_night_it_asks_about():
    """It stays a question; it just stops asking "TBD?"."""
    amendment = {
        "kind": "move",
        "bosses": ["HStar", "HFA"],
        "participants": [],
        "summary": None,
        "new_datetime": kl(2026, 9, 2, 21, 30),
        "day_ref": "wed",
        "time_ref": None,
        "is_question": True,
        "also_mentioned": [],
    }
    run = run_row("r1", ["HStar", "HFA"], kl(2026, 8, 31, 21, 30))
    _name, value = formatting.proposal_line(amendment, run, TZ)
    assert "TBD" not in value
    assert "Wed 02 Sep 21:30" in value


def test_a_move_with_no_run_still_reads_TBD():
    """Nothing to inherit from means the card must still say so."""
    amendment = {
        "kind": "move",
        "bosses": ["HStar"],
        "participants": [],
        "summary": None,
        "new_datetime": None,
        "day_ref": "wed",
        "time_ref": None,
        "also_mentioned": [],
    }
    _name, value = formatting.proposal_line(amendment, None, TZ)
    assert "TBD" in value


# --- 7. a decision beats a question for the same run ------------------------


def asked(kind, bosses=(), run=None, day=None, at=None):
    """The same as `entry`, but the thread was still asking."""
    planned = entry(kind, bosses, run, day, at)
    planned.amendment.is_question = True
    return planned


def test_a_settled_decision_beats_an_unanswered_question():
    """Live: `move HCarling -> wed` was asked and pushed back on; a later burst
    settled on own time. `move-with-day` outranks `otot`, so the abandoned
    question took the run and the decision became "(also mentioned: own time)".
    """
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    question = asked("move", ["HCarling"], run, day=date(2026, 9, 2))
    decision = entry("otot", ["HCarling"], run)
    kept = one_per_run([question, decision])
    assert [e.kind for e in kept] == ["otot"]
    assert kept[0].also_mentioned == ["move"]


def test_the_decision_wins_whichever_order_it_arrives_in():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    question = asked("move", ["HCarling"], run, day=date(2026, 9, 2))
    decision = entry("otot", ["HCarling"], run)
    assert [e.kind for e in one_per_run([decision, question])] == ["otot"]


def test_two_decisions_still_follow_precedence():
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=date(2026, 9, 2))
    assert [e.kind for e in one_per_run([entry("otot", ["HCarling"], run), move])] == ["move"]


def test_two_questions_still_follow_precedence():
    """Nothing is settled either way, so the stronger kind still wins."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = asked("move", ["HCarling"], run, day=date(2026, 9, 2))
    assert [e.kind for e in one_per_run([asked("otot", ["HCarling"], run), move])] == ["move"]


def test_a_cancel_that_was_only_asked_loses_to_a_settled_move():
    """Even the top of the precedence list does not outrank being decided."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    kept = one_per_run(
        [
            asked("cancel", ["HCarling"], run),
            entry("move", ["HCarling"], run, day=date(2026, 9, 2)),
        ]
    )
    assert [e.kind for e in kept] == ["move"]


# --- 8. never reach backwards into a later boss week ------------------------


THIS_WEEK = kl(2026, 8, 27)  # Thu reset
NEXT_WEEK = kl(2026, 9, 3)


def dated_run(run_id, bosses, at, week, participants=("1001", "1002")):
    row = run_row(run_id, bosses, at, participants=participants)
    row["week_start"] = week
    return row


def test_a_move_never_drags_a_later_weeks_run_backwards(bosses):
    """Live: "move HStar+HFA to Wed" matched next week's freshly-materialised
    Monday runs and proposed pulling them back to a Wednesday before their week.
    """
    next_weeks = dated_run("17d1e8be", ["HStar", "HFA"], kl(2026, 9, 7, 21, 30), NEXT_WEEK)
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar", "HFA"],
                    day_ref="wed",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[next_weeks],
        author_ids={"1": "1001"},
        now=NOW,
    )
    assert plan.planned == []
    assert "later boss week" in plan.dropped[0].match_reason


def test_a_move_forward_past_the_reset_is_still_allowed(bosses):
    """ "shift our monday run to next friday" is a real request."""
    this_weeks = dated_run("a1a1a1a1", ["HStar"], kl(2026, 8, 31, 21, 30), THIS_WEEK)
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    day_ref="fri",
                    time_ref="10pm",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=NOW,
        tz=TZ,
        channel_runs=[this_weeks],
        now=NOW,
    )
    (only,) = plan.planned
    assert only.run["id"] == "a1a1a1a1"
    assert only.resolved.at == kl(2026, 9, 4, 22, 0)


def test_a_backwards_move_inside_one_week_is_still_allowed(bosses):
    """Wed -> Mon of the same boss week changes nothing about which week it is."""
    wednesday = dated_run("a1a1a1a1", ["HStar"], kl(2026, 9, 2, 21, 30), THIS_WEEK)
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="move",
                    bosses=["HStar"],
                    day_ref="mon",
                    confidence=0.9,
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=kl(2026, 8, 30, 12, 0),
        tz=TZ,
        channel_runs=[wednesday],
        now=kl(2026, 8, 30, 12, 0),
    )
    (only,) = plan.planned
    assert only.run["id"] == "a1a1a1a1"
    assert only.resolved.day == date(2026, 8, 31)


def test_an_otot_spanning_two_weeks_only_lands_on_this_weeks_run(bosses):
    """Live: "otot HCarling on Mon" produced a row for next week's run too."""
    mine = dated_run("5e7dccd2", ["HCarling"], kl(2026, 8, 31, 23, 0), THIS_WEEK)
    next_weeks = dated_run("ec9eaecf", ["HCarling"], kl(2026, 9, 7, 23, 0), NEXT_WEEK)
    plan = plan_burst(
        Extraction(
            amendments=[
                Amendment(
                    kind="otot",
                    bosses=["HCarling"],
                    day_ref="mon",
                    confidence=0.9,
                    participants=["1001"],
                    evidence_message_ids=["1"],
                )
            ]
        ),
        anchor=kl(2026, 8, 30, 12, 0),
        tz=TZ,
        channel_runs=[mine, next_weeks],
        author_ids={"1": "1001"},
        now=kl(2026, 8, 30, 12, 0),
    )
    assert [e.run["id"] for e in plan.planned] == ["5e7dccd2"]


def test_a_run_with_no_week_recorded_is_left_alone():
    """Hand-built run dicts say nothing about which week they belong to."""
    from bot.extract.match import reachable, starts_after

    plain = run_row("r1", ["HStar"], kl(2026, 9, 7, 21, 30))
    assert starts_after(plain, date(2026, 9, 2), TZ) is False
    assert reachable([plain], date(2026, 9, 2), TZ) == [plain]


def test_no_target_day_means_no_week_filtering():
    """A bare "change it" names no night, so it cannot be pointed at one."""
    from bot.extract.match import reachable

    later = dated_run("r1", ["HStar"], kl(2026, 9, 7, 21, 30), NEXT_WEEK)
    assert reachable([later], None, TZ) == [later]
