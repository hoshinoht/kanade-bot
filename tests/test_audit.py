"""The audit trail: every change to the schedule, and who made it.

One shared admin token means the portal cannot tell two people apart on its own,
so the name has to come from the surface the change arrived on -- a tailnet
login in front of the portal, the operating-system user behind `bossctl`, a
Discord id on a card. These are the tests that each surface names the right
person, that a surface which can prove nothing names nobody, and that a failure
to write the row can never cost the change itself.
"""

from __future__ import annotations

import asyncio
from datetime import timedelta
from types import SimpleNamespace

from discord import app_commands

from bot import audit
from bot.api import create_app
from bot.api.app import resolve_actor
from bot.api.auth import HEADER_LOGIN
from bot.audit import HEADER_BOSSCTL
from bot.db import AUDIT_KEPT, SCHEMA_VERSION, Repo
from bot.rsvp import EMOJI_YES

from .conftest import kl
from .fake_bot import ADMIN_TOKEN, WATCHED_CHANNEL, make_settings

LOGIN = "me@example.com"
NOW = kl(2026, 8, 30, 12, 0)


def build(fake_bot, **settings):
    fake_bot.settings = make_settings(**settings)
    return create_app(fake_bot)


def client_for(app, peer: str = "127.0.0.1"):
    """A signed-in client whose connection appears to come from ``peer``.

    The default TestClient peer is the literal string "testclient", which is not
    an address at all -- and both header rules below turn on being able to say
    the request came from this machine.
    """
    from fastapi.testclient import TestClient

    client = TestClient(app, client=(peer, 51234))
    client.headers["Authorization"] = f"Bearer {ADMIN_TOKEN}"
    return client


def rows_for(fake_bot, action: str) -> list[dict]:
    return [row for row in fake_bot.repo.list_audit() if row["action"] == action]


# --- the schema -------------------------------------------------------------


def test_a_live_database_gains_the_trail_without_losing_anything(tmp_path):
    """v6 -> v7 adds a table and touches nothing else.

    Walked all the way to :data:`bot.db.SCHEMA_VERSION` rather than stopping at
    7: a database this old goes through every later step too, and what matters
    is that it arrives with the trail and with everything it started with.
    """
    path = tmp_path / "v6.sqlite"
    repo = Repo(path)
    repo.upsert_member(7, "harbour4417", "MY", True)
    fixed = repo.add_fixed_run(7, ["HStar"], 0, "21:30", ["7"], channel_id=900)
    repo._conn.execute("DROP TABLE audit")
    repo._conn.execute("UPDATE schema_version SET version = 6")
    repo.close()

    migrated = Repo(path)
    version = migrated._conn.execute("SELECT version FROM schema_version").fetchone()["version"]
    assert version == SCHEMA_VERSION
    assert migrated.list_audit() == []
    assert migrated.get_member("7")["display_name"] == "harbour4417"
    assert [f["id"] for f in migrated.list_fixed_runs()] == [fixed]
    migrated.close()


def test_the_newest_rows_are_kept_and_the_rest_go(repo):
    for day in range(1, 8):
        repo.log_audit(
            surface="portal",
            actor="token",
            action="config",
            subject="paused",
            detail=f"day {day}",
            at=kl(2026, 8, day),
            keep=3,
        )
    assert [row["detail"] for row in repo.list_audit()] == ["day 7", "day 6", "day 5"]


def test_the_default_cap_is_the_documented_one(repo):
    repo.log_audit(surface="system", actor="system", action="config")
    assert AUDIT_KEPT == 2000
    assert repo.count_audit() == 1


# --- who the portal thinks you are ------------------------------------------


def test_a_portal_amend_names_the_tailnet_login(fake_bot, seeded):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins=LOGIN)
    with client_for(app) as client:
        response = client.post(
            f"/api/runs/{seeded['run_star']}/amend",
            json={"to": "2026-09-02 21:45"},
            headers={HEADER_LOGIN: LOGIN},
        )

    assert response.status_code == 200
    (row,) = rows_for(fake_bot, "amend")
    assert (row["surface"], row["actor"]) == ("portal", LOGIN)
    assert row["subject"] == seeded["run_star"]
    assert "moved" in row["detail"]


def test_a_config_change_names_the_tailnet_login(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins=LOGIN)
    with client_for(app) as client:
        response = client.request(
            "PUT", "/api/config", json={"paused": True}, headers={HEADER_LOGIN: LOGIN}
        )

    assert response.status_code == 200
    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"], row["subject"]) == ("portal", LOGIN, "paused")
    assert row["detail"].endswith("-> 1")  # the value it moved to, whatever it was


