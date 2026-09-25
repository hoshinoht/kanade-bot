from __future__ import annotations

import stat
from pathlib import Path

import pytest
import yaml

from scripts import convert_personas_v5 as convert

SECRET = "SYNTHETIC-SECRET-TEXT"


def _write(path: Path, text: str | bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(text, bytes):
        path.write_bytes(text)
    else:
        path.write_text(text, encoding="utf-8", newline="")


@pytest.fixture
def source(tmp_path: Path) -> Path:
    root = tmp_path / "v4"
    _write(
        root / "personas.yaml",
        "schema_version: 1\n"
        "default: alpha\n"
        "personas:\n"
        "  - id: alpha\n    label: ' Alpha '\n    aliases: [alpha.md]\n"
        "  - id: beta\n    label: Beta\n    aliases: []\n"
        "  - id: kanade\n    label: Kanade\n    aliases: []\n",
    )
    alpha = root / "personas" / "alpha"
    _write(alpha / "identity.md", f"# Persona: Alpha\n\n{SECRET} identity\n")
    _write(alpha / "default.md", f"**Voice:** calm and brief\n\n{SECRET} behaviour\n\n")
    _write(
        alpha / "staging.yaml",
        "schedule: ' Checking… '\nguide: Reading\nguide-named: '{boss} notes'\n"
        "write: Drafting\ngeneric: Thinking\n",
    )
    _write(alpha / "default-compact.md", f"{SECRET} compact\n")
    beta = root / "personas" / "beta"
    _write(beta / "identity.md", "Voice: from identity\nBeta identity")
    _write(beta / "default.md", "Beta behaviour\n")
    _write(
        beta / "staging.yaml",
        "schedule: a\nguide: b\nguide_named: c\nwrite: d\ngeneric: e\n",
    )
    behaviours = root / "behaviours"
    _write(behaviours / "plain.md", f"_Voice_: plain voice\n\n{SECRET} plain\n")
    _write(behaviours / "crlf.md", b"# CRLF\r\n\r\ntext\r\n")
    _write(behaviours / "under_score.md", "underscored\n")
    _write(behaviours / "default.md", "reserved\n")
    _write(behaviours / "example.md", "template\n")
    _write(behaviours / "staging" / "plain.yaml", "generic: ' Plain thinking '\n")
    _write(behaviours / "staging" / "crlf.yaml", "unknown: x\n")
    _write(behaviours / "staging" / "orphan.yaml", "generic: x\n")
    return root


def _load(dest: Path, name: str) -> dict:
    return yaml.safe_load((dest / name).read_text(encoding="utf-8"))


def test_converts_with_v4_semantics(source: Path, tmp_path: Path) -> None:
    dest = tmp_path / "v5"
    plan = convert.convert(source, dest)

    assert sorted(plan.outputs) == [
        "bundles/alpha.yaml",
        "bundles/beta.yaml",
        "catalog.yaml",
        "profiles/crlf.yaml",
        "profiles/plain.yaml",
        "profiles/under-score.yaml",
    ]
    assert _load(dest, "catalog.yaml") == {
        "schema_version": 1,
        "default": "alpha",
        "personas": [
            {"id": "alpha", "label": "Alpha", "aliases": ["alpha.md"]},
            {"id": "beta", "label": "Beta", "aliases": []},
            {"id": "kanade", "label": "Kanade", "aliases": []},
        ],
    }
    alpha = _load(dest, "bundles/alpha.yaml")
    assert alpha["identity"] == (source / "personas/alpha/identity.md").read_text()
    assert alpha["behaviour"] == {
        "voice": "calm and brief",
        "prompt": (source / "personas/alpha/default.md").read_text(),
    }
    assert alpha["staging"] == {
        "schedule": "Checking…",
        "guide": "Reading",
        "guide_named": "{boss} notes",
        "write": "Drafting",
        "generic": "Thinking",
    }
    assert alpha["compact"] == {"header_rewrite": f"{SECRET} compact\n"}
    beta = _load(dest, "bundles/beta.yaml")
    assert beta["behaviour"]["voice"] == "from identity"
    assert "compact" not in beta

    plain = _load(dest, "profiles/plain.yaml")
    assert plain == {
        "schema_version": 1,
        "id": "plain",
        "label": "plain",
        "voice": "plain voice",
        "prompt": f"_Voice_: plain voice\n\n{SECRET} plain\n",
        "staging": {"generic": "Plain thinking"},
    }
    crlf = _load(dest, "profiles/crlf.yaml")
    assert crlf["prompt"] == "# CRLF\n\ntext\n"
    assert "staging" not in crlf and "voice" not in crlf
    assert _load(dest, "profiles/under-score.yaml")["label"] == "under_score"
    assert not (dest / "bundles/kanade.yaml").exists()
    assert not (dest / "profiles/example.yaml").exists()

    notes = "\n".join(plan.notes)
    for expected in (
        "profile under_score: renamed to under-score",
        "profile default: reserved v4 name skipped",
        "profile crlf: invalid v4 staging ignored",
        "profile staging orphan: no matching profile, skipped",
        "bundle kanade: tracked in v5, not converted",
    ):
        assert expected in notes

    for path in dest.rglob("*.yaml"):
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE((dest / "profiles").stat().st_mode) == 0o700


def test_multiline_text_uses_literal_blocks(source: Path, tmp_path: Path) -> None:
    dest = tmp_path / "v5"
    convert.convert(source, dest)
    assert "identity: |\n" in (dest / "bundles/alpha.yaml").read_text(encoding="utf-8")


def test_refuses_overwrite_unless_forced(source: Path, tmp_path: Path) -> None:
    dest = tmp_path / "v5"
    _write(dest / "profiles" / "plain.yaml", "keep\n")

    with pytest.raises(convert.ConversionError, match="profiles/plain.yaml"):
        convert.convert(source, dest)
    assert (dest / "profiles" / "plain.yaml").read_text() == "keep\n"
    assert not (dest / "catalog.yaml").exists()

    convert.convert(source, dest, force=True)
    assert _load(dest, "profiles/plain.yaml")["id"] == "plain"


def test_dry_run_prints_names_only(source: Path, tmp_path: Path, capsys) -> None:
    dest = tmp_path / "v5"
    assert convert.main(["--source", str(source), "--dest", str(dest), "--dry-run"]) == 0

    out = capsys.readouterr().out
    assert "would write bundles/alpha.yaml" in out
    assert SECRET not in out and "calm and brief" not in out
    assert not dest.exists()


def test_invalid_v4_bundle_stops_conversion(source: Path, tmp_path: Path) -> None:
    (source / "personas" / "beta" / "identity.md").write_text("  \n")
    with pytest.raises(convert.ConversionError, match="beta"):
        convert.convert(source, tmp_path / "v5")
    assert not (tmp_path / "v5").exists()
