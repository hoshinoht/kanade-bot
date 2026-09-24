#!/usr/bin/env python3
"""Identifier inventory over every locked dependency package license.

Reads docs/v5/stack-evidence/cargo-metadata.json (written with
`cargo metadata --locked --format-version 1`) and lists, for each package
whose license expression contains an identifier outside the common
permissive set, the package's ORIGINAL license expression verbatim.

This is an inventory aid, not a policy proof: it does not parse SPDX
operator semantics, so an OR-choice is never auto-approved here. Every
non-common identifier must have a manually recorded note below, or the
script exits 1 for an owner decision.
"""
import json
import re
import sys
from pathlib import Path

COMMON = {
    "MIT",
    "Apache-2.0",
    "ISC",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "OpenSSL",
    "Zlib",
    "CC0-1.0",
    "Unlicense",
    "MIT-0",
    "BSL-1.0",
    "LLVM-exception",
}
# Manually recorded notes. Each entry was read by a human from the cited
# expression; the script only checks the identifier inventory is unchanged.
NOTES = {
    "CDLA-Permissive-2.0": (
        "permissive data license on Mozilla-root-derived certificate data "
        "(webpki-roots*, webpki-root-certs); no copyleft obligation on the binary"
    ),
    "LGPL-2.1-or-later": (
        "appears only inside r-efi's 'MIT OR Apache-2.0 OR LGPL-2.1-or-later', "
        "so a permissive choice is available; r-efi itself is lockfile-only "
        "for non-host (wasi) targets and absent from the host/Linux build graphs"
    ),
}
KEYWORDS = {"AND", "OR", "WITH"}

EVIDENCE = Path(__file__).resolve().parents[3] / "docs" / "v5" / "stack-evidence"
metadata = json.loads((EVIDENCE / "cargo-metadata.json").read_text())

rows: list[str] = []
undecided: dict[str, list[str]] = {}
for package in sorted(metadata["packages"], key=lambda p: p["name"].lower()):
    license = package.get("license")
    if not license:
        undecided.setdefault("(missing license field)", []).append(
            f"{package['name']} {package['version']}"
        )
        continue
    identifiers = set(re.findall(r"[A-Za-z0-9][A-Za-z0-9._+\-]*", license)) - KEYWORDS
    if identifiers - COMMON:
        rows.append(f"{package['name']} {package['version']}: {license}")
        for identifier in sorted(identifiers - COMMON):
            if identifier not in NOTES:
                undecided.setdefault(identifier, []).append(
                    f"{package['name']} {package['version']}"
                )

lines = [f"packages={len(metadata['packages'])}", "", "non-common license expressions (verbatim):"]
lines += [f"  {row}" for row in rows] or ["  (none)"]
lines += ["", "manual notes:"]
lines += [f"  {identifier}: {note}" for identifier, note in sorted(NOTES.items())]
lines += ["", "identifiers without a manual note (require owner decision):"]
lines += [f"  {identifier}: {', '.join(owners)}" for identifier, owners in sorted(undecided.items())] or ["  (none)"]
(EVIDENCE / "license-review.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
sys.exit(1 if undecided else 0)
