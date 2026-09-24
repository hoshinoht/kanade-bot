"""The extractor's gateway call: exact request, validation, retry and failure paths."""

from __future__ import annotations

import json
import logging

import httpx
import pytest
import respx

from bot.extract.llm import JSON_INSTRUCTION, RETRY_INSTRUCTION, Extractor, parse_response
from bot.extract.schema import json_schema

from .fake_bot import make_settings

pytestmark = pytest.mark.anyio

BASE = "https://gateway.test"
KEY = "sk-kanata-extract-secret-51c2"
MESSAGES = [
    {"role": "system", "content": "you are an extractor"},
    {"role": "user", "content": "can we move hstar to wed 9pm?"},
]
VALID = json.dumps(
    {
        "amendments": [
            {
                "kind": "move",
                "bosses": ["HStar"],
                "day_ref": "wed",
                "time_ref": "9pm",
                "confidence": 0.9,
                "evidence_message_ids": ["1"],
            }
        ],
        "summary": "move",
    }
)


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.fixture
def extractor(tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text(KEY, encoding="utf-8")
    settings = make_settings(
        kanata_base_url=BASE,
        kanata_api_key_file=str(key),
        extract_model="extract-alias",
        kanata_timeout=5.0,
    )
    return Extractor(settings)


def completion(content: str | None, **extra) -> dict:
    return {
        "choices": [
            {"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}
        ],
        **extra,
    }


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


def listing(meta: dict | None, alias: str = "extract-alias") -> dict:
    entry = {"id": alias, "object": "model", "created": 0, "owned_by": "kanata"}
    if meta is not None:
        entry["kanata"] = meta
    return {"object": "list", "data": [entry]}


async def run(extractor: Extractor, *responses, models=None):
    models = httpx.Response(200, json=listing(FULL_META)) if models is None else models
    with respx.mock(base_url=BASE, assert_all_called=False) as mock:
        mock.get("/v1/models").mock(return_value=models)
        route = mock.post("/v1/chat/completions").mock(side_effect=list(responses))
        try:
            call = await extractor.extract(list(MESSAGES))
        finally:
            await extractor.close()
    return call, [json.loads(c.request.content) for c in route.calls], route


async def test_the_request_body_is_exactly_the_contract(extractor):
    call, bodies, route = await run(extractor, httpx.Response(200, json=completion(VALID)))

    assert call.ok and call.attempts == 1
    assert bodies == [
        {
            "model": "extract-alias",
            "messages": MESSAGES,
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "extraction", "schema": json_schema(), "strict": True},
            },
            "temperature": 0,
            "seed": 0,
            "reasoning_effort": "none",
        }
    ]
    assert route.calls[0].request.headers["authorization"] == f"Bearer {KEY}"
    sent = bodies[0]["response_format"]["json_schema"]["schema"]
    assert sent["additionalProperties"] is False
    assert sent["$defs"]["Amendment"]["additionalProperties"] is False
    assert sent["$defs"]["Amendment"]["required"] == list(sent["$defs"]["Amendment"]["properties"])


async def test_reasoning_levels_pass_through(extractor):
    extractor.settings = extractor.settings.model_copy(update={"extract_reasoning": "medium"})
    _call, bodies, _ = await run(extractor, httpx.Response(200, json=completion(VALID)))
    assert bodies[0]["reasoning_effort"] == "medium"


async def test_valid_json_becomes_an_extraction_and_keeps_reasoning(extractor):
    response = completion(VALID)
    response["choices"][0]["message"]["reasoning"] = "thinking it over"
    call, _bodies, _ = await run(extractor, httpx.Response(200, json=response))
    assert call.extraction.amendments[0].bosses == ["HStar"]
    assert call.thinking == "thinking it over"
    assert call.error is None


async def test_prose_is_retried_once_then_quarantined(extractor, caplog):
    caplog.set_level(logging.DEBUG)
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion("Sure! Here is the JSON you asked for.")),
        httpx.Response(200, json=completion("Still prose.")),
    )

    assert not call.ok
    assert call.attempts == 2
    assert call.raw == "Still prose."
    assert call.error.startswith("not JSON")
    retry = bodies[1]["messages"]
    assert retry[:2] == MESSAGES
    assert retry[2] == {"role": "assistant", "content": "Sure! Here is the JSON you asked for."}
    assert retry[3]["role"] == "user"
    assert retry[3]["content"].startswith(RETRY_INSTRUCTION.split("{error}")[0])
    assert KEY not in caplog.text


async def test_an_empty_answer_retries_without_an_empty_assistant_turn(extractor):
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion(None)),
        httpx.Response(200, json=completion(VALID)),
    )
    assert call.ok and call.attempts == 2
    assert [m["role"] for m in bodies[1]["messages"]] == ["system", "user", "user"]


async def test_a_single_json_fence_is_tolerated():
    assert parse_response(f"```json\n{VALID}\n```").amendments[0].kind == "move"
    assert parse_response(f"```\n{VALID}\n```").summary == "move"
    for bad in (f"Here:\n```json\n{VALID}\n```", f"```json\n{VALID}\n```\nthanks"):
        with pytest.raises(ValueError):
            parse_response(bad)


@pytest.mark.parametrize(
    ("response", "fragment"),
    [
        (httpx.Response(401), "HTTP 401"),
        (httpx.Response(404), "HTTP 404"),
        (httpx.Response(429), "HTTP 429"),
        (httpx.Response(502), "HTTP 502"),
        (httpx.ConnectError("tls handshake failed"), "unreachable"),
    ],
)
async def test_gateway_failures_are_returned_not_raised(extractor, caplog, response, fragment):
    caplog.set_level(logging.DEBUG)
    call, bodies, _ = await run(extractor, response)
    assert not call.ok
    assert call.attempts == 1 and len(bodies) == 1
    assert fragment in call.error
    assert KEY not in call.error and KEY not in caplog.text


