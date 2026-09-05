"""The log trail behind an answer.

The point of this is answering "why did it propose that?" from
`docker compose logs` alone, with no UI. So the assertions are about what a
person reading the logs can actually reconstruct: which tool was called, with
what arguments, how long it took, whether it obeyed, and which card came out.

DEBUG carries the per-call detail; one INFO line per interaction says whether
that detail is worth turning DEBUG on for.
"""

from __future__ import annotations

import logging
import re

import pytest

from bot.chat import tools
from bot.chat.agent import MAX_TOOL_ROUNDS, ChatPilot
from bot.domain.ids import short_id

from .chat_support import CHAT_CHANNEL, FakeOllama, message, says, wants

pytestmark = pytest.mark.anyio

AGENT = "bot.chat.agent"


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def lines(caplog, level: int) -> list[str]:
    return [r.getMessage() for r in caplog.records if r.levelno == level and r.name == AGENT]


#: A per-call trace line: `tool NAME(args) -> outcome`. Matched on the arrow as
#: well as the name, because a per-*round* line ends "(1 tool call(s))" and
#: anything looser counts that as a trace too.
_TRACE_RE = re.compile(r" tool \w*\(.*\) -> ")


def traces(caplog) -> list[str]:
    return [line for line in lines(caplog, logging.DEBUG) if _TRACE_RE.search(line)]


def context(bot, author_id: int | str = 1002):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id="950000000000000777",
    )


# ---------------------------------------------------------------------------
# the per-call DEBUG trace
# ---------------------------------------------------------------------------


async def test_a_tool_call_is_traced_with_its_arguments_and_duration(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot, "@bot what's on?"))

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool get_schedule" in line]
    assert len(traced) == 1
    assert "week='this'" in traced[0]
    assert "-> ok" in traced[0]
    assert "ms" in traced[0]


async def test_a_refusal_is_traced_as_a_refusal_not_a_success(chat_bot, chat_seeded, caplog):
    agent = pilot(
        chat_bot, wants("propose_cancel", run_query="nonsense"), says("Couldn't find it.")
    )
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot, "@bot cancel the thing"))

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool propose_cancel" in line]
    assert "run_query='nonsense'" in traced[0]
    assert f"-> {tools.REFUSED}" in traced[0]


async def test_an_unknown_tool_is_traced_as_such(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, wants("approve"), says("Can't."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool approve" in line]
    assert f"-> {tools.UNKNOWN}" in traced[0]


async def test_a_crashing_tool_is_traced_as_failed(chat_bot, chat_seeded, caplog, monkeypatch):
    def boom(*_args, **_kwargs):
        raise RuntimeError("the database went away")

    monkeypatch.setattr(chat_bot.repo, "list_runs", boom)
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Sorry."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool get_schedule" in line]
    assert f"-> {tools.FAILED}" in traced[0]


async def test_a_write_names_the_card_it_created(chat_bot, chat_seeded, caplog):
    """The link from "the model called this" to "this card appeared"."""
    agent = pilot(
        chat_bot,
        wants("propose_move", run_query=short_id(chat_seeded["star"]), to_when="sunday 22:00"),
        says("Card's up."),
    )
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        result = (await agent.offer(message(chat_bot, "@bot move hstar"))).answered

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool propose_move" in line]
    assert "to_when='sunday 22:00'" in traced[0]
    assert f"card {result.created[0]}" in traced[0]


async def test_every_round_is_traced_not_just_the_first(chat_bot, chat_seeded, caplog):
    agent = pilot(
        chat_bot,
        wants("get_schedule", week="this"),
        wants("get_run", query="kalos"),
        says("Here you go."),
    )
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))

    traced = traces(caplog)
    assert len(traced) == 2
    assert "round 1/4" in traced[0]
    assert "round 2/4" in traced[1]


async def test_the_model_round_itself_is_timed(chat_bot, chat_seeded, caplog):
    """Separating model latency from tool latency is the point of two lines."""
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))

    rounds = [line for line in lines(caplog, logging.DEBUG) if "model answered" in line]
    assert len(rounds) == 2
    assert "1 tool call(s)" in rounds[0]
    assert "in words" in rounds[1]


async def test_long_arguments_are_truncated_rather_than_dropped(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, wants("get_run", query="x" * 5000), says("No idea."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))

    traced = [line for line in lines(caplog, logging.DEBUG) if "tool get_run" in line]
    assert "query='xxx" in traced[0]
    assert "…" in traced[0]
    assert len(traced[0]) < 400


async def test_nothing_is_traced_at_debug_when_no_tool_is_called(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, says("Hi."))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot))
    assert traces(caplog) == []


