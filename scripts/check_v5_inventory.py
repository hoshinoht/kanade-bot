#!/usr/bin/env python3
"""Check the v5 compatibility inventory against the current v4 source tree.

This is deliberately a local, dependency-free evidence check. It parses the
source instead of importing the bot, so it cannot open private state, Discord,
Ollama, or a live database.
"""

from __future__ import annotations

import ast
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any

try:
    from v5_inventory.discovery import discover_entrypoints, discover_env_keys
    from v5_inventory.validation import (
        validate_entrypoint_inventory,
        validate_env_inventory,
        validate_healthcheck_inventory,
    )
except ModuleNotFoundError:  # pragma: no cover - package execution fallback
    from scripts.v5_inventory.discovery import discover_entrypoints, discover_env_keys
    from scripts.v5_inventory.validation import (
        validate_entrypoint_inventory,
        validate_env_inventory,
        validate_healthcheck_inventory,
    )

ROOT = Path(__file__).resolve().parents[1]
V4_ROOT = ROOT / "legacy" / "python"
INVENTORY = ROOT / "docs" / "notes" / "inventory.json"
DISPOSITIONS = {"Retain", "Import-only", "Remove", "Defer"}


def dotted(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        prefix = dotted(node.value)
        return f"{prefix}.{node.attr}" if prefix else node.attr
    return None


def literal(node: ast.AST | None) -> Any:
    if node is None:
        return None
    try:
        return ast.literal_eval(node)
    except (ValueError, TypeError, SyntaxError):
        return None


def parse(path: str | Path) -> ast.AST:
    return ast.parse((V4_ROOT / path).read_text(encoding="utf-8"), filename=str(path))


def source_path(ref: str) -> str:
    return ref.split("#", 1)[0]


def check_ref_paths(inventory: dict[str, Any], errors: list[str]) -> None:
    def visit(value: Any, key: str = "") -> None:
        if isinstance(value, dict):
            for name, child in value.items():
                visit(child, name)
        elif isinstance(value, list):
            for child in value:
                visit(child, key)
        elif isinstance(value, str):
            if (
                "#" in value or key in {"source", "tests", "test_refs", "configured_sources"}
            ) and value.startswith(
                (
                    "bot/",
                    "tests/",
                    "pyproject.toml",
                    ".env.example",
                    "deploy/",
                    "boss/",
                    "config/",
                    "docs/",
                    "scripts/",
                )
            ):
                source_root = ROOT if value.startswith("scripts/v5_inventory/") else V4_ROOT
                path = source_root / source_path(value)
                if not path.exists():
                    errors.append(f"missing cited path: {value}")

    visit(inventory)


def route_inventory() -> tuple[list[dict[str, str]], list[dict[str, str]]]:
    routes: list[dict[str, str]] = []
    registrations: list[dict[str, str]] = []
    files = ("bot/api/app.py", "bot/api/routes_api.py", "bot/api/routes_web.py")
    prefixes: dict[str, str] = {"bot/api/routes_api.py": "/api", "bot/api/routes_web.py": ""}
    for filename in files:
        tree = parse(filename)
        for node in ast.walk(tree):
            if isinstance(node, ast.Assign) and isinstance(node.value, ast.Call):
                if dotted(node.value.func) == "APIRouter":
                    prefix = next(
                        (
                            literal(keyword.value)
                            for keyword in node.value.keywords
                            if keyword.arg == "prefix"
                        ),
                        "",
                    )
                    prefixes[filename] = str(prefix or "")
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if not isinstance(decorator, ast.Call):
                    continue
                function = dotted(decorator.func) or ""
                method = function.rsplit(".", 1)[-1]
                if method not in {"get", "post", "put", "patch", "delete", "route", "api_route"}:
                    continue
                path = literal(decorator.args[0]) if decorator.args else None
                if not isinstance(path, str):
                    continue
                methods = literal(
                    next(
                        (
                            keyword.value
                            for keyword in decorator.keywords
                            if keyword.arg == "methods"
                        ),
                        None,
                    )
                )
                method_names = (
                    methods
                    if method in {"route", "api_route"} and isinstance(methods, list)
                    else [method.upper()]
                )
                for method_name in method_names:
                    full_path = (
                        path if filename == "bot/api/app.py" else prefixes.get(filename, "") + path
                    )
                    routes.append(
                        {
                            "id": f"{method_name} {full_path}",
                            "method": method_name,
                            "path": full_path,
                            "source": f"{filename}#{node.name}",
                        }
                    )
        if filename == "bot/api/app.py":
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call):
                    continue
                function = dotted(node.func)
                if (
                    function == "app.include_router"
                    and node.args
                    and isinstance(node.args[0], ast.Name)
                ):
                    registrations.append(
                        {
                            "operation": "include_router",
                            "name": node.args[0].id,
                            "source": "bot/api/app.py#create_app",
                        }
                    )
                if function == "app.mount" and node.args:
                    mount_path = literal(node.args[0])
                    name = next((literal(k.value) for k in node.keywords if k.arg == "name"), None)
                    registrations.append(
                        {
                            "operation": "mount",
                            "name": str(name or ""),
                            "path": str(mount_path or ""),
                            "source": "bot/api/app.py#create_app",
                        }
                    )
    return routes, registrations


