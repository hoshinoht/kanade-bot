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


def test_first_matching_plugin_wins_in_assignment_order(tmp_path):
    behaviour_plugins.write("first", "FIRST", tmp_path)
    behaviour_plugins.write("second", "SECOND", tmp_path)
    assignments = behaviour_plugins.validate(
        [
            {"role_id": "100", "plugin": "first"},
            {"role_id": "200", "plugin": "second"},
        ]
    )

    assert behaviour_plugins.overlay(assignments, [200, 100], tmp_path) == "FIRST"


def test_member_choice_applies_when_no_role_override_matches(tmp_path):
    behaviour_plugins.write("public", "PUBLIC", tmp_path)
    resolution = behaviour_plugins.resolve(
        selected="public",
        selectable=["public"],
        assignments=[],
        role_ids=[],
        default_instructions="DEFAULT",
        directory=tmp_path,
    )
    assert resolution.effective == "public"
    assert resolution.source == "member"
    assert resolution.prompt_instructions() == "PUBLIC"


def test_first_readable_role_override_wins_and_saved_choice_is_retained(tmp_path):
    behaviour_plugins.write("public", "PUBLIC", tmp_path)
    behaviour_plugins.write("forced", "FORCED", tmp_path)
    assignments = behaviour_plugins.validate(
        [
            {"role_id": "100", "plugin": "missing"},
            {"role_id": "200", "plugin": "forced"},
        ]
    )
    resolution = behaviour_plugins.resolve(
        selected="public",
        selectable=["public"],
        assignments=assignments,
        role_ids=[100, 200],
        default_instructions="DEFAULT",
        directory=tmp_path,
    )
    assert resolution.effective == "forced"
    assert resolution.source == "role"
    assert resolution.role_id == "200"
    assert resolution.selected == "public"
    assert resolution.member_view() == {"reply_style": "public", "available": True}


def test_stale_member_choice_falls_back_without_being_erased(tmp_path):
    resolution = behaviour_plugins.resolve(
        selected="gone",
        selectable=["gone"],
        assignments=[],
        role_ids=[],
        default_instructions="DEFAULT",
        directory=tmp_path,
    )
    assert resolution.effective == "default"
    assert resolution.selected == "gone"
    assert resolution.selected_available is False


def test_one_damaged_assignment_does_not_erase_valid_entries():
    assignments, issues = behaviour_plugins.decode_with_issues(
        '[{"role_id":"100","plugin":"first"},{"role_id":"bad","plugin":"second"},'
        '{"role_id":"300","plugin":"third"}]'
    )
    assert [item.role_id for item in assignments] == ["100", "300"]
    assert len(issues) == 1


def test_missing_profile_is_skipped_and_diagnosed(tmp_path):
    behaviour_plugins.write("valid", "VALID", tmp_path)
    raw = '[{"role_id":"100","plugin":"missing"},{"role_id":"200","plugin":"valid"}]'
    assignments, issues = behaviour_plugins.assignment_diagnostics(raw, tmp_path)
    resolution = behaviour_plugins.resolve(
        selected=None,
        selectable=[],
        assignments=assignments,
        role_ids=[100, 200],
        default_instructions="DEFAULT",
        directory=tmp_path,
    )
    assert resolution.effective == "valid"
    assert [issue.message for issue in issues] == ["role 100: profile `missing` is unreadable"]


def test_reserved_profile_names_are_rejected(tmp_path):
    with pytest.raises(ValueError, match="reserved"):
        behaviour_plugins.write("default", "instructions", tmp_path)
