"""Quality-of-life behaviours: stale pings, quieter countdowns, decline notices, v3 migration."""

from __future__ import annotations

import sqlite3
from datetime import datetime, time, timedelta
from zoneinfo import ZoneInfo

import pytest

from bot.agent import formatting
from bot.agent.materialise import DAY_OF, countdown_kind, is_stale, reconcile_day_of, reminder_specs
from bot.infrastructure.db import Repo

TZ = ZoneInfo("Asia/Kuala_Lumpur")
WS = datetime(2026, 8, 27, 0, 0, tzinfo=TZ)


def _run(repo: Repo, at: datetime, participants=("1", "2", "3"), status="planned") -> dict:
    run_id = repo.create_run(
        week_start=WS,
        bosses=["HMaleficStar", "HFA"],
        run_at=at,
        participants=list(participants),
        status=status,
        source="fixed",
        fixed_run_id=None,
        channel_id="42",
    )
    return repo.get_run(run_id)


def test_is_stale_grace_per_kind():
    now = datetime(2026, 8, 31, 12, 0, tzinfo=TZ)
    assert not is_stale(DAY_OF, now - timedelta(hours=11), now)
    assert is_stale(DAY_OF, now - timedelta(hours=13), now)
    assert not is_stale(countdown_kind(60), now - timedelta(minutes=29), now)
    assert is_stale(countdown_kind(60), now - timedelta(minutes=31), now)


def test_day_of_at_0100_stays_on_the_run_day():
    specs = reminder_specs(
        datetime(2026, 8, 31, 21, 30, tzinfo=TZ), "planned", TZ, time(1, 0), [60]
    )
    day_of = next(s for s in specs if s.kind == DAY_OF)
    assert day_of.fire_at == datetime(2026, 8, 31, 1, 0, tzinfo=TZ)


def test_reconcile_moves_unsent_day_of_to_new_ping_time():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 9, 7, 21, 30, tzinfo=TZ))
    for spec in reminder_specs(run["datetime"], run["status"], TZ, time(9, 0), [60]):
        repo.add_reminder(run["id"], spec.kind, spec.fire_at)
    now = datetime(2026, 8, 31, 12, 0, tzinfo=TZ)
    moved = reconcile_day_of(repo, TZ, time(1, 0), now=now)
    assert moved == 1
    fire = next(r for r in repo.list_reminders(run["id"]) if r["kind"] == DAY_OF)["fire_at"]
    assert fire.astimezone(TZ) == datetime(2026, 9, 7, 1, 0, tzinfo=TZ)
    assert reconcile_day_of(repo, TZ, time(1, 0), now=now) == 0  # idempotent


def test_reconcile_reopens_a_skipped_day_of_when_its_new_time_is_future():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 9, 7, 21, 30, tzinfo=TZ))
    reminder_id = repo.add_reminder(run["id"], DAY_OF, datetime(2026, 9, 7, 9, 0, tzinfo=TZ))
    now = datetime(2026, 9, 7, 9, 30, tzinfo=TZ)
    repo.mark_reminder_sent(reminder_id, at=now)

    assert reconcile_day_of(repo, TZ, time(10, 0), now=now) == 1

    (reminder,) = repo.list_reminders(run["id"])
    assert reminder["fire_at"] == datetime(2026, 9, 7, 10, 0, tzinfo=TZ)
    assert reminder["sent_at"] is None
    assert repo.due_reminders(now) == []


def test_reconcile_skips_a_queued_day_of_moved_into_the_past():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 9, 7, 21, 30, tzinfo=TZ))
    reminder_id = repo.add_reminder(run["id"], DAY_OF, datetime(2026, 9, 7, 9, 0, tzinfo=TZ))
    now = datetime(2026, 9, 7, 9, 30, tzinfo=TZ)

    assert reconcile_day_of(repo, TZ, time(8, 0), now=now) == 1

    reminder = repo.get_reminder(reminder_id)
    assert reminder["fire_at"] == datetime(2026, 9, 7, 8, 0, tzinfo=TZ)
    assert reminder["sent_at"] == now
    assert repo.due_reminders(now) == []
    assert reconcile_day_of(repo, TZ, time(8, 0), now=now) == 0


def test_reconcile_keeps_a_posted_day_of_and_its_message_mapping():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 9, 7, 21, 30, tzinfo=TZ))
    reminder_id = repo.add_reminder(run["id"], DAY_OF, datetime(2026, 9, 7, 9, 0, tzinfo=TZ))
    repo.mark_reminder_sent(reminder_id, message_id=4242, at=datetime(2026, 9, 7, 9, 0, tzinfo=TZ))

    assert reconcile_day_of(repo, TZ, time(10, 0), now=datetime(2026, 9, 7, 8, 0, tzinfo=TZ)) == 0

    reminder = repo.get_reminder(reminder_id)
    assert reminder["fire_at"] == datetime(2026, 9, 7, 9, 0, tzinfo=TZ)
    assert reminder["message_id"] == "4242"
    assert [row["id"] for row in repo.reminders_by_message(4242)] == [reminder_id]


def test_a_countdown_goes_to_the_whole_party_bar_the_decliners():
    """An hour out, the people who are coming want the reminder whether or not
    they have ticked. Only an explicit ❌ takes somebody off the list."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    rsvps = {"1": "yes", "2": "no"}
    assert formatting.not_declined(run, rsvps) == ["1", "3"]

    card = formatting.countdown_card(run, 60, TZ, rsvps)
    assert "<@1>" in card.content and "<@3>" in card.content
    assert "<@2> out" in card.content
    assert card.footer == formatting.REACT_HINT


def test_the_countdown_still_says_who_it_is_waiting_on():
    """The card names four people; the embed says which of them owe an answer."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    card = formatting.countdown_card(run, 60, TZ, {"1": "yes", "2": "no"})

    assert "Still to answer: <@3>" in card.description


