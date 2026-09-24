"""The Kanata gateway client: bearer auth, error mapping, and key secrecy."""

from __future__ import annotations

import json
import logging

import httpx
import pytest
import respx

from bot.infrastructure.llm import (
    FULL,
    MINIMAL,
    Capabilities,
    KanataClient,
    ModelAuthError,
    ModelConfigError,
    ModelResponseError,
    ModelTimeout,
    ModelUnavailable,
    chat_body,
    gateway_status,
    parse_reply,
    profile_of,
)
from bot.infrastructure.llm.capabilities import CAPABILITY_TIMEOUT_S, CapabilityCache, parse_models

from .fake_bot import make_settings

pytestmark = pytest.mark.anyio

BASE = "https://gateway.test"
KEY = "sk-kanata-test-secret-9f3a"


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.fixture
def key_file(tmp_path):
    path = tmp_path / "kanata.key"
    path.write_text(f"\n  {KEY}  \n", encoding="utf-8")
    return path


def client(key_file) -> KanataClient:
    return KanataClient(BASE, str(key_file), timeout=5.0)


async def test_chat_sends_the_bearer_key_and_the_exact_body(key_file):
    body = chat_body(model="alias", messages=[{"role": "user", "content": "hi"}])
    completion = {"choices": [{"message": {"role": "assistant", "content": "yo"}}]}
    with respx.mock(base_url=BASE) as mock:
        route = mock.post("/v1/chat/completions").respond(200, json=completion)
        gateway = client(key_file)
        try:
            assert await gateway.chat(**body) == completion
        finally:
            await gateway.close()
    request = route.calls[0].request
    assert request.headers["authorization"] == f"Bearer {KEY}"
    assert json.loads(request.content) == {
        "model": "alias",
        "messages": [{"role": "user", "content": "hi"}],
    }


async def test_models_lists_aliases(key_file):
    with respx.mock(base_url=BASE) as mock:
        route = mock.get("/v1/models").respond(
            200, json={"object": "list", "data": [{"id": "a"}, {"id": "b"}]}
        )
        gateway = client(key_file)
        try:
            assert await gateway.models() == ["a", "b"]
        finally:
            await gateway.close()
    assert route.calls[0].request.headers["authorization"] == f"Bearer {KEY}"


@pytest.mark.parametrize(
    ("status", "error"),
    [
        (401, ModelAuthError),
        (403, ModelAuthError),
        (404, ModelUnavailable),
        (429, ModelUnavailable),
        (500, ModelUnavailable),
        (503, ModelUnavailable),
        (400, ModelUnavailable),
    ],
)
async def test_http_errors_map_without_leaking_the_key(key_file, caplog, status, error):
    caplog.set_level(logging.DEBUG)
    with respx.mock(base_url=BASE) as mock:
        mock.post("/v1/chat/completions").respond(status, text=f"echo Bearer {KEY}")
        gateway = client(key_file)
        try:
            with pytest.raises(error) as raised:
                await gateway.chat(model="m", messages=[])
        finally:
            await gateway.close()
    assert str(status) in str(raised.value)
    assert KEY not in str(raised.value)
    assert KEY not in caplog.text


@pytest.mark.parametrize(
    ("failure", "error"),
    [
        (httpx.ReadTimeout("slow"), ModelTimeout),
        (httpx.ConnectTimeout("slow"), ModelTimeout),
        (httpx.ConnectError("certificate verify failed"), ModelUnavailable),
    ],
)
async def test_transport_failures_map_to_unavailable(key_file, caplog, failure, error):
    caplog.set_level(logging.DEBUG)
    with respx.mock(base_url=BASE) as mock:
        mock.post("/v1/chat/completions").mock(side_effect=failure)
        gateway = client(key_file)
        try:
            with pytest.raises(error) as raised:
                await gateway.chat(model="m", messages=[])
        finally:
            await gateway.close()
    assert KEY not in str(raised.value)
    assert raised.value.__cause__ is None
    assert KEY not in caplog.text


async def test_a_timeout_is_also_a_timeout_error(key_file):
    with respx.mock(base_url=BASE) as mock:
        mock.post("/v1/chat/completions").mock(side_effect=httpx.ReadTimeout("slow"))
        gateway = client(key_file)
        try:
            with pytest.raises(TimeoutError):
                await gateway.chat(model="m", messages=[])
        finally:
            await gateway.close()


