from __future__ import annotations

import os

import pytest

from bot.api import service
from bot.api.errors import BadRequest
from bot.chat import persona
from bot.chat.persona_catalog import (
    PersonaCatalogError,
    load_catalog,
    load_configured_runtime,
    prepare_runtime,
)
from bot.chat.progress import STAGING_KEYS


def _bundle(
    root,
    identifier: str,
    marker: str,
    identity: str = "identity.md",
    behaviour: str = "default.md",
    staging: str = "staging.yaml",
) -> None:
    directory = root / "personas" / identifier
    directory.mkdir(parents=True, exist_ok=True)
    (directory / identity).write_text(f"# Persona: {marker}\n", encoding="utf-8")
    (directory / behaviour).write_text(f"**Voice:** {marker}\n", encoding="utf-8")
    (directory / staging).write_text(
        "".join(f"{key}: {marker}-{key}\n" for key in STAGING_KEYS), encoding="utf-8"
    )


def _manifest(root, personas, default="yuuki-sakuna") -> None:
    lines = ["schema_version: 1", f"default: {default}", "personas:"]
    for entry in personas:
        identifier, label, aliases = entry[0], entry[1], entry[2]
        files = entry[3] if len(entry) > 3 else {}
        lines.extend([f"  - id: {identifier}", f"    label: {label}", "    aliases:"])
        lines.extend(f"      - {alias}" for alias in aliases)
        for key in ("identity", "behaviour", "staging"):
            if key in files:
                lines.append(f"    {key}: {files[key]}")
    (root / "personas.yaml").write_text("\n".join(lines) + "\n", encoding="utf-8")


@pytest.fixture
def catalog_root(tmp_path):
    _bundle(tmp_path, "yuuki-sakuna", "Yuuki")
    _bundle(tmp_path, "nazupi", "Nazupi")
    _bundle(tmp_path, "kanade", "Kanade")
    _manifest(
        tmp_path,
        [("yuuki-sakuna", "Yuuki Sakuna", ["persona.md"]), ("nazupi", "Nazupi", ["nazupi.md"])],
    )
    return tmp_path


def test_manifest_bundles_and_aliases_load_complete_runtime(catalog_root):
    catalog = load_catalog(catalog_root)

    assert [item.id for item in catalog.choices] == ["yuuki-sakuna", "nazupi"]
    assert catalog.resolve("persona.md").id == "yuuki-sakuna"
    runtime = prepare_runtime(catalog, "nazupi.md")
    assert runtime.bundle.id == "nazupi"
    assert runtime.bundle.label == "Nazupi"
    assert runtime.bundle.identity == "# Persona: Nazupi"
    assert runtime.bundle.default_behaviour == "**Voice:** Nazupi"
    assert tuple(getattr(runtime.bundle.staging, key) for key in STAGING_KEYS) == tuple(
        f"Nazupi-{key}" for key in STAGING_KEYS
    )


@pytest.mark.parametrize(
    "document",
    [
        "schema_version: 2\ndefault: yuuki-sakuna\npersonas: []\n",
        "schema_version: 1\ndefault: unknown\npersonas: []\n",
        "schema_version: 1\ndefault: yuuki-sakuna\nextra: no\npersonas: []\n",
        (
            "schema_version: 1\ndefault: yuuki-sakuna\npersonas:\n"
            "  - id: ../bad\n    label: Bad\n    aliases: []\n"
        ),
        (
            "schema_version: 1\ndefault: yuuki-sakuna\npersonas:\n"
            "  - id: yuuki-sakuna\n    label: Good\n    aliases: [../persona.md]\n"
        ),
        (
            "schema_version: 1\ndefault: yuuki-sakuna\npersonas:\n"
            "  - id: yuuki-sakuna\n    label: Good\n    aliases: []\n"
            "    identity: ../evil.md\n"
        ),
        (
            "schema_version: 1\ndefault: yuuki-sakuna\npersonas:\n"
            "  - id: yuuki-sakuna\n    label: Good\n    aliases: []\n"
            "    staging: staging.txt\n"
        ),
        (
            "schema_version: 1\ndefault: yuuki-sakuna\npersonas:\n"
            "  - id: yuuki-sakuna\n    label: Good\n    aliases: []\n"
            "    bogus: x\n"
        ),
    ],
)
def test_manifest_schema_is_strict(tmp_path, document):
    (tmp_path / "personas.yaml").write_text(document, encoding="utf-8")
    with pytest.raises(PersonaCatalogError):
        load_catalog(tmp_path)


