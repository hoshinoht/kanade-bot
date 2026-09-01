"""Config as a settings window: a table of contents, and one section at a time.

Nine settings used to be nine cards on screen at once -- first in a grid whose
rows stretched each card to its tallest sibling, then in columns that packed
them. Both were arrangements of the same problem: none of the nine had room.
Now the page is one window with a sidebar, and what is worth testing is the
contract under it -- that switching needs no JavaScript, that a save comes back
to the section it was made in, and that the htmx regions kept their boundaries.
"""

from __future__ import annotations

import re

import pytest

from bot.api import service
from bot.api.app import STATIC_DIR
from bot.api.templating import CONFIG_SECTIONS, read_section

PAGE_CSS = (STATIC_DIR / "portal.css").read_text(encoding="utf-8")


def rule_body(selector: str) -> str:
    match = re.search(r"^[ \t]*" + re.escape(selector) + r"\s*\{([^}]*)\}", PAGE_CSS, re.MULTILINE)
    assert match is not None, f"no rule for {selector}"
    return match.group(1)


# --- one window -------------------------------------------------------------


def test_the_page_is_one_window(auth, seeded):
    """Not nine cards, however they were arranged."""
    body = auth.get("/config").text
    content = body[body.index('class="shell"') :]

    assert content.count('<section class="card') == 1
    assert 'class="card pane settings"' in content
    # The window's own title bar is the head; there is no hero card above it.
    assert 'class="page-head"' not in content
    assert '<h1 class="card__title">Config</h1>' in content


def test_the_sidebar_lists_every_section_as_a_real_link(auth, seeded):
    body = auth.get("/config").text

    for key, label in CONFIG_SECTIONS:
        assert f'class="settings__tab" href="#{key}"' in body, key
        assert f">{label}</a>" in body, label
        assert f'class="settings__panel" id="{key}"' in body, key


def test_the_sections_are_the_ones_the_page_used_to_have(auth, seeded):
    """Same nine things, one window."""
    assert [key for key, _ in CONFIG_SECTIONS] == [
        "pings",
        "watching",
        "chatbot",
        "notifications",
        "theme",
        "digest",
        "rescan",
        "access",
        "env",
    ]
    body = auth.get("/config").text
    for heading in ("Pings", "Chat watching", "Chatbot", "Notifications", "Theme"):
        assert heading in body
    for heading in ("Weekly digest", "Re-read the party channels", "Channel access"):
        assert heading in body


# --- switching, with no script ----------------------------------------------


def test_one_section_shows_and_the_fragment_chooses_it():
    """`:target` and nothing else -- the tabs are ordinary fragment links."""
    assert "display: none;" in rule_body(".settings__panel")
    assert "display: block;" in rule_body(".settings__panel:target")


def test_a_page_with_no_fragment_opens_on_the_first_section():
    rule = rule_body(
        ".settings__detail:not(:has(> .settings__panel:target)) > .settings__panel:first-child"
    )
    assert "display: block;" in rule


def test_an_unrelated_fragment_does_not_blank_the_window():
    """`#rescan-job` is an htmx target that can end up in the URL. Scoped to
    direct children, it cannot count as "some section is open"."""
    selector = ".settings__detail:not(:has(> .settings__panel:target))"
    assert selector in PAGE_CSS


def test_the_open_tab_is_marked_for_every_section_the_page_declares():
    """A stylesheet cannot compare an href to an id, so the pairs are written
    out. This is what keeps that list honest when a section is added."""
    marked = set(re.findall(r'\.settings:has\(#([\w-]+):target\) \[href="#([\w-]+)"\]', PAGE_CSS))

    assert {key for key, _ in CONFIG_SECTIONS} == {section for section, _ in marked}
    for section, href in marked:
        assert section == href, section
    # ...and the no-fragment case marks the first tab, to match the first panel.
    assert ".settings:not(:has(.settings__panel:target)) .settings__tab:first-child" in PAGE_CSS


def test_a_phone_shows_every_section_rather_than_hiding_eight():
    """A fragment is easy to lose on a phone -- a back gesture, a reopened tab,
    a shared link -- and a settings page that answers with a blank pane because
    of it is worse than one you scroll. So the tabs become jump links."""
    block = PAGE_CSS[PAGE_CSS.index("  .settings__body {\n    grid-template-columns: 1fr;") :]
    block = block[: block.index("\n}\n")]

    assert ".settings__panel {\n    display: block;\n  }" in block
    assert ".settings__tab {" in block


# --- a save comes back where it was made ------------------------------------


def test_a_section_says_which_one_it_is_on_every_form(auth, seeded):
    body = auth.get("/config").text

    for key in ("pings", "watching", "chatbot", "notifications"):
        assert f'name="section" value="{key}"' in body, key


