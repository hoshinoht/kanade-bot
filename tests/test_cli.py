"""``bossctl``: argument handling, output, and what it sends over HTTP.

The API is mocked with respx rather than run, so these tests cover the CLI's own
job -- turning arguments into requests, rendering the answer, and exiting
non-zero with the server's message when something is refused.
"""

from __future__ import annotations

import json

import httpx
import pytest
import respx
from typer.testing import CliRunner

from bot import cli

BASE = "http://127.0.0.1:8080"
TOKEN = "cli-test-token"

runner = CliRunner()


@pytest.fixture(autouse=True)
def _env(monkeypatch, tmp_path):
    """Never read the developer's real ``.env`` or reach a real bot."""
    monkeypatch.setenv("ADMIN_TOKEN", TOKEN)
    monkeypatch.setenv("BOSSCTL_URL", BASE)
    monkeypatch.chdir(tmp_path)


@pytest.fixture
def api():
    with respx.mock(base_url=BASE, assert_all_called=False) as mock:
        yield mock


def run(*args: str):
    return runner.invoke(cli.app, list(args))


SCHEDULE = {
    "week": "this",
    "week_start": "2026-08-26T16:00:00+00:00",
    "week_label": "Thu 27 Aug",
    "timezone": "Asia/Kuala_Lumpur",
    "show_past": False,
    "hidden": 0,
    "count": 1,
    "runs": [],
    "days": [
        {
            "heading": "Mon 31 Aug",
            "runs": [
                {
                    "id": "aaaaaaaa-1111-2222-3333-444444444444",
                    "short_id": "aaaaaaaa",
                    "bosses": ["HStar", "HFA"],
                    "local_time": "21:30",
                    "local_day": "Mon 31 Aug",
                    "status": "planned",
                    "status_label": "⚠️ unconfirmed",
                    "participants": [{"id": "1", "name": "Alvin", "rsvp": "yes"}],
                    "yes": 1,
                    "no": 0,
                }
            ],
        }
    ],
}


# --- configuration ----------------------------------------------------------


def test_the_token_comes_from_the_environment(monkeypatch):
    assert cli.Api().token == TOKEN


def test_the_token_falls_back_to_a_dotenv_beside_the_working_directory(monkeypatch, tmp_path):
    monkeypatch.delenv("ADMIN_TOKEN")
    (tmp_path / ".env").write_text("ADMIN_TOKEN=from-dot-env\n")
    assert cli.load_token() == "from-dot-env"


def test_a_dotenv_above_the_working_directory_is_found(monkeypatch, tmp_path):
    monkeypatch.delenv("ADMIN_TOKEN")
    (tmp_path / ".env").write_text("ADMIN_TOKEN=from-above\n")
    nested = tmp_path / "a" / "b"
    nested.mkdir(parents=True)
    monkeypatch.chdir(nested)
    assert cli.load_token() == "from-above"


def test_a_commented_out_token_reads_as_missing(monkeypatch, tmp_path):
    monkeypatch.delenv("ADMIN_TOKEN")
    (tmp_path / ".env").write_text("ADMIN_TOKEN=  # generate with: openssl rand -hex 32\n")
    with pytest.raises(cli.ApiFailed, match="no ADMIN_TOKEN"):
        cli.load_token()


def test_no_token_anywhere_says_how_to_make_one(monkeypatch, tmp_path):
    monkeypatch.delenv("ADMIN_TOKEN")
    with pytest.raises(cli.ApiFailed, match="openssl rand -hex 32"):
        cli.load_token()


def test_the_url_defaults_to_loopback(monkeypatch):
    monkeypatch.delenv("BOSSCTL_URL")
    assert cli.base_url() == cli.DEFAULT_URL


# --- the happy paths --------------------------------------------------------


def test_schedule_prints_a_table(api):
    api.get("/api/schedule").mock(return_value=httpx.Response(200, json=SCHEDULE))
    result = run("schedule")
    assert result.exit_code == 0
    assert "Boss week of Thu 27 Aug" in result.output
    assert "Mon 31 Aug" in result.output
    assert "HStar + HFA" in result.output
    assert "aaaaaaaa" in result.output


def test_schedule_passes_its_filters_through(api):
    route = api.get("/api/schedule").mock(return_value=httpx.Response(200, json=SCHEDULE))
    run("schedule", "--week", "next", "--channel", "555", "--user", "1001")
    query = dict(route.calls.last.request.url.params)
    assert query == {"week": "next", "channel": "555", "user": "1001"}


def test_all_is_the_default_and_sends_no_filter(api):
    route = api.get("/api/schedule").mock(return_value=httpx.Response(200, json=SCHEDULE))
    run("schedule", "--all")
    assert dict(route.calls.last.request.url.params) == {"week": "this"}