async def test_non_json_is_a_response_error(key_file):
    with respx.mock(base_url=BASE) as mock:
        mock.post("/v1/chat/completions").respond(200, text="<html>")
        gateway = client(key_file)
        try:
            with pytest.raises(ModelResponseError):
                await gateway.chat(model="m", messages=[])
        finally:
            await gateway.close()


async def test_the_key_is_never_in_a_repr(key_file):
    gateway = client(key_file)
    try:
        assert KEY not in repr(gateway)
        assert KEY not in repr(vars(gateway))
    finally:
        await gateway.close()


@pytest.mark.parametrize("content", [None, "", " \n"])
def test_a_missing_or_empty_key_file_fails_clearly(tmp_path, content):
    path = tmp_path / "kanata.key"
    if content is not None:
        path.write_text(content, encoding="utf-8")
    with pytest.raises(ModelConfigError, match="KANATA_API_KEY_FILE"):
        KanataClient(BASE, str(path), timeout=1.0)


def test_an_unset_key_file_fails_clearly():
    with pytest.raises(ModelConfigError, match="KANATA_API_KEY_FILE is not set"):
        KanataClient(BASE, "", timeout=1.0)


def test_wire_messages_drop_provider_specific_keys_and_nulls():
    body = chat_body(
        model="m",
        messages=[
            {"role": "system", "content": "s", "images": None},
            {
                "role": "assistant",
                "content": "",
                "thinking": "hidden",
                "tool_calls": [{"id": "c1", "function": {"name": "t", "arguments": {"a": 1}}}],
            },
            {"role": "tool", "tool_call_id": "c1", "tool_name": "t", "name": "t", "content": "r"},
        ],
        tools=[],
        temperature=None,
    )
    assert body == {
        "model": "m",
        "messages": [
            {"role": "system", "content": "s"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "t", "arguments": '{"a": 1}'},
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "c1", "content": "r"},
        ],
    }


def test_parse_reply_reads_usage_reasoning_and_synthesizes_missing_ids():
    reply = parse_reply(
        {
            "choices": [
                {
                    "message": {
                        "content": None,
                        "reasoning_content": "hmm",
                        "tool_calls": [
                            {"type": "function", "function": {"name": "a", "arguments": "{}"}},
                            {"id": "x", "type": "function", "function": {"name": "b"}},
                            {"id": "x", "type": "function", "function": {"name": "c"}},
                        ],
                    },
                    "finish_reason": "tool_calls",
                }
            ],
            "usage": {"prompt_tokens": 12, "completion_tokens": 3},
        },
        id_prefix="call_2",
    )
    assert [(c.id, c.name) for c in reply.tool_calls] == [
        ("call_2_0", "a"),
        ("x", "b"),
        ("call_2_2", "c"),
    ]
    assert reply.reasoning == "hmm"
    assert reply.finish_reason == "tool_calls"
    assert (reply.prompt_tokens, reply.completion_tokens) == (12, 3)
    assert parse_reply({"choices": []}).content is None


async def test_gateway_status_reports_listed_aliases_without_the_key(key_file):
    settings = make_settings(
        kanata_base_url=BASE,
        kanata_api_key_file=str(key_file),
        extract_model="ex",
        chat_pilot_model="ch",
    )
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(200, json={"data": [{"id": "ex"}]})
        ok, detail = await gateway_status(settings)
    assert ok is True
    assert "extract `ex` listed" in detail
    assert "chat `ch` NOT listed" in detail
    assert KEY not in detail


async def test_gateway_status_reports_auth_and_config_failures(key_file, tmp_path):
    settings = make_settings(kanata_base_url=BASE, kanata_api_key_file=str(key_file))
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(401)
        ok, detail = await gateway_status(settings)
    assert ok is False and "401" in detail and KEY not in detail

    missing = make_settings(kanata_api_key_file=str(tmp_path / "none"))
    ok, detail = await gateway_status(missing)
    assert ok is False and "KANATA_API_KEY_FILE" in detail


async def test_a_404_names_the_endpoint_not_an_alias(key_file):
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(404)
        mock.post("/v1/chat/completions").respond(404)
        gateway = client(key_file)
        try:
            with pytest.raises(ModelUnavailable) as listing:
                await gateway.models()
            with pytest.raises(ModelUnavailable) as chatting:
                await gateway.chat(model="m", messages=[])
        finally:
            await gateway.close()
    assert str(listing.value) == (
        "model gateway returned HTTP 404 for /v1/models; check KANATA_BASE_URL"
    )
    assert "alias" in str(chatting.value) and "/v1/chat/completions" in str(chatting.value)


