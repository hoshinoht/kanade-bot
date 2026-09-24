"""Settings parsing, including hand-written `.env` files with inline comments."""

from __future__ import annotations

from datetime import time

import pytest
from pydantic import ValidationError

from bot.infrastructure.config import Settings, normalize_reasoning

REQUIRED = {
    "discord_token": "token",
    "guild_id": 1,
    "bossing_role_id": 3,
    "chat_channel_ids": "10",
}


def make(**overrides) -> Settings:
    # _env_file=None keeps the developer's real .env out of the tests.
    return Settings(_env_file=None, **{**REQUIRED, **overrides})


def test_defaults_match_the_design():
    settings = make()
    assert settings.tz == "Asia/Kuala_Lumpur"
    assert settings.reset_weekday == 3  # Thursday
    assert settings.reset_time == time(0, 0)
    assert settings.countdown_minute_list == [60]
    assert settings.day_of_ping_time == "01:00"
    assert settings.admin_role_id is None
    assert settings.db_owner_lock_dir is None
    # Runs post in their own home channel, so the guild-wide channel is optional.
    assert settings.post_channel_id is None


def test_comma_lists_are_parsed_not_json_decoded():
    settings = make(chat_channel_ids="10, 20,30", countdown_minutes="15;60;15")
    assert settings.chat_channel_id_list == [10, 20, 30]
    assert settings.countdown_minute_list == [60, 15]  # sorted, de-duplicated


def test_inline_comments_are_stripped():
    settings = make(guild_id="123   # right-click server -> Copy Server ID")
    assert settings.guild_id == 123


def test_a_blank_value_with_only_a_comment_falls_back_to_the_default():
    # `ADMIN_ROLE_ID=            # optional: role allowed to amend any run`
    settings = make(admin_role_id="            # optional: role allowed to amend any run")
    assert settings.admin_role_id is None


def test_a_blank_optional_value_falls_back_to_the_default():
    assert make(countdown_minutes="").countdown_minute_list == [60]


def test_secrets_are_never_comment_stripped():
    # A token is whatever the user pasted, even if it somehow contains a hash.
    assert make(discord_token="abc#def").discord_token == "abc#def"


def test_a_missing_required_value_says_so_plainly():
    with pytest.raises(ValidationError, match="bossing_role_id"):
        Settings(_env_file=None, discord_token="t", guild_id=1, chat_channel_ids="10")


def test_at_least_one_watched_channel_is_required():
    with pytest.raises(ValidationError, match="CHAT_CHANNEL_IDS"):
        make(chat_channel_ids="", chat_category_ids="")


def test_either_watched_channels_or_categories_will_do():
    assert make(chat_channel_ids="", chat_category_ids="7").chat_category_id_list == [7]
    assert make(chat_channel_ids="7", chat_category_ids="").chat_channel_id_list == [7]


def test_watched_channels_and_categories_are_both_parsed():
    settings = make(chat_channel_ids="1,2", chat_category_ids="3")
    assert settings.chat_channel_id_list == [1, 2]
    assert settings.chat_category_id_list == [3]


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("tz", "Mars/Olympus"),
        ("boss_week_reset_weekday", "caturday"),
        ("boss_week_reset_time", "25:00"),
        ("day_of_ping_time", "25:00"),
        ("countdown_minutes", "-5"),
    ],
)
def test_bad_values_are_rejected_at_startup(field, value):
    with pytest.raises(ValidationError):
        make(**{field: value})


def test_zoneinfo_and_derived_helpers():
    settings = make(tz="UTC", boss_week_reset_weekday="mon", boss_week_reset_time="12:30")
    assert settings.zoneinfo.key == "UTC"
    assert settings.reset_weekday == 0
    assert settings.reset_time == time(12, 30)


@pytest.mark.parametrize(
    ("value", "expected"),
    [("low", "low"), ("MEDIUM", "medium"), ("off", "none"), ("false", "none"), ("none", "none")],
)
def test_reasoning_off_is_sent_as_none_not_omitted(value, expected):
    """A model that reasons by default needs ``reasoning_effort="none"``, not silence."""
    assert make(extract_reasoning=value).reasoning_effort == expected


def test_reasoning_defaults_off():
    assert make().extract_reasoning == "off"
    assert make().reasoning_effort == "none"


def test_bad_reasoning_is_rejected_at_startup():
    with pytest.raises(ValidationError, match="EXTRACT_REASONING"):
        make(extract_reasoning="max")