async def test_a_timeout_is_reported_with_the_configured_budget(extractor):
    call, _bodies, _ = await run(extractor, httpx.ReadTimeout("slow"))
    assert not call.ok
    assert call.error == "the model did not answer within 5s"


async def test_a_missing_model_alias_fails_explicitly(extractor):
    extractor.settings = extractor.settings.model_copy(update={"extract_model": ""})
    call, bodies, _ = await run(extractor)
    assert bodies == []
    assert call.error == "ModelConfigError: EXTRACT_MODEL is not set"


async def test_a_missing_key_file_fails_explicitly(tmp_path):
    settings = make_settings(
        kanata_base_url=BASE,
        kanata_api_key_file=str(tmp_path / "absent.key"),
        extract_model="x",
    )
    call = await Extractor(settings).extract(list(MESSAGES))
    assert not call.ok
    assert call.error == "ModelConfigError: KANATA_API_KEY_FILE points to a missing file"


async def test_missing_configuration_is_reported_once_not_per_burst(extractor, caplog):
    caplog.set_level(logging.DEBUG, logger="bot.extract.llm")
    extractor.settings = extractor.settings.model_copy(update={"extract_model": ""})
    first = await extractor.extract(list(MESSAGES))
    second = await extractor.extract(list(MESSAGES))
    assert first.misconfigured and second.misconfigured
    errors = [r for r in caplog.records if r.levelno >= logging.WARNING]
    assert len(errors) == 1 and errors[0].levelno == logging.ERROR
    assert "EXTRACT_MODEL is not set" in errors[0].getMessage()


def json_only_messages() -> list[dict]:
    schema = json.dumps(json_schema(), ensure_ascii=False, separators=(",", ":"))
    instruction = JSON_INSTRUCTION.format(schema=schema)
    return [
        {"role": "system", "content": f"{MESSAGES[0]['content']}\n\n{instruction}"},
        MESSAGES[1],
    ]


async def test_a_cloud_alias_gets_the_schema_in_the_prompt_not_response_format(extractor):
    extractor.settings = extractor.settings.model_copy(update={"extract_reasoning": "low"})
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion(f"```json\n{VALID}\n```")),
        models=httpx.Response(200, json=listing(CLOUD_META)),
    )
    assert call.ok
    assert bodies == [
        {
            "model": "extract-alias",
            "messages": json_only_messages(),
            "temperature": 0,
            "seed": 0,
            "reasoning_effort": "low",
        }
    ]


async def test_a_codex_alias_gets_a_minimal_body(extractor):
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion(VALID)),
        models=httpx.Response(200, json=listing(CODEX_META)),
    )
    assert call.ok
    assert bodies == [{"model": "extract-alias", "messages": json_only_messages()}]


@pytest.mark.parametrize(("level", "sent"), [("high", {"reasoning_effort": "high"}), ("off", {})])
async def test_a_codex_alias_takes_listed_efforts_and_off_keeps_its_default(extractor, level, sent):
    extractor.settings = extractor.settings.model_copy(update={"extract_reasoning": level})
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion(VALID)),
        models=httpx.Response(200, json=listing(CODEX_CONFIRMED)),
    )
    assert call.ok
    assert bodies == [{"model": "extract-alias", "messages": json_only_messages(), **sent}]


@pytest.mark.parametrize(
    "models",
    [
        httpx.Response(200, json=listing(None)),
        httpx.Response(200, json=listing(FULL_META, alias="another-alias")),
        httpx.Response(502),
        httpx.Response(200, json={"unexpected": True}),
    ],
    ids=["metadata-absent", "alias-unlisted", "gateway-5xx", "malformed"],
)
async def test_unknown_capabilities_send_a_minimal_body(extractor, models):
    call, bodies, _ = await run(
        extractor, httpx.Response(200, json=completion(VALID)), models=models
    )
    assert call.ok
    assert bodies == [{"model": "extract-alias", "messages": json_only_messages()}]


async def test_minimal_retry_keeps_the_json_instruction_and_quarantines(extractor):
    call, bodies, _ = await run(
        extractor,
        httpx.Response(200, json=completion("prose")),
        httpx.Response(200, json=completion("more prose")),
        models=httpx.Response(200, json=listing(CODEX_META)),
    )
    assert not call.ok and call.attempts == 2
    assert bodies[1]["messages"][0] == json_only_messages()[0]
    assert set(bodies[1]) == {"model", "messages"}


async def test_the_prompt_budget_counts_the_schema_until_structured_output_is_seen(extractor):
    from bot.extract.llm import json_instruction_tokens
    from bot.extract.prompt import estimate_messages

    assert extractor.prompt_overhead_tokens() == json_instruction_tokens() > 0
    # The overhead is what the minimal body adds to the estimate, give or take rounding.
    added = estimate_messages(json_only_messages()) - estimate_messages(MESSAGES)
    assert abs(added - json_instruction_tokens()) <= 1
    await run(extractor, httpx.Response(200, json=completion(VALID)))
    assert extractor.prompt_overhead_tokens() == 0
    await run(
        extractor,
        httpx.Response(200, json=completion(VALID)),
        models=httpx.Response(200, json=listing(CODEX_META)),
    )
    assert extractor.prompt_overhead_tokens() == json_instruction_tokens()
