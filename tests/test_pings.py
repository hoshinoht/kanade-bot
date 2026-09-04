"""The mention policy (DESIGN.md §3) and the Manage Messages checks.

Two complaints drove this: the bot @mentioned people for things they could not
act on, and it silently could not enforce exclusive ✅/❌ because nobody knew it
was missing a permission. So these tests are mostly about what is *absent* from
a message -- an empty allow-list, and a `<@id>` that is not there.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime
from zoneinfo import ZoneInfo

import pytest

from bot.agent import formatting, pings
from bot.infrastructure.db import DEFAULT_PING_LEVEL, PING_LEVELS, Repo

from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

TZ = ZoneInfo("Asia/Kuala_Lumpur")
WS = datetime(2026, 8, 27, 0, 0, tzinfo=TZ)


@pytest.fixture
def roster(repo: Repo) -> Repo:
    """Three bossers, one on each level."""
    repo.upsert_member(1, "Alvin tan", None, True)  # essential (the default)
    repo.upsert_member(2, "kanon [AZUR]", "kanon", True)
    repo.upsert_member(3, "Priya", None, True)
    repo.set_ping_level(2, "all")
    repo.set_ping_level(3, "off")
    return repo


# ---------------------------------------------------------------------------
# the resolver
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("kind", sorted(pings.ESSENTIAL_KINDS))
def test_an_essential_post_mentions_everyone_but_the_opted_out(roster, kind):
    assert pings.resolve_mentions(roster, ["1", "2", "3"], kind) == ["1", "2"]


@pytest.mark.parametrize("kind", sorted(pings.INFORMATIONAL_KINDS))
def test_an_informational_post_only_mentions_the_people_who_asked(roster, kind):
    assert pings.resolve_mentions(roster, ["1", "2", "3"], kind) == ["2"]


def test_off_is_never_mentioned_by_anything(roster):
    for kind in pings.PING_KINDS:
        assert "3" not in pings.resolve_mentions(roster, ["3"], kind)


def test_the_default_level_is_essential(repo: Repo):
    repo.upsert_member(9, "New Bosser", None, True)
    assert repo.get_ping_level(9) == DEFAULT_PING_LEVEL
    assert pings.resolve_mentions(repo, ["9"], "day_of") == ["9"]
    assert pings.resolve_mentions(repo, ["9"], "swap") == []


def test_somebody_the_roster_has_never_seen_still_gets_their_reminders(repo: Repo):
    # A missing row must never cost someone the ping they need to answer.
    assert pings.resolve_mentions(repo, ["404"], "day_of") == ["404"]
    assert pings.resolve_mentions(repo, ["404"], "amend") == []


def test_the_resolver_keeps_order_and_drops_duplicates(roster):
    assert pings.resolve_mentions(roster, ["2", "1", "2", 1], "day_of") == ["2", "1"]


def test_an_unknown_kind_is_treated_as_informational(roster, caplog):
    # A typo must cost a missing ping, never a burst of notifications.
    assert pings.resolve_mentions(roster, ["1", "2", "3"], "nonsense") == ["2"]
    assert "unknown ping kind" in caplog.text


def test_upgrading_a_level_does_not_disturb_the_rest(roster):
    roster.set_ping_level(1, "all")
    assert pings.resolve_mentions(roster, ["1", "2", "3"], "swap") == ["1", "2"]


def test_an_invented_level_is_refused(roster):
    with pytest.raises(ValueError, match="ping level"):
        roster.set_ping_level(1, "loud")
    with pytest.raises(ValueError, match="ping level"):
        pings.normalise_level("loud")
    assert pings.normalise_level(" OFF ") == "off"
    assert roster.get_ping_level(1) == "essential"  # unchanged


def test_setting_the_level_of_somebody_who_is_not_on_the_roster(repo: Repo):
    with pytest.raises(KeyError):
        repo.set_ping_level(404, "off")


# ---------------------------------------------------------------------------
# rendering: named, or notified
# ---------------------------------------------------------------------------


def test_an_audience_mentions_the_resolved_and_names_the_rest(roster):
    who = pings.audience(roster, ["1", "2", "3"], "amend")
    assert who.render(["1", "2", "3"]) == "Alvin tan <@2> Priya"
    assert list(who.mentioned) == ["2"]


def test_a_nickname_wins_over_the_display_name(roster):
    who = pings.audience(roster, ["2"], "swap")
    assert pings.display_names(roster, ["2"]) == {"2": "kanon"}
    assert who.render(["2"]) == "<@2>"  # 2 is on `all`, so they are pinged here


def test_somebody_with_no_name_on_file_falls_back_to_a_mention(repo: Repo):
    # It still cannot notify them: they are absent from `mentioned`, and the
    # allow-list is what Discord obeys.
    who = pings.audience(repo, ["404"], "swap")
    assert who.render(["404"]) == "<@404>"
    assert who.mentioned == ()


def test_a_countdown_only_pings_the_people_who_have_not_answered(roster):
    who = pings.audience(roster, ["1", "2", "3"], "countdown", candidates=["2", "3"])
    assert list(who.mentioned) == ["2"]  # 3 is off; 1 has already said yes
    assert who.render(["1", "2", "3"]) == "Alvin tan <@2> Priya"


def test_without_an_audience_everyone_is_still_rendered_as_a_mention():
    # The cards' own unit tests rely on this, and so does any call site that has
    # not been handed one; `allowed_mentions` is still the thing that decides.
    assert formatting.format_participants(["1", "2"]) == "<@1> <@2>"
    assert formatting.format_participants([]) == "(nobody)"


def test_a_card_carries_the_allow_list_it_was_built_with(roster):
    run = {
        "id": "r",
        "bosses": ["HStar"],
        "datetime": datetime(2026, 8, 31, 21, 30, tzinfo=TZ),
        "participants": ["1", "2", "3"],
        "status": "planned",
    }
    who = pings.audience(roster, run["participants"], "day_of")
    card = formatting.day_of_card([run], TZ, {"r": {}}, who=who)
    assert card.mention_users == ["1", "2"]
    assert "<@1>" in card.content and "<@2>" in card.content
    assert "Priya" in card.content and "<@3>" not in card.content


# ---------------------------------------------------------------------------
# what actually gets posted
# ---------------------------------------------------------------------------


def _quiet(fake_bot) -> None:
    """Everyone on the seeded roster is on the default level."""
    for uid in ("1001", "1002", "1003"):
        assert fake_bot.repo.get_ping_level(uid) == "essential"


@pytest.mark.parametrize(
    ("method", "path", "body"),
    [
        ("post", "/api/runs/{run}/amend", {"to": "2026-09-02 21:45"}),
        ("post", "/api/runs/{run}/cancel", None),
        ("post", "/api/runs/{run}/otot", None),
        ("patch", "/api/runs/{run}/status", {"status": "done"}),
        ("patch", "/api/runs/{run}/participants", {"remove": ["1002"]}),
    ],
)
def test_informational_posts_notify_nobody(auth, fake_bot, seeded, method, path, body):
    _quiet(fake_bot)
    call = getattr(auth, method)
    url = path.format(run=seeded["run_star"])
    call(url, json=body) if body is not None else call(url)
    posted = fake_bot.posts[-1]
    assert posted.allowed_mentions == []
    assert "<@" not in posted.content
    # ...and the party is still named, so the post reads the same to a human.
    assert "Alvin tan" in posted.content or "kanon" in posted.content


def test_a_weekly_timing_change_notifies_nobody(auth, fake_bot, seeded):
    _quiet(fake_bot)
    auth.patch(f"/api/fixed/{seeded['fixed_star']}", json={"time": "22:00"})
    posted = fake_bot.posts[-1]
    assert "Weekly timing changed" in posted.content
    assert posted.allowed_mentions == []
    assert "<@" not in posted.content


def test_someone_on_all_is_mentioned_by_an_informational_post(auth, fake_bot, seeded):
    fake_bot.repo.set_ping_level(1002, "all")
    auth.post(f"/api/runs/{seeded['run_star']}/cancel")
    posted = fake_bot.posts[-1]
    assert posted.allowed_mentions == ["1002"]
    assert "<@1002>" in posted.content
    assert "Alvin tan" in posted.content and "<@1001>" not in posted.content


def test_a_test_ping_looks_real_but_summons_nobody(auth, fake_bot, seeded):
    auth.post("/api/debug/ping", json={"run_id": seeded["run_star"], "kind": "day_of"})
    posted = fake_bot.posts[-1]
    assert "🧪 TEST" in posted.content
    assert posted.allowed_mentions == []


def test_a_decline_still_summons_the_rest_of_the_run(roster):
    run = {
        "id": "r",
        "bosses": ["HStar", "HFA"],
        "datetime": datetime(2026, 8, 31, 21, 30, tzinfo=TZ),
        "participants": ["1", "2", "3"],
        "status": "planned",
    }
    others = ["2", "3"]
    who = pings.audience(roster, others, "decline")
    notice = formatting.decline_notice(run, "1", "Alvin", TZ, who)
    # 2 is pinged (they have to decide whether to re-plan); 3 chose `off`, so
    # they are named instead. The person who dropped out is not tagged at all.
    assert notice.startswith("<@2> Priya Alvin can't make")
    assert list(who.mentioned) == ["2"]
    assert "<@1>" not in notice and "<@3>" not in notice


# ---------------------------------------------------------------------------
# the API, the portal and the migration
# ---------------------------------------------------------------------------


def test_the_api_reports_and_sets_a_members_level(auth, seeded):
    rows = auth.get("/api/members").json()
    assert {r["ping_level"] for r in rows} == {"essential"}
    updated = auth.patch("/api/members/1002", json={"ping_level": "off"})
    assert updated.status_code == 200
    assert updated.json()["ping_level"] == "off"
    assert auth.get("/api/members").json()[1]["ping_level"] in {"off", "essential"}


def test_the_api_refuses_a_level_it_does_not_know(auth, seeded):
    response = auth.patch("/api/members/1002", json={"ping_level": "loud"})
    assert response.status_code == 400
    assert "ping level" in response.json()["error"]


def test_patching_a_member_with_nothing_to_change(auth, seeded):
    assert auth.patch("/api/members/1002", json={}).status_code == 400


def test_patching_somebody_who_is_not_on_the_roster(auth, seeded):
    assert auth.patch("/api/members/404", json={"ping_level": "off"}).status_code == 404


def test_the_members_page_shows_each_level(auth, fake_bot, seeded):
    fake_bot.repo.set_ping_level(1002, "off")
    body = auth.get("/members").text
    assert "@mentions" in body
    assert "off" in body


def test_a_v4_database_requires_the_previous_release(tmp_path):
    path = tmp_path / "v4.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (4);
        CREATE TABLE members (
            user_id TEXT PRIMARY KEY, display_name TEXT NOT NULL DEFAULT '', nickname TEXT,
            aliases TEXT NOT NULL DEFAULT '[]', has_role INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );
        INSERT INTO members
            VALUES ('7', 'harbour4417', 'MY', '["MY"]', 1, '2026-08-30T00:00:00+00:00');
        CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO config VALUES ('day_of_ping_time', '08:30');
        """
    )
    conn.close()

    with pytest.raises(RuntimeError, match="upgrades from v9 only"):
        Repo(path)