@pytest.mark.parametrize("component", ["identity.md", "default.md", "staging.yaml"])
def test_symlink_components_are_not_loadable(catalog_root, component):
    target = catalog_root / "personas" / "nazupi" / component
    path = catalog_root / "personas" / "yuuki-sakuna" / component
    path.unlink()
    os.symlink(target, path)

    catalog = load_catalog(catalog_root)
    assert "yuuki-sakuna" in catalog.issues
    with pytest.raises(PersonaCatalogError):
        prepare_runtime(catalog, "yuuki-sakuna")


def test_symlink_bundle_is_not_loadable(catalog_root):
    bundle = catalog_root / "personas" / "yuuki-sakuna"
    replacement = catalog_root / "personas" / "other"
    bundle.rename(replacement)
    os.symlink(replacement, bundle)

    assert "yuuki-sakuna" in load_catalog(catalog_root).issues


def test_invalid_selection_uses_complete_manifest_default(catalog_root):
    runtime = load_configured_runtime(catalog_root, "not-a-persona")

    assert runtime.bundle.id == "yuuki-sakuna"
    assert runtime.bundle.identity == "# Persona: Yuuki"
    assert runtime.bundle.default_behaviour == "**Voice:** Yuuki"
    assert runtime.bundle.staging.generic == "Yuuki-generic"
    assert runtime.bundle.fell_back is True


def test_invalid_default_uses_complete_example_bundle(catalog_root):
    (catalog_root / "personas" / "yuuki-sakuna" / "staging.yaml").write_text(
        "generic: only-one\n", encoding="utf-8"
    )

    runtime = load_configured_runtime(catalog_root, "nazupi")
    assert runtime.bundle.id == "nazupi"
    runtime = load_configured_runtime(catalog_root, "missing")
    assert runtime.bundle.id == "kanade"
    assert runtime.bundle.identity == "# Persona: Kanade"


def test_custom_bundle_filenames_load_from_manifest(tmp_path):
    _bundle(
        tmp_path,
        "yuuki-sakuna",
        "Yuuki",
        identity="sakuna.md",
        behaviour="sakuna-behaviour.md",
        staging="sakuna-staging.yaml",
    )
    _bundle(tmp_path, "kanade", "Kanade")
    _manifest(
        tmp_path,
        [
            (
                "yuuki-sakuna",
                "Yuuki Sakuna",
                ["persona.md"],
                {
                    "identity": "sakuna.md",
                    "behaviour": "sakuna-behaviour.md",
                    "staging": "sakuna-staging.yaml",
                },
            )
        ],
        default="yuuki-sakuna",
    )

    runtime = prepare_runtime(load_catalog(tmp_path), "persona.md")

    assert runtime.bundle.id == "yuuki-sakuna"
    assert runtime.bundle.identity == "# Persona: Yuuki"
    assert runtime.bundle.default_behaviour == "**Voice:** Yuuki"
    assert runtime.bundle.staging.generic == "Yuuki-generic"
    assert runtime.bundle.source.identity_path.name == "sakuna.md"
    assert runtime.bundle.source.default_behaviour_path.name == "sakuna-behaviour.md"
    assert runtime.bundle.source.staging_path.name == "sakuna-staging.yaml"


def test_profile_staging_is_partial_and_uses_selected_baseline(catalog_root):
    directory = catalog_root / "behaviours" / "staging"
    directory.mkdir(parents=True)
    (directory / "tsundere.yaml").write_text("generic: profile-generic\n", encoding="utf-8")

    runtime = prepare_runtime(load_catalog(catalog_root), "nazupi")
    assert runtime.profile_staging["tsundere"].generic == "profile-generic"
    assert runtime.profile_staging["tsundere"].schedule == "Nazupi-schedule"


def test_manifest_absence_uses_legacy_catalog(tmp_path):
    (tmp_path / "identities").mkdir()
    (tmp_path / "identities" / "persona.md").write_text("legacy identity", encoding="utf-8")
    (tmp_path / "behaviours").mkdir()
    (tmp_path / "behaviours" / "default.md").write_text("legacy default", encoding="utf-8")
    (tmp_path / "behaviours" / "staging.yaml").write_text("generic: legacy\n", encoding="utf-8")

    catalog = load_catalog(tmp_path)
    assert catalog.mode == "legacy"
    assert prepare_runtime(catalog, "persona.md").bundle.identity == "legacy identity"


