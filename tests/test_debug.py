"""`/debug` access control and its pure query helpers."""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.db import Repo
from bot.debug import format_uptime, may_debug, render_reminder_rows, upcoming_window

from .conftest import TZ, kl

OWNER = 100
ADMIN_ROLE = 200
LISTED = 300
NOBODY = 999


def allowed(user_id, role_ids=(), owner=OWNER, admin_role=ADMIN_ROLE, listed=(LISTED,)):
    return may_debug(user_id, list(role_ids), owner, admin_role, list(listed))


# -- access ------------------------------------------------------------------


def test_the_guild_owner_may_debug():
    assert allowed(OWNER)


def test_an_admin_role_holder_may_debug():
    assert allowed(NOBODY, role_ids=[ADMIN_ROLE])


def test_a_listed_user_may_debug():
    assert allowed(LISTED)


def test_everyone_else_may_not():
    assert not allowed(NOBODY)
    assert not allowed(NOBODY, role_ids=[12345])


def test_the_bossing_role_alone_is_not_enough():
    # /debug can post to real channels, so it is deliberately narrower than the
    # ordinary command check.
    assert not allowed(NOBODY, role_ids=[555])


def test_it_works_with_no_admin_role_configured():
    assert allowed(OWNER, admin_role=None)
    assert not allowed(NOBODY, admin_role=None)


def test_it_works_outside_a_guild_context():
    assert not allowed(NOBODY, owner=None)
    assert allowed(LISTED, owner=None)


def test_ids_given_as_strings_still_match():
    assert may_debug("300", [], OWNER, ADMIN_ROLE, ["300"])


def test_an_empty_debug_list_is_fine():
    assert not may_debug(NOBODY, [], OWNER, ADMIN_ROLE, [])


# -- upcoming window ---------------------------------------------------------


NOW = kl(2026, 8, 31, 8, 0)


def reminder(kind, fire_at, sent=False, run_id=1):
    return {
        "id": hash(kind) & 0xFF,
        "run_id": run_id,
        "kind": kind,
        "fire_at": fire_at,
        "sent_at": NOW if sent else None,
        "message_id": None,
    }


def test_only_reminders_inside_the_window_are_returned():
    rows = [
        reminder("day_of", kl(2026, 8, 31, 9, 0)),
        reminder("countdown_60", kl(2026, 8, 31, 20, 30)),
        reminder("far", kl(2026, 9, 5, 9, 0)),
    ]
    assert [r["kind"] for r in upcoming_window(rows, NOW, 24)] == ["day_of", "countdown_60"]


def test_already_sent_reminders_are_excluded():
    rows = [reminder("day_of", kl(2026, 8, 31, 9, 0), sent=True)]
    assert upcoming_window(rows, NOW, 24) == []


def test_past_reminders_are_excluded():
    rows = [reminder("day_of", kl(2026, 8, 30, 9, 0))]
    assert upcoming_window(rows, NOW, 24) == []


def test_results_are_soonest_first():
    rows = [
        reminder("countdown_15", kl(2026, 8, 31, 21, 15)),
        reminder("day_of", kl(2026, 8, 31, 9, 0)),
    ]
    assert [r["kind"] for r in upcoming_window(rows, NOW, 24)] == ["day_of", "countdown_15"]


def test_a_reminder_exactly_on_the_horizon_is_included():
    rows = [reminder("edge", NOW + timedelta(hours=24))]
    assert len(upcoming_window(rows, NOW, 24)) == 1


def test_a_zero_hour_window_only_catches_right_now():
    assert upcoming_window([reminder("later", NOW + timedelta(minutes=1))], NOW, 0) == []


# -- rendering ---------------------------------------------------------------


def test_reminder_rows_render_in_guild_time():
    text = render_reminder_rows([reminder("day_of", kl(2026, 8, 31, 9, 0))], TZ)
    assert "Mon 31 Aug 09:00" in text
    assert "pending" in text


