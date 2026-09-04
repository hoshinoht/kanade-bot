"""What one answered question leaves behind in the database.

The chat log exists to answer two questions the Discord log cannot: what is the
chatbot costing, and which interactions went wrong. So these tests are about
what a row can be *used* for -- the tokens, the model/tool split, the trace with
its arguments -- and about the two ways the feature is allowed to fail: a
message nobody paid for must not be counted, and a write that cannot happen
must not cost the member their answer.

Every id here is synthetic; see `tests/chat_support.py`.
"""

from __future__ import annotations

import asyncio
import logging

import pytest

from bot.chat import gate
from bot.chat.agent import FAILURE_REPLY, MAX_TOOL_ROUNDS, ChatPilot
from bot.infrastructure.db import CHAT_INTERACTIONS_KEPT

from .chat_support import CHAT_CHANNEL, FakeOllama, costed, message, says, wants

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def only_row(bot) -> dict:
    rows = bot.repo.recent_chat_interactions()
    assert len(rows) == 1
    return rows[0]


# ---------------------------------------------------------------------------
# one interaction, one row
# ---------------------------------------------------------------------------


async def test_an_answered_question_lands_one_row(chat_bot, chat_seeded):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs this week."))
    asked = message(chat_bot, "@bot what's on?")
    await agent.offer(asked)

    row = only_row(chat_bot)
    assert row["question"] == "@bot what's on?"
    assert row["reply"] == "Two runs this week."
    assert row["outcome"] == "answered"
    assert row["error"] is None
    assert row["rounds"] == 2
    assert row["model"] == chat_bot.settings.chat_pilot_model
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["author_id"] == "1002"
    assert row["message_id"] == str(asked.id)


async def test_the_tool_trace_survives_intact(chat_bot, chat_seeded):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    await agent.offer(message(chat_bot))

    (call,) = only_row(chat_bot)["tool_calls"]
    assert call["name"] == "get_schedule"
    assert call["arguments"] == "week='this'"
    assert call["outcome"] == "ok"
    assert call["created"] == []
    assert isinstance(call["ms"], int)


async def test_provider_rounds_and_full_tool_returns_are_preserved(chat_bot, chat_seeded):
    first = wants("get_schedule", week="this")
    first["message"]["content"] = "  looking it up\n"
    first["message"]["thinking"] = "check the schedule\nthen answer"
    second = {
        "message": {
            "content": "  Two runs.\n",
            "thinking": None,
            "tool_calls": [],
        }
    }
    agent = pilot(chat_bot, first, second)
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["model_rounds"] == [
        {
            "round": 1,
            "content": "  looking it up\n",
            "thinking": "check the schedule\nthen answer",
            "requested_tools": ["get_schedule"],
        },
        {"round": 2, "content": "  Two runs.\n", "thinking": None, "requested_tools": []},
    ]
    (call,) = row["tool_calls"]
    assert call["round"] == 1
    assert "Hard Star + Hard FA" in call["output"]


async def test_a_refusal_is_recorded_as_a_refusal(chat_bot, chat_seeded):
    """The row has to disagree with a successful call, or it cannot be triaged."""
    agent = pilot(
        chat_bot, wants("propose_cancel", run_query="nonsense"), says("Couldn't find it.")
    )
    await agent.offer(message(chat_bot, "@bot cancel the thing"))

    (call,) = only_row(chat_bot)["tool_calls"]
    assert call["outcome"] != "ok"
    assert call["arguments"] == "run_query='nonsense'"
    # The answer still went out: a refused tool is not a failed interaction.
    assert only_row(chat_bot)["outcome"] == "answered"


async def test_a_card_the_interaction_raised_is_traced_to_the_call(chat_bot, chat_seeded):
    agent = pilot(
        chat_bot,
        wants("propose_move", run_query="hstar", to_when="wed 21:30"),
        says("Posted a card."),
    )
    await agent.offer(message(chat_bot, "@bot move star to wednesday"))

    (call,) = only_row(chat_bot)["tool_calls"]
    assert len(call["created"]) == 1
    # And it is a real amendment, not just a string that looks like an id.
    assert chat_bot.repo.get_amendment(call["created"][0]) is not None


