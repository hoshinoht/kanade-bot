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

from bot import behaviour_plugins
from bot.api import service
from bot.api.templating import read_section

# --- one window -------------------------------------------------------------


# --- switching, with no script ----------------------------------------------


# --- a save comes back where it was made ------------------------------------


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


# --- the layout the sections replaced ---------------------------------------


# --- the persona the bot is actually wearing --------------------------------


def chatbot_panel(body: str) -> str:
    start = body.index('id="chatbot"')
    return body[start : body.index('id="notifications"')]


@pytest.fixture
def plugin_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    return tmp_path


def test_plugins_and_role_assignments_can_be_managed_in_the_portal(
    auth, fake_bot, seeded, plugin_dir
):
    role_id = "1540491480936751205"

    created = auth.post(
        "/config/behaviour-plugins",
        data={"name": "mesugaki", "instructions": "Use playful, smug banter."},
        follow_redirects=False,
    )
    assert created.status_code == 303
    assert created.headers["location"].endswith("#chatbot")
    assert behaviour_plugins.read("mesugaki").instructions == "Use playful, smug banter."

    assigned = auth.post(
        "/config/role-plugins",
        data={"role_id": role_id, "plugin": "mesugaki"},
        follow_redirects=False,
    )
    assert assigned.status_code == 303
    assert service.get_config(fake_bot)["chat_role_plugins"] == [
        {"role_id": role_id, "plugin": "mesugaki"}
    ]

    panel = chatbot_panel(auth.get("/config").text)
    assert role_id in panel
    assert "Use playful, smug banter." in panel
    assert "private" in panel
    assert f"/config/role-plugins/{role_id}/delete" in panel

    auth.post(
        "/config/behaviour-plugins",
        data={"name": "mesugaki", "instructions": "Use a different style."},
    )
    assert behaviour_plugins.read("mesugaki").instructions == "Use a different style."

    in_use = auth.post(
        "/config/behaviour-plugins/mesugaki/delete",
        follow_redirects=False,
    )
    assert "kind=error" in in_use.headers["location"]

    auth.post(f"/config/role-plugins/{role_id}/delete")
    deleted = auth.post(
        "/config/behaviour-plugins/mesugaki/delete",
        follow_redirects=False,
    )
    assert deleted.status_code == 303
    assert behaviour_plugins.read("mesugaki") is None


def test_publication_and_role_priority_are_portal_managed(auth, fake_bot, seeded, plugin_dir):
    behaviour_plugins.write("first", "FIRST")
    behaviour_plugins.write("second", "SECOND")
    first_role = "1540491480936751205"
    second_role = "1540491480936751206"
    service.set_role_plugin(fake_bot, first_role, "first")
    service.set_role_plugin(fake_bot, second_role, "second")

    auth.post(f"/config/role-plugins/{second_role}/move", data={"direction": "up"})
    assert [item["role_id"] for item in service.get_config(fake_bot)["chat_role_plugins"]] == [
        second_role,
        first_role,
    ]

    auth.post("/config/behaviour-plugins/first/selectable", data={"selectable": "1"})
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == ["first"]
    blocked = auth.post("/config/behaviour-plugins/first/delete", follow_redirects=False)
    assert "kind=error" in blocked.headers["location"]


def test_bulk_publish_and_private(auth, fake_bot, seeded, plugin_dir):
    behaviour_plugins.write("first", "FIRST")
    behaviour_plugins.write("second", "SECOND")
    behaviour_plugins.write("third", "THIRD")

    published = auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["first", "second"], "selectable": "1"},
        follow_redirects=False,
    )
    assert published.status_code == 303
    assert "Published+2+profiles" in published.headers["location"]
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == ["first", "second"]

    # Publishing again is a no-op rather than a duplicate entry.
    auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["first"], "selectable": "1"},
    )
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == ["first", "second"]

    made_private = auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["first"], "selectable": "0"},
        follow_redirects=False,
    )
    assert made_private.status_code == 303
    assert "Made+private+1+profile" in made_private.headers["location"]
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == ["second"]


def test_bulk_selectable_ignores_nothing_selected_and_unknown_names(
    auth, fake_bot, seeded, plugin_dir
):
    behaviour_plugins.write("first", "FIRST")

    empty = auth.post(
        "/config/behaviour-plugins/selectable",
        data={"selectable": "1"},
        follow_redirects=False,
    )
    assert empty.status_code == 303
    assert "No+profiles+selected" in empty.headers["location"]
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == []

    partial = auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["first", "ghost"], "selectable": "1"},
        follow_redirects=False,
    )
    assert partial.status_code == 303
    assert "Published+1+profile" in partial.headers["location"]
    assert service.get_config(fake_bot)["chat_selectable_plugins"] == ["first"]

    bad = auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["Not A Name!"], "selectable": "1"},
        follow_redirects=False,
    )
    assert "kind=error" in bad.headers["location"]