def slash_inventory() -> tuple[list[dict[str, str]], list[str]]:
    commands: list[dict[str, str]] = []
    for filename in ("bot/agent/commands.py", "bot/agent/debug.py"):
        tree = parse(filename)

        def visit_class(node: ast.ClassDef, *, source_file: str = filename) -> None:
            group_name: str | None = None
            for child in ast.walk(node):
                if not isinstance(child, ast.Call):
                    continue
                if not (
                    isinstance(child.func, ast.Attribute)
                    and child.func.attr == "__init__"
                    and isinstance(child.func.value, ast.Call)
                    and dotted(child.func.value.func) == "super"
                ):
                    continue
                group_name = next(
                    (literal(keyword.value) for keyword in child.keywords if keyword.arg == "name"),
                    None,
                )
                if group_name:
                    break
            for child in node.body:
                if not isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    continue
                for decorator in child.decorator_list:
                    if (
                        not isinstance(decorator, ast.Call)
                        or dotted(decorator.func) != "app_commands.command"
                    ):
                        continue
                    name = next((k.value for k in decorator.keywords if k.arg == "name"), None)
                    name = str(literal(name) or child.name)
                    path = f"{group_name} {name}" if group_name else name
                    commands.append(
                        {
                            "path": path,
                            "source": f"{source_file}#{node.name}.{child.name}",
                        }
                    )
            for child in node.body:
                if isinstance(child, ast.ClassDef):
                    visit_class(child, source_file=source_file)

        for node in tree.body:
            if isinstance(node, ast.ClassDef):
                visit_class(node)
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if (
                    isinstance(decorator, ast.Call)
                    and dotted(decorator.func) == "app_commands.command"
                ):
                    name = next((k.value for k in decorator.keywords if k.arg == "name"), None)
                    commands.append(
                        {
                            "path": str(literal(name) or node.name),
                            "source": f"{filename}#{node.name}",
                        }
                    )

    registered: list[str] = []
    tree = parse("bot/agent/commands.py")
    group_names = {
        "FixedGroup": "fixed",
        "BotGroup": "bot",
        "MemoryGroup": "memory",
        "DebugGroup": "debug",
    }
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and dotted(node.func) == "tree.add_command" and node.args:
            arg = node.args[0]
            if isinstance(arg, ast.Call) and isinstance(arg.func, ast.Name):
                if arg.func.id in group_names:
                    registered.append(group_names[arg.func.id])
        if (
            isinstance(node, ast.For)
            and isinstance(node.target, ast.Name)
            and node.target.id == "command"
        ):
            if isinstance(node.iter, (ast.Tuple, ast.List)):
                registered.extend(
                    element.id for element in node.iter.elts if isinstance(element, ast.Name)
                )
    return commands, registered