async def test_long_arguments_are_truncated_the_way_the_debug_log_truncates_them(
    chat_bot, chat_seeded
):
    agent = pilot(chat_bot, wants("get_run", query="x" * 500), says("Sorry."))
    await agent.offer(message(chat_bot))

    (call,) = only_row(chat_bot)["tool_calls"]
    assert len(call["arguments"]) <= 200
    assert call["arguments"].endswith("…")


# ---------------------------------------------------------------------------
# what it cost
# ---------------------------------------------------------------------------


async def test_token_counts_are_summed_across_rounds(chat_bot, chat_seeded):
    agent = pilot(
        chat_bot,
        costed(wants("get_schedule", week="this"), 1200, 40),
        costed(says("Two runs."), 1500, 60),
    )
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["prompt_tokens"] == 2700
    assert row["completion_tokens"] == 100


async def test_a_model_that_reports_no_usage_records_none_rather_than_zero(chat_bot, chat_seeded):
    """Zero tokens and "the server did not say" are different facts about a bill."""
    agent = pilot(chat_bot, says("Two runs."))
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["prompt_tokens"] is None
    assert row["completion_tokens"] is None


async def test_a_partial_report_is_kept_rather_than_discarded(chat_bot, chat_seeded):
    agent = pilot(
        chat_bot,
        costed(wants("get_schedule", week="this"), 1200, 40),
        says("Two runs."),  # this round reports nothing
    )
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["prompt_tokens"] == 1200
    assert row["completion_tokens"] == 40


async def test_the_model_and_tool_split_is_recorded(chat_bot, chat_seeded):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["latency_ms"] >= 0
    assert row["model_ms"] >= 0
    assert row["tools_ms"] >= 0
    # The parts are parts: neither can exceed the whole.
    assert row["model_ms"] <= row["latency_ms"]
    assert row["tools_ms"] <= row["latency_ms"]


# ---------------------------------------------------------------------------
# failures are rows; refusals to answer at all are not
# ---------------------------------------------------------------------------


async def test_a_failed_generation_is_recorded_as_failed(chat_bot, chat_seeded):
    agent = pilot(chat_bot, RuntimeError("ollama is down"))
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["outcome"] == "failed"
    assert "ollama is down" in row["error"]
    assert row["reply"] == FAILURE_REPLY


async def test_a_model_that_never_stops_calling_tools_is_a_failure(chat_bot, chat_seeded):
    agent = pilot(chat_bot, *[wants("get_schedule", week="this")] * MAX_TOOL_ROUNDS)
    await agent.offer(message(chat_bot))

    row = only_row(chat_bot)
    assert row["outcome"] == "failed"
    assert row["rounds"] == MAX_TOOL_ROUNDS
    assert len(row["tool_calls"]) == MAX_TOOL_ROUNDS


async def test_a_rate_limited_question_is_not_recorded(chat_bot, chat_seeded):
    """Nothing was asked of the model, so counting it would skew every average."""
    agent = pilot(chat_bot, *[says("ok")] * 4)
    agent.limiter.count = 1
    await agent.offer(message(chat_bot))
    limited = message(chat_bot)
    await agent.offer(limited)

    assert limited.reactions == [gate.RATE_LIMITED_REACTION]
    assert len(chat_bot.repo.recent_chat_interactions()) == 1


async def test_a_question_dropped_because_the_channel_was_busy_is_not_recorded(
    chat_bot, chat_seeded
):
    released = asyncio.Event()
    agent = pilot(chat_bot)

    async def slow(**_kwargs):
        await released.wait()
        return says("first")

    agent._client.chat = slow
    first = asyncio.create_task(agent.offer(message(chat_bot)))
    await asyncio.sleep(0)
    busy = message(chat_bot, author_id=1001)
    await agent.offer(busy)
    released.set()
    await first

    assert busy.reactions == [gate.CHANNEL_BUSY_REACTION]
    assert len(chat_bot.repo.recent_chat_interactions()) == 1


