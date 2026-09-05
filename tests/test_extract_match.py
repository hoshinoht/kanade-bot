"""Matching an extracted amendment to the run it is about (bosses ∩ participants)."""

from __future__ import annotations

import pytest

from bot.domain.ids import short_id
from bot.extract.match import match_run, needs_run
from bot.extract.schema import Amendment
from bot.infrastructure.db import Repo

from .conftest import kl

WEEK = kl(2026, 8, 27)
MY, ALVIN, PRIYA, KANON, ZEDRS, NOVA = "1", "2", "3", "4", "5", "6"
PARTY_CHANNEL = "900"
OTHER_CHANNEL = "901"
GENERAL = "902"


@pytest.fixture
def runs(repo: Repo) -> dict[str, dict]:
    """Two runs in one party channel, plus somebody else's run elsewhere."""
    made = {
        "hstar": repo.create_run(
            WEEK,
            ["HMaleficStar", "HFA"],
            kl(2026, 8, 31, 21, 30),
            [MY, ALVIN, PRIYA],
            channel_id=PARTY_CHANNEL,
        ),
        "xkalos": repo.create_run(
            WEEK,
            ["HCarling", "XKalos"],
            kl(2026, 9, 1, 23, 0),
            [MY, PRIYA, KANON, ALVIN, ZEDRS],
            channel_id=PARTY_CHANNEL,
        ),
        "nstar": repo.create_run(
            WEEK,
            ["NMaleficStar"],
            kl(2026, 9, 2, 22, 0),
            [KANON, NOVA],
            channel_id=OTHER_CHANNEL,
        ),
    }
    return {key: repo.get_run(rid) for key, rid in made.items()}


def channel_runs(runs, channel_id):
    return [r for r in runs.values() if r["channel_id"] == channel_id]


def amendment(**kwargs) -> Amendment:
    kwargs.setdefault("kind", "move")
    return Amendment(**kwargs)


# ---------------------------------------------------------------------------


def test_named_bosses_pick_the_run_in_this_channel(runs):
    result = match_run(
        amendment(bosses=["HMaleficStar", "HFA"]), channel_runs(runs, PARTY_CHANNEL), author_id=MY
    )
    assert result.run["id"] == runs["hstar"]["id"]


def test_a_partial_boss_overlap_still_matches(runs):
    result = match_run(amendment(bosses=["XKalos"]), channel_runs(runs, PARTY_CHANNEL))
    assert result.run["id"] == runs["xkalos"]["id"]


def test_another_channels_run_is_never_matched(runs):
    # "we doing our nstar tonight" said in the HMaleficStar channel is not kanon's NMaleficStar run.
    result = match_run(
        amendment(kind="add", bosses=["NMaleficStar"]),
        channel_runs(runs, PARTY_CHANNEL),
        author_id=MY,
    )
    assert result.run is None
    assert "bosses" in result.reason


def test_no_bosses_named_and_one_run_in_the_channel_is_unambiguous(runs):
    only = [runs["hstar"]]
    result = match_run(amendment(day_ref="wed"), only, author_id=MY)
    assert result.run["id"] == runs["hstar"]["id"]
    assert result.reason == "the only run in this channel"


def test_no_bosses_and_several_runs_falls_back_to_participants(runs):
    # ZedRS is only on the XKalos/HCarling run.
    result = match_run(amendment(day_ref="wed"), channel_runs(runs, PARTY_CHANNEL), author_id=ZEDRS)
    assert result.run["id"] == runs["xkalos"]["id"]


def test_no_bosses_and_no_participant_overlap_matches_nothing(runs):
    result = match_run(amendment(day_ref="wed"), channel_runs(runs, PARTY_CHANNEL), author_id="99")
    assert result.run is None
    assert "participant overlap" in result.reason


