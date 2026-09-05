"""The weekly digest card, and the pieces phase 3 added to the client."""

from __future__ import annotations

import asyncio

import pytest

from bot.agent import formatting
from bot.agent.client import CFG_LAST_DIGEST, BossBot
from bot.agent.materialise import materialise_week
from bot.domain.ids import short_id
from bot.domain.weeks import current_week_start

from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl
from .fake_bot import WATCHED_CHANNEL, FakeBot, make_settings


def _week(repo):
    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, kl(2026, 8, 30, 12, 0))
    repo.add_fixed_run(
        1, ["HMaleficStar", "HFA"], 0, "21:30", ["1", "2"], channel_id=WATCHED_CHANNEL
    )
    repo.add_fixed_run(2, ["XKalos"], 1, "23:00", ["2", "3"], channel_id=WATCHED_CHANNEL)
    # As of the reset, not of whenever the suite runs: `materialise_week` skips
    # a slot that has already passed, so against the wall clock this fixture
    # quietly lost its Monday run every evening after 21:30 and the digest had
    # nothing to group. Same fix as `conftest.seeded`.
    materialise_week(repo, ws, TZ, PING_TIME, COUNTDOWNS, now=ws)
    return ws


def test_an_empty_week_invites_the_first_timing(repo):
    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    card = formatting.digest_card([], ws, TZ, {})
    assert "Boss week of" in card.content
    assert "/fixed add" in card.description
    assert card.fields == []


def test_the_digest_groups_by_day_and_counts_what_is_unsettled(repo):
    ws = _week(repo)
    runs = repo.list_runs(week_start=ws)
    card = formatting.digest_card(runs, ws, TZ, {r["id"]: repo.get_rsvps(r["id"]) for r in runs})
    assert [name for name, _ in card.fields] == ["Mon 31 Aug", "Tue 01 Sep"]
    assert "2 run(s) across 2 day(s)" in card.description
    assert "**2** still unconfirmed" in card.description


def test_a_confirmed_run_is_not_flagged(repo):
    ws = _week(repo)
    runs = repo.list_runs(week_start=ws)
    for run in runs:
        repo.set_run_status(run["id"], "confirmed")
    runs = repo.list_runs(week_start=ws)
    card = formatting.digest_card(runs, ws, TZ, {})
    assert "unconfirmed" not in card.description
    assert "⚠️" not in "".join(value for _, value in card.fields)


def test_the_digest_names_people_rather_than_mentioning_them(repo):
    """A guild-wide post must not notify thirty bossers about every party's run."""
    ws = _week(repo)
    runs = repo.list_runs(week_start=ws)
    body = card_text(formatting.digest_card(runs, ws, TZ, {}))
    assert "<@" not in body


def test_a_cancelled_run_is_left_out(repo):
    ws = _week(repo)
    runs = repo.list_runs(week_start=ws)
    repo.set_run_status(runs[0]["id"], "cancelled")
    card = formatting.digest_card(repo.list_runs(week_start=ws), ws, TZ, {})
    assert len(card.fields) == 1


def test_an_own_time_run_reads_as_own_time(repo):
    ws = _week(repo)
    run = repo.list_runs(week_start=ws)[0]
    repo.set_run_status(run["id"], "otot")
    card = formatting.digest_card(repo.list_runs(week_start=ws), ws, TZ, {})
    assert "own time" in card_text(card)


def test_each_line_carries_the_id_you_would_type_back(repo):
    ws = _week(repo)
    runs = repo.list_runs(week_start=ws)
    body = card_text(formatting.digest_card(runs, ws, TZ, {}))
    for run in runs:
        assert short_id(run["id"]) in body


def card_text(card) -> str:
    return "\n".join([card.content, card.description or "", *(v for _, v in card.fields)])


# --- at-risk runs read differently from merely-unanswered ones --------------


def _statuses(repo, ws, *statuses: str) -> list[dict]:
    for run, status in zip(repo.list_runs(week_start=ws), statuses, strict=False):
        repo.set_run_status(run["id"], status)
    return repo.list_runs(week_start=ws)


def test_a_run_someone_declined_is_marked_apart_from_one_nobody_answered(repo):
    """Both are unconfirmed, but only one of them has a night to re-plan."""
    ws = _week(repo)
    runs = _statuses(repo, ws, "at_risk", "planned")
    lines = {run["bosses"][0]: formatting.digest_line(run, TZ, {}) for run in runs}

    assert lines["HMaleficStar"].startswith("❗")
    assert lines["XKalos"].startswith("⚠️")


def test_the_summary_counts_the_at_risk_runs_separately(repo):
    ws = _week(repo)
    runs = _statuses(repo, ws, "at_risk", "planned")
    description = formatting.digest_card(runs, ws, TZ, {}).description

    # Still counted among the unconfirmed -- an at-risk run is not settled --
    # but called out, because it is the one somebody has to act on.
    assert "**2** still unconfirmed" in description
    assert "**1** at risk" in description


def test_a_week_with_nothing_at_risk_says_nothing_about_it(repo):
    ws = _week(repo)
    card = formatting.digest_card(repo.list_runs(week_start=ws), ws, TZ, {})
    assert "at risk" not in card.description


# --- posting it at the reset (DESIGN.md §3) --------------------------------

SUNDAY = kl(2026, 8, 30, 12, 0)  # inside the boss week starting Thu 27 Aug
RESET = kl(2026, 9, 3, 0, 1)  # a minute after the next reset
LATER = kl(2026, 9, 6, 10, 0)  # the Sunday after it