def test_past_runs_are_hidden_and_the_count_is_reported(api):
    api.get("/api/schedule").mock(return_value=httpx.Response(200, json={**SCHEDULE, "hidden": 3}))
    # rich wraps the header at the terminal width, so match on the words only.
    assert "past/cancelled hidden" in run("schedule").output.replace("\n", " ")


def test_all_statuses_asks_for_them(api):
    route = api.get("/api/schedule").mock(return_value=httpx.Response(200, json=SCHEDULE))
    run("schedule", "--all-statuses")
    assert dict(route.calls.last.request.url.params) == {"week": "this", "show_past": "true"}


def test_an_empty_week_says_so_instead_of_printing_an_empty_table(api):
    api.get("/api/schedule").mock(
        return_value=httpx.Response(200, json={**SCHEDULE, "days": [], "count": 0})
    )
    result = run("schedule")
    assert "Nothing left" in result.output


def test_amend_sends_the_phrase_verbatim(api):
    route = api.post("/api/runs/aaaa/amend").mock(
        return_value=httpx.Response(
            200,
            json={"bosses": ["HStar"], "local_day": "Wed 02 Sep", "local_time": "21:45"},
        )
    )
    result = run("amend", "aaaa", "--to", "wed 9:45pm")
    assert result.exit_code == 0
    assert json.loads(route.calls.last.request.content) == {"to": "wed 9:45pm"}
    assert "Wed 02 Sep 21:45" in result.output


def test_cancel_and_otot(api):
    api.post("/api/runs/aaaa/cancel").mock(
        return_value=httpx.Response(200, json={"bosses": ["HStar"]})
    )
    api.post("/api/runs/aaaa/otot").mock(
        return_value=httpx.Response(200, json={"bosses": ["HStar"]})
    )
    assert run("cancel", "aaaa").exit_code == 0
    assert "own-time" in run("otot", "aaaa").output


def test_rsvp_needs_a_user_and_reports_the_new_status(api):
    route = api.post("/api/runs/aaaa/rsvp").mock(
        return_value=httpx.Response(
            200,
            json={
                "yes": 2,
                "no": 0,
                "participants": [{"id": "1"}, {"id": "2"}],
                "status_label": "✅ confirmed",
            },
        )
    )
    result = run("rsvp", "aaaa", "yes", "--user", "1001")
    assert json.loads(route.calls.last.request.content) == {"user_id": "1001", "answer": "yes"}
    assert "2/2 on" in result.output
    assert "confirmed" in result.output


def test_rsvp_without_a_user_is_a_usage_error(api):
    assert run("rsvp", "aaaa", "yes").exit_code != 0


def test_pending_prints_the_evidence(api):
    api.get("/api/pending").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "short_id": "bbbbbbbb",
                    "kind_label": "move",
                    "bosses": ["HStar"],
                    "confidence": 0.82,
                    "channel_name": "#hstar-party",
                    "channel_id": "1",
                    "when": "Wed 02 Sep 21:30",
                    "run": None,
                    "evidence": [
                        {
                            "missing": False,
                            "local_time": "Sun 30 Aug 11:50",
                            "author_name": "kanon",
                            "content": "can change to wed?",
                        }
                    ],
                }
            ],
        )
    )
    result = run("pending")
    assert "bbbbbbbb" in result.output
    assert "can change to wed?" in result.output


def test_pending_with_nothing_waiting(api):
    api.get("/api/pending").mock(return_value=httpx.Response(200, json=[]))
    assert "Nothing waiting" in run("pending").output


def test_approve_and_reject(api):
    approve = api.post("/api/amendments/bbbb/approve").mock(
        return_value=httpx.Response(200, json={"kind": "move", "short_id": "bbbbbbbb"})
    )
    api.post("/api/amendments/bbbb/reject").mock(
        return_value=httpx.Response(200, json={"short_id": "bbbbbbbb"})
    )
    assert "Applied the move" in run("approve", "bbbb").output
    assert json.loads(approve.calls.last.request.content) == {}
    assert "Rejected" in run("reject", "bbbb").output


def test_approve_can_name_the_actor(api):
    route = api.post("/api/amendments/bbbb/approve").mock(
        return_value=httpx.Response(200, json={"kind": "move", "short_id": "bbbbbbbb"})
    )
    run("approve", "bbbb", "--actor", "1001")
    assert json.loads(route.calls.last.request.content) == {"actor_id": "1001"}