def test_the_login_header_is_ignored_when_the_flag_is_off(fake_bot):
    """A header nobody vouched for must not sign somebody's name to a change."""
    app = build(fake_bot, trust_tailscale_headers=False, allowed_tailscale_logins=LOGIN)
    with client_for(app) as client:
        response = client.request(
            "PUT", "/api/config", json={"paused": True}, headers={HEADER_LOGIN: LOGIN}
        )

    assert response.status_code == 200
    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"]) == ("portal", "token")


def test_a_login_header_from_another_machine_is_ignored(fake_bot):
    app = build(fake_bot, trust_tailscale_headers=True, allowed_tailscale_logins=LOGIN)
    with client_for(app, peer="100.64.1.2") as client:
        client.request("PUT", "/api/config", json={"paused": True}, headers={HEADER_LOGIN: LOGIN})

    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"]) == ("portal", "token")


def test_with_nothing_to_go_on_the_change_is_the_tokens(fake_bot):
    app = build(fake_bot)
    with client_for(app) as client:
        client.request("PUT", "/api/config", json={"paused": True})

    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"]) == ("portal", "token")


# --- bossctl ----------------------------------------------------------------


def test_bossctl_is_named_over_loopback(fake_bot):
    app = build(fake_bot)
    with client_for(app) as client:
        client.request(
            "PUT", "/api/config", json={"paused": True}, headers={HEADER_BOSSCTL: "nanahoshi"}
        )

    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"]) == ("cli", "nanahoshi")


def test_the_bossctl_header_is_ignored_from_another_machine(fake_bot):
    """It vouches for nothing, so it counts only where the sender is already us."""
    app = build(fake_bot)
    with client_for(app, peer="100.64.1.2") as client:
        client.request(
            "PUT", "/api/config", json={"paused": True}, headers={HEADER_BOSSCTL: "somebody"}
        )

    (row,) = rows_for(fake_bot, "config")
    assert (row["surface"], row["actor"]) == ("portal", "token")


def test_bossctl_sends_the_operating_system_user():
    from bot.cli import Api, os_user

    client = Api(url="http://127.0.0.1:8080", token="t")._client()
    assert client.headers[HEADER_BOSSCTL] == os_user()
    assert client.headers[HEADER_BOSSCTL]
    client.close()


def test_a_request_that_carries_both_headers_keeps_the_vouched_name():
    settings = make_settings(trust_tailscale_headers=True, allowed_tailscale_logins=LOGIN)
    request = SimpleNamespace(
        client=SimpleNamespace(host="127.0.0.1"),
        headers={HEADER_LOGIN: LOGIN, HEADER_BOSSCTL: "nanahoshi"},
    )
    actor = resolve_actor(request, settings)
    assert (actor.surface, actor.who) == ("cli", LOGIN)


def test_an_absurdly_long_name_is_cut_down():
    settings = make_settings()
    request = SimpleNamespace(
        client=SimpleNamespace(host="127.0.0.1"),
        headers={HEADER_BOSSCTL: "n" * 500},
    )
    assert len(resolve_actor(request, settings).who) == 64


# --- the surfaces that are not the portal -----------------------------------


def effects_pipeline(fake_bot):
    """A real Pipeline over the fake bot; only the model is missing."""
    from bot.extract.pipeline import Pipeline

    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    pipeline._bursts = {}
    return pipeline


def planned(kind: str, bosses: list[str], run=None, at_time=None, rsvp=None, participants=()):
    from bot.extract.pipeline import Planned
    from bot.extract.resolve import Resolved
    from bot.extract.schema import Amendment

    return Planned(
        amendment=Amendment(
            kind=kind,
            bosses=bosses,
            confidence=0.9,
            rsvp=rsvp,
            participants=list(participants),
        ),
        resolved=Resolved(at=at_time),
        run=run,
    )


def test_a_card_the_extractor_raised_is_recorded_against_chat(fake_bot, seeded):
    run = fake_bot.repo.get_run(seeded["run_star"])
    pipeline = effects_pipeline(fake_bot)

    asyncio.run(
        pipeline.apply_plan(
            str(WATCHED_CHANNEL),
            [],
            [planned("move", ["HStar"], run, NOW + timedelta(days=4))],
            kl(2026, 8, 27),
            "",
        )
    )

    (row,) = rows_for(fake_bot, "propose")
    assert (row["surface"], row["actor"]) == ("chat", "extractor")
    assert "move" in row["detail"]