def cli_inventory() -> tuple[list[dict[str, str]], list[str]]:
    filename = "bot/cli.py"
    tree = parse(filename)
    commands: list[dict[str, str]] = []
    groups: list[str] = []
    group_map = {
        "fixed_app.command": "fixed",
        "config_app.command": "config",
        "member_app.command": "member",
        "memory_app.command": "memory",
        "limits_app.command": "limits",
        "limits_app.callback": "limits",
        "app.command": "",
    }
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        function = dotted(node.func)
        if function == "app.add_typer" and node.args:
            name = next((literal(k.value) for k in node.keywords if k.arg == "name"), None)
            if name:
                groups.append(str(name))
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for decorator in node.decorator_list:
            if not isinstance(decorator, ast.Call):
                continue
            function = dotted(decorator.func)
            if function not in group_map:
                continue
            name_node = (
                decorator.args[0]
                if decorator.args
                else next((k.value for k in decorator.keywords if k.arg == "name"), None)
            )
            command_name = literal(name_node)
            command_name = str(command_name or node.name)
            group = group_map[function]
            path = (
                command_name
                if function == "limits_app.callback"
                else f"{group} {command_name}".strip()
            )
            commands.append(
                {
                    "path": path,
                    "source": f"{filename}#{node.name}",
                }
            )
    return commands, groups


def table_inventory() -> list[str]:
    tree = parse("bot/infrastructure/db.py")
    sql = ""
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "SCHEMA_SQL" for target in node.targets
        ):
            sql = literal(node.value) or ""
            break
    return re.findall(r"CREATE TABLE IF NOT EXISTS\s+([A-Za-z_][A-Za-z0-9_]*)", sql, re.IGNORECASE)


def settings_inventory() -> list[str]:
    tree = parse("bot/infrastructure/config.py")
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name == "Settings":
            return [
                item.target.id
                for item in node.body
                if isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name)
            ]
    return []


def worker_inventory() -> Counter[str]:
    found: Counter[str] = Counter()

    def walk(node: ast.AST, stack: tuple[str, ...], filename: str) -> None:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            stack = (*stack, node.name)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                for decorator in node.decorator_list:
                    if isinstance(decorator, ast.Call) and dotted(decorator.func) == "tasks.loop":
                        found[f"{filename}#{'.'.join(stack)}:tasks.loop"] += 1
        if isinstance(node, ast.Call):
            function = dotted(node.func) or ""
            if function.endswith("create_task"):
                found[f"{filename}#{'.'.join(stack)}:{function}"] += 1
        for child in ast.iter_child_nodes(node):
            walk(child, stack, filename)

    for filename in (
        "bot/__main__.py",
        "bot/agent/client.py",
        "bot/agent/rescan.py",
        "bot/api/server.py",
        "bot/extract/pipeline.py",
    ):
        walk(parse(filename), (), filename)
    return found


def tool_inventory() -> tuple[list[str], list[str], list[str]]:
    tree = parse("bot/chat/tools/schemas.py")
    names: list[str] = []
    for node in ast.walk(tree):
        targets = (
            node.targets
            if isinstance(node, ast.Assign)
            else [node.target]
            if isinstance(node, ast.AnnAssign)
            else []
        )
        if any(isinstance(target, ast.Name) and target.id == "TOOLS" for target in targets):
            values = node.value.elts if isinstance(node.value, (ast.List, ast.Tuple)) else []
            for item in values:
                if isinstance(item, ast.Call) and item.args:
                    value = literal(item.args[0])
                    if isinstance(value, str):
                        names.append(value)
    dispatch = parse("bot/chat/tools/dispatching.py")
    read: list[str] = []
    write: list[str] = []
    for node in ast.walk(dispatch):
        if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.Dict):
            continue
        targets = [target.id for target in node.targets if isinstance(target, ast.Name)]
        if not targets or targets[0] not in {"_READ", "_WRITE"}:
            continue
        values = [literal(key) for key in node.value.keys]
        (read if targets[0] == "_READ" else write).extend(
            value for value in values if isinstance(value, str)
        )
    return names, read, write