def test_members_and_nick(api):
    api.get("/api/members").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "user_id": "1001",
                    "display_name": "Alvin",
                    "nickname": None,
                    "aliases": ["jo"],
                    "runs_this_week": 2,
                }
            ],
        )
    )
    assert "Alvin" in run("members").output
    route = api.post("/api/members/1001/nick").mock(
        return_value=httpx.Response(200, json={"name": "Alvin", "aliases": ["jo", "MY"]})
    )
    result = run("nick", "1001", "MY")
    assert json.loads(route.calls.last.request.content) == {"alias": "MY"}
    assert "also known as: jo, MY" in result.output


def test_member_pings_resolves_a_name_and_sets_the_level(api):
    api.get("/api/members").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "user_id": "1001",
                    "display_name": "Alvin tan",
                    "nickname": None,
                    "aliases": ["jo"],
                    "has_role": True,
                    "ping_level": "essential",
                    "runs_this_week": 2,
                },
                {
                    "user_id": "1002",
                    "display_name": "kanon [AZUR]",
                    "nickname": "kanon",
                    "aliases": [],
                    "has_role": True,
                    "ping_level": "essential",
                    "runs_this_week": 1,
                },
            ],
        )
    )
    route = api.patch("/api/members/1002").mock(
        return_value=httpx.Response(
            200, json={"user_id": "1002", "display_name": "kanon [AZUR]", "ping_level": "off"}
        )
    )
    # A name, not a snowflake: nobody remembers ids.
    result = run("member", "pings", "kanon", "off")
    assert json.loads(route.calls.last.request.content) == {"ping_level": "off"}
    assert "off" in result.output

    # ...and an id still works.
    run("member", "pings", "1002", "all")
    assert json.loads(route.calls.last.request.content) == {"ping_level": "all"}


def test_member_pings_says_so_when_the_name_matches_nobody(api):
    api.get("/api/members").mock(return_value=httpx.Response(200, json=[]))
    result = run("member", "pings", "nobody", "off")
    assert result.exit_code == 1
    assert "no member matches" in result.output


def test_the_members_table_shows_each_ping_level(api):
    api.get("/api/members").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "user_id": "1001",
                    "display_name": "Alvin",
                    "nickname": None,
                    "aliases": [],
                    "ping_level": "off",
                    "runs_this_week": 0,
                }
            ],
        )
    )
    assert "off" in run("members").output


def test_fixed_list_add_edit_and_rm(api):
    row = {
        "short_id": "cccccccc",
        "weekday_name": "Mon",
        "time": "21:30",
        "bosses": ["HStar", "HFA"],
        "participants": [{"id": "1", "name": "Alvin"}],
        "channel_name": "#hstar-party",
        "channel_id": "5",
        "owner_name": "Alvin",
    }
    api.get("/api/fixed").mock(return_value=httpx.Response(200, json=[row]))
    assert "cccccccc" in run("fixed", "list").output

    create = api.post("/api/fixed").mock(return_value=httpx.Response(201, json=row))
    result = run(
        "fixed",
        "add",
        "-b",
        "hstar, hfa",
        "-d",
        "mon",
        "-t",
        "21:30",
        "-c",
        "5",
        "-m",
        "1001",
        "-m",
        "1002",
    )
    assert result.exit_code == 0
    assert json.loads(create.calls.last.request.content) == {
        "bosses": "hstar, hfa",
        "day": "mon",
        "time": "21:30",
        "channel_id": "5",
        "participants": ["1001", "1002"],
        "owner_id": None,
        "note": None,
    }

    patch = api.patch("/api/fixed/cccc").mock(return_value=httpx.Response(200, json=row))
    run("fixed", "edit", "cccc", "-t", "22:00")
    assert json.loads(patch.calls.last.request.content) == {"time": "22:00"}

    api.delete("/api/fixed/cccc").mock(
        return_value=httpx.Response(200, json={"short_id": "cccccccc", "cancelled_runs": 1})
    )
    assert "1 upcoming run(s) cancelled" in run("fixed", "rm", "cccc").output


def test_fixed_edit_with_no_fields_exits_non_zero(api):
    result = run("fixed", "edit", "cccc")
    assert result.exit_code == 1
    assert "nothing to change" in result.output


def test_config_get_and_set(api):
    values = {
        "day_of_ping_time": "09:00",
        "countdown_minutes": "60,15",
        "paused": False,
        "extract_enabled": True,
        "watched_channels": ["5", "6"],
    }
    api.get("/api/config").mock(return_value=httpx.Response(200, json=values))
    assert "day_of_ping_time" in run("config", "get").output
    assert run("config", "get", "day_of_ping_time").output.strip() == "09:00"
    assert run("config", "get", "nope").exit_code == 1

    route = api.put("/api/config").mock(
        return_value=httpx.Response(200, json={**values, "day_of_ping_time": "08:30"})
    )
    result = run("config", "set", "day_of_ping_time", "08:30")
    assert json.loads(route.calls.last.request.content) == {"day_of_ping_time": "08:30"}
    assert "day_of_ping_time = 08:30" in result.output


