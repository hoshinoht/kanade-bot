"""Folding a burst's per-message pieces into one candidate per run (DESIGN §2b.2)."""

from __future__ import annotations

from bot.extract.merge import merge
from bot.extract.schema import Amendment


def a(
    kind,
    *,
    bosses=(),
    day=None,
    time=None,
    people=(),
    rsvp=None,
    evidence=(),
    q=False,
    conf=0.8,
    hint=None,
) -> Amendment:
    return Amendment(
        kind=kind,
        bosses=list(bosses),
        day_ref=day,
        time_ref=time,
        participants=list(people),
        rsvp=rsvp,
        is_question=q,
        confidence=conf,
        evidence_message_ids=list(evidence),
        target_run_hint=hint,
    )


ORDER = ["401", "402", "403", "404"]


def test_the_worked_example_three_messages_are_one_new_run():
    # "we doing our nstar and ncarl tonight?" -> "9pm i reach kk early" -> "amend to 9:45pm"
    merged = merge(
        [
            a("add", bosses=["NStar", "NCarling"], day="tonight", evidence=["401"], q=True),
            a("add", bosses=["NStar", "NCarling"], time="9pm", evidence=["402"]),
            a("add", bosses=["NStar", "NCarling"], time="9:45pm", evidence=["403"]),
        ],
        ORDER,
    )
    assert len(merged) == 1
    assert merged[0].day_ref == "tonight"
    assert merged[0].time_ref == "9:45pm"  # the latest value wins
    assert merged[0].evidence_message_ids == ["401", "402", "403"]
    assert merged[0].is_question is False  # the burst ended on a decision


def test_a_burst_that_ends_on_a_question_stays_a_question():
    merged = merge(
        [
            a("move", bosses=["HStar"], day="wed", evidence=["401"]),
            a("move", bosses=["HStar"], time="9:30pm", evidence=["402"], q=True),
        ],
        ORDER,
    )
    assert merged[0].is_question is True


def test_different_runs_are_never_folded_together():
    merged = merge(
        [
            a("move", bosses=["HStar", "HFA"], day="wed", evidence=["401"]),
            a("move", bosses=["HCarling", "XKalos"], day="wed", evidence=["401"]),
        ],
        ORDER,
    )
    assert len(merged) == 2


def test_a_settled_shared_time_settles_every_move_for_that_day():
    merged = merge(
        [
            a("move", bosses=["HStar"], day="wed", time="9:30pm", q=False),
            a("move", bosses=["HCarling"], day="wed", q=True),
        ]
    )
    assert [(item.time_ref, item.is_question) for item in merged] == [
        ("9:30pm", False),
        ("9:30pm", False),
    ]


def test_different_kinds_about_the_same_run_stay_apart():
    merged = merge(
        [
            a("move", bosses=["HStar"], day="wed", evidence=["401"]),
            a("cancel", bosses=["HStar"], evidence=["402"]),
        ],
        ORDER,
    )
    assert {m.kind for m in merged} == {"move", "cancel"}


def test_two_people_agreeing_are_two_answers():
    merged = merge(
        [
            a("rsvp", people=["2"], rsvp="yes", evidence=["402"]),
            a("rsvp", people=["3"], rsvp="yes", evidence=["403"]),
        ],
        ORDER,
    )
    assert len(merged) == 2


def test_one_person_changing_their_mind_is_one_answer_the_latest():
    merged = merge(
        [
            a("rsvp", people=["2"], rsvp="yes", evidence=["401"]),
            a("rsvp", people=["2"], rsvp="no", evidence=["403"]),
        ],
        ORDER,
    )
    assert len(merged) == 1 and merged[0].rsvp == "no"


def test_participants_and_bosses_are_unioned():
    merged = merge(
        [
            a("add", bosses=["NStar"], people=["1"], evidence=["401"]),
            a("add", bosses=["NCarling"], people=["2"], evidence=["402"]),
        ],
        ORDER,
    )
    # Same kind, different boss lists -> different candidates, by design.
    assert len(merged) == 2

    same = merge(
        [
            a("add", bosses=["NStar"], people=["1"], evidence=["401"]),
            a("add", bosses=["NStar"], people=["2"], evidence=["402"]),
        ],
        ORDER,
    )
    assert same[0].participants == ["1", "2"]


def test_a_null_field_never_overwrites_a_stated_one():
    merged = merge(
        [
            a("move", bosses=["HStar"], day="wed", time="9:30pm", evidence=["401"]),
            a("move", bosses=["HStar"], evidence=["403"]),
        ],
        ORDER,
    )
    assert (merged[0].day_ref, merged[0].time_ref) == ("wed", "9:30pm")


def test_the_highest_confidence_in_the_group_is_kept():
    merged = merge(
        [
            a("move", bosses=["HStar"], day="wed", conf=0.6, evidence=["401"]),
            a("move", bosses=["HStar"], time="9pm", conf=0.9, evidence=["402"]),
        ],
        ORDER,
    )
    assert merged[0].confidence == 0.9


def test_output_is_ordered_by_when_the_evidence_appeared():
    merged = merge(
        [
            a("cancel", bosses=["XKalos"], evidence=["404"]),
            a("move", bosses=["HStar"], evidence=["401"]),
        ],
        ORDER,
    )
    assert [m.kind for m in merged] == ["move", "cancel"]


def test_merging_nothing_gives_nothing():
    assert merge([], ORDER) == []


def test_merging_without_a_message_order_still_works():
    merged = merge(
        [a("move", bosses=["HStar"], day="wed"), a("move", bosses=["HStar"], time="9pm")]
    )
    assert len(merged) == 1 and merged[0].time_ref == "9pm"


def test_the_run_hint_keeps_two_same_boss_amendments_apart():
    merged = merge(
        [
            a("move", bosses=["HLimbo"], day="wed", hint="#aaaa1111", evidence=["401"]),
            a("move", bosses=["HLimbo"], day="thu", hint="#bbbb2222", evidence=["402"]),
        ],
        ORDER,
    )
    assert len(merged) == 2