def test_service_switches_complete_manifest_runtime_and_reports_contract(
    catalog_root, monkeypatch, repo, bosses
):
    from .chat_support import build_bot

    monkeypatch.setattr(persona, "PERSONA_DIR", catalog_root)
    bot = build_bot(repo, bosses)

    service.set_config(bot, "persona", "yuuki-sakuna")
    before = bot.chat.persona_runtime()
    result = service.set_config(bot, "persona", "nazupi")
    active = bot.chat.persona_runtime()

    assert bot.repo.get_config("persona") == "nazupi"
    assert active.bundle.identity == "# Persona: Nazupi"
    assert active.bundle.default_behaviour == "**Voice:** Nazupi"
    assert tuple(getattr(active.bundle.staging, key) for key in STAGING_KEYS) == tuple(
        f"Nazupi-{key}" for key in STAGING_KEYS
    )
    assert active is not before
    assert result["persona_choices"] == ["yuuki-sakuna", "nazupi"]
    assert result["persona"] == "nazupi"
    assert result["persona_labels"] == {"yuuki-sakuna": "Yuuki Sakuna", "nazupi": "Nazupi"}
    assert result["persona_effective"] == "nazupi"
    assert result["persona_effective_label"] == "Nazupi"
    assert result["persona_catalog_mode"] == "manifest"
    assert result["persona_issue"] is None
    assert result["persona_file"] == "identity.md"
    assert result["persona_fallback"] is False


def test_alias_selection_persists_canonical_id(catalog_root, monkeypatch, repo, bosses):
    from .chat_support import build_bot

    monkeypatch.setattr(persona, "PERSONA_DIR", catalog_root)
    bot = build_bot(repo, bosses)

    service.set_config(bot, "persona", "nazupi.md")

    assert bot.repo.get_config("persona") == "nazupi"
    assert bot.chat.persona_runtime().bundle.id == "nazupi"


def test_invalid_or_failed_swap_keeps_config_and_runtime(catalog_root, monkeypatch, repo, bosses):
    from .chat_support import build_bot

    monkeypatch.setattr(persona, "PERSONA_DIR", catalog_root)
    bot = build_bot(repo, bosses)
    service.set_config(bot, "persona", "yuuki-sakuna")
    active = bot.chat.persona_runtime()
    catalog_root.joinpath("personas", "nazupi", "staging.yaml").write_text("generic: bad\n")

    with pytest.raises(BadRequest):
        service.set_config(bot, "persona", "nazupi")
    assert bot.repo.get_config("persona") == "yuuki-sakuna"
    assert bot.chat.persona_runtime() is active

    def fail_set_config(*_args):
        raise OSError("DB down")

    monkeypatch.setattr(bot.repo, "set_config", fail_set_config)
    with pytest.raises(OSError, match="DB down"):
        service.set_config(bot, "persona", "yuuki-sakuna")
    assert bot.chat.persona_runtime() is active


@pytest.mark.parametrize(
    ("persona_path", "expected"),
    [("/deploy/persona.md", "yuuki-sakuna"), ("/deploy/unknown.md", "yuuki-sakuna")],
)
def test_manifest_seed_resolves_alias_or_default_without_overwriting(
    catalog_root, monkeypatch, tmp_path, persona_path, expected
):
    from bot import __main__ as entrypoint

    from .fake_bot import make_settings

    monkeypatch.setattr("bot.chat.persona_catalog.PERSONA_ROOT", catalog_root)
    monkeypatch.setattr(persona, "PERSONA_DIR", catalog_root)
    settings = make_settings(db_path=str(tmp_path / "seed.sqlite"), persona_path=persona_path)
    seeded = entrypoint.build_repo(settings)
    assert seeded.get_config("persona") == expected
    seeded.set_config("persona", "nazupi")
    seeded.close()

    restarted = entrypoint.build_repo(settings)
    assert restarted.get_config("persona") == "nazupi"
    restarted.close()


def test_malformed_manifest_falls_back_to_complete_example(catalog_root):
    (catalog_root / "personas.yaml").write_text("default: [unclosed\n", encoding="utf-8")

    runtime = load_configured_runtime(catalog_root, "nazupi")

    assert runtime.bundle.id == "kanade"
    assert runtime.bundle.fell_back is True
    assert runtime.bundle.issue
    assert "nazupi" not in runtime.bundle.issue


