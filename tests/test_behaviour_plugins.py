"""Behaviour plugins as persisted persona add-ons and ordered role assignments."""

from __future__ import annotations

import pytest

from bot import behaviour_plugins


def test_plugins_are_stored_as_markdown_and_read_back(tmp_path):
    saved = behaviour_plugins.write("playful", "Use playful banter.", tmp_path)

    assert saved == behaviour_plugins.Plugin("playful", "Use playful banter.")
    assert (tmp_path / "playful.md").read_text(encoding="utf-8") == "Use playful banter.\n"
    assert behaviour_plugins.read("playful", tmp_path) == saved


def test_the_tracked_example_is_documentation_not_an_assignable_plugin(tmp_path):
    (tmp_path / "example.md").write_text("Template text", encoding="utf-8")
    (tmp_path / "real.md").write_text("Real instructions", encoding="utf-8")

    assert behaviour_plugins.available(tmp_path) == ["real"]


@pytest.mark.parametrize("name", ["../escape", "/absolute", "has spaces", ""])
def test_plugin_names_cannot_escape_the_plugin_directory(tmp_path, name):
    with pytest.raises(ValueError):
        behaviour_plugins.write(name, "instructions", tmp_path)


def test_environment_seed_syntax_becomes_ordered_assignments():
    stored = behaviour_plugins.seed_value("100=mesugaki, 200=concise")

    assert behaviour_plugins.decode(stored) == [
        behaviour_plugins.RolePlugin("100", "mesugaki"),
        behaviour_plugins.RolePlugin("200", "concise"),
    ]


def test_matching_plugins_compose_in_assignment_order(tmp_path):
    behaviour_plugins.write("first", "FIRST", tmp_path)
    behaviour_plugins.write("second", "SECOND", tmp_path)
    assignments = behaviour_plugins.validate(
        [
            {"role_id": "100", "plugin": "first"},
            {"role_id": "200", "plugin": "second"},
        ]
    )

    assert behaviour_plugins.overlay(assignments, [200, 100], tmp_path) == "FIRST\n\nSECOND"