def test_every_level_round_trips_through_the_database(repo: Repo):
    repo.upsert_member(1, "Alvin", None, True)
    for level in PING_LEVELS:
        assert repo.set_ping_level(1, level) == level
        assert repo.get_ping_level(1) == level
    # A roster sync must not reset somebody's choice.
    repo.sync_roster([(1, "Alvin tan", None, True)])
    assert repo.get_ping_level(1) == PING_LEVELS[-1]


# ---------------------------------------------------------------------------
# Manage Messages
# ---------------------------------------------------------------------------


def test_the_access_report_says_whether_reactions_can_be_kept_exclusive(fake_bot):
    rows = {row["name"]: row for row in fake_bot.access_report()}
    assert rows["#hstar-party"]["manage_messages"] is True
    fake_bot.channels[WATCHED_CHANNEL].permissions.manage_messages = False
    rows = {row["name"]: row for row in fake_bot.access_report()}
    assert rows["#hstar-party"]["manage_messages"] is False
    assert rows["#xkalos-party"]["manage_messages"] is True


def test_the_missing_list_is_read_live_from_the_guild(fake_bot):
    assert fake_bot.missing_manage_messages() == []
    fake_bot.channels[OTHER_CHANNEL].permissions.manage_messages = False
    assert fake_bot.missing_manage_messages() == ["#xkalos-party"]
    # Granting it in Discord clears the banner without a restart.
    fake_bot.channels[OTHER_CHANNEL].permissions.manage_messages = True
    assert fake_bot.missing_manage_messages() == []


