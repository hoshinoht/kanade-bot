"""Runtime model selection: portal/API picker, seeding, startup checks, and bundle keys."""

from __future__ import annotations

import asyncio
import logging
import os

import httpx
import pytest
import respx

import bot.__main__ as entry
from bot.infrastructure.bundle.config import RUNTIME_CONFIG_KEYS, Config
from bot.infrastructure.config import MODEL_CONFIG_KEYS

from .fake_bot import make_settings

BASE = "https://kanata.test"
KEY = "sk-kanata-picker-secret-4e1b"
LOCAL = {
    "structured_output": True,
    "sampling_controls": True,
    "reasoning_control": True,
    "function_tools": True,
    "trust_zone": "local",
}
# Kanata's confirmed shape: Ollama's cloud proxy reports `local`, so the suffix marks it.
CLOUD = {
    **LOCAL,
    "operations": ["chat"],
    "streaming": True,
    "input_audio": False,
    "structured_output": False,
    "reasoning_efforts": None,
}
CODEX = {
    "operations": ["chat"],
    "structured_output": False,
    "sampling_controls": False,
    "reasoning_control": True,
    "function_tools": True,
    "streaming": False,
    "input_audio": False,
    "trust_zone": "external",
    "reasoning_efforts": ["low", "high"],
}
FIXED = {**LOCAL, "reasoning_control": False, "trust_zone": "private_network"}
NO_TOOLS = {**LOCAL, "function_tools": False}


def listing() -> dict:
    entries = [
        ("gpt-oss-20b", LOCAL),
        ("gpt-oss-120b-cloud", CLOUD),
        ("gpt-6-luna:low", CODEX),
        ("bare-alias", None),
        ("no-tools", NO_TOOLS),
        ("fixed-reasoning", FIXED),
    ]
    data = []
    for alias, meta in entries:
        item = {"id": alias, "object": "model", "created": 0, "owned_by": "kanata"}
        if meta is not None:
            item["kanata"] = meta
        data.append(item)
    return {"object": "list", "data": data}