@pytest.mark.parametrize("key", ["sk-kanäta-secret", "sk-kanata\x01secret"])
async def test_a_key_a_header_cannot_carry_fails_as_config_not_a_crash(tmp_path, key):
    path = tmp_path / "kanata.key"
    path.write_text(key, encoding="utf-8")
    with pytest.raises(ModelConfigError) as raised:
        KanataClient(BASE, str(path), timeout=1.0)
    assert "secret" not in str(raised.value)

    ok, detail = await gateway_status(make_settings(kanata_api_key_file=str(path)))
    assert ok is False and "KANATA_API_KEY_FILE" in detail and "secret" not in detail


def test_reserved_ids_keep_tool_call_ids_unique_across_rounds():
    reserved = {"call_0"}
    first = parse_reply(
        {"choices": [{"message": {"tool_calls": [{"id": "call_0", "function": {"name": "a"}}]}}]},
        id_prefix="call_2",
        reserved=reserved,
    )
    assert first.tool_calls[0].id == "call_2_0"
    assert reserved == {"call_0", "call_2_0"}


FULL_META = {
    "structured_output": True,
    "sampling_controls": True,
    "reasoning_control": True,
    "function_tools": True,
    "trust_zone": "local",
}


def model_list(*entries: tuple[str, dict | None]) -> dict:
    data = []
    for alias, meta in entries:
        item = {"id": alias, "object": "model", "created": 0, "owned_by": "kanata"}
        if meta is not None:
            item["kanata"] = meta
        data.append(item)
    return {"object": "list", "data": data}


@pytest.mark.parametrize("suffix", ["/v1", "/v1/", "/"])
async def test_a_trailing_v1_is_normalized_by_the_client(key_file, suffix):
    gateway = KanataClient(BASE + suffix, str(key_file), timeout=5.0)
    assert gateway.base_url == BASE
    with respx.mock(base_url=BASE) as mock:
        route = mock.post("/v1/chat/completions").respond(200, json={"choices": []})
        try:
            await gateway.chat(model="a", messages=[])
        finally:
            await gateway.close()
    assert route.calls[0].request.url.path == "/v1/chat/completions"


async def test_model_entries_read_optional_kanata_metadata(key_file):
    odd = {"structured_output": "yes", "trust_zone": "moon"}
    payload = model_list(("local", FULL_META), ("bare", None), ("odd", odd))
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(200, json=payload)
        gateway = client(key_file)
        try:
            entries = await gateway.model_entries()
        finally:
            await gateway.close()
    assert list(entries) == ["local", "bare", "odd"]
    assert entries["local"] == Capabilities(True, True, True, True, "local", declared=True)
    assert entries["bare"] == MINIMAL and not MINIMAL.declared
    # Unreadable fields fall back conservatively.
    assert entries["odd"] == Capabilities(False, False, False, True, None, declared=True)


async def test_profiles_are_cached_for_the_ttl_then_refetched(key_file):
    clock = [0.0]
    gateway = client(key_file)
    gateway._capabilities = CapabilityCache(gateway.model_entries, ttl=10, clock=lambda: clock[0])
    with respx.mock(base_url=BASE) as mock:
        route = mock.get("/v1/models").respond(200, json=model_list(("a", FULL_META)))
        try:
            assert (await gateway.profile("a")).structured_output
            clock[0] = 9.0
            await gateway.profile("a")
            assert route.call_count == 1
            clock[0] = 10.5
            await gateway.profile("a")
            assert route.call_count == 2
            # A miss refetches at once: a newly added alias need not wait out the TTL.
            assert await gateway.profile("new") == MINIMAL
            assert route.call_count == 3
        finally:
            await gateway.close()


async def test_a_failed_lookup_is_minimal_and_not_cached(key_file, caplog):
    gateway = client(key_file)
    with respx.mock(base_url=BASE) as mock:
        route = mock.get("/v1/models").mock(
            side_effect=[
                httpx.ConnectError("down"),
                httpx.Response(200, json=model_list(("a", FULL_META))),
            ]
        )
        try:
            assert await gateway.profile("a") == MINIMAL
            assert (await gateway.profile("a")).reasoning_control
        finally:
            await gateway.close()
    assert route.call_count == 2
    assert "minimal request" in caplog.text and KEY not in caplog.text