def test_broken_role_entries_are_visible_without_hiding_valid_ones(
    auth, fake_bot, seeded, plugin_dir
):
    behaviour_plugins.write("valid", "VALID")
    fake_bot.repo.set_config(
        behaviour_plugins.CONFIG_KEY,
        '[{"role_id":"bad","plugin":"ignored"},'
        '{"role_id":"1540491480936751205","plugin":"missing"},'
        '{"role_id":"1540491480936751206","plugin":"valid"}]',
    )

    panel = chatbot_panel(auth.get("/config").text)
    assert "1540491480936751206" in panel
    assert "Skipped configuration" in panel
    assert "digits only" in panel
    assert "profile `missing` is unreadable" in panel


@pytest.fixture
def staged_personas(tmp_path, monkeypatch):
    """Two selectable manifest bundles plus the tracked-style example fallback."""
    from bot.chat import persona
    from bot.chat.progress import STAGING_KEYS

    def _bundle(identifier: str, label: str, marker: str) -> None:
        directory = tmp_path / "personas" / identifier
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "identity.md").write_text(f"You are {marker}.\n", encoding="utf-8")
        (directory / "default.md").write_text(f"Default {marker}.\n", encoding="utf-8")
        (directory / "staging.yaml").write_text(
            "".join(f"{key}: {marker} {key}\n" for key in STAGING_KEYS), encoding="utf-8"
        )

    _bundle("kanade", "Kanade", "Kanade")
    _bundle("gruff", "Gruff", "Gruff")
    _bundle("kanade", "Kanade", "Kanade")
    (tmp_path / "personas.yaml").write_text(
        """schema_version: 1
default: kanade
personas:
  - id: kanade
    label: Kanade
    aliases: [kanade.md]
  - id: gruff
    label: Gruff
    aliases: [gruff.md]
""",
        encoding="utf-8",
    )
    (tmp_path / "README.md").write_text("# Personas\n", encoding="utf-8")
    monkeypatch.setattr(persona, "PERSONA_DIR", tmp_path)
    monkeypatch.setattr("bot.chat.persona_catalog.PERSONA_ROOT", tmp_path)
    return tmp_path


def _write_portal_bundle(root, identifier: str, label: str) -> None:
    from bot.chat.progress import STAGING_KEYS

    directory = root / "personas" / identifier
    directory.mkdir(parents=True)
    (directory / "identity.md").write_text(
        f"PRIVATE IDENTITY {label} — never render this prompt text", encoding="utf-8"
    )
    (directory / "default.md").write_text(
        f"PRIVATE BEHAVIOUR {label} — never render this prompt text", encoding="utf-8"
    )
    (directory / "staging.yaml").write_text(
        "".join(f"{key}: {label} staging\n" for key in STAGING_KEYS), encoding="utf-8"
    )


