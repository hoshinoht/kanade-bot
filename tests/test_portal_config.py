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
from bot.api.app import STATIC_DIR
from bot.api.templating import CONFIG_SECTIONS, read_section
from bot.portal_styles import build_stylesheet

PAGE_CSS = build_stylesheet()
PAGE_JS = (STATIC_DIR / "portal.js").read_text(encoding="utf-8")


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
    block = PAGE_CSS[
        PAGE_CSS.index("  .settings__body {\n    grid-template-columns: minmax(0, 1fr);") :
    ]
    block = block[: block.index("\n}\n")]

    assert "overflow-x: clip;" in block
    assert "gap: 0.35rem;" in block
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
def plugin_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(behaviour_plugins, "PLUGIN_DIR", tmp_path)
    return tmp_path


def test_the_chatbot_panel_has_behaviour_plugin_and_role_editors(auth, seeded, plugin_dir):
    panel = chatbot_panel(auth.get("/config").text)

    assert "Reply profiles" in panel
    assert "Role assignments" in panel
    assert 'action="/config/behaviour-plugins"' in panel
    assert 'action="/config/role-plugins"' in panel
    assert 'name="role_id"' in panel
    assert 'name="plugin"' in panel


def test_behaviour_plugin_editors_collapse_and_paginate_without_hiding_server_markup(
    auth, seeded, plugin_dir
):
    for number in range(6):
        behaviour_plugins.write(f"style-{number}", f"STYLE {number}")

    panel = chatbot_panel(auth.get("/config").text)

    assert 'data-pagination-key="behaviour-plugins"' in panel
    assert 'data-pagination-key="role-plugins"' in panel
    assert panel.count('data-page-size="5"') == 2
    assert panel.count("data-page-previous") == 2
    assert panel.count("data-page-next") == 2
    assert '<details class="behaviour-editor behaviour-editor--new">' in panel
    for number in range(6):
        assert f"style-{number}" in panel

    assert 'querySelectorAll("[data-page-item]")' in PAGE_JS
    assert "window.sessionStorage" in PAGE_JS


def test_profile_editors_open_as_modals_with_search_and_bulk_selection(auth, seeded, plugin_dir):
    behaviour_plugins.write("mesugaki", "Use playful, smug banter.")
    behaviour_plugins.write("concise", "Always answer in short lines.")

    panel = chatbot_panel(auth.get("/config").text)

    assert 'data-search-input="behaviour-plugins"' in panel
    assert "data-search-empty" in panel
    assert '<a class="behaviour-editor behaviour-editor__summary"' in panel
    assert 'data-dialog="profile-mesugaki"' in panel
    assert 'href="#profile-mesugaki"' in panel
    assert "data-page-item" in panel
    assert '<dialog class="modal" id="profile-mesugaki"' in panel
    assert 'aria-labelledby="profile-mesugaki-title"' in panel
    assert '<form id="bulk-profiles" method="post"' in panel
    assert 'action="/config/behaviour-plugins/selectable"' in panel
    assert 'value="mesugaki"' in panel
    assert 'form="bulk-profiles"' in panel
    assert 'form="bulk-profiles"' in panel
    assert 'name="selectable" value="1"' in panel
    assert 'name="selectable" value="0"' in panel

    assert "data-search-hidden" in PAGE_JS
    assert "data-search-input" in PAGE_JS
    assert "data-bulk-toolbar" in PAGE_JS
    assert "Select page (" in PAGE_JS
    assert "visibilityWord" in PAGE_JS


def test_profile_list_filters_by_visibility_and_toggle_names_the_filter(auth, seeded, plugin_dir):
    behaviour_plugins.write("mesugaki", "Use playful, smug banter.")
    behaviour_plugins.write("concise", "Always answer in short lines.")
    auth.post(
        "/config/behaviour-plugins/selectable",
        data={"profiles": ["mesugaki"], "selectable": "1"},
    )

    panel = chatbot_panel(auth.get("/config").text)

    assert 'data-filter-row="behaviour-plugins" hidden' in panel
    assert 'data-visibility-filter="behaviour-plugins"' in panel
    assert '<option value="public">Public</option>' in panel
    assert '<option value="private">Private</option>' in panel
    assert 'data-visibility="public"' in panel
    assert 'data-visibility="private"' in panel

    assert "data-visibility" in PAGE_JS
    assert '"Select all" + visibilityWord()' in PAGE_JS


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


def test_the_portal_renders_multiple_role_plugin_assignments_in_order(
    auth, fake_bot, seeded, plugin_dir
):
    behaviour_plugins.write("first", "FIRST STYLE")
    behaviour_plugins.write("second", "SECOND STYLE")
    service.set_config(
        fake_bot,
        behaviour_plugins.CONFIG_KEY,
        [
            {"role_id": "1540491480936751205", "plugin": "first"},
            {"role_id": "1540491480936751206", "plugin": "second"},
        ],
    )

    panel = chatbot_panel(auth.get("/config").text)

    assert panel.index("1540491480936751205") < panel.index("1540491480936751206")


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