def portal_assets() -> list[str]:
    assets = [
        path.relative_to(V4_ROOT).as_posix()
        for path in (V4_ROOT / "bot/api/templates").rglob("*.html")
    ]
    assets += ["bot/api/static/portal.js", "bot/api/static/portal.scss"]
    assets.extend(
        path.relative_to(V4_ROOT).as_posix()
        for path in (V4_ROOT / "bot/api/static/portal").glob("*.scss")
    )
    return sorted(assets)


def expected_items(inventory: dict[str, Any]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for key in ("routes", "slash_commands", "cli_commands", "tables", "surfaces"):
        items.extend(item for item in inventory.get(key, []) if isinstance(item, dict))
    for key in ("settings", "runtime_config"):
        items.extend(
            item for item in inventory.get(key, {}).get("items", []) if isinstance(item, dict)
        )
    items.extend(item for item in inventory.get("entrypoints", []) if isinstance(item, dict))
    items.extend(item for item in inventory.get("healthchecks", []) if isinstance(item, dict))
    items.extend(
        item for item in inventory.get("env_example", {}).get("items", []) if isinstance(item, dict)
    )
    return items


def validate_dispositions(inventory: dict[str, Any], errors: list[str]) -> None:
    for item in expected_items(inventory):
        if item.get("disposition") not in DISPOSITIONS:
            errors.append(f"invalid/missing disposition for {item.get('id') or item.get('path')}")


def main() -> int:
    if not INVENTORY.is_file():
        print(f"missing inventory: {INVENTORY}", file=sys.stderr)
        return 2
    inventory = json.loads(INVENTORY.read_text(encoding="utf-8"))
    errors: list[str] = []
    validate_dispositions(inventory, errors)
    check_ref_paths(inventory, errors)

    discovered_entrypoints = discover_entrypoints(V4_ROOT)
    errors.extend(
        validate_entrypoint_inventory(
            inventory.get("entrypoints", []), discovered_entrypoints["entrypoints"]
        )
    )
    errors.extend(
        validate_healthcheck_inventory(
            inventory.get("healthchecks", []),
            discovered_entrypoints["entrypoints"],
            discovered_entrypoints["healthcheck_sources"],
        )
    )
    env_inventory = inventory.get("env_example", {})
    errors.extend(
        validate_env_inventory(
            discover_env_keys(V4_ROOT),
            env_inventory.get("items", []),
            env_inventory.get("policies", {}),
        )
    )

    discovered_routes, discovered_registrations = route_inventory()
    expected_routes = inventory.get("routes", [])
    discovered_route_ids = {item["id"] for item in discovered_routes}
    expected_route_ids = {item.get("id") for item in expected_routes}
    if discovered_route_ids != expected_route_ids:
        errors.append(
            f"route set mismatch: missing={sorted(discovered_route_ids - expected_route_ids)}, "
            f"extra={sorted(expected_route_ids - discovered_route_ids)}"
        )
    discovered_route_sources = {route["id"]: route["source"] for route in discovered_routes}
    for item in expected_routes:
        if discovered_route_sources.get(item.get("id")) != item.get("source"):
            errors.append(f"route source mismatch: {item.get('id')} -> {item.get('source')}")
        if item.get("policy") not in inventory.get("route_policies", {}):
            errors.append(f"route has no policy: {item.get('id')}")
    expected_registration_ids = {
        (item.get("operation"), item.get("name"), item.get("path"))
        for item in inventory.get("registrations", [])
    }
    discovered_registration_ids = {
        (item.get("operation"), item.get("name"), item.get("path"))
        for item in discovered_registrations
    }
    if expected_registration_ids != discovered_registration_ids:
        errors.append(
            f"registration mismatch: discovered={sorted(discovered_registration_ids)}, "
            f"expected={sorted(expected_registration_ids)}"
        )

    discovered_slash, registered_slash = slash_inventory()
    expected_slash = inventory.get("slash_commands", [])
    if {item["path"] for item in discovered_slash} != {item.get("path") for item in expected_slash}:
        errors.append("slash-command path set mismatch")
    discovered_by_path = {item["path"]: item["source"] for item in discovered_slash}
    for item in expected_slash:
        if discovered_by_path.get(item.get("path")) != item.get("source"):
            errors.append(f"slash source mismatch: {item.get('path')} -> {item.get('source')}")
    if set(inventory.get("slash_registrations", [])) != set(registered_slash):
        errors.append("slash registration mismatch")

    discovered_cli, discovered_groups = cli_inventory()
    expected_cli = inventory.get("cli_commands", [])
    if {item["path"] for item in discovered_cli} != {item.get("path") for item in expected_cli}:
        errors.append("CLI command path set mismatch")
    discovered_cli_by_path = {item["path"]: item["source"] for item in discovered_cli}
    for item in expected_cli:
        if discovered_cli_by_path.get(item.get("path")) != item.get("source"):
            errors.append(f"CLI source mismatch: {item.get('path')} -> {item.get('source')}")
    if set(discovered_groups) != set(inventory.get("cli_groups", [])):
        errors.append("CLI group registration mismatch")

    discovered_tables = table_inventory()
    expected_tables = [item.get("name") for item in inventory.get("tables", [])]
    if discovered_tables != expected_tables:
        errors.append(
            f"table order/set mismatch: discovered={discovered_tables}, expected={expected_tables}"
        )

    discovered_settings = settings_inventory()
    expected_settings = [
        item.get("field") for item in inventory.get("settings", {}).get("items", [])
    ]
    if discovered_settings != expected_settings:
        errors.append("Settings field order/set mismatch")
    expected_env = inventory.get("env_example", {}).get("items", [])

    discovered_workers = worker_inventory()
    expected_workers = Counter(
        {item["signature"]: item["count"] for item in inventory.get("workers", [])}
    )
    if discovered_workers != expected_workers:
        errors.append(
            f"worker task set mismatch: discovered={dict(discovered_workers)}, "
            f"expected={dict(expected_workers)}"
        )

    discovered_tools, discovered_read, discovered_write = tool_inventory()
    expected_tools = inventory.get("chat_tools", {}).get("names", [])
    if discovered_tools != expected_tools:
        errors.append("chat tool schema order/set mismatch")
    if set(discovered_read) != set(inventory.get("chat_tools", {}).get("read", [])):
        errors.append("chat read-tool registry mismatch")
    if set(discovered_write) != set(inventory.get("chat_tools", {}).get("write", [])):
        errors.append("chat write-tool registry mismatch")

    discovered_assets = portal_assets()
    expected_assets = inventory.get("portal_assets", {}).get("paths", [])
    if sorted(discovered_assets) != sorted(expected_assets):
        errors.append("portal asset path set/order mismatch")

    if errors:
        print("v5 inventory check FAILED", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        "v5 inventory OK: "
        f"routes={len(expected_routes)} registrations={len(discovered_registrations)} "
        f"slash={len(expected_slash)} cli={len(expected_cli)} tables={len(expected_tables)} "
        f"entrypoints={len(inventory.get('entrypoints', []))} "
        f"healthchecks={len(inventory.get('healthchecks', []))} "
        f"settings={len(expected_settings)} env={len(expected_env)} "
        f"workers={sum(expected_workers.values())} tools={len(expected_tools)} "
        f"portal_assets={len(expected_assets)} surfaces={len(inventory.get('surfaces', []))}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