def test_chat_reasoning_falls_back_to_extract_reasoning_by_default():
    assert make().chat_reasoning_effort == "none"
    assert make(extract_reasoning="low").chat_reasoning_effort == "low"
    assert make(chat_pilot_think="").chat_reasoning_effort == "none"


def test_chat_reasoning_overrides_extract_reasoning_independently():
    settings = make(extract_reasoning="low", chat_pilot_think="medium")
    assert settings.reasoning_effort == "low"
    assert settings.chat_reasoning_effort == "medium"
    assert make(extract_reasoning="low", chat_pilot_think="OFF").chat_reasoning_effort == "none"


def test_gateway_defaults_and_budget():
    settings = make()
    assert settings.kanata_base_url == "https://sumi.kanata.hoshinoht.dev"
    assert settings.kanata_api_key_file == ""
    assert settings.kanata_timeout == 120.0
    assert settings.model_context_tokens == 8192
    # No committed aliases: the operator names them.
    assert settings.extract_model == ""
    assert settings.chat_pilot_model == ""


@pytest.mark.parametrize(
    "url",
    [
        "http://sumi.kanata.hoshinoht.dev",
        "http://127.0.0.1:8080",
        "https://user:pw@sumi.kanata.hoshinoht.dev",
        "https://sumi.kanata.hoshinoht.dev/?key=x",
        "sumi.kanata.hoshinoht.dev",
    ],
)
def test_the_gateway_url_must_be_plain_https(url):
    with pytest.raises(ValidationError, match="KANATA_BASE_URL"):
        make(kanata_base_url=url)


def test_the_gateway_url_loses_its_trailing_slash():
    assert make(kanata_base_url="https://gw.example/ ").kanata_base_url == "https://gw.example"


@pytest.mark.parametrize(
    ("url", "expected"),
    [
        ("https://gw.example/v1", "https://gw.example"),
        ("https://gw.example/v1/", "https://gw.example"),
        ("https://gw.example/kanata/v1", "https://gw.example/kanata"),
        ("https://gw.example/v10", "https://gw.example/v10"),
    ],
)
def test_an_sdk_style_v1_suffix_is_trimmed(url, expected):
    """The client appends /v1 itself; keeping it would request /v1/v1/..."""
    assert make(kanata_base_url=url).kanata_base_url == expected


@pytest.mark.parametrize("value", [0, 2047])
def test_the_context_budget_has_a_floor(value):
    with pytest.raises(ValidationError):
        make(model_context_tokens=value)


