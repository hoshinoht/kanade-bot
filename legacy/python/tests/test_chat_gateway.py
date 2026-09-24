"""The chatbot over the real gateway client, with the gateway mocked by respx."""

from __future__ import annotations

import json
import logging

import httpx
import pytest
import respx

from bot.chat import tools
from bot.chat.agent import FAILURE_REPLY, ChatPilot

from .chat_support import message

pytestmark = pytest.mark.anyio

KEY = "sk-kanata-chat-secret-77d0"
ROUTE = "/v1/chat/completions"


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.fixture
def live_client(chat_bot, tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text(KEY + "\n", encoding="utf-8")
    chat_bot.settings.kanata_api_key_file = str(key)
    return chat_bot.settings.kanata_base_url


def completion(message: dict, finish_reason: str = "stop", usage: dict | None = None) -> dict:
    body = {
        "choices": [{"message": {"role": "assistant", **message}, "finish_reason": finish_reason}]
    }
    if usage is not None:
        body["usage"] = usage
    return body


def call(call_id: str | None, name: str, arguments: str) -> dict:
    item = {"type": "function", "function": {"name": name, "arguments": arguments}}
    if call_id is not None:
        item["id"] = call_id
    return item


FULL_META = {
    "structured_output": True,
    "sampling_controls": True,
    "reasoning_control": True,
    "function_tools": True,
    "trust_zone": "local",
}
CLOUD_META = {**FULL_META, "structured_output": False, "trust_zone": "external"}
CODEX_META = {
    "structured_output": False,
    "sampling_controls": False,
    "reasoning_control": False,
    "function_tools": True,
    "trust_zone": "external",
}
#: Confirmed Kanata contract for Codex aliases: effort is selectable from a list.
CODEX_CONFIRMED = {
    **CODEX_META,
    "operations": ["chat"],
    "streaming": False,
    "input_audio": False,
    "reasoning_control": True,
    "reasoning_efforts": ["low", "medium", "high"],
}


def listing(meta: dict | None, alias: str = "test-chat-model") -> dict:
    entry = {"id": alias, "object": "model", "created": 0, "owned_by": "kanata"}
    if meta is not None:
        entry["kanata"] = meta
    return {"object": "list", "data": [entry]}


async def ask(
    chat_bot,
    base: str,
    *responses,
    text: str = "@bot what's on this week?",
    models: dict | None = None,
):
    agent = ChatPilot(chat_bot)
    with respx.mock(base_url=base, assert_all_called=False) as mock:
        mock.get("/v1/models").mock(
            return_value=httpx.Response(200, json=models or listing(FULL_META))
        )
        route = mock.post(ROUTE).mock(side_effect=list(responses))
        try:
            handled = await agent.offer(message(chat_bot, text))
        finally:
            await agent.close()
    return handled.answered, [json.loads(c.request.content) for c in route.calls], route


async def test_a_tool_round_sends_exact_openai_bodies(chat_bot, chat_seeded, live_client, caplog):
    caplog.set_level(logging.DEBUG)
    result, bodies, route = await ask(
        chat_bot,
        live_client,
        httpx.Response(
            200,
            json=completion(
                {
                    "content": None,
                    "tool_calls": [call("call_abc", "get_schedule", '{"week":"this"}')],
                },
                "tool_calls",
                {"prompt_tokens": 100, "completion_tokens": 7},
            ),
        ),
        httpx.Response(
            200,
            json=completion(
                {"content": "Two runs.", "reasoning": "done"},
                usage={"prompt_tokens": 150, "completion_tokens": 3},
            ),
        ),
    )

    # Schedule grounding may replace the words; the answer itself must succeed.
    assert result.error is None and result.reply
    assert result.tool_calls == ["get_schedule"]
    assert (result.prompt_tokens, result.completion_tokens) == (250, 10)
    for request in route.calls:
        assert request.request.headers["authorization"] == f"Bearer {KEY}"

    first, second = bodies
    offered = tools.TOOLS
    assert set(first) == {"model", "messages", "tools", "temperature", "reasoning_effort"}
    assert first["model"] == "test-chat-model"
    assert first["tools"] == offered
    assert first["temperature"] == chat_bot.settings.chat_pilot_temperature
    assert first["reasoning_effort"] == "none"
    assert first["messages"][0]["role"] == "system"
    assert first["messages"][-2] == {"role": "user", "content": "kanon: @bot what's on this week?"}

    assert set(second) == set(first)
    assert second["messages"][: len(first["messages"]) - 1] == first["messages"][:-1]
    assistant, tool_result, reminder = second["messages"][-3:]
    assert assistant == {
        "role": "assistant",
        "content": "",
        "tool_calls": [
            {
                "id": "call_abc",
                "type": "function",
                "function": {"name": "get_schedule", "arguments": '{"week":"this"}'},
            }
        ],
    }
    assert tool_result == {
        "role": "tool",
        "tool_call_id": "call_abc",
        "content": result.outcomes[0].output,
    }
    assert reminder == first["messages"][-1]
    allowed = {"role", "content", "tool_calls", "tool_call_id"}
    assert all(set(m) <= allowed for body in bodies for m in body["messages"])
    assert KEY not in caplog.text


async def test_missing_provider_ids_are_synthesized_and_correlated(
    chat_bot, chat_seeded, live_client
):
    result, bodies, _ = await ask(
        chat_bot,
        live_client,
        httpx.Response(
            200,
            json=completion(
                {
                    "content": "",
                    "tool_calls": [
                        call(None, "get_schedule", '{"week":"this"}'),
                        call(None, "list_bosses", "{}"),
                    ],
                },
                "tool_calls",
            ),
        ),
        httpx.Response(200, json=completion({"content": "Done."})),
    )
    assert result.error is None
    messages = bodies[1]["messages"]
    assistant = messages[-4]
    ids = [c["id"] for c in assistant["tool_calls"]]
    assert ids == ["call_1_0", "call_1_1"]
    assert [m["tool_call_id"] for m in messages[-3:-1]] == ids


async def test_malformed_arguments_reach_the_bad_arguments_path(chat_bot, chat_seeded, live_client):
    result, bodies, _ = await ask(
        chat_bot,
        live_client,
        httpx.Response(
            200,
            json=completion(
                {"tool_calls": [call("c1", "get_run", '{"run_query": ')]}, "tool_calls"
            ),
        ),
        httpx.Response(200, json=completion({"content": "Which run?"})),
    )
    assert result.outcomes[0].name == "get_run"
    assert result.outcomes[0].arguments == {}
    assert not result.outcomes[0].ok
    # The malformed string is replayed verbatim; the gateway only needs a string.
    assert bodies[1]["messages"][-3]["tool_calls"][0]["function"]["arguments"] == '{"run_query": '
    assert result.reply == "Which run?"


async def test_a_length_finish_keeps_the_reply_and_logs_it(
    chat_bot, chat_seeded, live_client, caplog
):
    caplog.set_level(logging.WARNING)
    result, _bodies, _ = await ask(
        chat_bot,
        live_client,
        httpx.Response(200, json=completion({"content": "Two runs, the first"}, "length")),
    )
    assert result.reply == "Two runs, the first"
    assert "hit the output limit" in caplog.text


@pytest.mark.parametrize("status", [401, 403, 404, 429, 500])
async def test_gateway_errors_become_the_failure_reply(
    chat_bot, chat_seeded, live_client, caplog, status
):
    caplog.set_level(logging.DEBUG)
    result, _bodies, _ = await ask(chat_bot, live_client, httpx.Response(status))
    assert result.reply == FAILURE_REPLY or result.error
    assert f"HTTP {status}" in result.error
    assert KEY not in result.error and KEY not in caplog.text
    row = chat_bot.repo.recent_chat_interactions()[0]
    assert KEY not in json.dumps(row, default=str)


async def test_a_missing_chat_model_fails_explicitly(chat_bot, chat_seeded, live_client):
    chat_bot.settings.chat_pilot_model = ""
    agent = ChatPilot(chat_bot)
    with respx.mock(base_url=live_client, assert_all_called=False) as mock:
        route = mock.post(ROUTE)
        result = (await agent.offer(message(chat_bot))).answered
    assert not route.called
    assert result.error == "ModelConfigError: CHAT_PILOT_MODEL is not set"


async def test_a_provider_reusing_ids_across_rounds_never_duplicates_them(
    chat_bot, chat_seeded, live_client
):
    reused = completion(
        {"content": "", "tool_calls": [call("call_0", "list_bosses", "{}")]}, "tool_calls"
    )
    result, bodies, _ = await ask(
        chat_bot,
        live_client,
        httpx.Response(200, json=reused),
        httpx.Response(200, json=reused),
        httpx.Response(200, json=completion({"content": "Done."})),
    )
    assert result.error is None
    messages = bodies[2]["messages"]
    call_ids = [c["id"] for m in messages for c in m.get("tool_calls", [])]
    result_ids = [m["tool_call_id"] for m in messages if m["role"] == "tool"]
    assert call_ids == ["call_0", "call_2_0"]
    assert result_ids == call_ids


@pytest.mark.parametrize(
    ("meta", "expected"),
    [
        (CLOUD_META, {"model", "messages", "tools", "temperature", "reasoning_effort"}),
        (CODEX_META, {"model", "messages", "tools"}),
        (CODEX_CONFIRMED, {"model", "messages", "tools", "reasoning_effort"}),
        (None, {"model", "messages", "tools"}),
    ],
    ids=["cloud", "fixed-reasoning", "codex", "metadata-absent"],
)
async def test_chat_bodies_follow_the_alias_profile(
    chat_bot, chat_seeded, live_client, meta, expected
):
    chat_bot.settings.chat_pilot_think = "low"
    result, bodies, _ = await ask(
        chat_bot,
        live_client,
        httpx.Response(200, json=completion({"content": "Two runs."})),
        models=listing(meta),
    )
    assert result.error is None
    assert set(bodies[0]) == expected
    assert bodies[0]["model"] == "test-chat-model"
    assert bodies[0]["tools"] == tools.TOOLS
    if "reasoning_effort" in expected:
        assert bodies[0]["reasoning_effort"] == "low"
    if "temperature" in expected:
        assert bodies[0]["temperature"] == chat_bot.settings.chat_pilot_temperature


async def test_an_alias_without_tools_fails_clearly(chat_bot, chat_seeded, live_client):
    result, bodies, _ = await ask(
        chat_bot, live_client, models=listing({**FULL_META, "function_tools": False})
    )
    assert bodies == []
    assert result.error == (
        "ModelConfigError: chat model `test-chat-model` does not support function tools"
    )