def test_config_set_turns_a_flag_into_a_boolean(api):
    route = api.put("/api/config").mock(
        return_value=httpx.Response(200, json={"paused": True, "extract_enabled": False})
    )
    run("config", "set", "paused", "true")
    assert json.loads(route.calls.last.request.content) == {"paused": True}
    run("config", "set", "extract_enabled", "off")
    assert json.loads(route.calls.last.request.content) == {"extract_enabled": False}


CHANNEL_RESCAN = {
    "channel_id": "5",
    "channel_name": "#hstar-party",
    "asked": True,
    "window": "week",
    "since": "2026-08-26T16:00:00+00:00",
    "widened": False,
    "backfilled": 312,
    "stored": 40,
    "gated": 18,
    "bursts": 4,
    "extracted": 4,
    "proposals": 1,
    "dropped": 3,
    "stale": 2,
    "elapsed_ms": 42000,
    "error": None,
    "summary": "",
    "proposed": [{"kind": "move", "bosses": ["HStar"], "confidence": 0.9, "run_id": None}],
}

RESCAN = {
    "window": "week",
    "channels": [CHANNEL_RESCAN],
    "asked": True,
    "widened": False,
    "backfilled": 312,
    "gated": 18,
    "bursts": 4,
    "extracted": 4,
    "proposals": 1,
    "dropped": 3,
    "stale": 2,
    "elapsed_ms": 42000,
    "errors": [],
    "proposed": [{"kind": "move", "bosses": ["HStar"], "confidence": 0.9, "run_id": None}],
}


JOB = {
    "job_id": "job-1234-5678",
    "short_id": "job-1234",
    "status": "done",
    "window": "week",
    "source": "manual",
    "automated": False,
    "channels": ["5"],
    "channel_names": ["#hstar-party"],
    "current": None,
    "done": 1,
    "total": 1,
    "percent": 100,
    "elapsed_ms": 42000,
    "running": False,
    "error": None,
    "results": [CHANNEL_RESCAN],
    "totals": RESCAN,
}


def test_rescan_queues_and_then_follows(api):
    queued = api.post("/api/rescan").mock(
        return_value=httpx.Response(202, json={**JOB, "status": "queued", "running": True})
    )
    api.get("/api/rescan/job-1234-5678").mock(return_value=httpx.Response(200, json=JOB))
    result = run("rescan")
    assert json.loads(queued.calls.last.request.content) == {"channels": [], "window": "week"}
    output = " ".join(result.output.split())
    assert "Queued job-1234" in output
    assert "#hstar-party: 312 pulled" in output
    assert "1 card(s) posted, 3 dropped (2 already passed)" in output


def test_rescan_can_just_queue_and_leave(api):
    api.post("/api/rescan").mock(
        return_value=httpx.Response(202, json={**JOB, "status": "queued", "running": True})
    )
    polled = api.get("/api/rescan/job-1234-5678").mock(return_value=httpx.Response(200, json=JOB))
    run("rescan", "--no-wait")
    assert not polled.calls


def test_rescan_takes_channels_and_a_window(api):
    route = api.post("/api/rescan").mock(return_value=httpx.Response(202, json=JOB))
    api.get("/api/rescan/job-1234-5678").mock(return_value=httpx.Response(200, json=JOB))
    run("rescan", "-c", "5", "-c", "6", "-w", "2weeks")
    assert json.loads(route.calls.last.request.content) == {
        "channels": ["5", "6"],
        "window": "2weeks",
    }


def test_rescan_says_when_a_channel_widened_to_last_week(api):
    api.post("/api/rescan").mock(return_value=httpx.Response(202, json=JOB))
    api.get("/api/rescan/job-1234-5678").mock(
        return_value=httpx.Response(
            200, json={**JOB, "results": [{**CHANNEL_RESCAN, "widened": True}]}
        )
    )
    assert "checked last week too" in " ".join(run("rescan").output.split())


def test_rescan_reports_a_model_failure_and_exits_non_zero(api):
    api.post("/api/rescan").mock(return_value=httpx.Response(202, json=JOB))
    api.get("/api/rescan/job-1234-5678").mock(
        return_value=httpx.Response(
            200, json={**JOB, "totals": {**RESCAN, "errors": ["connection refused"]}}
        )
    )
    result = run("rescan")
    assert result.exit_code == 1
    assert "connection refused" in result.output


def test_a_cancelled_rescan_says_how_far_it_got(api):
    api.post("/api/rescan").mock(return_value=httpx.Response(202, json=JOB))
    api.get("/api/rescan/job-1234-5678").mock(
        return_value=httpx.Response(200, json={**JOB, "status": "cancelled", "done": 1, "total": 3})
    )
    assert "Stopped after 1 of 3" in " ".join(run("rescan").output.split())


