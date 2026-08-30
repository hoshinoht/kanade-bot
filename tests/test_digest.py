"""The weekly digest card, and the pieces phase 3 added to the client."""

from __future__ import annotations

from bot import formatting
from bot.ids import short_id
from bot.materialise import materialise_week
from bot.weeks import current_week_start

from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl
from .fake_bot import WATCHED_CHANNEL


def _week(repo):
    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, kl(2026, 8, 30, 12, 0))
    repo.add_fixed_run(1, ["HStar", "HFA"], 0, "21:30", ["1", "2"], channel_id=WATCHED_CHANNEL)
    repo.add_fixed_run(2, ["XKalos"], 1, "23:00", ["2", "3"], channel_id=WATCHED_CHANNEL)
    materialise_week(repo, ws, TZ, PING_TIME, COUNTDOWNS)
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
    from bot.client import CFG_EXTRACT

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