def test_a_maybe_is_still_someone_the_countdown_is_waiting_on():
    """A "maybe" is exactly the person a T-1h nudge exists for -- and unlike a
    ❌ they are still on the run, so they are pinged as well as chased."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    rsvps = {"1": "yes", "2": "maybe", "3": "no"}

    assert formatting.unanswered(run, rsvps) == ["2"]
    assert formatting.not_declined(run, rsvps) == ["1", "2"]


def test_a_countdown_with_everyone_on_is_the_quiet_green_one():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    everyone = {"1": "yes", "2": "yes", "3": "yes"}
    assert formatting.unanswered(run, everyone) == []

    card = formatting.countdown_card(run, 60, TZ, everyone)
    assert "everyone's confirmed" in card.content
    assert card.footer is None
    assert card.colour == formatting.COLOUR_ALL_SET


def test_a_countdown_nobody_is_left_to_answer_still_says_who_is_out():
    """Everybody has answered and one of them can't come: nothing left to ask,
    but "everyone's confirmed ✅" in green would be a lie."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    card = formatting.countdown_card(run, 60, TZ, {"1": "yes", "2": "no", "3": "yes"})

    assert "<@1> <@3> · <@2> out" in card.content
    assert "everyone's confirmed" not in card.content
    assert "Still to answer" not in card.description
    assert card.footer is None
    assert card.colour == formatting.COLOUR_COUNTDOWN


def test_reminder_cards_wear_the_lead_boss_colors(bosses):
    """Day-of and countdown stripes come from the catalog, lead boss first."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    second_first = {**run, "bosses": ["HFA", "HMaleficStar"]}

    morning = formatting.day_of_card([run], TZ, {run["id"]: {}}, table=bosses)
    assert morning.colour == 0xF8DD4A  # MaleficStar leads HMaleficStar + HFA
    swapped = formatting.day_of_card([second_first], TZ, {run["id"]: {}}, table=bosses)
    assert swapped.colour == 0xA9D8F5  # FA leads this time

    pending = formatting.countdown_card(run, 60, TZ, {"1": "yes"}, table=bosses)
    assert pending.colour == 0xF8DD4A  # boss color, not countdown yellow
    settled = formatting.countdown_card(
        run, 60, TZ, {"1": "yes", "2": "yes", "3": "yes"}, table=bosses
    )
    assert settled.colour == 0xF8DD4A  # boss color, not all-set green


def test_reminder_cards_without_a_table_keep_their_kind_colors():
    """No table (or no colour, or an unknown token) keeps the old semantics."""
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    everyone = {"1": "yes", "2": "yes", "3": "yes"}

    assert formatting.day_of_card([run], TZ, {}).colour == formatting.COLOUR_DAY_OF
    assert formatting.day_of_card([run], TZ, {}, table=_Table()).colour == (
        formatting.COLOUR_DAY_OF
    )
    assert formatting.countdown_card(run, 60, TZ, everyone).colour == (formatting.COLOUR_ALL_SET)
    ghost = {**run, "bosses": ["???"]}
    assert formatting.countdown_card(ghost, 60, TZ, {}, table=_Table()).colour == (
        formatting.COLOUR_COUNTDOWN
    )


class _Table:
    def describe(self, name):
        return {"HMaleficStar": "Radiant Malefic Star (Hard, Lv280)"}.get(name, name)


def test_day_of_card_has_mentions_in_content_and_detail_in_fields():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    card = formatting.day_of_card([run], TZ, {run["id"]: {"1": "yes"}}, table=_Table())
    assert "Mon 31 Aug" in card.content
    assert all(f"<@{uid}>" in card.content for uid in ("1", "2", "3"))
    assert len(card.fields) == 1
    name, value = card.fields[0]
    assert "21:30" in name and "HMaleficStar + HFA" in name
    assert "Radiant Malefic Star (Hard, Lv280)" in value
    assert "1/3 ✅" in value
    assert card.has_embed


def test_decline_notice_round_trip():
    repo = Repo(":memory:")
    run = _run(repo, datetime(2026, 8, 31, 21, 30, tzinfo=TZ))
    assert repo.get_decline_notice(run["id"], "2") is None
    at = datetime(2026, 8, 31, 10, 0, tzinfo=TZ)
    repo.set_decline_notice(run["id"], "2", 555, 42, at)
    row = repo.get_decline_notice(run["id"], 2)
    assert row["message_id"] == "555" and row["channel_id"] == "42"
    assert row["notified_at"] == at
    repo.clear_decline_notice_message(run["id"], "2")
    row = repo.get_decline_notice(run["id"], "2")
    assert row["message_id"] is None and row["notified_at"] == at  # cooldown survives


def test_a_pre_v9_database_requires_the_previous_release(tmp_path):
    path = tmp_path / "v2.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (2);
        CREATE TABLE amendments (
            id TEXT PRIMARY KEY, week_start TEXT NOT NULL, kind TEXT NOT NULL,
            bosses TEXT NOT NULL DEFAULT '[]', run_id TEXT, new_datetime TEXT,
            participants TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'proposed',
            confidence REAL, evidence_msg_ids TEXT NOT NULL DEFAULT '[]',
            proposal_message_id TEXT, created_at TEXT NOT NULL
        );
        INSERT INTO amendments (id, week_start, kind, created_at) VALUES ('a', 'w', 'move', 't');
        CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO config VALUES ('day_of_ping_time', '09:00');
        """
    )
    conn.close()

    with pytest.raises(RuntimeError, match="upgrades from v9 only"):
        Repo(path)
