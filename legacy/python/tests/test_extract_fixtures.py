"""The fixture suite: the real ``EXTRACT_MODEL`` alias against the guild's own chat.

    KANATA_BASE_URL=... KANATA_API_KEY_FILE=... EXTRACT_MODEL=... \
        uv run pytest -m live_model -v

Slow (roughly 5-30 s per fixture) and excluded from the default run by
``addopts = -m 'not live_model'``.  Skipped entirely unless the gateway URL, key
file and alias are set in the process environment, so it is safe to leave in CI.

Every fixture is real, anonymised chat with the amendments a correct extraction
produces.  Scoring is strict -- everything expected must be found and nothing
extra invented -- and each kind is scored on the fields its commit path actually
uses (``tests/fixture_loader.SCORED_BOSSES`` / ``SCORED_TIME``).
"""

from __future__ import annotations

import asyncio

import pytest

from bot.extract import prompt as prompt_mod
from bot.extract.llm import Extractor, json_instruction_tokens
from bot.infrastructure.config import Settings

from . import fixture_loader as fl
from .live_model import require_live_gateway

pytestmark = pytest.mark.live_model

#: A prompt bigger than this would not fit the budget in DESIGN.md §4 (~3k tokens).
#: A fixture prompt should stay well under the real ceiling, which is
#: `prompt_budget(8192)` = 5692. Raised from 3000 when `estimate_tokens` stopped
#: undercounting by a third, not because the prompts grew: the same fixtures
#: that measured ~1900 now measure ~2800, and the model always did read ~2500.
MAX_PROMPT_TOKENS = 4400


@pytest.fixture(scope="session")
def settings() -> Settings:
    live = require_live_gateway("EXTRACT_MODEL")
    return Settings(
        discord_token="unused",
        guild_id=1,
        bossing_role_id=1,
        chat_channel_ids="1",
        kanata_base_url=live["KANATA_BASE_URL"],
        kanata_api_key_file=live["KANATA_API_KEY_FILE"],
        extract_model=live["EXTRACT_MODEL"],
    )


@pytest.fixture(scope="session")
def results(settings, bosses) -> dict[str, fl.Score]:
    """One model call per fixture, shared by every assertion below.

    All of them run inside a single :func:`asyncio.run`, because the underlying
    httpx client belongs to the loop it was created on -- closing it from a
    second loop raises. The calls are made once for the whole session because
    each is seconds long and the model is ~8 GB.
    """

    async def run_all():
        extractor = Extractor(settings)
        scored: dict[str, fl.Score] = {}
        try:
            for path in fl.fixture_paths():
                scenario = fl.load(path)
                messages = prompt_mod.build_messages(scenario.context_for(bosses))
                call = await extractor.extract(messages)
                assert call.ok, f"{scenario.name}: the model failed: {call.error}"
                scored[scenario.name] = fl.score(
                    scenario,
                    call.extraction,
                    min_confidence=settings.extract_min_confidence,
                    table=bosses,
                )
        finally:
            await extractor.close()
        return scored

    return asyncio.run(run_all())


def fixture_names() -> list[str]:
    return [fl.load(path).name for path in fl.fixture_paths()]


@pytest.mark.parametrize("name", fixture_names())
def test_fixture(results, name):
    """One test per fixture, so a report says exactly which cases regressed."""
    score = results[name]
    assert score.passed, f"{name}: {score.reason()}\n" + "\n".join(
        f"  got: {a.describe(score.scenario.tz)}" for a in score.actuals
    )


def test_banter_never_produces_an_amendment(results):
    """The one class of failure that must never happen: inventing a change."""
    for name in ("banter", "banter-channel-number"):
        assert results[name].actuals == [], f"{name} invented {results[name].reason()}"


@pytest.mark.parametrize("path", fl.fixture_paths(), ids=lambda p: p.stem)
def test_every_prompt_stays_inside_the_budget(path, bosses):
    scenario = fl.load(path)
    messages = prompt_mod.build_messages(scenario.context_for(bosses))
    tokens = prompt_mod.estimate_tokens("\n".join(m["content"] for m in messages))
    # Aliases without structured output also carry the JSON schema in the prompt.
    tokens += json_instruction_tokens()
    assert tokens < MAX_PROMPT_TOKENS, f"{scenario.name}: prompt is ~{tokens} tokens"