def test_a_sent_row_shows_its_message_id():
    row = reminder("day_of", kl(2026, 8, 31, 9, 0), sent=True)
    row["message_id"] = "4242"
    text = render_reminder_rows([row], TZ)
    assert "sent" in text and "4242" in text


def test_nothing_renders_as_none():
    assert render_reminder_rows([], TZ) == "_none_"


@pytest.mark.parametrize(
    ("seconds", "expected"),
    [(0, "0m"), (90, "1m"), (3600, "1h 0m"), (5400, "1h 30m"), (90000, "1d 1h 0m"), (-5, "0m")],
)
def test_format_uptime(seconds, expected):
    assert format_uptime(seconds) == expected


# -- debug message bookkeeping ----------------------------------------------


def test_a_debug_message_maps_back_to_its_run(repo: Repo):
    run_id = repo.create_run(kl(2026, 8, 27), ["HStar"], kl(2026, 8, 31, 21, 30), ["1"])
    repo.add_debug_message(4242, run_id, 900, "day_of")
    rows = repo.debug_messages_for(4242)
    assert [r["run_id"] for r in rows] == [run_id]


def test_a_debug_message_is_not_a_reminder(repo: Repo):
    # The scheduled ping must still fire: /debug ping must not satisfy it.
    run_id = repo.create_run(kl(2026, 8, 27), ["HStar"], kl(2026, 8, 31, 21, 30), ["1"])
    repo.add_debug_message(4242, run_id, 900, "day_of")
    assert repo.list_reminders(run_id) == []
    assert repo.reminders_by_message(4242) == []


def test_recent_debug_messages_are_scoped_by_channel_and_age(repo: Repo):
    run_id = repo.create_run(kl(2026, 8, 27), ["HStar"], kl(2026, 8, 31, 21, 30), ["1"])
    repo.add_debug_message(1, run_id, 900, "day_of")
    repo.add_debug_message(2, run_id, 901, "day_of")
    from bot.timeutil import utcnow

    cutoff = utcnow() - timedelta(hours=24)
    assert len(repo.recent_debug_messages(cutoff)) == 2
    assert len(repo.recent_debug_messages(cutoff, channel_id=900)) == 1
    assert repo.recent_debug_messages(utcnow() + timedelta(hours=1)) == []


def test_deleting_a_debug_message_row(repo: Repo):
    run_id = repo.create_run(kl(2026, 8, 27), ["HStar"], kl(2026, 8, 31, 21, 30), ["1"])
    repo.add_debug_message(4242, run_id, 900, "day_of")
    repo.delete_debug_message(4242)
    assert repo.debug_messages_for(4242) == []


# --- /debug extract's reply ---------------------------------------------------


def _report(**kw):
    from bot.extract.pipeline import RescanReport

    from .conftest import kl

    base = dict(channel_id="900", window="week", since=kl(2026, 8, 27, 0, 0), elapsed_ms=1500)
    base.update(kw)
    return RescanReport(**base)


def test_rescan_summary_is_importable_by_debug_extract():
    """The worker refactor once dropped this and `/debug extract` raised ImportError."""
    from bot.commands import rescan_summary  # noqa: F401


def test_rescan_summary_says_when_the_model_was_not_asked():
    from bot.commands import rescan_summary

    text = rescan_summary(_report(backfilled=12, gated=0))
    assert "12 message(s) pulled" in text
    assert "wasn't asked" in text


def test_rescan_summary_leads_with_the_model_error():
    from bot.commands import rescan_summary

    text = rescan_summary(_report(gated=3, bursts=1, extracted=1, errors=["ReadTimeout"]))
    assert "ReadTimeout" in text


def test_rescan_summary_reports_no_change_with_the_drop_reasons():
    from bot.commands import rescan_summary

    text = rescan_summary(_report(gated=3, bursts=1, extracted=1, dropped=2, stale=1))
    assert "No change found" in text
    assert "1 already passed" in text
    assert "1 below threshold" in text