@pytest.fixture
def manifest_personas(tmp_path, monkeypatch):
    from bot.chat import persona

    _write_portal_bundle(tmp_path, "yuuki-sakuna", "Yuuki")
    _write_portal_bundle(tmp_path, "nazupi", "Nazupi")
    _write_portal_bundle(tmp_path, "kanade", "Kanade")
    (tmp_path / "personas.yaml").write_text(
        """schema_version: 1
default: yuuki-sakuna
personas:
  - id: yuuki-sakuna
    label: Yuuki Sakuna
    aliases: [persona.md]
  - id: nazupi
    label: Nazupi
    aliases: [nazupi.md]
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(persona, "PERSONA_DIR", tmp_path)
    return tmp_path


def test_manifest_personas_lead_with_human_labels(auth, fake_bot, manifest_personas, seeded):
    fake_bot.repo.set_config("persona", "nazupi")
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert '<option value="yuuki-sakuna">Yuuki Sakuna</option>' in panel
    assert '<option value="nazupi" selected>Nazupi</option>' in panel
    assert ">yuuki-sakuna</option>" not in panel
    assert "Changing Persona swaps identity, default behavior, and staging together." in panel
    assert "Legacy compatibility" not in panel


def test_manifest_persona_select_submits_and_marks_the_canonical_id(
    auth, fake_bot, manifest_personas, seeded
):
    fake_bot.repo.set_config("persona", "nazupi")
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert '<select id="persona-select" name="persona" aria-describedby="persona-help"' in panel
    assert '<option value="nazupi" selected>Nazupi</option>' in panel
    assert "Use this persona" in panel


def test_manifest_fallback_names_configured_and_effective_personas_without_prompt_text(
    auth, fake_bot, manifest_personas, seeded
):
    fake_bot.repo.set_config("persona", "retired-persona")
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert "Persona recovery needed" in panel
    assert 'Configured: <span class="mono">retired-persona</span>' in panel
    assert "Effective: <strong>Yuuki Sakuna</strong>" in panel
    assert "unknown persona selection" in panel
    assert "PRIVATE IDENTITY" not in panel
    assert "PRIVATE BEHAVIOUR" not in panel


def test_manifest_with_no_loadable_choices_names_the_recovery(
    auth, fake_bot, manifest_personas, seeded
):
    (manifest_personas / "personas.yaml").write_text(
        """schema_version: 1
default: unavailable
personas:
  - id: unavailable
    label: Unavailable
    aliases: []
""",
        encoding="utf-8",
    )
    fake_bot.repo.set_config("persona", "unavailable")
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    select = panel[panel.index('<select id="persona-select"') : panel.index("</select>")]
    assert "disabled" in select
    assert "No selectable Personas are available" in panel
    assert "restore a valid manifest and complete bundle" in panel
    assert "PRIVATE IDENTITY" not in panel


def test_the_panel_offers_every_persona_on_the_mount(auth, fake_bot, staged_personas, seeded):
    """Manifest choices are read live, so a new bundle appears without a restart."""
    fake_bot.repo.set_config("persona", "kanade")
    fake_bot.chat.reload_persona()
    panel = chatbot_panel(auth.get("/config").text)

    assert '<label class="label" for="persona-select">Persona</label>' in panel
    assert '<select id="persona-select" name="persona" aria-describedby="persona-help"' in panel
    assert '<option value="kanade" selected>Kanade</option>' in panel
    assert '<option value="gruff">Gruff</option>' in panel
    assert "README.md" not in panel
    assert "Use this persona" in panel
    assert "Legacy compatibility" not in panel


def test_choosing_a_persona_takes_effect_on_the_next_answer(
    auth, fake_bot, staged_personas, seeded
):
    """The selection replaces the whole active bundle without a restart."""
    fake_bot.repo.set_config("persona", "kanade")
    fake_bot.chat.reload_persona()
    assert "Kanade" in fake_bot.chat.persona_text()

    auth.post("/config", data={"section": "chatbot", "persona": "gruff"})

    assert "Gruff" in fake_bot.chat.persona_text()
    assert fake_bot.repo.get_config("persona") == "gruff"


def test_a_persona_that_is_not_on_the_mount_is_refused(auth, fake_bot, staged_personas, seeded):
    """Membership, not sanitising -- failure names IDs, never private text."""
    from bot.api.errors import ApiError

    for attempt in ("../persona.example.md", "/etc/passwd", "README.md", "nope.md"):
        with pytest.raises(ApiError) as raised:
            service.set_config(fake_bot, "persona", attempt)
        assert "kanade" in raised.value.message  # what it offers instead
        assert "You are" not in raised.value.message  # never the text itself

    assert fake_bot.repo.get_config("persona") is None


def test_the_persona_change_is_audited_by_id_and_nothing_else(
    auth, fake_bot, staged_personas, seeded
):
    fake_bot.repo.set_config("persona", "kanade")

    auth.post("/config", data={"section": "chatbot", "persona": "gruff"})

    row = next(r for r in fake_bot.repo.list_audit() if r["subject"] == "persona")
    assert row["surface"] == "portal"
    assert "kanade -> gruff" in row["detail"]
    # The voice is private: its words never reach the audit trail.
    assert "You are" not in row["detail"]


def test_a_chosen_persona_that_has_gone_missing_shows_configured_and_effective(
    auth, fake_bot, staged_personas, seeded
):
    """The bundle was there when picked and is not now -- the bot answers from
    the complete default bundle instead of nothing, and the panel says which."""
    fake_bot.repo.set_config("persona", "gruff")
    (staged_personas / "personas" / "gruff" / "identity.md").unlink()
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert "Persona recovery needed" in panel
    assert 'Configured: <span class="mono">gruff</span>' in panel
    assert "Effective: <strong>Kanade</strong>" in panel
    assert "status--at_risk" in panel


def test_the_setting_seeds_canonical_ids_without_overwriting(tmp_path, staged_personas):
    """A fresh database seeds the manifest default (or alias target); `seed_config`
    only inserts what is missing, so a persona chosen in the portal survives restarts."""
    from bot.__main__ import build_repo
    from bot.agent.client import CFG_PERSONA

    from .fake_bot import make_settings

    settings = make_settings(
        db_path=str(tmp_path / "bot.sqlite"), persona_path="/app/personas/kanade.md"
    )
    repo = build_repo(settings)
    try:
        # `kanade.md` is a legacy alias for the canonical `kanade` bundle.
        assert repo.get_config(CFG_PERSONA) == "kanade"
        repo.set_config(CFG_PERSONA, "gruff")
        build_repo(settings).close()
        assert repo.get_config(CFG_PERSONA) == "gruff"
    finally:
        repo.close()


def test_a_deploy_with_no_persona_of_its_own_explains_recovery(auth, fake_bot, seeded):
    """A tracked fallback is named as a recoverable deployment state."""
    fake_bot.settings.persona_path = ""
    fake_bot.chat.reload_persona()

    panel = chatbot_panel(auth.get("/config").text)

    assert "Persona recovery needed" in panel
    assert "Effective: <strong>" in panel
    assert "Configured:" in panel
    assert "status--at_risk" in panel


# --- what .env holds --------------------------------------------------------


def test_a_host_with_no_speech_model_says_so_rather_than_showing_a_gap(auth, fake_bot, seeded):
    """The same way the digest channel's row does, two rows below."""
    fake_bot.settings.chat_pilot_model = ""
    env = auth.get("/config").text
    env = env[env.index('id="env"') :]

    assert ">Speech model</th>" in env
    assert "not set" in env