def test_model_settings_are_required_only_for_enabled_features(tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text("  sk-secret-value \n", encoding="utf-8")
    chat = {"chat_pilot_role_id": 7, "chat_pilot_channel_ids": "8"}

    assert make(extract_enabled=False).model_setting_errors() == []
    errors = make(**chat).model_setting_errors()
    assert [e.split(" ")[0] for e in errors] == [
        "EXTRACT_MODEL",
        "CHAT_PILOT_MODEL",
        "KANATA_API_KEY_FILE",
    ]
    ready = make(**chat, extract_model="x", chat_pilot_model="y", kanata_api_key_file=str(key))
    assert ready.model_setting_errors() == []


@pytest.mark.parametrize("content", ["", "  \n\t"])
def test_an_empty_key_file_is_a_clear_error(tmp_path, content):
    key = tmp_path / "kanata.key"
    key.write_text(content, encoding="utf-8")
    errors = make(extract_model="x", kanata_api_key_file=str(key)).model_setting_errors()
    assert errors == ["KANATA_API_KEY_FILE holds an empty key"]


def test_a_missing_or_unreadable_key_file_is_a_clear_error(tmp_path):
    missing = tmp_path / "absent.key"
    errors = make(extract_model="x", kanata_api_key_file=str(missing)).model_setting_errors()
    assert errors == ["KANATA_API_KEY_FILE points to a missing file"]
    errors = make(extract_model="x", kanata_api_key_file=str(tmp_path)).model_setting_errors()
    assert errors == ["KANATA_API_KEY_FILE is unreadable (IsADirectoryError)"]


@pytest.mark.parametrize("pasted", ["sk-kanata-pasted-9b1e", "sk-kanata\x00pasted"])
def test_a_key_pasted_as_the_path_is_never_echoed(pasted):
    errors = make(extract_model="x", kanata_api_key_file=pasted).model_setting_errors()
    assert len(errors) == 1 and errors[0].startswith("KANATA_API_KEY_FILE ")
    assert "pasted" not in errors[0]


@pytest.mark.parametrize("key", ["sk-kanäta", "sk kanata", "sk-\x07kanata"])
def test_a_key_a_header_cannot_carry_is_rejected(tmp_path, key):
    path = tmp_path / "kanata.key"
    path.write_text(key, encoding="utf-8")
    errors = make(extract_model="x", kanata_api_key_file=str(path)).model_setting_errors()
    assert errors == [
        "KANATA_API_KEY_FILE holds a key with spaces, control or non-ASCII characters"
    ]


def test_the_runtime_extractor_switch_can_require_model_settings():
    settings = make(extract_enabled=False)
    assert settings.model_setting_errors() == []
    assert [e.split(" ")[0] for e in settings.model_setting_errors(extract_on=True)] == [
        "EXTRACT_MODEL",
        "KANATA_API_KEY_FILE",
    ]


def test_bad_chat_think_is_rejected_at_startup():
    with pytest.raises(ValidationError):
        make(chat_pilot_think="ultra")


def test_removed_memory_settings_are_ignored_and_reported(monkeypatch, tmp_path):
    env = tmp_path / ".env"
    env.write_text("CHAT_MEMORY_ENABLED=true  # left over\n", encoding="utf-8")
    monkeypatch.setenv("CHAT_MEMORY_RETENTION_DAYS", "30")

    settings = Settings(_env_file=env, **REQUIRED)

    assert settings.removed_settings == ("CHAT_MEMORY_ENABLED", "CHAT_MEMORY_RETENTION_DAYS")
    assert not hasattr(settings, "chat_memory_enabled")
    assert not [name for name in Settings.model_fields if "memory" in name]


def test_removed_ollama_settings_are_reported_with_their_replacements(monkeypatch):
    from bot.infrastructure.config import removed_settings_notice

    monkeypatch.setenv("OLLAMA_HOST", "http://host.docker.internal:11434")
    settings = make(ollama_num_ctx="16384", ollama_model="gemma4:12b")

    assert settings.removed_settings == ("OLLAMA_HOST", "OLLAMA_MODEL", "OLLAMA_NUM_CTX")
    assert settings.model_context_tokens == 8192
    assert removed_settings_notice(settings.removed_settings) == (
        "OLLAMA_HOST (use KANATA_BASE_URL), OLLAMA_MODEL (use EXTRACT_MODEL), "
        "OLLAMA_NUM_CTX (use MODEL_CONTEXT_TOKENS)"
    )
    assert removed_settings_notice(("CHAT_MEMORY_ENABLED", "OLLAMA_THINK", "OLLAMA_TIMEOUT")) == (
        "CHAT_MEMORY_ENABLED (personal memory was removed), "
        "OLLAMA_THINK (use EXTRACT_REASONING), OLLAMA_TIMEOUT (use KANATA_TIMEOUT)"
    )


def test_clean_settings_report_nothing_removed(monkeypatch):
    from bot.infrastructure.config import REPLACED_SETTINGS

    for name in REPLACED_SETTINGS:
        monkeypatch.delenv(name, raising=False)
    assert make().removed_settings == ()


def test_the_env_example_no_longer_documents_memory():
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    assert "CHAT_MEMORY_" not in text


def test_the_env_example_documents_the_gateway_not_ollama():
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    assert not [line for line in text.splitlines() if line.startswith("OLLAMA_")]
    for key in ("KANATA_API_KEY_FILE=", "EXTRACT_MODEL="):
        assert key in text
    guide = (REPO_ROOT / "docs" / "setup.md").read_text(encoding="utf-8")
    for key in ("`KANATA_BASE_URL`", "`EXTRACT_REASONING`"):
        assert key in guide


def test_stored_model_selection_satisfies_the_model_checks(tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text("sk-ok", encoding="utf-8")
    chat = {"chat_pilot_role_id": 7, "chat_pilot_channel_ids": "8"}
    settings = make(**chat, kanata_api_key_file=str(key))
    assert len(settings.model_setting_errors()) == 2
    rows = {"extract_model": "db-a", "chat_pilot_model": "db-b", "chat_pilot_think": "none"}
    settings.apply_runtime_models(rows.get)
    assert settings.model_setting_errors() == []
    assert settings.chat_reasoning_effort == "none"


@pytest.mark.parametrize(
    ("value", "inherit", "expected"),
    [("LOW", False, "low"), ("none", False, "off"), ("false", False, "off"), ("", True, "")],
)
def test_reasoning_values_normalize(value, inherit, expected):
    assert normalize_reasoning(value, inherit=inherit) == expected


@pytest.mark.parametrize(("value", "inherit"), [("", False), ("max", True)])
def test_bad_reasoning_values_are_rejected(value, inherit):
    with pytest.raises(ValueError):
        normalize_reasoning(value, inherit=inherit)