def test_non_utf8_manifest_falls_back_to_complete_example(catalog_root):
    (catalog_root / "personas.yaml").write_bytes(b"default: \xff\xfe\n")

    runtime = load_configured_runtime(catalog_root, "nazupi")

    assert runtime.bundle.id == "kanade"
    assert runtime.bundle.fell_back is True


def test_manifest_symlink_is_rejected(catalog_root, tmp_path):
    real = tmp_path / "real.yaml"
    (catalog_root / "personas.yaml").rename(real)
    os.symlink(real, catalog_root / "personas.yaml")

    with pytest.raises(PersonaCatalogError):
        load_catalog(catalog_root)


def test_symlinked_personas_directory_is_rejected(catalog_root, tmp_path):
    target = tmp_path / "personas-target"
    (catalog_root / "personas").rename(target)
    os.symlink(target, catalog_root / "personas")

    catalog = load_catalog(catalog_root)
    # Every manifest bundle is unusable when the parent directory is a link.
    assert set(catalog.issues) == {"yuuki-sakuna", "nazupi"}
    assert catalog.choices == ()


def test_legacy_missing_default_uses_complete_example(tmp_path):
    _bundle(tmp_path, "kanade", "Kanade")
    (tmp_path / "identities").mkdir()
    (tmp_path / "identities" / "persona.md").write_text("legacy identity", encoding="utf-8")

    runtime = load_configured_runtime(tmp_path, "persona.md")

    assert runtime.bundle.id == "kanade"
    assert runtime.bundle.fell_back is True
    assert runtime.bundle.label == "Kanade"


def test_legacy_invalid_staging_uses_complete_example_without_parser_detail(tmp_path):
    _bundle(tmp_path, "kanade", "Kanade")
    (tmp_path / "identities").mkdir()
    (tmp_path / "identities" / "persona.md").write_text("legacy identity", encoding="utf-8")
    (tmp_path / "behaviours").mkdir()
    (tmp_path / "behaviours" / "default.md").write_text("legacy default", encoding="utf-8")
    (tmp_path / "behaviours" / "staging.yaml").write_text("default: [unclosed\n", encoding="utf-8")

    runtime = load_configured_runtime(tmp_path, "persona.md")

    assert runtime.bundle.id == "kanade"
    assert runtime.bundle.fell_back is True
    assert "unclosed" not in (runtime.bundle.issue or "")


def test_legacy_symlink_identity_is_rejected(tmp_path):
    (tmp_path / "identities").mkdir()
    real = tmp_path / "real.md"
    real.write_text("legacy identity", encoding="utf-8")
    os.symlink(real, tmp_path / "identities" / "persona.md")
    (tmp_path / "behaviours").mkdir()
    (tmp_path / "behaviours" / "default.md").write_text("legacy default", encoding="utf-8")
    (tmp_path / "behaviours" / "staging.yaml").write_text("generic: legacy\n", encoding="utf-8")

    catalog = load_catalog(tmp_path)
    with pytest.raises(PersonaCatalogError):
        prepare_runtime(catalog, "persona.md")


def test_invalid_profile_staging_warns_and_uses_baseline(catalog_root, caplog):
    directory = catalog_root / "behaviours" / "staging"
    directory.mkdir(parents=True)
    (directory / "broken.yaml").write_text("generic: [unclosed\n", encoding="utf-8")

    with caplog.at_level("WARNING", logger="bot.chat.persona_catalog"):
        runtime = prepare_runtime(load_catalog(catalog_root), "nazupi")

    assert runtime.bundle.id == "nazupi"
    assert runtime.bundle.staging.generic == "Nazupi-generic"
    assert "broken.yaml" in caplog.text
    assert "unclosed" not in caplog.text


def test_legacy_external_identity_and_custom_staging(tmp_path):
    (tmp_path / "behaviours").mkdir(parents=True)
    (tmp_path / "behaviours" / "default.md").write_text("legacy default", encoding="utf-8")
    custom_staging = tmp_path / "custom-staging.yaml"
    custom_staging.write_text("generic: custom\nschedule: custom\n", encoding="utf-8")
    external = tmp_path / "external-persona.md"
    external.write_text("external identity", encoding="utf-8")

    runtime = load_configured_runtime(
        tmp_path,
        "external-persona.md",
        legacy_identity_path=external,
        legacy_staging_path=custom_staging,
    )

    assert runtime.bundle.identity == "external identity"
    assert runtime.bundle.default_behaviour == "legacy default"
    assert runtime.bundle.staging.generic == "custom"