async def test_a_message_the_gate_ignored_is_not_recorded(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("never asked"))
    await agent.offer(message(chat_bot, "not talking to the bot", mentions=()))

    assert chat_bot.repo.recent_chat_interactions() == []


# ---------------------------------------------------------------------------
# the write must never cost the answer
# ---------------------------------------------------------------------------


async def test_a_write_that_fails_still_leaves_the_member_with_an_answer(
    chat_bot, chat_seeded, caplog
):
    def explode(**_kwargs):
        raise RuntimeError("disk is full")

    chat_bot.repo.log_chat_interaction = explode
    agent = pilot(chat_bot, says("Two runs this week."))
    with caplog.at_level(logging.ERROR, logger="bot.chat.agent"):
        handling = await agent.offer(message(chat_bot))

    assert handling.answered.reply == "Two runs this week."
    assert chat_bot.posts[-1].content == "Two runs this week."
    assert any("could not record" in r.getMessage() for r in caplog.records)


# ---------------------------------------------------------------------------
# the log is bounded
# ---------------------------------------------------------------------------


def test_the_log_is_pruned_to_the_cap_on_insert(repo):
    for n in range(12):
        repo.log_chat_interaction(
            model="m", question=f"q{n}", reply="a", outcome="answered", keep=5
        )
    assert repo.count_chat_interactions() == 5


def test_pruning_keeps_the_newest(repo):
    from .conftest import kl

    for day in range(1, 8):
        repo.log_chat_interaction(
            model="m",
            question=f"day {day}",
            reply="a",
            outcome="answered",
            at=kl(2026, 8, day),
            keep=3,
        )
    kept = [row["question"] for row in repo.recent_chat_interactions()]
    assert kept == ["day 7", "day 6", "day 5"]


def test_the_default_cap_is_the_documented_one(repo):
    repo.log_chat_interaction(model="m", question="q", reply="a", outcome="answered")
    assert CHAT_INTERACTIONS_KEPT == 500
    assert repo.count_chat_interactions() == 1


# ---------------------------------------------------------------------------
# the per-model totals
# ---------------------------------------------------------------------------


def test_totals_are_grouped_by_model(repo):
    for model, outcome, latency in (
        ("a", "answered", 100),
        ("a", "failed", 300),
        ("b", "answered", 900),
    ):
        repo.log_chat_interaction(
            model=model,
            question="q",
            reply="a",
            outcome=outcome,
            latency_ms=latency,
            prompt_tokens=10,
            completion_tokens=2,
        )
    stats = {s["model"]: s for s in repo.chat_interaction_stats()}
    assert stats["a"]["count"] == 2
    assert stats["a"]["answered"] == 1 and stats["a"]["failed"] == 1
    assert stats["a"]["avg_latency_ms"] == 200
    assert stats["a"]["prompt_tokens"] == 20
    assert stats["b"]["count"] == 1
    assert stats["b"]["p95_latency_ms"] == 900


def test_the_p95_is_a_measurement_that_actually_happened(repo):
    """Nearest-rank: the answer it names is one of the answers, not an average."""
    for latency in range(1, 21):  # 1..20 hundred ms
        repo.log_chat_interaction(
            model="m", question="q", reply="a", outcome="answered", latency_ms=latency * 100
        )
    (stat,) = repo.chat_interaction_stats()
    assert stat["p95_latency_ms"] == 1900
    assert stat["avg_latency_ms"] == 1050


def test_rows_without_a_latency_do_not_break_the_totals(repo):
    repo.log_chat_interaction(model="m", question="q", reply="a", outcome="failed")
    (stat,) = repo.chat_interaction_stats()
    assert stat["count"] == 1
    assert stat["avg_latency_ms"] is None
    assert stat["p95_latency_ms"] is None


def test_an_empty_log_has_no_totals(repo):
    assert repo.chat_interaction_stats() == []
