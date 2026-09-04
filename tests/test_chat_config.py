"""`chat_mode`, the settings behind it, and where `on_message` hands a message over.

The kill switch has to reach every surface the other runtime flags reach -- the
config table, the API, the portal, `bossctl` and `/debug status` -- or turning
the chatbot off in one place leaves it answering from another.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime

import pytest

from bot import behaviour_plugins
from bot.agent.client import CFG_CHAT, BossBot
from bot.api import service
from bot.infrastructure.config import Settings
from bot.infrastructure.db import Repo

from .chat_support import CHAT_CATEGORY, CHAT_CHANNEL, CHAT_ROLE, chat_settings, message
from .fake_bot import UNWATCHED_CHANNEL, WATCHED_CHANNEL

# ---------------------------------------------------------------------------
# settings
# ---------------------------------------------------------------------------


def test_the_pilot_uses_its_own_names_not_the_extractors():
    """`CHAT_CHANNEL_IDS`/`CHAT_CATEGORY_IDS` are the extractor's watch lists."""
    settings = chat_settings()
    assert settings.chat_pilot_channel_id_list == [CHAT_CHANNEL]
    assert settings.chat_pilot_category_id_list == [CHAT_CATEGORY]
    assert CHAT_CHANNEL not in settings.chat_channel_id_list
    assert CHAT_CATEGORY not in settings.chat_category_id_list
    assert settings.chat_channel_id_list == [WATCHED_CHANNEL, 333333333333333333]


@pytest.mark.parametrize(
    ("overrides", "configured"),
    [
        ({}, True),
        # A role plus *either* list is enough.
        ({"chat_pilot_category_ids": ""}, True),
        ({"chat_pilot_channel_ids": ""}, True),
        ({"chat_pilot_role_id": None}, False),
        ({"chat_pilot_channel_ids": "", "chat_pilot_category_ids": ""}, False),
        (
            {
                "chat_pilot_role_id": None,
                "chat_pilot_channel_ids": "",
                "chat_pilot_category_ids": "",
            },
            False,
        ),
    ],
)
def test_a_role_and_somewhere_to_listen_are_both_needed(overrides, configured):
    assert chat_settings(**overrides).chat_pilot_configured is configured


def test_the_category_list_parses_like_the_extractors():
    """Same comma/semicolon tolerance as `CHAT_CATEGORY_IDS`, via the same helper."""
    assert chat_settings(chat_pilot_category_ids="1,2;3 , 4").chat_pilot_category_id_list == [
        1,
        2,
        3,
        4,
    ]
    assert chat_settings(chat_pilot_category_ids="").chat_pilot_category_id_list == []


def test_the_defaults_are_the_documented_ones():
    settings = chat_settings()
    assert settings.chat_pilot_rate_count == 4
    assert settings.chat_pilot_rate_window_s == 300.0
    assert settings.chat_pilot_global_rate_count == 12
    assert settings.chat_pilot_global_rate_window_s == 900.0
    assert settings.chat_pilot_lock_wait_s == 2.0
    assert settings.chat_pilot_model == "gpt-oss:20b"
    assert settings.chat_pilot_timeout == 60.0
    assert settings.persona_path == "config/personas/identities/example.md"
    assert Settings.model_fields["persona_path"].default == "config/personas/identities/persona.md"


def test_the_capacity_settings_are_read_from_the_environment():
    """The two knobs an operator reaches for when the host is struggling."""
    settings = chat_settings(
        chat_pilot_global_rate_count="30",
        chat_pilot_global_rate_window_s="3600",
        chat_pilot_lock_wait_s="0.5",
    )
    assert settings.chat_pilot_global_rate_count == 30
    assert settings.chat_pilot_global_rate_window_s == 3600.0
    assert settings.chat_pilot_lock_wait_s == 0.5