def test_rescan_stop_targets_the_running_job(api):
    api.get("/api/rescan").mock(
        return_value=httpx.Response(200, json=[{**JOB, "status": "running", "running": True}])
    )
    route = api.delete("/api/rescan/job-1234-5678").mock(
        return_value=httpx.Response(200, json={**JOB, "status": "cancelled"})
    )
    assert "will stop" in run("rescan-stop").output
    assert route.calls


def test_rescan_stop_with_nothing_running(api):
    api.get("/api/rescan").mock(return_value=httpx.Response(200, json=[]))
    assert "Nothing is running" in run("rescan-stop").output


def test_rescans_lists_the_last_few(api):
    api.get("/api/rescan").mock(return_value=httpx.Response(200, json=[JOB]))
    output = " ".join(run("rescans").output.split())
    assert "job-1234" in output
    assert "#hstar-party" in output


def test_channels_lists_what_a_rescan_would_cover(api):
    api.get("/api/rescan/targets").mock(
        return_value=httpx.Response(
            200,
            json=[
                {"id": "5", "name": "#hstar-party", "has_runs": True},
                {"id": "9", "name": "#boss-chat-general", "has_runs": False},
            ],
        )
    )
    output = run("channels").output
    assert "#hstar-party" in output
    assert "#boss-chat-general" in output


def test_digest_and_ping_print_the_message_link(api):
    api.post("/api/digest").mock(
        return_value=httpx.Response(
            200, json={"week": "this", "url": "https://discord.com/channels/1/2/3"}
        )
    )
    assert "discord.com/channels/1/2/3" in run("digest").output
    api.post("/api/debug/ping").mock(
        return_value=httpx.Response(200, json={"url": "https://discord.com/channels/1/2/4"})
    )
    assert "channels/1/2/4" in run("ping", "aaaa", "day_of").output


def test_extractions_and_one_extraction(api):
    api.get("/api/extractions").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "short_id": "dddddddd",
                    "local_time": "Sun 30 Aug 12:00:00",
                    "model": "gpt-oss:20b",
                    "latency_ms": 900,
                    "message_ids": ["1", "2"],
                    "amendment_count": 1,
                }
            ],
        )
    )
    assert "dddddddd" in run("extractions").output
    api.get("/api/extractions/dddd").mock(
        return_value=httpx.Response(
            200,
            json={
                "short_id": "dddddddd",
                "local_time": "Sun 30 Aug 12:00:00",
                "model": "gpt-oss:20b",
                "latency_ms": 900,
                "prompt": "THE PROMPT",
                "raw_response": "{}",
                "amendments": [
                    {"kind": "move", "bosses": ["HStar"], "when": "Wed 21:30", "status": "proposed"}
                ],
            },
        )
    )
    result = run("extraction", "dddd")
    assert "THE PROMPT" in result.output
    assert "move HStar" in result.output


def test_chat_and_one_interaction(api):
    api.get("/api/chat").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "short_id": "eeeeeeee",
                    "local_time": "Sun 30 Aug 12:00:00",
                    "author_name": "kanon",
                    "model": "qwen3:32b",
                    "rounds": 2,
                    "tool_names": ["get_schedule"],
                    "latency_ms": 8400,
                    "outcome": "answered",
                }
            ],
        )
    )
    api.get("/api/chat/summary").mock(
        return_value=httpx.Response(
            200,
            json={
                "models": [
                    {
                        "model": "qwen3:32b",
                        "count": 1,
                        "failed": 0,
                        "prompt_tokens": 3120,
                        "completion_tokens": 64,
                    }
                ]
            },
        )
    )
    listing = run("chat").output
    assert "eeeeeeee" in listing
    assert "get_schedule" in listing
    assert "3,120" in listing

    api.get("/api/chat/eeee").mock(
        return_value=httpx.Response(
            200,
            json={
                "short_id": "eeeeeeee",
                "local_time": "Sun 30 Aug 12:00:00",
                "model": "qwen3:32b",
                "latency_ms": 8400,
                "rounds": 2,
                "outcome": "answered",
                "author_name": "kanon",
                "question": "what's on tonight?",
                "reply": "Star at 21:30.",
                "tool_calls": [
                    {
                        "name": "get_schedule",
                        "arguments": "week='this'",
                        "ms": 120,
                        "outcome": "ok",
                        "created": [{"short_id": "ffffffff"}],
                    }
                ],
            },
        )
    )
    result = run("interaction", "eeee")
    assert "what's on tonight?" in result.output
    assert "Star at 21:30." in result.output
    assert "get_schedule(week='this')" in result.output
    assert "card ffffffff" in result.output
    assert "THE PROMPT" not in run("extraction", "dddd", "--no-prompt").output


