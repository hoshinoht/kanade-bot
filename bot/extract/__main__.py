"""``python -m bot.extract`` -- run the extractor over an exported channel, offline.

Reads a JSONL file written by ``python -m bot.export``, groups it into bursts the
way the live pipeline would, and prints what each burst produced.  It talks to
Ollama and nothing else: no Discord connection, no posting, and no database
writes unless ``--record`` is given.

    uv run python -m bot.extract --file data/exports/hstar-....jsonl \\
        --since 2026-08-28 --host http://127.0.0.1:11434

This is the prompt-tuning loop: change ``bot/extract/prompt.py``, re-run, read
the diff in the output.  ``tests/fixtures/extract/*.json`` are the regression
version of the same thing (``uv run pytest -m ollama``).
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
from collections.abc import Iterator
from datetime import UTC, datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

from bot.domain.bosses import BossTable
from bot.domain.ids import short_id
from bot.infrastructure.config import Settings, get_settings
from bot.infrastructure.db import Repo

from . import gate
from . import prompt as prompt_mod
from .llm import Extractor
from .merge import merge
from .resolve import resolve

log = logging.getLogger("bot.extract")

#: Silence that ends a burst when replaying an export.  The live debounce is 90 s;
#: history is replayed with a wider gap so a whole evening's planning arrives as
#: one burst rather than a dozen single messages.
DEFAULT_BURST_GAP = 900


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m bot.extract",
        description="Dry-run the chat extractor over an exported channel. No Discord.",
    )
    parser.add_argument("--file", required=True, metavar="PATH", help="a .jsonl from bot.export")
    parser.add_argument("--since", metavar="WHEN", help="YYYY-MM-DD or ISO timestamp")
    parser.add_argument("--until", metavar="WHEN", help="YYYY-MM-DD or ISO timestamp")
    parser.add_argument(
        "--burst-gap",
        type=int,
        default=DEFAULT_BURST_GAP,
        metavar="SECONDS",
        help=f"silence that ends a burst (default {DEFAULT_BURST_GAP})",
    )
    parser.add_argument("--host", metavar="URL", help="override OLLAMA_HOST (host: 127.0.0.1)")
    parser.add_argument("--model", metavar="NAME", help="override OLLAMA_MODEL")
    parser.add_argument("--limit", type=int, metavar="N", help="stop after N bursts")
    parser.add_argument(
        "--record",
        metavar="PATH",
        help="write the extractions to this SQLite file (default: nothing is written)",
    )
    parser.add_argument("--json", action="store_true", help="print raw model JSON as well")
    return parser


# ---------------------------------------------------------------------------
# reading the export
# ---------------------------------------------------------------------------


def read_jsonl(path: Path) -> list[dict]:
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    rows.sort(key=lambda r: r["created_at"])
    return rows


def parse_when(text: str, tz: ZoneInfo) -> datetime:
    parsed = datetime.fromisoformat(text.strip())
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=tz)
    return parsed.astimezone(UTC)


def to_msg(row: dict) -> prompt_mod.Msg:
    return prompt_mod.Msg(
        id=str(row["id"]),
        author_id=str(row["author_id"]),
        author_name=row.get("author_name") or str(row["author_id"]),
        created_at=datetime.fromisoformat(row["created_at"]),
        content=row.get("content") or "",
    )


def bursts(rows: list[dict], gap_seconds: int) -> Iterator[list[dict]]:
    """Split a channel's history wherever it went quiet for ``gap_seconds``."""
    gap = timedelta(seconds=gap_seconds)
    current: list[dict] = []
    previous: datetime | None = None
    for row in rows:
        when = datetime.fromisoformat(row["created_at"])
        if previous is not None and when - previous > gap and current:
            yield current
            current = []
        current.append(row)
        previous = when
    if current:
        yield current


def roster_from(rows: list[dict]) -> list[dict]:
    """Everyone who spoke in the export, as roster rows."""
    seen: dict[str, str] = {}
    for row in rows:
        seen.setdefault(str(row["author_id"]), row.get("author_name") or str(row["author_id"]))
    return [
        {"user_id": uid, "display_name": name, "nickname": None, "aliases": [], "has_role": True}
        for uid, name in seen.items()
    ]


