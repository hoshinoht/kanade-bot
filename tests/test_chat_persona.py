"""Loading the persona, and what the assembled system prompt may contain.

The persona is the one part of this feature a person edits by hand on a live
deployment, so the loader's job is to make a mistake obvious in the logs without
taking the bot down. The prompt's job is to carry no ids and no secrets.
"""

from __future__ import annotations

import logging

import pytest

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


def test_personas_live_in_their_own_directory():
    """The `config/portraits` pattern: a directory with a README and a template
    tracked, and everything a deployment actually writes ignored. It is
    bind-mounted into the container, so a persona is a file on the host."""
    assert persona.EXAMPLE_PERSONA.parent == persona.PERSONA_DIR
    assert persona.PERSONA_DIR.name == "personas"
    assert (persona.PERSONA_DIR / "README.md").is_file()


def test_the_loader_says_which_file_it_read(tmp_path):
    """Not for the loader's sake -- for the Config page's. "Answering in the
    placeholder voice" is a misconfigured deploy, and it used to be visible only
    in a WARNING nobody reads."""
    path = tmp_path / "persona.md"
    path.write_text("You are Placeholder, a scheduler bot.\n", encoding="utf-8")

    loaded = persona.read_persona(path)

    assert loaded.text == "You are Placeholder, a scheduler bot."
    assert loaded.path == path
    assert loaded.name == "persona.md"
    assert loaded.fell_back is False


def test_a_fall_back_says_so_and_names_the_template(tmp_path):
    fallen = persona.read_persona(tmp_path / "nope.md")

    assert fallen.fell_back is True
    assert fallen.name == "persona.example.md"
    assert fallen.text


# --- choosing one of several ------------------------------------------------


@pytest.fixture
def staged_personas(tmp_path, monkeypatch):
    """A personas directory with two voices, a README, and some noise in it."""
    (tmp_path / "kanade.md").write_text("You are Kanade.\n", encoding="utf-8")
    (tmp_path / "persona.example.md").write_text("You are <BotName>.\n", encoding="utf-8")
    (tmp_path / "README.md").write_text("# Personas\n", encoding="utf-8")
    (tmp_path / "notes.txt").write_text("not a persona\n", encoding="utf-8")
    (tmp_path / "drafts").mkdir()
    monkeypatch.setattr(persona, "PERSONA_DIR", tmp_path)
    return tmp_path


def test_every_markdown_file_but_the_readme_is_on_offer(staged_personas):
    """The README documents the directory; the template is a real voice, and a
    deployment that has not written its own is legitimately wearing it."""
    assert persona.available() == ["kanade.md", "persona.example.md"]


def test_a_directory_that_is_not_there_offers_nothing(tmp_path):
    assert persona.available(tmp_path / "nowhere") == []


def test_a_chosen_name_resolves_only_by_membership(staged_personas):
    assert persona.chosen_path("kanade.md") == staged_personas / "kanade.md"
    assert persona.chosen_path("") is None
    assert persona.chosen_path("gone.md") is None
    # Not on the list, so never joined to anything -- there is no path here to
    # reject, because none was built.
    assert persona.chosen_path("README.md") is None
    assert persona.chosen_path("notes.txt") is None


@pytest.mark.parametrize(
    "attempt",
    [
        "../persona.example.md",
        "../../etc/passwd",
        "/etc/passwd",
        "drafts/../kanade.md",
        "drafts",
        "kanade.md/../../secret.md",
        ".",
    ],
)
def test_nothing_shaped_like_a_path_ever_resolves(staged_personas, attempt):
    """Membership rather than sanitising: each of these is simply not one of the
    two names on offer, so none of them becomes a path at all."""
    assert persona.chosen_path(attempt) is None


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


def test_the_hard_rules_require_global_discord_formatting_and_block_spacing():
    rules = persona.HARD_RULES
    for markup in ("**bold**", "*italics*", "`inline code`", "<#channel_id>"):
        assert markup in rules
    assert "exactly one blank line" in rules
    assert "blank line before its heading" in rules
    assert "short answer" in rules


