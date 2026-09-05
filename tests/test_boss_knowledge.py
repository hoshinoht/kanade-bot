"""Strict local boss knowledge documents."""

from __future__ import annotations

from pathlib import Path

import pytest

from bot.domain.boss_knowledge import BossKnowledgeBase, BossKnowledgeError
from bot.domain.bosses import BossReference, BossTable

from .conftest import REPO_ROOT


def test_shipped_knowledge_covers_the_catalog_and_renders_markdown(bosses: BossTable):
    knowledge = BossKnowledgeBase.load(REPO_ROOT / "boss" / "knowledge", bosses)

    assert set(knowledge.documents) == set(bosses.bosses)
    rendered = knowledge.render(BossReference("Jupiter", "h"))
    assert "# Jupiter (Jupiter)" in rendered
    assert "Researched as of 2026-09-05" in rendered
    assert "## Core" in rendered and "## Danger" in rendered and "## Tips" in rendered
    assert "## Difficulty notes" in rendered
    assert "Every third combination" in rendered
    assert "## Sources" in rendered


def test_startup_loads_catalog_without_knowledge_when_chat_is_unconfigured(tmp_path: Path):
    from bot.__main__ import load_boss_resources

    from .conftest import REPO_ROOT
    from .fake_bot import make_settings

    settings = make_settings(
        bosses_path=str(REPO_ROOT / "boss" / "bosses.yaml"),
        boss_knowledge_path=str(tmp_path / "not-required"),
    )

    bosses, knowledge = load_boss_resources(settings)

    assert bosses.bosses["FA"].full == "The First Adversary"
    assert knowledge is None


def test_startup_requires_knowledge_when_chat_is_configured(tmp_path: Path, caplog):
    from bot.__main__ import load_boss_resources

    from .conftest import REPO_ROOT
    from .fake_bot import make_settings

    knowledge_path = tmp_path / "missing-knowledge"
    settings = make_settings(
        bosses_path=str(REPO_ROOT / "boss" / "bosses.yaml"),
        boss_knowledge_path=str(knowledge_path),
        chat_pilot_role_id=1,
        chat_pilot_channel_ids="2",
    )

    with pytest.raises(BossKnowledgeError, match="knowledge directory does not exist"):
        load_boss_resources(settings)

    assert settings.bosses_path in caplog.text
    assert str(knowledge_path) in caplog.text


def _table() -> BossTable:
    return BossTable.from_dict(
        {
            "difficulties": {"n": "Normal", "h": "Hard"},
            "bosses": {"Foo": {"difficulties": ["n", "h"]}},
        }
    )


def _write_meta(directory: Path, body: str = 'schema_version: 1\nresearched_as_of: "2026-09-05"\n'):
    (directory / "_meta.yaml").write_text(body, encoding="utf-8")


def _write_foo(directory: Path, extra: str = "", replacements: tuple[tuple[str, str], ...] = ()):
    body = """boss: Foo
summary: Summary.
core: [Core.]
danger: [Danger.]
tips: [Tip.]
sources: [https://example.test/source]
"""
    for old, new in replacements:
        body = body.replace(old, new)
    (directory / "foo.yaml").write_text(body + extra, encoding="utf-8")


def test_knowledge_rejects_duplicate_yaml_fields(tmp_path: Path):
    _write_meta(tmp_path)
    _write_foo(tmp_path, "summary: Duplicate.\n")

    with pytest.raises(BossKnowledgeError, match="duplicate YAML key 'summary'"):
        BossKnowledgeBase.load(tmp_path, _table())


def test_knowledge_rejects_non_scalar_mapping_keys_with_a_file_specific_error(tmp_path: Path):
    _write_meta(tmp_path)
    _write_foo(
        tmp_path,
        "? [not, a, scalar]\n: invalid\n",
    )

    with pytest.raises(
        BossKnowledgeError, match=r"foo\.yaml: mapping keys must be hashable scalars"
    ):
        BossKnowledgeBase.load(tmp_path, _table())


@pytest.mark.parametrize(
    ("replacements", "extra", "match"),
    [
        ((), "extra: nope\n", "unknown"),
        ((("https://example.test/source", "http://example.test"),), "", "HTTPS"),
        ((("[Core.]", "[one, two, three, four, five, six, seven, eight, nine]"),), "", "1..8"),
        ((), "difficulty_notes: {x: nope}\n", "unsupported difficulty"),
    ],
)
def test_knowledge_rejects_strict_document_bounds(
    tmp_path: Path, replacements: tuple[tuple[str, str], ...], extra: str, match: str
):
    _write_meta(tmp_path)
    _write_foo(tmp_path, extra, replacements)

    with pytest.raises(BossKnowledgeError, match=match):
        BossKnowledgeBase.load(tmp_path, _table())


def test_knowledge_names_missing_and_extra_document_ids(tmp_path: Path):
    _write_meta(tmp_path)
    (tmp_path / "other.yaml").write_text("boss: Other\n", encoding="utf-8")

    with pytest.raises(BossKnowledgeError, match=r"missing: foo; extra: other"):
        BossKnowledgeBase.load(tmp_path, _table())


def test_meta_is_strict_and_cannot_be_future_dated(tmp_path: Path):
    _write_meta(tmp_path, 'schema_version: 1\nresearched_as_of: "2999-01-01"\nunknown: nope\n')
    _write_foo(tmp_path)

    with pytest.raises(BossKnowledgeError, match="fields invalid"):
        BossKnowledgeBase.load(tmp_path, _table())


def test_meta_rejects_a_future_date(tmp_path: Path):
    _write_meta(tmp_path, 'schema_version: 1\nresearched_as_of: "2999-01-01"\n')
    _write_foo(tmp_path)

    with pytest.raises(BossKnowledgeError, match="must not be in the future"):
        BossKnowledgeBase.load(tmp_path, _table())