def test_the_config_endpoint_carries_the_banner(auth, fake_bot):
    assert auth.get("/api/config").json()["missing_manage_messages"] == []
    fake_bot.channels[WATCHED_CHANNEL].permissions.manage_messages = False
    assert auth.get("/api/config").json()["missing_manage_messages"] == ["#hstar-party"]


def test_the_config_page_shows_the_banner_and_the_column(auth, fake_bot):
    assert "Manage Messages" not in auth.get("/config").text
    fake_bot.channels[WATCHED_CHANNEL].permissions.manage_messages = False
    body = auth.get("/config").text
    assert "Missing “Manage Messages” in #hstar-party" in body
    assert "Manage messages" in body  # the access table's column


def test_the_access_fragment_has_a_manage_messages_column(auth, fake_bot):
    fake_bot.channels[WATCHED_CHANNEL].permissions.manage_messages = False
    body = auth.get("/access").text
    assert "Manage messages" in body
    # The portal draws its own marks; the ✅/❌ below are Discord's own output,
    # and stay Discord's.
    table = body.split("</table>")[0]
    assert table.count('data-icon="x"') == 1  # exactly one cell, in the channel that lacks it
    assert table.count('data-icon="check"') == 11  # the other five columns, twice, plus #xkalos


def test_debug_status_lists_manage_messages_per_channel(fake_bot):
    from bot.agent.debug import manage_messages_lines

    fake_bot.channels[OTHER_CHANNEL].permissions.manage_messages = False
    line = manage_messages_lines(fake_bot.access_report())[0]
    assert "#hstar-party ✅" in line
    assert "#xkalos-party ❌" in line
    assert "missing in 1" in line
    assert "all good" in manage_messages_lines(FakeAccess.all_good())[0]
    assert "not connected" in manage_messages_lines([])[0]


class FakeAccess:
    @staticmethod
    def all_good() -> list[dict]:
        return [{"name": "#hstar-party", "watched": True, "manage_messages": True}]