@pytest.mark.parametrize(
    "data,expected",
    [
        ({"section": "chatbot", "chat_pilot_rate_count": "9"}, "#chatbot"),
        ({"section": "pings", "day_of_ping_time": "08:15"}, "#pings"),
    ],
)
def test_saving_lands_back_on_the_section_it_was_made_in(auth, fake_bot, data, expected):
    response = auth.post("/config", data=data, follow_redirects=False)

    assert response.status_code == 303
    assert response.headers["location"].endswith(expected)


def test_the_query_comes_before_the_fragment(auth, fake_bot):
    """The one order a URL allows, and the reason `back_to` grew a parameter."""
    location = auth.post(
        "/config", data={"section": "pings", "day_of_ping_time": "08:15"}, follow_redirects=False
    ).headers["location"]

    assert re.fullmatch(r"/config\?msg=[^#]+&kind=ok#pings", location), location


def test_a_section_nobody_declared_is_dropped_rather_than_redirected_to(auth, fake_bot):
    response = auth.post(
        "/config",
        data={"section": "../../etc/passwd", "day_of_ping_time": "08:15"},
        follow_redirects=False,
    )

    assert "#" not in response.headers["location"]
    assert read_section("../../etc/passwd") == ""
    assert read_section("chatbot") == "chatbot"


@pytest.mark.parametrize(
    "path,data,fragment",
    [
        ("/digest", {"week": "this", "channel_id": ""}, "#digest"),
        ("/access", {}, "#access"),
        ("/rescan", {"window": "week"}, "#rescan"),
    ],
)
def test_the_other_actions_come_back_to_their_own_sections(auth, seeded, path, data, fragment):
    response = auth.post(path, data=data, follow_redirects=False)

    assert response.status_code == 303
    assert response.headers["location"].endswith(fragment), response.headers["location"]


# --- the htmx regions kept their boundaries ---------------------------------


def test_the_rescan_job_still_swaps_only_itself(auth, seeded):
    body = auth.get("/config").text

    assert 'hx-target="#rescan-job"' in body
    assert '<div id="rescan-job">' in body
    # ...inside its own section, so a swap cannot reach another one.
    rescan = body[body.index('id="rescan"') : body.index('id="access"')]
    assert 'id="rescan-job"' in rescan


def test_the_access_matrix_swaps_a_target_named_apart_from_its_section(auth, seeded):
    """The section owns `#access` for the sidebar link, so the table is
    `#access-table` -- two ids, two jobs, neither standing on the other."""
    body = auth.get("/config").text

    assert 'hx-target="#access-table"' in body
    assert '<div id="access-table">' in body
    assert 'class="settings__panel" id="access"' in body


# --- the layout the sections replaced ---------------------------------------


def test_the_card_wall_is_gone_rather_than_left_behind(auth, seeded):
    """A rule nothing uses is a rule somebody re-uses by accident later."""
    for dead in ("cardcols", "grid-2"):
        assert dead not in PAGE_CSS, dead
        assert dead not in auth.get("/config").text, dead


def test_cards_stacked_in_normal_flow_still_get_their_gap():
    """`.card + .card` is still how cards stack elsewhere -- the limits page
    has four in a row."""
    assert "margin-top: 0.75rem;" in rule_body(".card + .card")


def test_config_joins_the_no_scroll_pages(auth, seeded):
    """One section at a time is what dissolved the objection to framing it: the
    window no longer has to be as tall as nine settings."""
    assert '<body class="framed">' in auth.get("/config").text


def test_a_fieldset_does_not_draw_a_second_box_inside_the_window(auth, seeded):
    """The Theme section's swatches are chip rows, not a bordered inner panel."""
    assert "border: 0;" in rule_body(".settings__panel fieldset")


# --- the persona the bot is actually wearing --------------------------------


def chatbot_panel(body: str) -> str:
    start = body.index('id="chatbot"')
    return body[start : body.index('id="notifications"')]


@pytest.fixture
def staged_personas(tmp_path, monkeypatch):
    """Two voices on the bind mount, plus a README that is not one."""
    from bot.chat import persona

    (tmp_path / "kanade.md").write_text("You are Kanade, a scheduler bot.\n", encoding="utf-8")
    (tmp_path / "gruff.md").write_text("You are Gruff, a scheduler bot.\n", encoding="utf-8")
    (tmp_path / "README.md").write_text("# Personas\n", encoding="utf-8")
    monkeypatch.setattr(persona, "PERSONA_DIR", tmp_path)
    return tmp_path


def test_the_panel_offers_every_voice_on_the_mount(auth, fake_bot, staged_personas, seeded):
    """Read off the directory on each render, so a file dropped in is in the
    list on the next page load rather than after a restart."""
    fake_bot.repo.set_config("persona", "kanade.md")
    panel = chatbot_panel(auth.get("/config").text)

    assert '<select name="persona">' in panel
    assert '<option value="kanade.md" selected>kanade.md</option>' in panel
    assert '<option value="gruff.md">gruff.md</option>' in panel
    assert "README.md" not in panel