def test_the_env_example_documents_every_new_setting():
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    for key in (
        "CHAT_PILOT_ROLE_ID",
        "CHAT_ROLE_PLUGINS",
        "CHAT_PILOT_CHANNEL_IDS",
        "CHAT_PILOT_CATEGORY_IDS",
        "CHAT_PILOT_RATE_COUNT",
        "CHAT_PILOT_RATE_WINDOW_S",
        "CHAT_PILOT_GLOBAL_RATE_COUNT",
        "CHAT_PILOT_GLOBAL_RATE_WINDOW_S",
        "CHAT_PILOT_LOCK_WAIT_S",
        "CHAT_PILOT_MODEL",
        "CHAT_PILOT_TIMEOUT",
        "PERSONA_PATH",
    ):
        assert f"\n{key}=" in text


def test_the_env_example_carries_no_real_ids():
    """Placeholders only: the real ids live in the git-ignored `.env`."""
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    for key in (
        "CHAT_PILOT_ROLE_ID",
        "CHAT_ROLE_PLUGINS",
        "CHAT_PILOT_CHANNEL_IDS",
        "CHAT_PILOT_CATEGORY_IDS",
    ):
        line = next(ln for ln in text.splitlines() if ln.startswith(f"{key}="))
        assert line == f"{key}="


# ---------------------------------------------------------------------------
# the capacity numbers, as runtime settings
# ---------------------------------------------------------------------------


def test_the_capacity_numbers_seed_from_the_environment(chat_bot):
    """No stored row yet, so `.env` is what the bot is running on."""
    assert chat_bot.chat_rate_count == chat_bot.settings.chat_pilot_rate_count
    assert chat_bot.chat_rate_window_s == chat_bot.settings.chat_pilot_rate_window_s
    assert chat_bot.chat_pool_count == chat_bot.settings.chat_pilot_global_rate_count
    assert chat_bot.chat_pool_window_s == chat_bot.settings.chat_pilot_global_rate_window_s


def test_the_numbers_are_seeded_on_first_run_and_not_re_seeded_after(tmp_path):
    """`.env` fills the rows once; a value tuned at 9pm survives the next restart."""
    from bot import __main__ as entrypoint

    settings = chat_settings(db_path=str(tmp_path / "seed.sqlite"))
    repo = entrypoint.build_repo(settings)
    assert repo.get_config("chat_pilot_rate_count") == str(settings.chat_pilot_rate_count)
    assert repo.get_config("chat_pilot_global_rate_window_s") == str(
        settings.chat_pilot_global_rate_window_s
    )

    repo.set_config("chat_pilot_rate_count", "9")
    repo.close()
    restarted = entrypoint.build_repo(settings)

    assert restarted.get_config("chat_pilot_rate_count") == "9"
    restarted.close()


def test_role_plugins_are_seeded_once_from_the_environment(tmp_path):
    from bot import __main__ as entrypoint

    settings = chat_settings(
        db_path=str(tmp_path / "plugins.sqlite"),
        chat_role_plugins=f"{CHAT_ROLE}=mesugaki",
    )
    repo = entrypoint.build_repo(settings)
    assert behaviour_plugins.decode(repo.get_config(behaviour_plugins.CONFIG_KEY)) == [
        behaviour_plugins.RolePlugin(str(CHAT_ROLE), "mesugaki")
    ]

    repo.set_config(behaviour_plugins.CONFIG_KEY, "[]")
    repo.close()
    restarted = entrypoint.build_repo(settings)
    assert restarted.get_config(behaviour_plugins.CONFIG_KEY) == "[]"
    restarted.close()


def test_a_stored_number_wins_over_the_environment(chat_bot):
    chat_bot.repo.set_config("chat_pilot_rate_count", "9")
    chat_bot.repo.set_config("chat_pilot_global_rate_window_s", "1800")
    assert chat_bot.chat_rate_count == 9
    assert chat_bot.chat_pool_window_s == 1800.0