def test_participants_break_a_boss_tie(repo: Repo):
    """Two runs of the same boss in one channel: the author's own run wins."""
    a = repo.get_run(
        repo.create_run(WEEK, ["HLimbo"], kl(2026, 9, 1, 22, 0), [MY, ALVIN], channel_id=GENERAL)
    )
    b = repo.get_run(
        repo.create_run(WEEK, ["HLimbo"], kl(2026, 9, 3, 22, 0), [KANON, NOVA], channel_id=GENERAL)
    )
    result = match_run(amendment(bosses=["HLimbo"]), [a, b], author_id=KANON)
    assert result.run["id"] == b["id"]
    assert not result.ambiguous


def test_an_unbreakable_tie_is_reported_as_ambiguous(repo: Repo):
    a = repo.get_run(
        repo.create_run(WEEK, ["HLimbo"], kl(2026, 9, 1, 22, 0), [MY], channel_id=GENERAL)
    )
    b = repo.get_run(
        repo.create_run(WEEK, ["HLimbo"], kl(2026, 9, 3, 22, 0), [ALVIN], channel_id=GENERAL)
    )
    result = match_run(amendment(bosses=["HLimbo"]), [a, b], author_id="99")
    assert result.ambiguous and result.run is not None


def test_a_channel_with_no_runs_falls_back_to_the_guild(runs):
    result = match_run(
        amendment(bosses=["NMaleficStar"]),
        channel_runs(runs, GENERAL),
        guild_runs=list(runs.values()),
        author_id=KANON,
    )
    assert result.run["id"] == runs["nstar"]["id"]
    assert "guild-wide" in result.reason


def test_guild_wide_matching_needs_someone_it_recognises(runs):
    result = match_run(amendment(day_ref="wed"), [], guild_runs=list(runs.values()), author_id="99")
    assert result.run is None


def test_the_models_run_hint_wins_when_it_is_real(runs):
    """A hint the bosses agree with is followed."""
    hint = short_id(runs["xkalos"]["id"])
    result = match_run(
        amendment(bosses=["XKalos"], target_run_hint=f"#{hint}"),
        channel_runs(runs, PARTY_CHANNEL),
    )
    assert result.run["id"] == runs["xkalos"]["id"]


def test_a_hint_at_a_run_with_none_of_those_bosses_is_refused(runs):
    """The model will happily point a move about one boss at another's run.

    Live: `move · NBaldrix` was hinted at next week's HMaleficStar run and followed,
    which renamed somebody else's night. A hint is a hint, not an override.
    """
    hint = short_id(runs["xkalos"]["id"])
    result = match_run(
        amendment(bosses=["HMaleficStar"], target_run_hint=f"#{hint}"),
        channel_runs(runs, PARTY_CHANNEL),
    )
    assert result.run["id"] == runs["hstar"]["id"], "it falls back to scoring, which is right"


def test_a_hint_is_still_followed_when_no_bosses_are_named(runs):
    """ "change it to wed" plus a hint is exactly what the hint is for."""
    hint = short_id(runs["xkalos"]["id"])
    result = match_run(amendment(target_run_hint=f"#{hint}"), channel_runs(runs, PARTY_CHANNEL))
    assert result.run["id"] == runs["xkalos"]["id"]
    assert "pointed at" in result.reason


def test_a_made_up_run_hint_is_ignored(runs):
    result = match_run(
        amendment(bosses=["HMaleficStar"], target_run_hint="#deadbeef"),
        channel_runs(runs, PARTY_CHANNEL),
    )
    assert result.run["id"] == runs["hstar"]["id"]


def test_cancelled_runs_are_never_matched(repo: Repo, runs):
    repo.set_run_status(runs["hstar"]["id"], "cancelled")
    fresh = [repo.get_run(r["id"]) for r in channel_runs(runs, PARTY_CHANNEL)]
    result = match_run(amendment(bosses=["HMaleficStar"]), fresh)
    assert result.run is None


def test_with_no_runs_at_all_nothing_matches():
    assert match_run(amendment(bosses=["HMaleficStar"]), [], []).run is None


@pytest.mark.parametrize("kind", ["move", "cancel", "otot", "split", "sub", "rsvp"])
def test_these_kinds_are_meaningless_without_a_run(kind):
    assert needs_run(kind)


@pytest.mark.parametrize("kind", ["add", "fix"])
def test_add_and_fix_can_stand_alone(kind):
    assert not needs_run(kind)