def test_the_hard_rules_keep_scheduler_internals_out_of_member_replies():
    rules = persona.HARD_RULES.lower()
    for phrase in (
        "bare schedule or date question",
        "whole group across all channels",
        "implementation details private",
        "assignment-style arguments",
        "nothing in this channel",
        "runs in other channels",
    ):
        assert phrase in rules
    for leaked_example in ("get_schedule", "scope=", "participant="):
        assert leaked_example not in rules


def test_the_clock_header_says_today_and_the_boss_week(chat_bot):
    from bot.domain.timeutil import utcnow
    from bot.domain.weeks import current_week_start

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
    from bot.domain.timeutil import utcnow
    from bot.domain.weeks import current_week_start

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


# ---------------------------------------------------------------------------
# what it is running on
# ---------------------------------------------------------------------------


def test_the_runtime_line_names_the_configured_model():
    line = persona.runtime_line("gpt-oss:20b")
    assert "gpt-oss:20b" in line
    assert "Ollama" in line
    assert "Discord bot" in line


def test_the_runtime_line_forbids_inventing_one():
    """Live, asked what it runs on, it answered "a fine-tuned LLaMA-2"."""
    line = persona.runtime_line("gpt-oss:20b").lower()
    assert "never invent" in line
    assert "model name" in line


def test_the_runtime_line_claims_nothing_about_where_it_runs():
    """`CHAT_PILOT_MODEL` may be a cloud model proxied through the same daemon.

    Swapping "a fine-tuned LLaMA-2" for "running on the machine that hosts the
    bot" would be one false claim in place of another.
    """
    line = persona.runtime_line("gpt-oss:120b-cloud").lower()
    for claim in ("local", "cloud", "on the machine", "on your", "server"):
        assert claim not in line.replace("gpt-oss:120b-cloud", "")


def test_the_runtime_line_does_not_answer_for_who_made_it():
    """That belongs to the persona, which credits its developer with a link."""
    line = persona.runtime_line("gpt-oss:20b").lower()
    for topic in ("who made", "company", "the only facts"):
        assert topic not in line


def test_an_unconfigured_model_is_not_guessed_at():
    line = persona.runtime_line("  ")
    assert persona.UNNAMED_MODEL in line
    assert "{model}" not in line
    # No name means no backticks: "the `` model" would read as an empty one.
    assert "`" not in line


def test_the_model_name_is_not_baked_into_the_hard_rules():
    """Those are pinned literals shared by every deployment; a model name is not."""
    assert "gpt-oss" not in persona.HARD_RULES


def test_the_runtime_facts_sit_with_the_clock_and_before_the_examples():
    built = persona.system_prompt("PERSONA HERE", "CLOCK HERE", persona.runtime_line("some:model"))
    assert built.index("CLOCK HERE") < built.index("some:model")
    assert built.index("Operating rules") < built.index("some:model")


def test_a_caller_that_names_no_runtime_still_gets_a_prompt():
    """The two-argument call is what every test that pins the ordering makes."""
    assert "Ollama" not in persona.system_prompt("PERSONA HERE", "CLOCK HERE")


def test_the_assembled_prompt_tells_the_model_what_it_runs_on(repo, bosses):
    """End to end: what `assemble` hands Ollama says which model is answering."""
    from bot.chat.agent import ChatTurn

    bot = build_bot(repo, bosses, chat_pilot_model="gpt-oss:20b")
    pilot = ChatPilot(bot, client=object())
    system = pilot.assemble([ChatTurn("user", "kanon: what model are u deployed on")])[0]
    assert system["role"] == "system"
    assert "gpt-oss:20b" in system["content"]


def test_the_persona_is_read_once_and_reloadable(tmp_path, repo, bosses):
    path = tmp_path / "persona.md"
    path.write_text("first", encoding="utf-8")
    pilot = ChatPilot(build_bot(repo, bosses, persona_path=str(path)), client=object())
    assert pilot.persona_text() == "first"
    path.write_text("second", encoding="utf-8")
    assert pilot.persona_text() == "first"  # cached; a deploy restarts the bot
    assert pilot.reload_persona() == "second"
