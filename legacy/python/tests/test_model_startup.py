"""Startup handling of missing model-gateway settings."""

from __future__ import annotations

import asyncio
import logging
import os

import bot.__main__ as entry
from bot.agent.client import CFG_EXTRACT
from bot.infrastructure.db import Repo

from .fake_bot import make_settings


def test_run_exits_with_code_2_when_model_settings_are_missing(monkeypatch, caplog, tmp_path):
    pasted = "sk-kanata-pasted-into-path-3c7d"
    settings = make_settings(extract_model="", kanata_api_key_file=pasted)
    monkeypatch.setattr(entry, "get_settings", lambda: settings)
    monkeypatch.setattr(entry, "configure_logging", lambda _level: None)
    monkeypatch.setattr(entry, "FATAL_EXIT_DELAY", 0.01)
    monkeypatch.setenv("TZ", os.environ.get("TZ", settings.tz))

    def never(_settings):
        raise AssertionError("startup went past the model-settings check")

    monkeypatch.setattr(entry, "load_boss_resources", never)
    caplog.set_level(logging.ERROR, logger="bot")

    assert asyncio.run(entry.run()) == 2
    assert "EXTRACT_MODEL is not set" in caplog.text
    assert "KANATA_API_KEY_FILE" in caplog.text
    assert pasted not in caplog.text


def repo_with_extractor(flag: str) -> Repo:
    repo = Repo(":memory:")
    repo.set_config(CFG_EXTRACT, flag)
    return repo


def test_a_stored_extractor_switch_reveals_missing_settings():
    settings = make_settings(extract_enabled=False, extract_model="")
    repo = repo_with_extractor("1")
    try:
        problems = entry.runtime_model_problems(settings, repo)
    finally:
        repo.close()
    assert [p.split(" ")[0] for p in problems] == ["EXTRACT_MODEL", "KANATA_API_KEY_FILE"]
    assert all("overrides EXTRACT_ENABLED=false" in p for p in problems)


def test_nothing_extra_is_reported_when_the_switch_agrees(tmp_path):
    key = tmp_path / "kanata.key"
    key.write_text("sk-ok", encoding="utf-8")
    off = make_settings(extract_enabled=False, extract_model="")
    ready = make_settings(extract_enabled=False, extract_model="x", kanata_api_key_file=str(key))
    for settings, flag in ((off, "0"), (ready, "1")):
        repo = repo_with_extractor(flag)
        try:
            assert entry.runtime_model_problems(settings, repo) == []
        finally:
            repo.close()