def test_a_card_raised_on_somebodys_behalf_names_them(fake_bot, seeded):
    """The seam the chat pilot uses: same call, same surface, a person's name."""
    run = fake_bot.repo.get_run(seeded["run_star"])
    pipeline = effects_pipeline(fake_bot)

    asyncio.run(
        pipeline.apply_plan(
            str(WATCHED_CHANNEL),
            [],
            [planned("move", ["HStar"], run, NOW + timedelta(days=4))],
            kl(2026, 8, 27),
            "",
            actor="1002",
        )
    )

    (row,) = rows_for(fake_bot, "propose")
    assert (row["surface"], row["actor"]) == ("chat", "1002")


def test_an_answer_read_out_of_chat_names_the_person_who_gave_it(fake_bot, seeded):
    run = fake_bot.repo.get_run(seeded["run_star"])
    pipeline = effects_pipeline(fake_bot)

    asyncio.run(
        pipeline.apply_plan(
            str(WATCHED_CHANNEL),
            [planned("rsvp", [], run, rsvp="yes", participants=["1002"])],
            [],
            kl(2026, 8, 27),
            "",
        )
    )

    (row,) = rows_for(fake_bot, "rsvp")
    assert (row["surface"], row["actor"], row["subject"]) == ("chat", "1002", run["id"])
    assert "kanon" in row["detail"]


def test_a_tick_on_a_card_names_the_member_who_reacted(fake_bot, seeded):
    """The one change that always knows exactly whose decision it was."""
    from bot.client import BossBot

    class Wired(BossBot):
        user = SimpleNamespace(id=5555555555555555555)

    client = Wired.__new__(Wired)
    client.repo = fake_bot.repo
    client.settings = fake_bot.settings
    client.tz = fake_bot.tz
    client.materialise_weeks = fake_bot.materialise_weeks

    async def nothing(*_args, **_kwargs):
        return None

    client._mark_superseded = nothing
    client._announce_move = nothing

    amendment = fake_bot.repo.get_amendment(seeded["amendment"])
    payload = SimpleNamespace(
        user_id=1002,
        emoji=EMOJI_YES,
        message_id=900000000000000001,
        channel_id=WATCHED_CHANNEL,
        member=None,
    )
    result = asyncio.run(client._commit_one(amendment, payload.user_id, payload))

    assert result.applied
    (row,) = rows_for(fake_bot, "move")
    assert (row["surface"], row["actor"]) == ("card", "1002")
    assert "kanon" in row["detail"]


class SlashInteraction:
    """Only what these command bodies reach for."""

    def __init__(self, bot, user_id: int):
        self.client = bot
        self.user = SimpleNamespace(id=user_id, display_name=f"member-{user_id}")
        self.guild = bot.guild
        self.channel = bot.channels[WATCHED_CHANNEL]
        self.channel_id = WATCHED_CHANNEL
        self.sent: list[str] = []
        self.response = SimpleNamespace(defer=self._defer, send_message=self._send)
        self.followup = SimpleNamespace(send=self._send)

    async def _defer(self, ephemeral: bool = False) -> None:
        pass

    async def _send(self, content: str, ephemeral: bool = False) -> None:
        self.sent.append(content)


def yes() -> app_commands.Choice:
    return app_commands.Choice(name="yes", value="yes")


def test_a_slash_command_names_the_member_who_ran_it(fake_bot, seeded):
    """`/status` and friends share the portal's service functions.

    Without an actor around the call they would be filed as `system` -- the one
    surface that always knows exactly whose decision a change was, recorded as
    nobody's.
    """
    from bot.commands import _set_status

    fake_bot.is_admin = lambda _user: False
    interaction = SlashInteraction(fake_bot, user_id=1001)

    asyncio.run(_set_status(interaction, seeded["run_star"], "cancelled"))

    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "cancelled"
    (row,) = rows_for(fake_bot, "cancel")
    assert (row["surface"], row["actor"]) == ("discord", "1001")


def test_a_slash_amend_names_the_member_who_ran_it(fake_bot, seeded):
    """`/amend` moves a run through the repository, never through the service."""
    from bot.commands import amend

    fake_bot.is_admin = lambda _user: False
    interaction = SlashInteraction(fake_bot, user_id=1001)

    asyncio.run(amend.callback(interaction, run_id=seeded["run_star"], to="2026-09-02 21:45"))

    (row,) = rows_for(fake_bot, "amend")
    assert (row["surface"], row["actor"]) == ("discord", "1001")
    assert row["subject"] == seeded["run_star"]
    assert row["detail"].startswith("moved ")


def test_a_slash_rsvp_names_the_member_who_ran_it(fake_bot, seeded):
    from bot.commands import rsvp

    interaction = SlashInteraction(fake_bot, user_id=1002)

    asyncio.run(rsvp.callback(interaction, run_id=seeded["run_star"], answer=yes()))

    (row,) = rows_for(fake_bot, "rsvp")
    assert (row["surface"], row["actor"], row["subject"]) == ("discord", "1002", seeded["run_star"])
    assert row["detail"].startswith("yes for member-1002 on ")