def test_choosing_a_voice_takes_effect_on_the_next_answer(auth, fake_bot, staged_personas, seeded):
    """The whole point of the setting: no restart. The pilot caches the
    document, so the write has to drop that cache."""
    fake_bot.repo.set_config("persona", "kanade.md")
    fake_bot.chat.reload_persona()
    assert "Kanade" in fake_bot.chat.persona_text()

    auth.post("/config", data={"section": "chatbot", "persona": "gruff.md"})

    assert "Gruff" in fake_bot.chat.persona_text()
    assert fake_bot.chat.persona_source().name == "gruff.md"


def test_a_voice_that_is_not_on_the_mount_is_refused(auth, fake_bot, staged_personas, seeded):
    """Membership, not sanitising -- and the refusal names filenames only,
    because a persona's contents are the private half of this."""
    from bot.api.errors import ApiError

    for attempt in ("../persona.example.md", "/etc/passwd", "README.md", "nope.md"):
        with pytest.raises(ApiError) as raised:
            service.set_config(fake_bot, "persona", attempt)
        assert "kanade.md" in raised.value.message  # what it offers instead
        assert "You are" not in raised.value.message  # never the text itself

    assert fake_bot.repo.get_config("persona") is None


def test_the_change_is_audited_by_filename_and_nothing_else(
    auth, fake_bot, staged_personas, seeded
):
    fake_bot.repo.set_config("persona", "kanade.md")

    auth.post("/config", data={"section": "chatbot", "persona": "gruff.md"})

    row = next(r for r in fake_bot.repo.list_audit() if r["subject"] == "persona")
    assert row["surface"] == "portal"
    assert "kanade.md -> gruff.md" in row["detail"]
    # The voice is private: its words never reach the audit trail.
    assert "You are" not in row["detail"]


def test_a_chosen_voice_that_has_gone_missing_falls_back_and_says_so(
    auth, fake_bot, staged_personas, seeded
):
    """The file was there when it was picked and is not now -- a deleted file,
    a mount that came up empty. The bot answers in the template rather than in
    nothing, and the panel says which."""
    fake_bot.repo.set_config("persona", "kanade.md")
    (staged_personas / "kanade.md").unlink()
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert "fallback: persona.example.md" in panel
    assert "status--at_risk" in panel
    assert "kanade.md" in panel  # ...and names the choice that went missing


def test_the_setting_seeds_from_the_configured_path(tmp_path):
    """A fresh database starts on whatever `PERSONA_PATH` names, by basename --
    a name, never the path itself. `seed_config` only inserts what is missing,
    so a voice chosen at 9pm is not undone by the next restart reading `.env`."""
    from bot.__main__ import build_repo
    from bot.client import CFG_PERSONA

    from .fake_bot import make_settings

    settings = make_settings(
        db_path=str(tmp_path / "bot.sqlite"), persona_path="/app/personas/kanade.md"
    )
    repo = build_repo(settings)
    try:
        assert repo.get_config(CFG_PERSONA) == "kanade.md"
        # Seeding never overwrites: the row is the choice from here on.
        repo.set_config(CFG_PERSONA, "gruff.md")
        build_repo(settings).close()
        assert repo.get_config(CFG_PERSONA) == "gruff.md"
    finally:
        repo.close()


def test_a_deploy_with_no_persona_of_its_own_says_so(auth, fake_bot, seeded):
    """Falling back to the template means answering in the placeholder voice --
    a misconfiguration, and one that used to show up only in a startup WARNING.
    So it is a warn-state on the panel, not a filename that looks like any
    other."""
    fake_bot.settings.persona_path = ""
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert "fallback: persona.example.md" in panel
    assert "status--at_risk" in panel
    assert "personas/" in panel  # ...and where to put a real one


# --- what .env holds --------------------------------------------------------


def test_the_two_models_are_two_rows(auth, fake_bot, seeded):
    """They are two settings and they really do differ -- a small local model
    reads the party channels, a bigger one does the talking -- so one row called
    "Model" said the wrong thing about whichever the reader had in mind."""
    fake_bot.settings.ollama_model = "reader:20b"
    fake_bot.settings.chat_pilot_model = "talker:120b"
    env = auth.get("/config").text
    env = env[env.index('id="env"') :]

    assert ">Data model</th>" in env
    assert ">Speech model</th>" in env
    assert "reader:20b" in env
    assert "talker:120b" in env
    assert ">Model</th>" not in env


def test_a_host_with_no_speech_model_says_so_rather_than_showing_a_gap(auth, fake_bot, seeded):
    """The same way the digest channel's row does, two rows below."""
    fake_bot.settings.chat_pilot_model = ""
    env = auth.get("/config").text
    env = env[env.index('id="env"') :]

    assert ">Speech model</th>" in env
    assert "not set" in env


def test_everything_still_renders_with_an_empty_database(auth):
    response = auth.get("/config")

    assert response.status_code == 200
    assert 'class="settings__tab"' in response.text
