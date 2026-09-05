from __future__ import annotations

import pytest

from bot.chat import progress
from bot.chat.agent import ChatPilot

from .chat_support import FakeOllama, message, says

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def strategy_ready(bot):
    from bot.domain.boss_knowledge import BossKnowledgeBase

    from .conftest import REPO_ROOT

    bot.boss_knowledge = BossKnowledgeBase.load(REPO_ROOT / "boss" / "knowledge", bot.bosses)


def test_schedule_copy(chat_bot):
    assert progress.placeholder_for("@bot what's on tonight?", chat_bot.bosses) == (
        progress.STAGING_SCHEDULE
    )


def test_guide_named_copy(chat_bot):
    strategy_ready(chat_bot)
    assert progress.placeholder_for("@bot how to beat fa", chat_bot.bosses) == (
        "Reading checked-in notes for FA…"
    )


def test_write_copy(chat_bot):
    assert progress.placeholder_for("@bot move hstar to wed", chat_bot.bosses) == (
        progress.STAGING_WRITE
    )


def test_generic_copy(chat_bot):
    assert progress.placeholder_for("@bot hello", chat_bot.bosses) == progress.STAGING_GENERIC


def test_staging_lines_have_no_mentions(chat_bot):
    for line in (
        progress.STAGING_SCHEDULE,
        progress.STAGING_GUIDE,
        progress.STAGING_WRITE,
        progress.STAGING_GENERIC,
        progress.placeholder_for("@bot how to beat fa", chat_bot.bosses),
    ):
        assert "<@" not in line
        assert "@everyone" not in line
        assert "@here" not in line


async def test_placeholder_is_silent_and_final_pings(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("Wed 21:30."))
    await agent.offer(message(chat_bot))
    assert len(chat_bot.staging_posts) == 1
    assert chat_bot.staging_posts[0].mentions == []
    assert chat_bot.staging_posts[0].roles == []
    assert len(chat_bot.posts) == 1
    assert chat_bot.posts[0].content == "Wed 21:30."
    assert chat_bot.posts[0].mentions == ["1002"]


async def test_empty_reply_edits_placeholder_silently(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("   "))
    await agent.offer(message(chat_bot))
    assert len(chat_bot.posts) == 1
    assert chat_bot.posts[0].mentions == []
    assert "couldn't complete" in chat_bot.posts[0].content


async def test_placeholder_failure_still_posts(chat_bot, chat_seeded):
    orig = chat_bot.post_plain

    async def flaky(channel, content, mention_users, reference_id=None, mention_roles=None,
                   silent: bool = False):
        if silent:
            raise RuntimeError("staging failed")
        return await orig(channel, content, mention_users, reference_id, mention_roles)

    chat_bot.post_plain = flaky
    agent: ChatPilot = pilot(chat_bot, says("Wed 21:30."))
    await agent.offer(message(chat_bot))
    assert chat_bot.posts[-1].content == "Wed 21:30."
    assert chat_bot.posts[-1].mentions == ["1002"]


def test_presets_yaml_overrides_defaults(tmp_path, chat_bot):
    staging = tmp_path / "staging.yaml"
    staging.write_text("schedule: Custom schedule…\ngeneric: Custom generic…\n", encoding="utf-8")
    table = progress.load_staging_file(staging)
    assert table["schedule"] == "Custom schedule…"
    assert table["generic"] == "Custom generic…"
    assert (
        progress.placeholder_for("@bot what's on tonight?", chat_bot.bosses, staging=table)
        == "Custom schedule…"
    )
    assert progress.placeholder_for("@bot hello", chat_bot.bosses, staging=table) == (
        "Custom generic…"
    )


def test_missing_presets_file_falls_back():
    table = progress.load_staging_file("/nonexistent/staging.yaml")
    assert table["schedule"] == progress.STAGING_SCHEDULE
    assert table["generic"] == progress.STAGING_GENERIC


