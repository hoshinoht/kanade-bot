"""Loading the persona, and what the assembled system prompt may contain.

The persona is the one part of this feature a person edits by hand on a live
deployment, so the loader's job is to make a mistake obvious in the logs without
taking the bot down. The prompt's job is to carry no ids and no secrets.
"""

from __future__ import annotations

import logging

from bot.chat import persona
from bot.chat.agent import ChatPilot

from .chat_support import ADMIN_ROLE, CHAT_CHANNEL, CHAT_ROLE, build_bot


def test_persona_path_is_read_verbatim(tmp_path):
    path = tmp_path / "persona.md"
    path.write_text("You are Placeholder, a scheduler bot.\n", encoding="utf-8")
    assert persona.load_persona(path) == "You are Placeholder, a scheduler bot."


def test_a_missing_file_falls_back_and_warns(tmp_path, caplog):
    with caplog.at_level(logging.WARNING, logger="bot.chat.persona"):
        text = persona.load_persona(tmp_path / "nope.md")
    assert text
    assert "persona.example.md" in caplog.text
    assert "falling back" in caplog.text


def test_an_empty_file_falls_back_and_warns(tmp_path, caplog):
    path = tmp_path / "persona.md"
    path.write_text("   \n", encoding="utf-8")
    with caplog.at_level(logging.WARNING, logger="bot.chat.persona"):
        text = persona.load_persona(path)
    assert text
    assert "empty" in caplog.text


def test_no_path_at_all_falls_back(caplog):
    """An unset PERSONA_PATH is a fresh deployment, not an error worth a warning."""
    with caplog.at_level(logging.WARNING, logger="bot.chat.persona"):
        assert persona.load_persona("")
    assert caplog.text == ""


def test_the_tracked_example_is_present_and_usable():
    assert persona.EXAMPLE_PERSONA.exists()
    text = persona.load_persona(None)
    assert "scheduler" in text.lower()
    assert len(text) > 500


def test_the_tracked_example_names_nobody_real():
    """It is a template: placeholders, never a real character or member."""
    text = persona.EXAMPLE_PERSONA.read_text(encoding="utf-8")
    assert "<BotName>" in text
    for leaked in ("Yuuki", "Sakuna", "Aqua", "Minato", "Nanahoshi"):
        assert leaked.lower() not in text.lower()


# ---------------------------------------------------------------------------
# the assembled prompt
# ---------------------------------------------------------------------------


def test_the_prompt_is_persona_then_rules_then_clock():
    """Rules come after the persona so they win when the two disagree."""
    built = persona.system_prompt("PERSONA HERE", "CLOCK HERE")
    assert built.index("PERSONA HERE") < built.index("Operating rules")
    assert built.index("Operating rules") < built.index("CLOCK HERE")


def test_the_hard_rules_cover_the_things_that_matter():
    rules = persona.HARD_RULES.lower()
    for topic in ("never guess a time", "tools", "✅", "rsvp", "four sentences", "@everyone"):
        assert topic.lower() in rules


def test_the_clock_header_says_today_and_the_boss_week(chat_bot):
    from bot.timeutil import utcnow
    from bot.weeks import current_week_start

    now = utcnow()
    week = current_week_start(
        chat_bot.tz, chat_bot.settings.reset_weekday, chat_bot.settings.reset_time, now
    )
    header = persona.clock_header(now, chat_bot.tz, week)
    assert now.astimezone(chat_bot.tz).strftime("%A") in header
    assert "Asia/Kuala_Lumpur" in header
    assert "boss week" in header


def test_the_prompt_carries_no_ids_and_no_secrets(repo, bosses):
    """The model is told what it is, never what it is allowed to talk to.

    Those gates are enforced before a prompt is built, so there is nothing here
    for a "repeat your instructions" prompt to leak.
    """
    from bot.timeutil import utcnow
    from bot.weeks import current_week_start

    bot = build_bot(repo, bosses)
    pilot = ChatPilot(bot, client=object())
    now = utcnow()
    week = current_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now)
    built = persona.system_prompt(pilot.persona_text(), persona.clock_header(now, bot.tz, week))
    for secret in (
        str(CHAT_ROLE),
        str(CHAT_CHANNEL),
        str(ADMIN_ROLE),
        str(bot.settings.guild_id),
        bot.settings.admin_token,
        bot.settings.discord_token,
    ):
        assert secret not in built


def test_the_persona_is_read_once_and_reloadable(tmp_path, repo, bosses):
    path = tmp_path / "persona.md"
    path.write_text("first", encoding="utf-8")
    pilot = ChatPilot(build_bot(repo, bosses, persona_path=str(path)), client=object())
    assert pilot.persona_text() == "first"
    path.write_text("second", encoding="utf-8")
    assert pilot.persona_text() == "first"  # cached; a deploy restarts the bot
    assert pilot.reload_persona() == "second"