def test_a_nonsense_row_falls_back_rather_than_taking_the_bot_down(chat_bot):
    """Read on the hot path of every message; a hand-edited row must not raise."""
    for stored in ("", "lots", "0", "-3"):
        chat_bot.repo.set_config("chat_pilot_rate_count", stored)
        assert chat_bot.chat_rate_count == chat_bot.settings.chat_pilot_rate_count


def test_saving_a_number_moves_the_live_limiters_at_once(chat_bot):
    """The limiters outlive the request, so a new value means nothing until told."""
    pilot = chat_bot.chat
    assert pilot.limiter.count == 4

    service.set_config(chat_bot, "chat_pilot_rate_count", 7)
    service.set_config(chat_bot, "chat_pilot_global_rate_window_s", 60)

    assert pilot.limiter.count == 7
    assert pilot.global_limiter.window == 60.0


def test_the_capacity_numbers_are_validated_like_every_other_setting(chat_bot):
    from bot.api.errors import BadRequest

    for key, bad in (
        ("chat_pilot_rate_count", "none"),
        ("chat_pilot_rate_count", 0),
        ("chat_pilot_global_rate_window_s", "soon"),
        ("chat_pilot_global_rate_window_s", 0),
    ):
        with pytest.raises(BadRequest):
            service.set_config(chat_bot, key, bad)


def test_the_numbers_are_reported_by_get_config(chat_bot):
    chat_bot.repo.set_config("chat_pilot_rate_count", "6")
    values = service.get_config(chat_bot)
    assert values["chat_pilot_rate_count"] == 6
    assert values["chat_pilot_global_rate_count"] == 12


# ---------------------------------------------------------------------------
# the runtime flag
# ---------------------------------------------------------------------------


def test_chat_mode_defaults_to_on_when_configured_and_off_when_not(repo, bosses):
    from .chat_support import build_bot

    assert build_bot(repo, bosses).chat_mode is True
    assert (
        build_bot(repo, bosses, chat_pilot_channel_ids="", chat_pilot_category_ids="").chat_mode
        is False
    )


def test_chat_mode_is_writable_at_runtime(chat_bot):
    assert "chat_mode" in service.CONFIG_KEYS
    assert service.set_config(chat_bot, "chat_mode", "off")["chat_mode"] is False
    assert chat_bot.chat_mode is False
    assert service.set_config(chat_bot, "chat_mode", "on")["chat_mode"] is True


def test_get_config_reports_how_the_pilot_is_set_up(chat_bot):
    values = service.get_config(chat_bot)
    assert values["chat_mode"] is True
    assert values["chat_configured"] is True
    assert values["chat_channels"] == [str(CHAT_CHANNEL)]
    assert values["chat_categories"] == [str(CHAT_CATEGORY)]
    assert values["chat_model"] == "gpt-oss:20b"


def test_the_chat_role_id_is_never_exposed_by_the_api(chat_bot):
    """The channel list is already public in the portal; the role gate is not."""
    assert str(CHAT_ROLE) not in str(service.get_config(chat_bot))


def test_the_api_and_the_portal_can_toggle_it(auth, fake_bot):
    assert auth.put("/api/config", json={"chat_mode": True}).json()["chat_mode"] is True
    assert fake_bot.chat_mode is True
    assert auth.put("/api/config", json={"chat_mode": False}).json()["chat_mode"] is False
    assert fake_bot.chat_mode is False

    response = auth.post("/config", data={"chat_mode": "1"}, follow_redirects=False)
    assert response.status_code == 303
    assert fake_bot.chat_mode is True


def test_the_config_page_offers_the_toggle(auth):
    page = auth.get("/config").text
    assert 'name="chat_mode"' in page
    assert "chatbot" in page.lower()


def test_bossctl_sends_chat_mode_as_a_bool():
    """Otherwise `bossctl config set chat_mode on` would send the string "on"."""
    import inspect

    from bot import cli

    source = inspect.getsource(cli.config_set)
    assert '"chat_mode"' in source