@pytest.fixture
def gateway(fake_bot, tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text(KEY, encoding="utf-8")
    fake_bot.settings.kanata_base_url = BASE
    fake_bot.settings.kanata_api_key_file = str(key)
    fake_bot.settings.extract_model = "gpt-oss-20b"
    fake_bot.settings.chat_pilot_model = "gpt-oss-20b"
    with respx.mock(base_url=BASE, assert_all_called=False) as mock:
        mock.route_models = mock.get("/v1/models").respond(200, json=listing())
        yield mock


def audit_details(fake_bot) -> list[str]:
    return [row["detail"] for row in fake_bot.repo.list_audit() if row["action"] == "config"]


def models_panel(body: str) -> str:
    return body[body.index('id="models"') : body.index('id="notifications"')]


# --- API ---------------------------------------------------------------------


def test_the_api_lists_aliases_with_capabilities_and_no_key(auth, gateway, caplog):
    caplog.set_level(logging.DEBUG)
    response = auth.get("/api/config/models")
    assert response.status_code == 200
    body = response.json()
    assert body["reachable"] is True
    by_id = {item["id"]: item for item in body["models"]}
    assert list(by_id) == [
        "gpt-oss-20b",
        "gpt-oss-120b-cloud",
        "gpt-6-luna:low",
        "bare-alias",
        "no-tools",
        "fixed-reasoning",
    ]
    assert by_id["gpt-6-luna:low"]["reasoning_efforts"] == ["low", "high"]
    assert by_id["gpt-oss-20b"]["reasoning_efforts"] is None
    assert by_id["gpt-oss-120b-cloud"]["trust_zone"] == "local"
    assert by_id["gpt-oss-120b-cloud"]["leaves_homelab"] is True
    assert by_id["gpt-6-luna:low"]["leaves_homelab"] is True
    assert by_id["gpt-oss-20b"]["leaves_homelab"] is False
    assert by_id["bare-alias"] == {
        "id": "bare-alias",
        "structured_output": False,
        "sampling_controls": False,
        "reasoning_control": False,
        "function_tools": True,
        "trust_zone": None,
        "declared": False,
        "reasoning_efforts": None,
        "leaves_homelab": False,
    }
    assert body["extract_model"] == "gpt-oss-20b"
    assert gateway.route_models.calls[0].request.headers["authorization"] == f"Bearer {KEY}"
    assert KEY not in response.text and KEY not in caplog.text


def test_selecting_through_the_api_applies_audits_and_persists(auth, gateway, fake_bot):
    response = auth.put(
        "/api/config",
        json={"extract_model": "gpt-oss-120b-cloud", "chat_pilot_model": "gpt-6-luna:low"},
    )
    assert response.status_code == 200, response.text
    assert response.json()["extract_model"] == "gpt-oss-120b-cloud"
    assert response.json()["chat_pilot_model"] == "gpt-6-luna:low"
    # The shared settings object is what the next model call reads.
    assert fake_bot.settings.extract_model == "gpt-oss-120b-cloud"
    assert fake_bot.settings.chat_pilot_model == "gpt-6-luna:low"
    assert fake_bot.repo.get_config("extract_model") == "gpt-oss-120b-cloud"
    assert "extract_model: unset -> gpt-oss-120b-cloud" in audit_details(fake_bot)
    assert "chat_pilot_model: unset -> gpt-6-luna:low" in audit_details(fake_bot)
    assert gateway.route_models.call_count == 1


def test_reasoning_levels_are_validated_and_inherit_is_allowed_for_chat(auth, gateway, fake_bot):
    assert (
        auth.put("/api/config", json={"extract_reasoning": "HIGH"}).json()["extract_reasoning"]
        == "high"
    )
    assert fake_bot.settings.reasoning_effort == "high"
    assert auth.put("/api/config", json={"chat_pilot_think": ""}).json()["chat_pilot_think"] == ""
    assert fake_bot.settings.chat_reasoning_effort == "high"
    assert "chat_pilot_think: unset -> inherit" in audit_details(fake_bot)
    assert auth.put("/api/config", json={"extract_reasoning": "maximum"}).status_code == 400
    assert auth.put("/api/config", json={"extract_reasoning": ""}).status_code == 400


def test_a_published_effort_list_limits_the_accepted_levels(auth, gateway, fake_bot):
    fake_bot.settings.chat_pilot_model = "gpt-6-luna:low"
    rejected = auth.put("/api/config", json={"chat_pilot_think": "medium"})
    assert rejected.status_code == 400
    assert "accepts reasoning `off`, `low`, `high`" in rejected.json()["error"]
    assert auth.put("/api/config", json={"chat_pilot_think": "high"}).status_code == 200
    # "off" omits the field so the alias's own default applies.
    assert auth.put("/api/config", json={"chat_pilot_think": "off"}).status_code == 200
    # Checked against the alias saved in the same request, whatever the key order.
    both = auth.put(
        "/api/config", json={"extract_reasoning": "medium", "extract_model": "gpt-6-luna:low"}
    )
    assert both.status_code == 400
    assert fake_bot.repo.get_config("extract_model") == "gpt-6-luna:low"


def test_reasoning_still_saves_when_the_gateway_is_down(auth, gateway, fake_bot):
    gateway.route_models.mock(side_effect=httpx.ConnectError("down"))
    assert auth.put("/api/config", json={"extract_reasoning": "low"}).status_code == 200
    assert fake_bot.settings.extract_reasoning == "low"


@pytest.mark.parametrize(
    ("changes", "fragment"),
    [
        ({"extract_model": "invented"}, "unknown model alias `invented`"),
        ({"extract_model": " "}, "needs a model alias"),
        ({"chat_pilot_model": "no-tools"}, "cannot call tools"),
    ],
)
def test_unknown_or_unusable_aliases_are_rejected(auth, gateway, fake_bot, changes, fragment):
    response = auth.put("/api/config", json=changes)
    assert response.status_code == 400
    assert fragment in response.json()["error"]
    assert fake_bot.settings.extract_model == "gpt-oss-20b"
    assert fake_bot.repo.get_config("extract_model") is None
    assert audit_details(fake_bot) == []


def test_an_unreachable_gateway_refuses_the_save(auth, gateway, fake_bot, caplog):
    gateway.route_models.mock(side_effect=httpx.ConnectError("down"))
    response = auth.put("/api/config", json={"extract_model": "gpt-oss-120b-cloud"})
    assert response.status_code == 503
    assert "nothing was saved" in response.json()["error"]
    assert fake_bot.repo.get_config("extract_model") is None
    assert fake_bot.settings.extract_model == "gpt-oss-20b"
    listed = auth.get("/api/config/models").json()
    assert listed["reachable"] is False and listed["models"] == []
    assert "unreachable" in listed["error"]
    assert KEY not in response.text and KEY not in caplog.text


# --- portal -----------------------------------------------------------------


def test_the_portal_shows_aliases_badges_and_no_key(auth, gateway):
    body = auth.get("/config").text
    panel = models_panel(body)
    assert '<a class="settings__tab" href="#models">Models</a>' in body
    for alias in ("gpt-oss-20b", "gpt-oss-120b-cloud", "gpt-6-luna:low", "bare-alias"):
        assert f'value="{alias}"' in panel
    assert '<option value="gpt-oss-20b" selected>' in panel
    assert "structured output" in panel and "local" in panel
    assert "Leaves the homelab" not in panel
    assert KEY not in body


def chat_part(panel: str) -> str:
    return panel[panel.index('id="chat-model"') :]


def reasoning_select(part: str, role: str) -> str:
    return part[part.index(f'id="{role}-reasoning"') :].split("</select>")[0]


@pytest.mark.parametrize("alias", ["gpt-6-luna:low", "gpt-oss-120b-cloud"])
def test_external_and_cloud_aliases_warn(auth, gateway, fake_bot, alias):
    fake_bot.settings.chat_pilot_model = alias
    chat = chat_part(models_panel(auth.get("/config").text))
    assert "member names and chat text leave the homelab" in chat.lower()
    assert "v4 has no pseudonymization" in chat
    assert "Trust zone unknown" not in chat


def test_a_local_alias_does_not_warn(auth, gateway, fake_bot):
    panel = models_panel(auth.get("/config").text)
    assert "Leaves the homelab" not in panel.split('id="chat-model"')[0]


def test_the_reasoning_selector_follows_the_alias(auth, gateway, fake_bot):
    fake_bot.settings.chat_pilot_model = "gpt-6-luna:low"
    fake_bot.settings.extract_model = "fixed-reasoning"
    panel = models_panel(auth.get("/config").text)
    chat = reasoning_select(chat_part(panel), "chat")
    assert "disabled" not in chat.split(">")[0]
    assert "off (alias default)" in chat
    assert 'value="low"' in chat and 'value="high"' in chat and 'value="medium"' not in chat
    extract = reasoning_select(panel, "extract")
    assert "disabled>" in extract
    fake_bot.settings.extract_model = "gpt-oss-20b"
    extract = reasoning_select(models_panel(auth.get("/config").text), "extract")
    assert all(f'value="{level}"' in extract for level in ("off", "low", "medium", "high"))
    assert "alias default" not in extract


def test_a_metadata_free_alias_is_flagged(auth, gateway, fake_bot):
    fake_bot.settings.extract_model = "bare-alias"
    panel = models_panel(auth.get("/config").text)
    assert "Kanata published no capabilities" in panel
    assert "Trust zone unknown" in panel


def test_the_portal_form_saves_without_javascript(auth, gateway, fake_bot):
    response = auth.post(
        "/config",
        data={"section": "models", "extract_model": "gpt-6-luna:low"},
        follow_redirects=False,
    )
    assert response.status_code == 303
    assert response.headers["location"].endswith("#models")
    assert fake_bot.settings.extract_model == "gpt-6-luna:low"

    rejected = auth.post(
        "/config", data={"section": "models", "extract_model": "nope"}, follow_redirects=False
    )
    assert "kind=error" in rejected.headers["location"]
    assert fake_bot.settings.extract_model == "gpt-6-luna:low"


def test_the_portal_explains_an_unreachable_gateway(auth, gateway):
    gateway.route_models.mock(return_value=httpx.Response(502))
    panel = models_panel(auth.get("/config").text)
    assert "Kanata unreachable" in panel
    assert "HTTP 502" in panel


# --- seeding, restart and startup -----------------------------------------------


def seed_settings(tmp_path, owner_lock_dir, **overrides):
    return make_settings(
        db_path=str(tmp_path / "bot.sqlite"), db_owner_lock_dir=str(owner_lock_dir), **overrides
    )


def test_env_seeds_once_and_the_stored_selection_wins_after_restart(tmp_path, owner_lock_dir):
    first = seed_settings(
        tmp_path,
        owner_lock_dir,
        extract_model="env-a",
        extract_reasoning="low",
        chat_pilot_think="",
    )
    repo = asyncio.run(entry.build_repo(first))
    try:
        assert repo.get_config("extract_model") == "env-a"
        assert repo.get_config("chat_pilot_model") == "test-chat-model"
        assert repo.get_config("extract_reasoning") == "low"
        # Empty CHAT_PILOT_THINK is not seeded: absent still means "inherit".
        assert repo.get_config("chat_pilot_think") is None
        repo.set_config("extract_model", "portal-choice")
        repo.set_config("chat_pilot_think", "high")
    finally:
        repo.close()

    restarted = seed_settings(tmp_path, owner_lock_dir, extract_model="env-b")
    repo = asyncio.run(entry.build_repo(restarted))
    try:
        restarted.apply_runtime_models(repo.get_config)
    finally:
        repo.close()
    assert restarted.extract_model == "portal-choice"
    assert restarted.extract_reasoning == "low"
    assert restarted.chat_reasoning_effort == "high"


def test_empty_model_env_is_not_seeded(tmp_path, owner_lock_dir):
    settings = seed_settings(tmp_path, owner_lock_dir, extract_model="", chat_pilot_model="")
    repo = asyncio.run(entry.build_repo(settings))
    try:
        assert repo.get_config("extract_model") is None
        assert repo.get_config("chat_pilot_model") is None
    finally:
        repo.close()


def test_unreadable_stored_reasoning_keeps_the_seed(tmp_path):
    settings = make_settings(extract_reasoning="medium")
    rows = {"extract_reasoning": "turbo", "extract_model": "  "}
    settings.apply_runtime_models(rows.get)
    assert settings.extract_reasoning == "medium"
    assert settings.extract_model == "test-extract-model"


def test_startup_accepts_a_db_only_selection(monkeypatch, tmp_path, owner_lock_dir):
    key = tmp_path / "kanata.key"
    key.write_text(KEY, encoding="utf-8")
    db = tmp_path / "bot.sqlite"
    seeded = seed_settings(tmp_path, owner_lock_dir, extract_model="db-extract")
    repo = asyncio.run(entry.build_repo(seeded))
    repo.set_config("chat_pilot_model", "db-chat")
    repo.close()

    settings = seed_settings(
        tmp_path,
        owner_lock_dir,
        extract_model="",
        chat_pilot_model="",
        chat_pilot_role_id=1,
        chat_pilot_channel_ids="2",
        kanata_api_key_file=str(key),
    )
    monkeypatch.setattr(entry, "get_settings", lambda: settings)
    monkeypatch.setattr(entry, "configure_logging", lambda _level: None)
    monkeypatch.setattr(entry, "FATAL_EXIT_DELAY", 0.01)
    monkeypatch.setenv("TZ", os.environ.get("TZ", settings.tz))

    class Reached(Exception):
        pass

    def reached(_settings):
        raise Reached

    monkeypatch.setattr(entry, "load_boss_resources", reached)
    with pytest.raises(Reached):
        asyncio.run(entry.run())
    assert settings.extract_model == "db-extract"
    assert settings.chat_pilot_model == "db-chat"
    assert db.exists()


def test_startup_still_exits_without_any_selection(monkeypatch, tmp_path, owner_lock_dir, caplog):
    key = tmp_path / "kanata.key"
    key.write_text(KEY, encoding="utf-8")
    settings = seed_settings(
        tmp_path, owner_lock_dir, extract_model="", kanata_api_key_file=str(key)
    )
    monkeypatch.setattr(entry, "get_settings", lambda: settings)
    monkeypatch.setattr(entry, "configure_logging", lambda _level: None)
    monkeypatch.setattr(entry, "FATAL_EXIT_DELAY", 0.01)
    monkeypatch.setenv("TZ", os.environ.get("TZ", settings.tz))
    caplog.set_level(logging.ERROR, logger="bot")
    assert asyncio.run(entry.run()) == 2
    assert "no extraction model is selected" in caplog.text


# --- bundle -------------------------------------------------------------------


def test_bundle_runtime_config_carries_the_model_keys():
    assert set(MODEL_CONFIG_KEYS) <= RUNTIME_CONFIG_KEYS
    config = Config.model_validate(
        {
            "timezone": "Asia/Kuala_Lumpur",
            "boss_week_reset_time": "00:00",
            "runtime": {
                "extract_model": "gpt-oss-20b",
                "extract_reasoning": "low",
                "chat_pilot_model": "gpt-6-luna:low",
                "chat_pilot_think": "",
            },
        }
    )
    assert config.runtime["chat_pilot_model"] == "gpt-6-luna:low"