def test_full_override():
    config = {
        "default": {k: f"base-{k}" for k in progress.STAGING_KEYS},
        "profiles": {"tsundere": {k: f"tsun-{k}" for k in progress.STAGING_KEYS}},
    }
    lines = progress.load_profile_staging(config, "tsundere")
    assert (lines.schedule, lines.guide, lines.guide_named, lines.write, lines.generic) == (
        "tsun-schedule",
        "tsun-guide",
        "tsun-guide_named",
        "tsun-write",
        "tsun-generic",
    )


def test_partial_override_inherits_default():
    config = {"default": {k: f"base-{k}" for k in progress.STAGING_KEYS},
              "profiles": {"kuudere": {"generic": "Custom thinking"}}}
    lines = progress.load_profile_staging(config, "kuudere")
    assert lines.generic == "Custom thinking"
    assert lines.schedule == "base-schedule"
    assert lines.guide == "base-guide"
    assert lines.guide_named == "base-guide_named"
    assert lines.write == "base-write"


def test_unknown_profile_uses_default():
    config = {"default": {k: f"base-{k}" for k in progress.STAGING_KEYS}}
    assert progress.load_profile_staging(config, "does-not-exist").schedule == "base-schedule"
    assert progress.load_profile_staging(config, None).generic == "base-generic"


def test_named_boss_interpolation():
    config = {"default": {k: f"base-{k}" for k in progress.STAGING_KEYS}}
    config["default"]["guide_named"] = "{boss} combat profile identified."
    lines = progress.load_profile_staging(config, None)
    assert lines.guide_named.format(boss="FA") == "FA combat profile identified."


def test_invalid_configs_rejected():
    import pytest

    base = {k: f"base-{k}" for k in progress.STAGING_KEYS}
    with pytest.raises(progress.StagingConfigError):
        progress.parse_staging_config({"profiles": {}})
    with pytest.raises(progress.StagingConfigError):
        progress.parse_staging_config({"default": {"schedule": "x"}})
    with pytest.raises(progress.StagingConfigError):
        progress.parse_staging_config({"default": {**base, "bogus": "x"}})
    with pytest.raises(progress.StagingConfigError):
        progress.parse_staging_config({"default": {**base, "generic": 123}})
    with pytest.raises(progress.StagingConfigError):
        progress.parse_staging_config("not-a-mapping")


def test_split_dir_partial_override(tmp_path):
    default_file = tmp_path / "staging.yaml"
    default_file.write_text(
        "default:\n" + "".join(f"  {k}: base-{k}\n" for k in progress.STAGING_KEYS),
        encoding="utf-8",
    )
    profiles_dir = tmp_path / "profiles"
    profiles_dir.mkdir()
    (profiles_dir / "kuudere.yaml").write_text("generic: Custom thinking\n", encoding="utf-8")
    default, profiles = progress.load_staging_split(default_file, profiles_dir)
    assert default.schedule == "base-schedule"
    assert profiles["kuudere"].generic == "Custom thinking"
    assert profiles["kuudere"].schedule == "base-schedule"
    assert progress.load_profile_staging((default, profiles), "does-not-exist") == default


def test_staging_linkage():
    orphans, missing = progress.staging_linkage(
        {"tsundere": progress.DEFAULT_LINES}, ["tsundere", "encik"]
    )
    assert orphans == []
    assert missing == ["encik"]
    orphans, _ = progress.staging_linkage(
        {"ghost": progress.DEFAULT_LINES}, ["tsundere"]
    )
    assert orphans == ["ghost"]


async def test_ask_instead_uses_profile_generic(chat_bot, chat_seeded, monkeypatch):
    from .test_chat_followup import reject, synthetic_card

    agent = pilot(chat_bot, says("What should it be?"))
    agent._staging = (
        progress.DEFAULT_LINES,
        {
            "tsundere": progress.StagingLines(
                schedule="s",
                guide="g",
                guide_named="{boss} n",
                write="w",
                generic="Profile thinking…",
            )
        },
    )
    monkeypatch.setattr(agent, "active_profile_name", lambda *a, **k: "tsundere")
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])
    assert chat_bot.staging_posts
    assert chat_bot.staging_posts[0].content == "Profile thinking…"
    assert chat_bot.staging_posts[0].mentions == []
