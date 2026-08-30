"""The card bugs from the live rescan of #hstar-hfa-xkalos-alvin-priya-my.

Each test here is one thing the owner saw on a real card that should never have
been on it.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot import formatting
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
    assert "HStar" in name
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
    assert "NCarling" in name
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
    from bot.client import BossBot

    amendment = seeded["amendment"]
    assert fake_bot.repo.get_amendment(amendment)["status"] == "proposed"
    withdrawn = BossBot.withdraw_card(fake_bot, 900000000000000001)
    assert [a["id"] for a in withdrawn] == [amendment]
    assert fake_bot.repo.get_amendment(amendment)["status"] == "withdrawn"


def test_deleting_an_unrelated_message_does_nothing(fake_bot, seeded):
    from bot.client import BossBot

    assert BossBot.withdraw_card(fake_bot, 424242) == []
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "proposed"


def test_an_already_answered_card_is_left_alone(fake_bot, seeded):
    """Deleting the card afterwards must not rewrite what was decided."""
    from bot.client import BossBot

    fake_bot.repo.set_amendment_status(seeded["amendment"], "confirmed")
    assert BossBot.withdraw_card(fake_bot, 900000000000000001) == []
    assert fake_bot.repo.get_amendment(seeded["amendment"])["status"] == "confirmed"


def test_a_withdrawn_proposal_leaves_the_inbox(auth, fake_bot, seeded):
    from bot.client import BossBot

    assert len(auth.get("/api/pending").json()) == 1
    BossBot.withdraw_card(fake_bot, 900000000000000001)
    assert auth.get("/api/pending").json() == []


def test_withdrawn_is_a_real_status():
    from bot.db import AMENDMENT_STATUSES

    assert "withdrawn" in AMENDMENT_STATUSES


def test_a_stand_in_survives_beside_a_move_for_the_same_run():
    """ "ZedRS's out, X fills, and we do Wed" is one decision: both lines stay."""
    run = run_row("r1", ["HCarling"], NOW + timedelta(days=1))
    move = entry("move", ["HCarling"], run, day=(NOW + timedelta(days=1)).date())
    sub = entry("sub", ["HCarling"], run)
    kept = one_per_run([sub, move, entry("otot", ["HCarling"], run)])
    assert [e.kind for e in kept] == ["sub", "move"]
    assert kept[1].also_mentioned == ["otot"]
    assert kept[0].also_mentioned == []