def test_export_writes_jsonl_to_a_file(api, tmp_path):
    body = '{"id":"1","content":"hi"}\n{"id":"2","content":"can"}\n'
    api.get("/api/messages").mock(
        return_value=httpx.Response(
            200, text=body, headers={"content-type": "application/x-ndjson"}
        )
    )
    out = tmp_path / "exports" / "chat.jsonl"
    result = run("export", "--channel", "5", "--since", "2026-08-01", "--out", str(out))
    assert result.exit_code == 0
    assert out.read_text().count("\n") == 2
    assert "2 message(s)" in result.output


def test_export_to_stdout(api):
    api.get("/api/messages").mock(
        return_value=httpx.Response(
            200, text='{"id":"1"}\n', headers={"content-type": "application/x-ndjson"}
        )
    )
    result = run("export", "-c", "5", "--since", "2026-08-01")
    assert '{"id":"1"}' in result.output


def test_reminders(api):
    api.get("/api/reminders").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "run_short_id": "aaaaaaaa",
                    "kind": "day_of",
                    "local_fire_at": "Mon 31 Aug 09:00",
                    "sent_at": None,
                    "bosses": ["HStar"],
                }
            ],
        )
    )
    output = run("reminders").output
    assert "day_of" in output
    assert "queued" in output


# --- failures ---------------------------------------------------------------


def test_an_api_error_is_printed_and_exits_non_zero(api):
    api.get("/api/schedule").mock(
        return_value=httpx.Response(400, json={"error": "week must be `this` or `next`"})
    )
    result = runner.invoke(cli.app, ["schedule"], standalone_mode=False)
    assert isinstance(result.exception, cli.ApiFailed)
    assert "week must be" in str(result.exception)


def test_main_turns_an_api_error_into_exit_code_1(api, capsys):
    api.get("/api/schedule").mock(return_value=httpx.Response(404, json={"error": "gone"}))
    import sys

    argv = sys.argv
    sys.argv = ["bossctl", "schedule"]
    try:
        assert cli.main() == 1
    finally:
        sys.argv = argv
    assert "gone" in capsys.readouterr().err


def test_an_unreachable_bot_says_to_check_the_container(api):
    api.get("/api/schedule").mock(side_effect=httpx.ConnectError("connection refused"))
    with pytest.raises(cli.ApiFailed, match="docker compose ps"):
        cli.Api().get("/api/schedule")


def test_a_non_json_error_body_is_still_reported(api):
    api.get("/api/schedule").mock(return_value=httpx.Response(502, text="Bad Gateway"))
    with pytest.raises(cli.ApiFailed, match="HTTP 502"):
        cli.Api().get("/api/schedule")


def test_an_error_while_streaming_an_export_is_reported(api):
    api.get("/api/messages").mock(
        return_value=httpx.Response(400, json={"error": "only watched channels"})
    )
    with pytest.raises(cli.ApiFailed, match="only watched channels"):
        list(cli.Api().stream_lines("/api/messages", channel="9", since="2026-08-01"))


def test_every_command_carries_the_bearer_token(api):
    route = api.get("/api/members").mock(return_value=httpx.Response(200, json=[]))
    run("members")
    assert route.calls.last.request.headers["authorization"] == f"Bearer {TOKEN}"


# --- status, restore and access (items 8 and 10) ----------------------------

RUN_RESULT = {
    "bosses": ["HStar"],
    "local_day": "Wed 02 Sep",
    "local_time": "21:30",
    "status": "otot",
    "status_label": "🕒 own time",
}


def test_status_patches_the_run(api):
    route = api.patch("/api/runs/aaaa/status").mock(
        return_value=httpx.Response(200, json=RUN_RESULT)
    )
    result = run("status", "aaaa", "otot")
    assert json.loads(route.calls.last.request.content) == {"status": "otot"}
    assert "own time" in result.output


def test_an_invalid_status_comes_back_from_the_server(api):
    """The server owns the list of statuses; the CLI just relays the refusal."""
    api.patch("/api/runs/aaaa/status").mock(
        return_value=httpx.Response(422, json={"error": "`maybe` is not a status you can set"})
    )
    result = runner.invoke(cli.app, ["status", "aaaa", "maybe"], standalone_mode=False)
    assert isinstance(result.exception, cli.ApiFailed)
    assert "not a status you can set" in str(result.exception)


def test_restore_puts_a_run_back(api):
    api.post("/api/runs/aaaa/restore").mock(
        return_value=httpx.Response(
            200, json={**RUN_RESULT, "status": "planned", "status_label": "⚠️ unconfirmed"}
        )
    )
    assert "back on for Wed 02 Sep 21:30" in run("restore", "aaaa").output