def test_the_flag_is_seeded_on_first_run():
    import inspect

    from bot import __main__ as entrypoint

    source = inspect.getsource(entrypoint.build_repo)
    assert "CFG_CHAT" in source
    assert CFG_CHAT == "chat_mode"


def test_debug_status_names_the_chatbot_without_printing_its_ids(chat_bot):
    from bot.agent.debug import _chat_state

    line = _chat_state(chat_bot)
    assert "on" in line
    assert "gpt-oss:20b" in line
    assert "1 channel(s), 1 categor" in line
    for secret in (str(CHAT_ROLE), str(CHAT_CHANNEL), str(CHAT_CATEGORY)):
        assert secret not in line


def test_debug_status_says_when_it_is_not_configured(repo, bosses):
    from bot.agent.debug import _chat_state

    from .chat_support import build_bot

    assert "not configured" in _chat_state(build_bot(repo, bosses, chat_pilot_role_id=None))


# ---------------------------------------------------------------------------
# on_message: two independent offers
# ---------------------------------------------------------------------------


class Recorder:
    def __init__(self):
        self.seen = []

    async def offer(self, msg):
        self.seen.append(msg)
        return None


@pytest.fixture
def wired(repo: Repo):
    """A real client with only what `on_message` reaches for."""
    client = BossBot.__new__(BossBot)
    client.repo = repo
    client.settings = chat_settings()
    client.extractor = Recorder()
    client.chat = Recorder()
    return client


def deliver(client, msg):
    asyncio.run(BossBot.on_message(client, msg))


def test_a_chat_channel_message_reaches_chat_and_is_not_stored(wired, chat_bot):
    msg = message(chat_bot, "@bot what's on?", channel_id=CHAT_CHANNEL)
    deliver(wired, msg)
    assert wired.chat.seen == [msg]
    # The chat channel is not watched, so nothing is recorded and the extractor
    # never sees it: talking to the bot must not become schedule proposals.
    assert wired.extractor.seen == []
    assert wired.repo.get_message(msg.id) is None


def test_a_watched_channel_message_reaches_both(wired, chat_bot):
    msg = message(chat_bot, "can wed?", channel_id=WATCHED_CHANNEL, mentions=())
    msg.created_at = datetime.now(UTC)
    deliver(wired, msg)
    assert wired.extractor.seen == [msg]
    assert wired.chat.seen == [msg]
    assert wired.repo.get_message(msg.id) is not None


def test_an_unwatched_non_chat_channel_still_reaches_chat_and_is_dropped_there(wired, chat_bot):
    """The gate, not `on_message`, is what refuses it -- one place to reason about."""
    msg = message(chat_bot, "hello", channel_id=UNWATCHED_CHANNEL)
    deliver(wired, msg)
    assert wired.extractor.seen == []
    assert wired.chat.seen == [msg]


def test_a_broken_extractor_does_not_stop_the_chatbot(wired, chat_bot):
    class Broken:
        async def offer(self, _msg):
            raise RuntimeError("the model exploded")

    wired.extractor = Broken()
    msg = message(chat_bot, "can wed?", channel_id=WATCHED_CHANNEL, mentions=())
    msg.created_at = datetime.now(UTC)
    deliver(wired, msg)
    assert wired.chat.seen == [msg]


def test_a_broken_chatbot_does_not_take_the_bot_down(wired, chat_bot):
    class Broken:
        async def offer(self, _msg):
            raise RuntimeError("the model exploded")

    wired.chat = Broken()
    deliver(wired, message(chat_bot, "@bot hi", channel_id=CHAT_CHANNEL))  # must not raise


def test_bots_and_dms_never_reach_either_offer(wired, chat_bot):
    for msg in (
        message(chat_bot, "hi", channel_id=CHAT_CHANNEL, is_bot=True),
        message(chat_bot, "hi", channel_id=CHAT_CHANNEL),
    ):
        if not msg.author.bot:
            msg.guild = None
        deliver(wired, msg)
    assert wired.chat.seen == []
    assert wired.extractor.seen == []