async def test_the_capability_fetch_has_its_own_short_timeout(key_file):
    gateway = KanataClient(BASE, str(key_file), timeout=120.0)
    with respx.mock(base_url=BASE) as mock:
        route = mock.get("/v1/models").respond(200, json=model_list())
        try:
            await gateway.model_entries()
        finally:
            await gateway.close()
    timeouts = route.calls[0].request.extensions["timeout"]
    assert timeouts["read"] == CAPABILITY_TIMEOUT_S


async def test_clients_without_a_lookup_keep_the_legacy_full_profile():
    class Bare:
        async def chat(self, **_body):
            return {}

    assert await profile_of(Bare(), "x") == FULL


def test_confirmed_contract_fields_parse_and_extras_are_ignored():
    entry = {
        "id": "gpt-6-luna:low",
        "kanata": {
            "operations": ["chat"],
            "structured_output": False,
            "sampling_controls": False,
            "reasoning_control": True,
            "function_tools": True,
            "streaming": False,
            "input_audio": False,
            "trust_zone": "external",
            "reasoning_efforts": ["low", "medium", "high"],
        },
    }
    caps = parse_models({"data": [entry]})["gpt-6-luna:low"]
    assert caps.reasoning_efforts == ("low", "medium", "high")
    assert caps.effort("medium") == "medium"
    # "none" (off) is outside the list: omitted, so the alias default applies.
    assert caps.effort("none") is None
    ollama = parse_models(
        {"data": [{"id": "o", "kanata": {**entry["kanata"], "reasoning_efforts": None}}]}
    )
    assert ollama["o"].reasoning_efforts is None and ollama["o"].effort("none") == "none"
    assert MINIMAL.effort("high") is None


def rejected(param: str) -> httpx.Response:
    return httpx.Response(
        400, json={"error": {"type": "invalid_request", "param": param, "message": "no"}}
    )


async def test_a_400_naming_a_sent_field_drops_it_once_and_remembers(key_file, caplog):
    gateway = client(key_file)
    body = {"model": "a", "messages": [], "temperature": 0, "seed": 0, "reasoning_effort": "low"}
    ok = {"choices": []}
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(200, json=model_list(("a", FULL_META)))
        route = mock.post("/v1/chat/completions").mock(
            side_effect=[rejected("seed"), httpx.Response(200, json=ok)]
        )
        try:
            assert await gateway.chat(**body) == ok
            caps = await gateway.profile("a")
        finally:
            await gateway.close()
    sent = [json.loads(call.request.content) for call in route.calls]
    # Both sampling fields share one capability, so the retry drops both.
    assert sent[1] == {"model": "a", "messages": [], "reasoning_effort": "low"}
    assert not caps.sampling_controls and caps.reasoning_control
    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert len(warnings) == 1 and "`seed`" in warnings[0].getMessage()
    assert KEY not in caplog.text


@pytest.mark.parametrize(
    ("responses", "body_extra"),
    [
        ([rejected("messages")], {"temperature": 0}),
        ([rejected("top_p")], {"temperature": 0}),
        ([httpx.Response(400, json={"error": {"message": "bad"}})], {"temperature": 0}),
        (
            [rejected("temperature"), rejected("reasoning_effort")],
            {"temperature": 0, "reasoning_effort": "low"},
        ),
    ],
    ids=["other-param", "field-not-sent", "no-param", "retries-once-only"],
)
async def test_other_400s_are_not_retried_more_than_once(key_file, responses, body_extra):
    gateway = client(key_file)
    with respx.mock(base_url=BASE) as mock:
        route = mock.post("/v1/chat/completions").mock(side_effect=responses)
        try:
            with pytest.raises(ModelUnavailable, match="HTTP 400"):
                await gateway.chat(model="a", messages=[], **body_extra)
        finally:
            await gateway.close()
    assert route.call_count == len(responses)


async def test_a_revoked_capability_expires_with_the_ttl(key_file):
    clock = [0.0]
    gateway = client(key_file)
    gateway._capabilities = CapabilityCache(gateway.model_entries, ttl=10, clock=lambda: clock[0])
    with respx.mock(base_url=BASE) as mock:
        mock.get("/v1/models").respond(200, json=model_list(("a", FULL_META)))
        try:
            gateway._capabilities.revoke("a", "structured_output")
            assert not (await gateway.profile("a")).structured_output
            clock[0] = 11.0
            assert (await gateway.profile("a")).structured_output
        finally:
            await gateway.close()
