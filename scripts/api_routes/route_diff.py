#!/usr/bin/env python3
"""Diff the API paths the PWA calls and the pwa-mock serves against the
routes the Rust `serve` mounts (`src/api`).

Dependency-free and source-only: it parses `.route("…", get(…).post(…))`
calls and the string/template literals under `web/apps` and `web/packages`
that start with `/api/`. Exits 1 when a gap is not one of the documented
exceptions below (see docs/notes/admin-api.md "Wiring inventory"), or when
Rust mounts a route still listed as planned.

Usage (from the repository root): python3 scripts/api_routes/route_diff.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# (method, path) the mock serves that Rust deliberately does not.
MOCK_ONLY = {
    ("POST", "/api/admin/reset"): "pwa-mock e2e control, not an admin route",
    ("POST", "/csp-report"): "pwa-mock dev-only CSP report sink",
}
# Paths the web apps call that Rust deliberately does not serve.
WEB_ONLY: dict[str, str] = {}
# (method, path) contracted but not mounted yet. The web and mock may use
# them first; once Rust mounts one, its entry must go. (The member realm's
# routes, docs/notes/member-auth-contract.md, are all mounted.)
PLANNED: dict[tuple[str, str], str] = {}
# (method, path) Rust serves that the mock does not need.
RUST_ONLY = {
    ("GET", "/healthz"): "container health probe",
}
# Prefixes that are dev tooling, never product routes.
MOCK_CONTROL_PREFIXES = ("/__mock/",)

METHOD = re.compile(r"\b(get|post|patch|put|delete|any)\(")
PARAM = re.compile(r"\{[^}*]+\}")


def balanced(text: str, start: int, open_: str = "(", close: str = ")") -> int:
    """Index just past the bracket that closes the one at `start`."""
    depth = 0
    i = start
    quote = None
    while i < len(text):
        ch = text[i]
        if quote:
            if ch == "\\":
                i += 1
            elif ch == quote:
                quote = None
        elif ch == '"':
            quote = ch
        elif ch == open_:
            depth += 1
        elif ch == close:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise ValueError(f"unbalanced {open_} at {start}")


def axum_routes(files: list[Path]) -> set[tuple[str, str]]:
    """(METHOD, path) for every `.route("path", …)`; catch-alls are skipped
    because they only answer `not_found`/`closed`."""
    found: set[tuple[str, str]] = set()
    for file in files:
        text = file.read_text(encoding="utf-8")
        for match in re.finditer(r"\.route\(", text):
            open_at = match.end() - 1
            call = text[open_at : balanced(text, open_at)]
            path = re.search(r'"([^"]+)"', call)
            if not path or "{*" in path.group(1):
                continue
            handler = call[path.end() :]
            for method in METHOD.findall(handler):
                found.add((method.upper(), PARAM.sub("{}", path.group(1))))
    return found


TEMPLATE = re.compile(r"\$\{")
LITERAL = re.compile(r"""(['"`])(/api/[^'"`]*)\1""")


def web_paths() -> dict[str, set[str]]:
    """Normalised `/api/` paths the apps call, with the files naming them."""
    found: dict[str, set[str]] = {}
    roots = [ROOT / "web/apps", ROOT / "web/packages"]
    for root in roots:
        for file in root.rglob("*"):
            if file.suffix not in {".ts", ".svelte"} or any(
                part in {"node_modules", "dist", "test", "e2e"} for part in file.parts
            ):
                continue
            if file.name.endswith((".test.ts", ".spec.ts", "generated.ts", "manual.ts")):
                continue
            text = file.read_text(encoding="utf-8")
            for match in LITERAL.finditer(text):
                raw = match.group(2)
                if match.group(1) == "`":
                    raw = untemplate(text, match.start(2))
                path = raw.split("?", 1)[0]
                # A bare prefix (`'/api/admin/'`) is a guard, not a call.
                if path.endswith("/"):
                    continue
                found.setdefault(path, set()).add(str(file.relative_to(ROOT)))
    return found


def untemplate(text: str, start: int) -> str:
    """A template literal with `${…}` after `/` as `{}` and any other
    `${…}` (a query suffix) dropped."""
    out = []
    i = start
    while i < len(text) and text[i] != "`":
        if text.startswith("${", i):
            end = balanced(text, i + 1, "{", "}")
            if out and out[-1] == "/":
                out.append("{}")
            i = end
            continue
        out.append(text[i])
        i += 1
    return "".join(out)


def matches(web: str, served: str) -> bool:
    a, b = web.strip("/").split("/"), served.strip("/").split("/")
    return len(a) == len(b) and all(x == y or "{}" in (x, y) for x, y in zip(a, b, strict=True))


def main() -> int:
    rust = axum_routes(sorted((ROOT / "src/api").rglob("*.rs")))
    mock = {
        route
        for route in axum_routes([ROOT / "devtools/pwa-mock/src/main.rs"])
        if not route[1].startswith(MOCK_CONTROL_PREFIXES)
    }
    web = web_paths()
    rust_paths = {path for _, path in rust}
    planned = {(method, PARAM.sub("{}", path)): why for (method, path), why in PLANNED.items()}
    planned_paths = {path for _, path in planned}

    problems: list[str] = []
    notes: list[str] = []
    for method, path in sorted(planned):
        if (method, path) in rust:
            problems.append(f"Rust mounts {method} {path}: drop it from PLANNED")
        else:
            notes.append(f"planned {method} {path}  (not yet mounted: {planned[(method, path)]})")
    for path in sorted(web):
        if any(matches(path, served) for served in rust_paths | planned_paths):
            continue
        if path in WEB_ONLY:
            notes.append(f"web  {path}  ({WEB_ONLY[path]})")
        else:
            callers = ", ".join(sorted(web[path]))
            problems.append(f"web calls {path} but Rust serves no such route ({callers})")
    for method, path in sorted(mock - rust - planned.keys()):
        if (method, path) in MOCK_ONLY:
            notes.append(f"mock {method} {path}  ({MOCK_ONLY[(method, path)]})")
        else:
            problems.append(f"mock serves {method} {path} but Rust does not")
    for method, path in sorted(rust - mock):
        if (method, path) in RUST_ONLY:
            notes.append(f"rust {method} {path}  ({RUST_ONLY[(method, path)]})")
        else:
            problems.append(f"Rust serves {method} {path} but the pwa-mock does not")

    print(f"{len(rust)} Rust routes, {len(mock)} mock routes, {len(web)} web paths")
    for line in notes:
        print(f"documented: {line}")
    for line in problems:
        print(f"GAP: {line}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