# ---------------------------------------------------------------------------
# the run
# ---------------------------------------------------------------------------


async def run(args: argparse.Namespace, settings: Settings, table: BossTable) -> int:
    tz = settings.zoneinfo
    path = Path(args.file)
    if not path.exists():
        print(f"no such file: {path}", file=sys.stderr)
        return 2

    rows = read_jsonl(path)
    rows = [r for r in rows if not r.get("author_bot")]
    if args.since:
        since = parse_when(args.since, tz)
        rows = [r for r in rows if datetime.fromisoformat(r["created_at"]) >= since]
    if args.until:
        until = parse_when(args.until, tz)
        rows = [r for r in rows if datetime.fromisoformat(r["created_at"]) < until]
    if not rows:
        print("nothing in that window")
        return 0

    roster = roster_from(rows)
    roster_ids = [m["user_id"] for m in roster]
    channel_name = rows[0].get("channel_name") or ""
    repo = Repo(args.record) if args.record else None
    extractor = Extractor(settings, host=args.host)

    considered = extracted = amendments = 0
    try:
        for index, burst_rows in enumerate(bursts(rows, args.burst_gap), start=1):
            if args.limit and extracted >= args.limit:
                break
            results = [gate.evaluate(r.get("content") or "", table, roster_ids) for r in burst_rows]
            gated = [row for row, res in zip(burst_rows, results, strict=True) if res.hit]
            considered += 1
            if not gated or not gate.should_extract([r for r in results if r.hit]):
                continue

            burst = [to_msg(r) for r in gated]
            context = [to_msg(r) for r in burst_rows if r not in gated]
            prompt_context = prompt_mod.PromptContext(
                tz=tz,
                table=table,
                burst=burst,
                context=context[-settings.extract_context_messages :],
                roster=roster,
                channel_name=channel_name,
            )
            messages = prompt_mod.build_messages(prompt_context)
            call = await extractor.extract(messages)
            extracted += 1

            print(f"\n{'=' * 78}\nBURST {index} — {len(burst)} message(s)")
            for msg in burst:
                print("  " + prompt_mod.render_message(msg, tz))
            if not call.ok:
                print(f"  !! model error: {call.error}")
                continue

            merged = merge(call.extraction.amendments, [m.id for m in burst])
            amendments += len(merged)
            print(f"  -> {call.latency_ms} ms · {call.extraction.summary or '(no summary)'}")
            for amendment in merged:
                when = resolve(amendment.day_ref, amendment.time_ref, burst[-1].created_at, tz)
                stamp = (
                    when.at.strftime("%Y-%m-%d %H:%M")
                    if when.at
                    else (str(when.day) if when.day else "TBD")
                )
                print(
                    f"     {amendment.kind:6} {'+'.join(amendment.bosses) or '-':28} "
                    f"{stamp:16} conf {amendment.confidence:.2f}"
                    + ("  question" if amendment.is_question else "")
                    + (f"  rsvp={amendment.rsvp}" if amendment.rsvp else "")
                )
            if args.json:
                print("  " + call.raw)
            if repo is not None:
                extraction_id = repo.log_extraction(
                    model=settings.ollama_model,
                    prompt=call.prompt,
                    raw_response=call.raw,
                    latency_ms=call.latency_ms,
                    message_ids=[m.id for m in burst],
                )
                print(f"     recorded as extraction #{short_id(extraction_id)}")
    finally:
        await extractor.close()
        if repo is not None:
            repo.close()

    print(
        f"\n{len(rows)} message(s), {considered} burst(s), {extracted} sent to the model, "
        f"{amendments} amendment(s)."
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(
        level=logging.WARNING, format="%(levelname)-8s %(name)s: %(message)s", stream=sys.stderr
    )
    args = build_parser().parse_args(argv)
    settings = get_settings()
    if args.model:
        settings = settings.model_copy(update={"ollama_model": args.model})
    table = BossTable.load(settings.bosses_path)
    return asyncio.run(run(args, settings, table))


if __name__ == "__main__":
    raise SystemExit(main())
