"""Repository discovery helpers used by the v5 inventory checker."""

from __future__ import annotations

import ast
import json
import re
import tomllib
from pathlib import Path
from typing import Any

_MODULE_COMMAND_RE = re.compile(r"\bpython\s+-m\s+([A-Za-z_][\w.]*)")
_JSON_MODULE_COMMAND_RE = re.compile(
    r"[\"']python[\"']\s*,\s*[\"']-m[\"']\s*,\s*[\"']([A-Za-z_][\w.]*)[\"']"
)
_ENV_KEY_RE = re.compile(r"^([A-Z][A-Z0-9_]*)=")


def _module_command(text: str) -> str | None:
    # This is a frozen exec-form baseline, not a shell-command analyzer.
    try:
        tokens = json.loads(text)
    except (ValueError, TypeError):
        return None
    return "python -m bot.health" if tokens == ["python", "-m", "bot.health"] else None


def _docker_healthcheck(text: str) -> str | None:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if not re.match(r"^\s*HEALTHCHECK\b", line):
            continue
        block = [line]
        current = line
        cursor = index + 1
        while cursor < len(lines) and (
            current.rstrip().endswith("\\")
            or not lines[cursor].strip()
            or lines[cursor].startswith((" ", "\t"))
        ):
            current = lines[cursor]
            block.append(current)
            cursor += 1
        command = "\n".join(block)
        match = re.search(r"\bCMD\s+", command)
        return _module_command(command[match.end() :].strip()) if match else None
    return None


def _compose_healthcheck(text: str) -> str | None:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.strip() != "healthcheck:":
            continue
        indent = len(line) - len(line.lstrip())
        block: list[str] = []
        for child in lines[index + 1 :]:
            child_indent = len(child) - len(child.lstrip())
            if child.strip() and child_indent <= indent:
                break
            block.append(child)
        test_line = next((child for child in block if child.lstrip().startswith("test:")), "")
        try:
            tokens = json.loads(test_line.split(":", 1)[1].strip())
        except (ValueError, IndexError):
            return None
        if not isinstance(tokens, list) or not tokens or tokens[0] != "CMD":
            return None
        return _module_command(json.dumps(tokens[1:]))
    return None


def _is_main_guard(node: ast.If) -> bool:
    test = node.test
    if not isinstance(test, ast.Compare) or len(test.ops) != 1:
        return False
    if not isinstance(test.ops[0], ast.Eq):
        return False
    left = test.left
    comparators = test.comparators
    if len(comparators) != 1:
        return False
    right = comparators[0]
    return (
        isinstance(left, ast.Name)
        and left.id == "__name__"
        and isinstance(right, ast.Constant)
        and right.value == "__main__"
    ) or (
        isinstance(right, ast.Name)
        and right.id == "__name__"
        and isinstance(left, ast.Constant)
        and left.value == "__main__"
    )


def _module_name(path: Path, root: Path) -> str:
    parts = list(path.relative_to(root).with_suffix("").parts)
    if parts[-1] == "__main__":
        parts.pop()
    return ".".join(parts)


def discover_module_entrypoints(root: Path) -> set[str]:
    found: set[str] = set()
    bot_root = root / "bot"
    for path in bot_root.rglob("*.py"):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        if any(isinstance(node, ast.If) and _is_main_guard(node) for node in ast.walk(tree)):
            found.add(f"python -m {_module_name(path, root)}")
    return found


def discover_project_scripts(root: Path) -> set[str]:
    with (root / "pyproject.toml").open("rb") as handle:
        project: dict[str, Any] = tomllib.load(handle).get("project", {})
    scripts = project.get("scripts", {})
    return set(scripts) if isinstance(scripts, dict) else set()


def discover_configured_module_commands(root: Path) -> set[str]:
    found: set[str] = set()
    for name in ("deploy/Dockerfile", "deploy/compose.yaml"):
        text = (root / name).read_text(encoding="utf-8")
        found.update(f"python -m {module}" for module in _MODULE_COMMAND_RE.findall(text))
        found.update(f"python -m {module}" for module in _JSON_MODULE_COMMAND_RE.findall(text))
    return found


def discover_configured_healthchecks(root: Path) -> dict[str, str | None]:
    found: dict[str, str | None] = {}
    dockerfile = (root / "deploy" / "Dockerfile").read_text(encoding="utf-8")
    found["deploy/Dockerfile#HEALTHCHECK"] = _docker_healthcheck(dockerfile)
    compose = (root / "deploy" / "compose.yaml").read_text(encoding="utf-8")
    found["deploy/compose.yaml#services.bot.healthcheck"] = _compose_healthcheck(compose)
    return found


def discover_entrypoints(root: Path) -> dict[str, tuple[str, ...]]:
    entrypoints = (
        discover_module_entrypoints(root)
        | discover_project_scripts(root)
        | discover_configured_module_commands(root)
    )
    return {
        "entrypoints": tuple(sorted(entrypoints)),
        "healthchecks": tuple(
            sorted(
                command for command in discover_configured_healthchecks(root).values() if command
            )
        ),
        "healthcheck_sources": discover_configured_healthchecks(root),
    }


def discover_env_keys(root: Path) -> tuple[str, ...]:
    return tuple(
        match.group(1)
        for line in (root / ".env.example").read_text(encoding="utf-8").splitlines()
        if (match := _ENV_KEY_RE.match(line))
    )