# ---------------------------------------------------------------------------
# the per-interaction INFO summary
# ---------------------------------------------------------------------------


async def test_one_info_line_summarises_the_whole_interaction(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    with caplog.at_level(logging.INFO, logger=AGENT):
        await agent.offer(message(chat_bot, "@bot what's on?", author_id=1002))

    summary = [line for line in lines(caplog, logging.INFO) if "answered 1002" in line]
    assert len(summary) == 1
    assert "2 round(s)" in summary[0]
    assert "1 tool call(s)" in summary[0]
    assert "get_schedule:ok" in summary[0]
    assert "ms" in summary[0]
    assert str(CHAT_CHANNEL) in summary[0]


async def test_the_summary_names_each_tool_and_how_it_went(chat_bot, chat_seeded, caplog):
    agent = pilot(
        chat_bot,
        wants("get_schedule", week="this"),
        wants("propose_cancel", run_query="nonsense"),
        says("Couldn't."),
    )
    with caplog.at_level(logging.INFO, logger=AGENT):
        result = (await agent.offer(message(chat_bot))).answered

    assert result.trace == f"get_schedule:ok, propose_cancel:{tools.REFUSED}"
    summary = next(line for line in lines(caplog, logging.INFO) if "answered" in line)
    assert result.trace in summary


async def test_the_summary_names_the_cards_that_came_out(chat_bot, chat_seeded, caplog):
    agent = pilot(
        chat_bot,
        wants("propose_move", run_query=short_id(chat_seeded["star"]), to_when="sunday 22:00"),
        says("Card's up."),
    )
    with caplog.at_level(logging.INFO, logger=AGENT):
        result = (await agent.offer(message(chat_bot))).answered

    summary = next(line for line in lines(caplog, logging.INFO) if "answered" in line)
    assert f"proposal {result.created[0]}" in summary


async def test_a_failed_interaction_still_gets_its_summary_with_the_reason(
    chat_bot, chat_seeded, caplog
):
    agent = pilot(chat_bot, ConnectionError("ollama is not running"))
    with caplog.at_level(logging.INFO, logger=AGENT):
        await agent.offer(message(chat_bot))

    summary = next(line for line in lines(caplog, logging.INFO) if "answered" in line)
    assert "ConnectionError" in summary


async def test_a_looping_model_is_summarised_as_such(chat_bot, chat_seeded, caplog):
    agent = pilot(chat_bot, *[wants("get_schedule", week="this")] * (MAX_TOOL_ROUNDS + 2))
    with caplog.at_level(logging.INFO, logger=AGENT):
        await agent.offer(message(chat_bot))

    summary = next(line for line in lines(caplog, logging.INFO) if "answered" in line)
    assert f"{MAX_TOOL_ROUNDS} round(s)" in summary
    assert "kept calling tools" in summary


async def test_an_ignored_message_produces_no_interaction_summary(chat_bot, chat_seeded, caplog):
    """The gate refuses almost everything; a line each would drown the log."""
    from .chat_support import OTHER_ROLE

    agent = pilot(chat_bot, says("never"))
    with caplog.at_level(logging.DEBUG, logger=AGENT):
        await agent.offer(message(chat_bot, roles=(OTHER_ROLE,)))
    assert [line for line in lines(caplog, logging.INFO) if "answered" in line] == []


# ---------------------------------------------------------------------------
# the outcome record itself
# ---------------------------------------------------------------------------


async def test_run_reports_what_dispatch_only_summarises(chat_bot, chat_seeded):
    outcome = await tools.run(context(chat_bot), "get_schedule", {"week": "this"})
    assert outcome.ok is True
    assert outcome.error is None
    assert outcome.outcome == "ok"
    assert outcome.name == "get_schedule"
    assert outcome.arguments == {"week": "this"}
    assert "Hard MaleficStar + Hard FA" in outcome.output
    assert outcome.created == []


async def test_run_attributes_a_card_to_the_call_that_made_it(chat_bot, chat_seeded):
    ctx = context(chat_bot)
    first = await tools.run(ctx, "get_schedule", {"week": "this"})
    second = await tools.run(ctx, "propose_cancel", {"run_query": short_id(chat_seeded["star"])})
    assert first.created == []
    assert len(second.created) == 1
    # ...and the turn's running total still holds both calls' output.
    assert ctx.created == second.created


async def test_dispatch_still_returns_only_the_text_the_model_reads(chat_bot, chat_seeded):
    """The old contract is unchanged; `run` is additive."""
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    outcome = await tools.run(context(chat_bot), "get_schedule", {"week": "this"})
    assert isinstance(answer, str)
    assert answer == outcome.output