def test_a_noop_pingtime_leaves_reminders_and_audit_alone(fake_bot, seeded):
    from bot.commands import pingtime

    fake_bot.repo.set_config("day_of_ping_time", "09:00")
    morning = next(
        reminder
        for reminder in fake_bot.repo.list_reminders(seeded["run_star"])
        if reminder["kind"] == "day_of"
    )
    fake_bot.repo.mark_reminder_sent(morning["id"], 900000000000000012)
    interaction = SlashInteraction(fake_bot, user_id=1001)

    asyncio.run(pingtime.callback(interaction, time="9:00"))

    assert [row["id"] for row in fake_bot.repo.reminders_by_message(900000000000000012)] == [
        morning["id"]
    ]
    assert fake_bot.repo.list_audit() == []
    assert "already go out" in interaction.sent[0]


def test_a_slash_fixed_remove_names_the_member_who_ran_it(fake_bot, seeded):
    """The `/fixed` group writes through the repository too."""
    from bot.commands import FixedGroup

    fake_bot.is_admin = lambda _user: False
    group = FixedGroup()
    interaction = SlashInteraction(fake_bot, user_id=1001)

    asyncio.run(group.remove.callback(group, interaction, seeded["fixed_star"]))

    assert fake_bot.repo.get_fixed_run(seeded["fixed_star"]) is None
    (row,) = rows_for(fake_bot, "fixed_remove")
    assert (row["surface"], row["actor"]) == ("discord", "1001")
    assert row["subject"] == seeded["fixed_star"]
    assert "upcoming run(s) cancelled" in row["detail"]


def test_stopping_a_rescan_from_discord_names_the_member(fake_bot):
    """`/rescan cancel:True` talks to the worker directly, not to the service."""
    from bot.commands import _cancel_rescan

    job = fake_bot.rescans.submit([str(WATCHED_CHANNEL)], window="week", source="slash")
    interaction = SlashInteraction(fake_bot, user_id=1001)

    asyncio.run(_cancel_rescan(interaction, fake_bot))

    (row,) = rows_for(fake_bot, "rescan_stop")
    assert (row["surface"], row["actor"], row["subject"]) == ("discord", "1001", job.id)


def test_a_refused_command_writes_nothing(fake_bot, seeded):
    """1003 is on the other party's run, so `/rsvp` turns them down."""
    from bot.commands import rsvp

    interaction = SlashInteraction(fake_bot, user_id=1003)

    asyncio.run(rsvp.callback(interaction, run_id=seeded["run_star"], answer=yes()))

    assert fake_bot.repo.list_audit() == []
    assert interaction.sent[0].startswith("❌")


def test_an_actor_does_not_outlive_the_block_it_was_set_for():
    """Whatever a command is doing, the next thing to run is not that member."""
    actor = audit.Actor("discord", "1001")
    with audit.acting(actor):
        assert audit.current() is actor
    assert audit.current() is audit.SYSTEM


# --- the trail must never cost a change -------------------------------------


def test_a_failed_audit_write_does_not_break_the_mutation(fake_bot, seeded, monkeypatch):
    def boom(**_kwargs):
        raise RuntimeError("the disk is full")

    monkeypatch.setattr(fake_bot.repo, "log_audit", boom)
    app = build(fake_bot)
    with client_for(app) as client:
        response = client.post(f"/api/runs/{seeded['run_star']}/cancel")

    assert response.status_code == 200
    assert fake_bot.repo.get_run(seeded["run_star"])["status"] == "cancelled"


# --- reading it back --------------------------------------------------------


def test_the_api_lists_the_trail_newest_first(fake_bot, seeded):
    app = build(fake_bot)
    with client_for(app) as client:
        client.post(f"/api/runs/{seeded['run_star']}/cancel")
        client.request("PUT", "/api/config", json={"paused": True})
        rows = client.get("/api/audit").json()

    assert [row["action"] for row in rows] == ["config", "cancel"]
    assert rows[0]["surface"] == "portal"
    assert rows[0]["short_subject"] == "paused"  # not a uuid, so not cut in half
    assert rows[1]["short_subject"] == seeded["run_star"][:8]


def test_the_audit_page_renders_empty_and_populated(auth, seeded):
    empty = auth.get("/audit")
    assert empty.status_code == 200
    assert "Nothing recorded yet" in empty.text
    assert 'href="/audit"' in empty.text  # and it is in the nav

    auth.post(f"/api/runs/{seeded['run_star']}/cancel")
    body = auth.get("/audit").text
    assert "planned -&gt; cancelled" in body
    assert "token" in body