def test_access_shows_a_tick_per_permission(api):
    api.get("/api/access").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "id": "5",
                    "name": "#hstar-party",
                    "watched": True,
                    "is_digest_channel": False,
                    "view": True,
                    "send": False,
                    "history": True,
                    "embed": True,
                    "react": True,
                    "unknown": False,
                }
            ],
        )
    )
    output = run("access").output
    assert "#hstar-party" in output
    assert "cannot post there" in output


def test_access_when_the_bot_is_offline_exits_non_zero(api):
    api.get("/api/access").mock(return_value=httpx.Response(200, json=[]))
    result = run("access")
    assert result.exit_code == 1
    assert "isn't connected" in result.output


# --- limits -----------------------------------------------------------------

IDLE_LIMITS = {
    "model": {"busy": False, "holder": None, "held_for_s": 0.0},
    "global_pool": {"count": 12, "window_s": 900.0, "used": 0, "remaining": 12},
    "per_user": {"count": 4, "window_s": 300.0, "windows": [], "overrides": []},
    "pilots": [],
    "jobs": {
        "answering": [],
        "extracting": False,
        "rescan": {"worker_running": True, "queued": 0, "job": None, "channel": None},
    },
}


def busy_limits() -> dict:
    """The same payload with something happening in every part of it."""
    return {
        "model": {"busy": True, "holder": "extractor", "held_for_s": 12.5},
        "global_pool": {"count": 12, "window_s": 900.0, "used": 5, "remaining": 7},
        "per_user": {
            "count": 4,
            "window_s": 300.0,
            "windows": [
                {
                    "user_id": "1002",
                    "name": "kanon",
                    "used": 4,
                    "remaining": 0,
                    "count": 4,
                    "window_s": 300.0,
                    "overridden": False,
                },
                {
                    "user_id": "1001",
                    "name": "Alvin",
                    "used": 6,
                    "remaining": 4,
                    "count": 10,
                    "window_s": 600.0,
                    "overridden": True,
                },
            ],
            "overrides": [{"user_id": "1001", "name": "Alvin", "count": 10, "window_s": 600.0}],
        },
        "pilots": [
            {
                "user_id": "1002",
                "name": "kanon",
                "staff": False,
                "count": 4,
                "window_s": 300.0,
                "overridden": False,
                "used": 4,
                "remaining": 0,
                "has_window": True,
            },
            {
                "user_id": "1001",
                "name": "Alvin",
                "staff": False,
                "count": 10,
                "window_s": 600.0,
                "overridden": True,
                "used": 6,
                "remaining": 4,
                "has_window": True,
            },
            {
                "user_id": "1003",
                "name": "Priya",
                "staff": False,
                "count": 4,
                "window_s": 300.0,
                "overridden": False,
                "used": 0,
                "remaining": 4,
                "has_window": False,
            },
            {
                "user_id": "1009",
                "name": "Hoshino",
                "staff": True,
                "count": 4,
                "window_s": 300.0,
                "overridden": False,
                "used": 0,
                "remaining": 4,
                "has_window": False,
            },
        ],
        "jobs": {
            "answering": [{"channel_id": "5", "channel_name": "#ask-the-bot"}],
            "extracting": True,
            "rescan": {
                "worker_running": True,
                "queued": 2,
                "job": "abcd1234",
                "channel": "#hstar-party",
            },
        },
    }


def test_limits_says_the_model_is_idle(api):
    api.get("/api/limits").mock(return_value=httpx.Response(200, json=IDLE_LIMITS))
    output = run("limits").output
    assert "idle" in output
    assert "12 of 12 left" in output


def test_limits_reports_the_holder_the_budgets_and_the_queue(api):
    api.get("/api/limits").mock(return_value=httpx.Response(200, json=busy_limits()))
    output = run("limits").output
    assert "busy" in output and "extractor" in output
    assert "7 of 12 left" in output
    assert "2 queued" in output and "#hstar-party" in output
    assert "#ask-the-bot" in output
    # The window table names the member and what they have spent.
    assert "kanon" in output and "4 of 4" in output


def test_limits_reset_takes_a_bare_user_id_without_the_roster(api):
    """Holding the chat role does not require being on the bossing roster."""
    route = api.delete("/api/limits/windows/1002").mock(
        return_value=httpx.Response(200, json={"user_id": "1002", "name": "kanon"})
    )
    output = run("limits", "reset", "1002").output
    assert route.called
    assert "kanon's answer window is clear" in output


