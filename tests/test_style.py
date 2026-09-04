"""Member-selectable chatbot reply styles."""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

import pytest

from bot import behaviour_plugins
from bot.agent.commands import MissingChatAccess, style, style_autocomplete


class Response:
    def __init__(self):
        self.sent: list[tuple[str, bool]] = []

    async def send_message(self, content: str, ephemeral: bool = False) -> None:
        self.sent.append((content, ephemeral))


class Interaction:
    def __init__(self, bot, *, roles=(), administrator=False):
        self.client = bot
        self.user = SimpleNamespace(
            id=901,
            display_name="Style Tester",
            nick=None,
            roles=[SimpleNamespace(id=role_id) for role_id in roles],
            guild_permissions=SimpleNamespace(administrator=administrator),
        )
        self.guild = bot.guild
        self.response = Response()


@pytest.fixture
def styles(fake_bot, tmp_path, monkeypatch):
    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    for name in ("calm", "brief"):
        behaviour_plugins.write(name, name.upper())
    fake_bot.repo.set_config(behaviour_plugins.SELECTABLE_CONFIG_KEY, '["brief","calm"]')
    return fake_bot


def run_style(bot, profile=None):
    interaction = Interaction(bot)
    asyncio.run(style.callback(interaction, profile=profile))
    return interaction


def test_style_saves_only_a_public_choice(styles):
    interaction = run_style(styles, "calm")
    assert styles.repo.get_reply_style(901) == "calm"
    assert interaction.response.sent == [
        ("Reply style preference saved as **calm**.", True)
    ]
    audit = styles.repo.list_audit()[0]
    assert "saved reply style `calm`" in audit["detail"]


def test_style_rejects_private_or_missing_profiles(styles):
    behaviour_plugins.write("private", "PRIVATE")
    interaction = run_style(styles, "private")
    assert styles.repo.get_reply_style(901) is None
    assert "not available" in interaction.response.sent[0][0]
    assert interaction.response.sent[0][1] is True


def test_style_reset_is_described_as_saved_not_active(styles):
    styles.repo.upsert_member(901, "Style Tester", None, False)
    styles.repo.set_reply_style(901, "calm")
    interaction = run_style(styles, "default")
    assert styles.repo.get_reply_style(901) is None
    message, ephemeral = interaction.response.sent[0]
    assert message == "Reply style preference saved as **default**."
    assert "active" not in message.lower()
    assert ephemeral is True


def test_style_readback_does_not_disclose_role_resolution(styles):
    styles.repo.upsert_member(901, "Style Tester", None, False)
    styles.repo.set_reply_style(901, "brief")
    interaction = run_style(styles)
    message, ephemeral = interaction.response.sent[0]
    assert "saved reply style" in message
    assert "role" not in message.lower()
    assert ephemeral is True


def test_style_autocomplete_is_dynamic_ordered_and_capped(styles):
    for index in range(28):
        name = f"style-{index:02d}"
        behaviour_plugins.write(name, name)
    catalog = ["brief", "calm", *(f"style-{index:02d}" for index in range(28))]
    styles.repo.set_config(
        behaviour_plugins.SELECTABLE_CONFIG_KEY,
        behaviour_plugins.encode_catalog(catalog),
    )
    choices = asyncio.run(style_autocomplete(Interaction(styles), ""))
    assert [choice.value for choice in choices[:3]] == ["default", "brief", "calm"]
    assert len(choices) == 25


def test_style_requires_chat_access_or_staff(styles):
    styles.settings.chat_pilot_role_id = 555
    with pytest.raises(MissingChatAccess):
        asyncio.run(style.checks[0](Interaction(styles)))
    assert asyncio.run(style.checks[0](Interaction(styles, roles=[555]))) is True
    assert asyncio.run(style.checks[0](Interaction(styles, administrator=True))) is True