def digest(bot, now) -> object:
    """One tick's worth of the reset digest."""
    return asyncio.run(BossBot.post_week_digest(bot, now))


def test_the_first_tick_on_a_new_database_starts_the_clock_rather_than_posting(fake_bot):
    """The bot was not there for this week's reset, so it has no reset to report."""
    assert digest(fake_bot, SUNDAY) is None
    assert fake_bot.digests == []
    assert fake_bot.repo.get_config(CFG_LAST_DIGEST) is not None


def test_the_reset_posts_the_digest(fake_bot):
    digest(fake_bot, SUNDAY)
    assert digest(fake_bot, RESET) is not None
    assert len(fake_bot.digests) == 1


def test_the_rest_of_the_week_posts_nothing_more(fake_bot):
    """The tick runs every 30 s; the digest is a once-a-week post."""
    digest(fake_bot, SUNDAY)
    digest(fake_bot, RESET)
    for now in (kl(2026, 9, 3, 0, 2), kl(2026, 9, 4, 9, 0), LATER):
        assert digest(fake_bot, now) is None
    assert len(fake_bot.digests) == 1


def test_a_host_that_slept_through_the_reset_posts_exactly_one_digest(fake_bot):
    """The Mac was shut on Thursday midnight and opened on Sunday: one digest,
    for the week that is running -- not zero, and not one per missed tick."""
    digest(fake_bot, SUNDAY)
    assert digest(fake_bot, LATER) is not None
    assert digest(fake_bot, LATER) is None
    assert len(fake_bot.digests) == 1


def test_a_restart_between_the_reset_and_the_first_tick_still_posts(fake_bot):
    """`materialise_weeks` runs on every `on_ready` and stamps the *materialised*
    week, so sharing that key would swallow the digest of any week the bot
    restarted into."""
    from bot.agent.client import CFG_LAST_WEEK

    digest(fake_bot, SUNDAY)
    fake_bot.repo.set_config(CFG_LAST_WEEK, "whatever on_ready wrote")
    fake_bot.materialise_weeks()

    assert digest(fake_bot, RESET) is not None


def test_a_failed_post_is_tried_again_on_the_next_tick(fake_bot):
    digest(fake_bot, SUNDAY)
    fake_bot.digest_fails = True
    assert digest(fake_bot, RESET) is None

    fake_bot.digest_fails = False
    assert digest(fake_bot, kl(2026, 9, 3, 0, 2)) is not None
    assert len(fake_bot.digests) == 1


@pytest.fixture
def quiet_guild(repo, bosses):
    """A guild with no `POST_CHANNEL_ID`: there is nowhere guild-wide to post."""
    return FakeBot(repo, bosses, make_settings(post_channel_id=None))


def test_nothing_is_posted_when_there_is_no_digest_channel(quiet_guild):
    digest(quiet_guild, SUNDAY)
    assert digest(quiet_guild, RESET) is None
    assert digest(quiet_guild, LATER) is None
    assert quiet_guild.digests == []


def test_setting_the_channel_later_does_not_back_post_a_half_finished_week(
    quiet_guild, repo, bosses
):
    """The weeks that passed unpostable are gone; the next reset is the first one."""
    digest(quiet_guild, SUNDAY)
    digest(quiet_guild, RESET)

    configured = FakeBot(repo, bosses)  # same database, POST_CHANNEL_ID now set
    assert digest(configured, kl(2026, 9, 5, 12, 0)) is None
    assert digest(configured, kl(2026, 9, 10, 0, 1)) is not None
    assert len(configured.digests) == 1


# --- the runtime extractor switch ------------------------------------------


def test_the_extractor_flag_is_db_backed_and_seeded_from_the_environment(fake_bot):
    assert fake_bot.extract_enabled is True
    fake_bot.repo.set_config("extract_enabled", "0")
    assert fake_bot.extract_enabled is False


def test_a_pipeline_reads_the_runtime_flag_not_the_env_var(fake_bot):
    from bot.extract.pipeline import Pipeline

    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    assert pipeline.enabled is True
    fake_bot.repo.set_config("extract_enabled", "0")
    assert pipeline.enabled is False
    fake_bot.repo.set_config("extract_enabled", "1")
    fake_bot.repo.set_config("paused", "1")
    assert pipeline.enabled is False


def test_build_repo_seeds_the_flag(tmp_path):
    from bot.__main__ import build_repo
    from bot.agent.client import CFG_EXTRACT

    from .fake_bot import make_settings

    settings = make_settings(db_path=str(tmp_path / "bot.sqlite"), extract_enabled=False)
    repo = build_repo(settings)
    try:
        assert repo.get_config(CFG_EXTRACT) == "0"
    finally:
        repo.close()


# --- amendments the portal edits before applying ---------------------------


def test_a_proposals_time_can_be_corrected_before_it_is_applied(repo):
    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    amendment = repo.create_amendment(
        week_start=ws, kind="move", new_datetime=kl(2026, 9, 2, 21, 30)
    )
    repo.set_amendment_datetime(amendment, kl(2026, 9, 3, 22, 15))
    assert repo.get_amendment(amendment)["new_datetime"].astimezone(TZ).hour == 22
    repo.set_amendment_datetime(amendment, None)
    assert repo.get_amendment(amendment)["new_datetime"] is None


def test_one_extraction_can_be_fetched_by_id(repo):
    first = repo.log_extraction("m", "prompt one", "{}", 10)
    second = repo.log_extraction("m", "prompt two", "{}", 20)
    assert repo.get_extraction(first)["prompt"] == "prompt one"
    assert repo.get_extraction("nope") is None
    assert set(repo.list_extraction_ids()) == {first, second}