def test_limits_reset_accepts_a_mention(api):
    route = api.delete("/api/limits/windows/1002").mock(
        return_value=httpx.Response(200, json={"user_id": "1002", "name": "kanon"})
    )
    assert run("limits", "reset", "<@1002>").exit_code == 0
    assert route.called


def test_limits_reset_resolves_a_name_through_the_roster(api):
    api.get("/api/members").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "user_id": "1002",
                    "display_name": "kanon [AZUR]",
                    "nickname": "kanon",
                    "aliases": [],
                    "has_role": True,
                    "ping_level": "essential",
                    "runs_this_week": 1,
                }
            ],
        )
    )
    route = api.delete("/api/limits/windows/1002").mock(
        return_value=httpx.Response(200, json={"user_id": "1002", "name": "kanon"})
    )
    assert run("limits", "reset", "kanon").exit_code == 0
    assert route.called


def test_limits_reset_says_so_when_the_name_matches_nobody(api):
    api.get("/api/members").mock(return_value=httpx.Response(200, json=[]))
    result = run("limits", "reset", "nobody")
    assert result.exit_code == 1
    assert "no member matches" in result.output


def test_limits_lists_who_is_on_their_own_allowance(api):
    api.get("/api/limits").mock(return_value=httpx.Response(200, json=busy_limits()))
    output = run("limits").output
    assert "own allowance" in output
    # The overridden window is counted against its own number, not the default.
    assert "6 of 10" in output
    assert "Alvin" in output and "600s" in output


def test_limits_set_sends_the_allowance_as_numbers(api):
    route = api.put("/api/limits/overrides/1002").mock(
        return_value=httpx.Response(
            200, json={"user_id": "1002", "name": "kanon", "count": 10, "window_s": 60.0}
        )
    )
    output = run("limits", "set", "1002", "10", "60").output
    assert json.loads(route.calls[0].request.content) == {"count": 10, "window_s": 60.0}
    assert "kanon now gets 10 answer(s) per 60s" in output


def test_limits_set_resolves_a_name_like_the_other_commands(api):
    api.get("/api/members").mock(
        return_value=httpx.Response(
            200,
            json=[
                {
                    "user_id": "1002",
                    "display_name": "kanon [AZUR]",
                    "nickname": "kanon",
                    "aliases": [],
                    "has_role": True,
                    "ping_level": "essential",
                    "runs_this_week": 1,
                }
            ],
        )
    )
    route = api.put("/api/limits/overrides/1002").mock(
        return_value=httpx.Response(
            200, json={"user_id": "1002", "name": "kanon", "count": 3, "window_s": 30.0}
        )
    )
    assert run("limits", "set", "kanon", "3", "30").exit_code == 0
    assert route.called


def test_limits_unset_puts_them_back_on_the_default(api):
    route = api.delete("/api/limits/overrides/1002").mock(
        return_value=httpx.Response(200, json={"user_id": "1002", "name": "kanon"})
    )
    output = run("limits", "unset", "1002").output
    assert route.called
    assert "back on the default allowance" in output


def test_limits_set_reports_what_the_api_refused(api):
    api.put("/api/limits/overrides/1002").mock(
        return_value=httpx.Response(400, json={"error": "count must be at least 1"})
    )
    result = runner.invoke(cli.app, ["limits", "set", "1002", "0", "60"], standalone_mode=False)
    assert isinstance(result.exception, cli.ApiFailed)
    assert "count must be at least 1" in str(result.exception)


def test_config_set_sends_a_capacity_number_as_a_number(api):
    route = api.put("/api/config").mock(
        return_value=httpx.Response(200, json={"chat_pilot_rate_count": 9})
    )
    output = run("config", "set", "chat_pilot_rate_count", "9").output
    assert json.loads(route.calls[0].request.content) == {"chat_pilot_rate_count": 9}
    assert "chat_pilot_rate_count = 9" in output


def test_config_set_refuses_a_capacity_value_that_is_not_a_number(api):
    result = run("config", "set", "chat_pilot_rate_window_s", "soon")
    assert result.exit_code == 1
    assert "must be a number" in result.output


def test_limits_lists_the_chat_role_holders(api):
    api.get("/api/limits").mock(return_value=httpx.Response(200, json=busy_limits()))
    output = run("limits").output
    assert "4 chat-role holder(s)" in output
    assert "4 per 300s" in output
    assert "10 per 600s" in output and "(own)" in output
    # Staff are marked and given no allowance to read.
    assert "Hoshino (staff)" in output
    assert "exempt" in output
    assert "idle" in output


def test_limits_says_when_no_holders_can_be_read(api):
    api.get("/api/limits").mock(return_value=httpx.Response(200, json=IDLE_LIMITS))
    assert "No chat-role holders to show" in run("limits").output.replace("\n", " ")
