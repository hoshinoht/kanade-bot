#!/usr/bin/env python3
"""Cache public boss-guide Google Docs as plain text for local review only.

Output stays under the git-ignored data/research/boss-guides/ directory; the
cached prose must never be committed (see README.md).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import urllib.request
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = ROOT / "data" / "research" / "boss-guides"

# boss file stem -> (Google Doc id, author)
GUIDES: dict[str, tuple[str, str]] = {
    "jupiter": ("1DwlX_-lXPA6-KRKCxGZ2g6Xzwl3DPniq-BcBudOyr98", "iSIingGunz"),
    "maleficstar": ("1qGre_k2_m68imDiKPzoyEQaVv7XO0RTea0mGct-TXH4", "iSIingGunz"),
    "fa": ("1kVb7FMPvHtntji69dnfAsU025yT_hqdDM1aWVRodxXU", "iSIingGunz"),
    "baldrix": ("1uBXXBbUt86N2T67HY0dQrFjo7fbOthVrWBMWfUyi4NY", "iSIingGunz"),
    "limbo": ("1OJ9-xZvQVaXZ4p1DmhnIjYurSuK1gtUZlZdzoFk6fHA", "iSIingGunz"),
    "kai": ("1oPyMyovLuS8aPh87MP3B_vJJcbnYoXWXrLp8Q4By0BI", "iSIingGunz"),
    "lotus": ("1ozaBBT0D7rZJr_KurQYNHLdKJU7PWw437bq5auyom5c", "iSIingGunz"),
}

_MAX_BYTES = 5 * 1024 * 1024
_TIMEOUT = 30


def doc_url(doc_id: str) -> str:
    return f"https://docs.google.com/document/d/{doc_id}/edit"


def export_url(doc_id: str) -> str:
    return f"https://docs.google.com/document/d/{doc_id}/export?format=txt"


def download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "kanade-boss-knowledge/1"})
    with urllib.request.urlopen(request, timeout=_TIMEOUT) as response:
        if response.status != 200:
            raise RuntimeError(f"HTTP {response.status}")
        body = response.read(_MAX_BYTES + 1)
    if len(body) > _MAX_BYTES:
        raise RuntimeError("response exceeds 5 MiB")
    # A private/unshared doc redirects to an HTML sign-in page instead of text.
    if body.lstrip()[:15].lower().startswith((b"<!doctype", b"<html")):
        raise RuntimeError("got HTML instead of text; is the doc public?")
    return body


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bosses", nargs="*", help=f"subset of: {', '.join(GUIDES)}")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)

    selected = args.bosses or list(GUIDES)
    unknown = sorted(set(selected) - set(GUIDES))
    if unknown:
        parser.error(f"unknown boss: {', '.join(unknown)}")

    args.out.mkdir(parents=True, exist_ok=True)
    index_path = args.out / "index.json"
    index = json.loads(index_path.read_text()) if index_path.exists() else {}
    today = date.today().isoformat()
    failures = 0
    for boss in selected:
        doc_id, author = GUIDES[boss]
        try:
            body = download(export_url(doc_id))
        except Exception as exc:  # noqa: BLE001 - report and continue per doc
            print(f"FAIL {boss}: {exc}", file=sys.stderr)
            failures += 1
            continue
        text = body.decode("utf-8-sig").replace("\r\n", "\n")
        (args.out / f"{boss}.txt").write_text(text, encoding="utf-8")
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
        index[boss] = {
            "url": doc_url(doc_id),
            "export_url": export_url(doc_id),
            "author": author,
            "fetched": today,
            "sha256": digest,
            "bytes": len(text.encode("utf-8")),
        }
        print(f"ok   {boss}: {len(text)} chars sha256={digest[:12]}")
    index_path.write_text(json.dumps(index, indent=2, sort_keys=True) + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
