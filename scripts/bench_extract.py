"""Compare extractor models over ``tests/fixtures/extract/*.json``.

Runs the *same* fixtures the ``pytest -m ollama`` suite runs, judged by the *same*
code (``tests/fixture_loader.score`` -> ``bot.extract.pipeline.plan_burst``), across
one or more (model, think) configurations, and reports pass/fail, burst latency,
JSON-schema retries and resident memory.

    uv run python scripts/bench_extract.py \\
        --model gpt-oss:20b --think low --think medium \\
        --model 'hf.co/.../Gemma4-12B-...:Q4_K_M' --think off \\
        --reps 2 --md data/bench/latest.md

``--think`` applies to the most recent ``--model``.  Models run strictly
sequentially -- every config and rep of model A, then ``ollama stop`` it, then
model B -- because ``llm.py`` pins ``keep_alive=-1`` and two of these will not fit
in 24 GB together.  Pass ``--no-unload`` to leave the last model resident.

Nothing here writes to the database, Discord, or ``.env``; the model and think
level are overridden on a locally constructed :class:`Settings`.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    # `tests` is not part of the installed wheel (hatch packages only `bot`), and a
    # script's sys.path[0] is `scripts/`, so the repo root has to go on explicitly.
    sys.path.insert(0, str(REPO_ROOT))

from bot.bosses import BossTable  # noqa: E402
from bot.config import Settings  # noqa: E402
from bot.extract import prompt as prompt_mod  # noqa: E402
from bot.extract.llm import Extractor  # noqa: E402
from tests import fixture_loader as fl  # noqa: E402

#: The host runs Ollama natively; the container's `host.docker.internal` is wrong here.
DEFAULT_HOST = "http://127.0.0.1:11434"

#: Thinking levels Ollama accepts on a *thinking-capable* model (docs/capabilities/
#: thinking.mdx, and `api.ThinkValue`).  A model without the `thinking` capability
#: rejects every one of these *and* `think=true` with a 400; only `false`/omitted works.
THINK_LEVELS = ("low", "medium", "high", "max")
THINK_OFF_WORDS = ("off", "false", "no", "0")
THINK_ON_WORDS = ("on", "true", "yes", "1")
#: Send no `think` key at all -- which is *not* the same as off.  Omitting it leaves
#: the model's own default in force: the Gemma4 build here then emits a reasoning
#: preamble that Ollama splits into `message.thinking`, and `message.content` can come
#: back empty (which `parse_response` rejects).  `think=false` suppresses that.
#: `Settings.think` returns None for OLLAMA_THINK=off, so the bot sends "default".
THINK_DEFAULT_WORDS = ("default", "omit", "unset")

_SIZE_RE = re.compile(r"(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)\b", re.IGNORECASE)


# ---------------------------------------------------------------------------
# settings: model / think overridden for this process only
# ---------------------------------------------------------------------------


class BenchSettings(Settings):
    """:class:`Settings` whose ``think`` may be any value Ollama accepts.

    ``Settings.ollama_think`` is validated down to ``low|medium|high|off`` because
    that is all the bot ever needs.  A benchmark also has to be able to send
    ``true``/``false``/``max`` verbatim, so ``think`` is overridden here rather than
    by loosening the real config (or, worse, by editing ``.env``).
    """

    think_value: str | bool | None = None

    @property
    def think(self) -> str | bool | None:
        return self.think_value


def build_settings(model: str, think_value: str | bool | None, host: str, timeout: float):
    """A Settings for one config.  Explicit kwargs beat anything in ``.env``."""
    return BenchSettings(
        discord_token="unused",
        guild_id=1,
        bossing_role_id=1,
        chat_channel_ids="1",
        ollama_host=host,
        ollama_model=model,
        ollama_timeout=timeout,
        ollama_think="off",  # unused: `think` below is what reaches the client
        think_value=think_value,
    )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


@dataclass
class ModelSpec:
    model: str
    thinks: list[str] = field(default_factory=list)


class _ModelAction(argparse.Action):
    def __call__(self, parser, namespace, value, option_string=None):
        specs = getattr(namespace, "specs", None) or []
        specs.append(ModelSpec(model=value))
        namespace.specs = specs


class _ThinkAction(argparse.Action):
    def __call__(self, parser, namespace, value, option_string=None):
        specs = getattr(namespace, "specs", None) or []
        if not specs:
            parser.error("--think must come after a --model")
        specs[-1].thinks.append(value)
        namespace.specs = specs


def parse_think(value: str) -> tuple[str, str | bool | None]:
    """``"low"`` -> ``("low", "low")``, ``"off"`` -> ``("off", False)``.

    Returns ``(label, value_to_send)``; the label is what the report shows.  ``off``
    sends ``false`` (thinking actively suppressed); ``default`` sends nothing at all,
    which is what the bot does today for ``OLLAMA_THINK=off``.
    """
    key = value.strip().lower()
    if key in THINK_DEFAULT_WORDS:
        return "default", None
    if key in THINK_OFF_WORDS:
        return "off", False
    if key in THINK_ON_WORDS:
        return "on", True
    if key in THINK_LEVELS:
        return key, key
    raise ValueError(
        f"unknown --think {value!r}: use {', '.join(THINK_LEVELS)}, on, off or default"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python scripts/bench_extract.py",
        description="Score extractor models against tests/fixtures/extract/*.json.",
        epilog="--think applies to the --model before it.",
    )
    parser.add_argument(
        "--model", action=_ModelAction, metavar="NAME", help="an Ollama model (repeatable)"
    )
    parser.add_argument(
        "--think",
        action=_ThinkAction,
        metavar="LEVEL",
        help="low|medium|high|max|on|off|default for the preceding --model "
        "(repeatable; default off). off sends think=false; `default` omits the key",
    )
    parser.add_argument("--reps", type=int, default=2, metavar="N", help="runs per fixture")
    parser.add_argument(
        "--fixture",
        action="append",
        default=[],
        metavar="NAME",
        help="only this fixture (repeatable; default all)",
    )
    parser.add_argument("--out", metavar="PATH", help="JSON results (default data/bench/<ts>.json)")
    parser.add_argument("--md", metavar="PATH", help="markdown summary (default: alongside --out)")
    parser.add_argument("--host", default=DEFAULT_HOST, metavar="URL", help=f"({DEFAULT_HOST})")
    parser.add_argument(
        "--timeout",
        type=float,
        default=120.0,
        metavar="SECONDS",
        help="per-call timeout; 120 matches the pytest suite",
    )
    parser.add_argument(
        "--no-unload",
        action="store_true",
        help="do not `ollama stop` a model after its block (leaves it resident)",
    )
    parser.add_argument(
        "--no-warmup",
        action="store_true",
        help="skip the unrecorded first call per config (see `warmup_ms` in the JSON)",
    )
    return parser


# ---------------------------------------------------------------------------
# talking to the ollama host / process table
# ---------------------------------------------------------------------------


def model_capabilities(host: str, model: str) -> tuple[list[str], str | None]:
    """``POST /api/show`` -> the model's capability list (``thinking``, ``vision``, ...)."""
    request = urllib.request.Request(  # noqa: S310 - a local http:// host from --host
        host.rstrip("/") + "/api/show",
        data=json.dumps({"model": model}).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
            return list(json.load(response).get("capabilities") or []), None
    except Exception as exc:  # noqa: BLE001 - a probe must never abort the run
        return [], f"{type(exc).__name__}: {exc}"


def _run(argv: list[str], timeout: float = 30.0) -> tuple[str, str | None]:
    try:
        done = subprocess.run(argv, capture_output=True, text=True, timeout=timeout, check=False)
    except (OSError, subprocess.SubprocessError) as exc:
        return "", f"{type(exc).__name__}: {exc}"
    return done.stdout, (done.stderr.strip() or None)


def ollama_ps(model: str | None = None) -> dict[str, Any]:
    """``ollama ps``: the whole table, plus the SIZE column of ``model``'s row."""
    out, error = _run(["ollama", "ps"])
    size = None
    for line in out.splitlines()[1:]:
        if model is not None and model not in line:
            continue
        # The NAME column can itself look like a size ("Gemma4-12B-..." -> "12 B"),
        # so take the model name out of the line before looking for the SIZE column.
        found = _SIZE_RE.search(line.replace(model, " ", 1) if model else line)
        if found:
            size = f"{found.group(1)} {found.group(2).upper()}"
            break
    return {"size": size, "raw": out.strip() or None, "error": error}


def runner_rss() -> dict[str, Any]:
    """RSS of the model-runner process(es) -- ``ps -axo rss,command``, grepped here.

    Ollama 0.33 on macOS runs the weights in a ``llama-server`` child of ``ollama
    serve``; older builds spawn ``ollama runner``.  Both are matched, because either
    one -- not the small ``serve`` parent -- is where the resident gigabytes live.

    Matching is on the *executable*, not on the line: other tools ship a
    ``llama-server`` of their own (this Mac runs one for ``opencode-recall``'s
    embeddings), and any process whose arguments merely mention Ollama -- this
    script's own command line included -- would otherwise be counted as gigabytes.
    """
    out, error = _run(["ps", "-axo", "rss,command"])
    processes: list[dict[str, Any]] = []
    total_kb = 0
    for line in out.splitlines():
        head, _, command = line.strip().partition(" ")
        if not head.isdigit() or not command.strip():
            continue
        argv = command.split()
        exe = argv[0].lower()
        name = exe.rsplit("/", 1)[-1]
        is_runner = name == "llama-server" and "ollama" in exe
        is_runner |= name == "ollama" and len(argv) > 1 and argv[1] == "runner"
        if not is_runner:
            continue
        total_kb += int(head)
        processes.append({"rss_kb": int(head), "command": command.strip()[:200]})
    return {
        "total_rss_kb": total_kb,
        "total_rss_gb": round(total_kb / 1024 / 1024, 2) if total_kb else None,
        "processes": processes,
        "error": error,
    }


def ollama_stop(model: str) -> dict[str, Any]:
    out, error = _run(["ollama", "stop", model], timeout=120.0)
    return {"stdout": out.strip() or None, "error": error}


# ---------------------------------------------------------------------------
# one fixture, one call
# ---------------------------------------------------------------------------


async def run_one(
    extractor: Extractor,
    scenario: fl.Scenario,
    table: BossTable,
    min_confidence: float,
) -> dict[str, Any]:
    """One model call, scored exactly the way ``tests/test_extract_fixtures.py`` scores it."""
    row: dict[str, Any] = {
        "fixture": scenario.name,
        "passed": False,
        "reason": "",
        "latency_ms": None,
        "attempts": 0,
        "schema_retries": 0,
        "error": None,
        "got": [],
        "summary": None,
    }
    messages = prompt_mod.build_messages(scenario.context_for(table))
    started = time.monotonic()
    try:
        call = await extractor.extract(messages)
    except Exception as exc:  # noqa: BLE001 - `extract` shouldn't raise, but a bench survives it
        row["latency_ms"] = int((time.monotonic() - started) * 1000)
        row["error"] = f"{type(exc).__name__}: {exc}"
        row["reason"] = f"call raised {row['error']}"
        return row

    # `attempts == 2` is exactly the "extraction did not validate; retrying once" path.
    row["latency_ms"] = call.latency_ms
    row["attempts"] = call.attempts
    row["schema_retries"] = max(call.attempts - 1, 0)
    row["error"] = call.error
    if not call.ok:
        row["reason"] = f"model error: {call.error}"
        return row

    try:
        score = fl.score(scenario, call.extraction, min_confidence=min_confidence)
    except Exception as exc:  # noqa: BLE001 - a broken pipeline is a result, not a crash
        row["reason"] = f"scoring raised {type(exc).__name__}: {exc}"
        return row
    row["passed"] = score.passed
    row["reason"] = score.reason()
    row["got"] = [actual.describe(scenario.tz) for actual in score.actuals]
    row["summary"] = call.extraction.summary
    return row


# ---------------------------------------------------------------------------
# the matrix
# ---------------------------------------------------------------------------


async def run_bench(
    specs: list[ModelSpec],
    scenarios: list[fl.Scenario],
    table: BossTable,
    args: argparse.Namespace,
) -> dict[str, Any]:
    """Every config, model by model.  One event loop for all of it.

    The httpx client under ``Extractor`` belongs to the loop that created it, so the
    whole matrix runs inside a single :func:`asyncio.run` -- same reason the pytest
    fixture does.
    """
    results: dict[str, Any] = {
        "started_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "host": args.host,
        "reps": args.reps,
        "timeout": args.timeout,
        "fixtures": [s.name for s in scenarios],
        "models": [],
        "rows": [],
    }

    for spec in specs:
        capabilities, capability_error = model_capabilities(args.host, spec.model)
        thinks = spec.thinks or ["off"]
        entry: dict[str, Any] = {
            "model": spec.model,
            "capabilities": capabilities,
            "capabilities_error": capability_error,
            "ps_before_model": ollama_ps(spec.model),
            "ps_after_first_burst": None,
            "runner_rss_after_first_burst": None,
            "configs": [],
            "unloaded": None,
        }
        results["models"].append(entry)
        first_burst_recorded = False

        for raw_think in thinks:
            try:
                label, requested = parse_think(raw_think)
            except ValueError as exc:
                entry["configs"].append({"think": raw_think, "skipped": str(exc)})
                print(f"!! {spec.model}: {exc}", file=sys.stderr)
                continue

            # A model without the `thinking` capability 400s on any think level *and*
            # on `think=true`; the only value it accepts is off.  Map instead of
            # burning the whole block on identical errors, and say so in the report.
            sent: str | bool | None = requested
            note = ""
            if requested not in (None, False) and "thinking" not in capabilities:
                sent = False
                note = (
                    f"{spec.model} has no `thinking` capability; "
                    f"requested think={label!r}, sent think=false"
                )
                print(f"!! {note}", file=sys.stderr)

            config_id = f"{spec.model} | think={label}"
            config: dict[str, Any] = {
                "config_id": config_id,
                "model": spec.model,
                "think_requested": label,
                "think_sent": sent,
                "note": note,
                "errors": 0,
            }
            entry["configs"].append(config)
            results.setdefault("configs", []).append(config)

            settings = build_settings(spec.model, sent, args.host, args.timeout)
            extractor = Extractor(settings, host=args.host)
            print(f"\n=== {config_id} ===", file=sys.stderr)

            # `entry`/`model` are bound as defaults: this closure is only ever called
            # within the iteration that made it, and ruff (B023) should not have to
            # take that on trust.
            def record_first_burst(entry: dict[str, Any] = entry, model: str = spec.model) -> None:
                nonlocal first_burst_recorded
                if first_burst_recorded:
                    return
                snapshot = ollama_ps(model)
                # An empty `ollama ps` means the model is mid-swap (something else took
                # the GPU). That is not a memory reading -- leave the slot unfilled and
                # try again after the next burst rather than recording a blank.
                entry["ps_after_first_burst"] = snapshot
                entry["runner_rss_after_first_burst"] = runner_rss()
                first_burst_recorded = snapshot["size"] is not None

            try:
                # One unrecorded call first. `llm.py` pins num_ctx=8192, so if the model
                # is already resident at a different context length Ollama *reloads* the
                # weights on the first call -- 78 s instead of ~15 s on this Mac. Timing
                # that would measure a disk read, not the model.
                if not args.no_warmup:
                    warm = await run_one(
                        extractor, scenarios[0], table, settings.extract_min_confidence
                    )
                    config["warmup_ms"] = warm["latency_ms"]
                    config["warmup_error"] = warm["error"]
                    print(
                        f"  warmup {scenarios[0].name} {(warm['latency_ms'] or 0) / 1000:.1f}s"
                        + (f" !! {warm['error']}" if warm["error"] else ""),
                        file=sys.stderr,
                    )
                    record_first_burst()

                for rep in range(1, args.reps + 1):
                    for scenario in scenarios:
                        row = await run_one(
                            extractor, scenario, table, settings.extract_min_confidence
                        )
                        row.update(
                            config_id=config_id,
                            model=spec.model,
                            think_requested=label,
                            think_sent=sent,
                            rep=rep,
                        )
                        results["rows"].append(row)
                        if row["error"]:
                            config["errors"] += 1
                        mark = "PASS" if row["passed"] else "FAIL"
                        print(
                            f"  rep{rep} {row['fixture']:<26} {mark} "
                            f"{(row['latency_ms'] or 0) / 1000:6.1f}s  {row['reason'][:80]}",
                            file=sys.stderr,
                        )
                        record_first_burst()
            finally:
                await extractor.close()

        if not args.no_unload:
            entry["unloaded"] = ollama_stop(spec.model)
            print(f"--- unloaded {spec.model}", file=sys.stderr)

    results["finished_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
    return results


# ---------------------------------------------------------------------------
# reporting
# ---------------------------------------------------------------------------


def _resident(entry: dict[str, Any]) -> str:
    after = entry.get("ps_after_first_burst") or {}
    rss = entry.get("runner_rss_after_first_burst") or {}
    size = after.get("size") or (entry.get("ps_before_model") or {}).get("size") or "?"
    gb = rss.get("total_rss_gb")
    return f"{size} (runner RSS {gb} GB)" if gb else str(size)


def render_markdown(results: dict[str, Any]) -> str:
    configs = results.get("configs", [])
    fixtures = results["fixtures"]
    reps = results["reps"]
    by_key: dict[tuple[str, str, int], dict[str, Any]] = {
        (row["config_id"], row["fixture"], row["rep"]): row for row in results["rows"]
    }
    entry_of = {entry["model"]: entry for entry in results["models"]}
    short = {config["config_id"]: f"C{n}" for n, config in enumerate(configs, start=1)}

    lines = [
        "# extractor benchmark",
        "",
        f"- run: {results['started_at']} -> {results.get('finished_at', '?')}",
        f"- host: `{results['host']}` · reps: {reps} · fixtures: {len(fixtures)}"
        f" · timeout: {results['timeout']:.0f}s",
        "- scored by `tests/fixture_loader.score` -- the same code `pytest -m ollama` uses",
        "- each config's first call is an unrecorded warm-up (`warmup_ms` in the JSON):"
        " `llm.py` pins `num_ctx=8192`, so a model resident at another context length"
        " reloads its weights on the first call",
        "",
        "## configs",
        "",
        "| key | model | think requested | think sent | note |",
        "| --- | --- | --- | --- | --- |",
    ]
    for config in configs:
        lines.append(
            f"| {short[config['config_id']]} | `{config['model']}` | "
            f"{config['think_requested']} | `{config['think_sent']}` | {config['note'] or '-'} |"
        )

    lines += [
        "",
        "## fixtures",
        "",
        "| fixture | " + " | ".join(short[c["config_id"]] for c in configs) + " |",
        "| --- | " + " | ".join("---" for _ in configs) + " |",
    ]
    for fixture in fixtures:
        cells = []
        for config in configs:
            marks = ""
            for rep in range(1, reps + 1):
                row = by_key.get((config["config_id"], fixture, rep))
                marks += "·" if row is None else ("✅" if row["passed"] else "❌")
            cells.append(marks)
        lines.append(f"| `{fixture}` | " + " | ".join(cells) + " |")

    lines += [
        "",
        "## totals",
        "",
        "| key | passes | mean burst | max burst | resident | schema retries | errors |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for config in configs:
        rows = [r for r in results["rows"] if r["config_id"] == config["config_id"]]
        latencies = [r["latency_ms"] for r in rows if r["latency_ms"]]
        passes = sum(1 for r in rows if r["passed"])
        mean = f"{statistics.mean(latencies) / 1000:.1f}s" if latencies else "-"
        peak = f"{max(latencies) / 1000:.1f}s" if latencies else "-"
        lines.append(
            f"| {short[config['config_id']]} | {passes}/{len(fixtures) * reps} | {mean} | {peak} "
            f"| {_resident(entry_of[config['model']])} "
            f"| {sum(r['schema_retries'] for r in rows)} | {config['errors']} |"
        )

    failures = [r for r in results["rows"] if not r["passed"]]
    if failures:
        lines += ["", "## failures", ""]
        for row in failures:
            lines.append(
                f"- `{row['fixture']}` {short[row['config_id']]} rep{row['rep']}: {row['reason']}"
            )
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# entry point
# ---------------------------------------------------------------------------


def select_scenarios(wanted: list[str]) -> list[fl.Scenario]:
    scenarios = [fl.load(path) for path in fl.fixture_paths()]
    if not wanted:
        return scenarios
    keys = {w.strip().lower() for w in wanted}
    chosen = [s for s in scenarios if s.name.lower() in keys]
    missing = keys - {s.name.lower() for s in scenarios}
    if missing:
        raise SystemExit(
            f"no such fixture(s): {', '.join(sorted(missing))}\n"
            f"available: {', '.join(s.name for s in scenarios)}"
        )
    return chosen


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    specs: list[ModelSpec] = getattr(args, "specs", None) or []
    if not specs:
        print("give at least one --model", file=sys.stderr)
        return 2
    if args.reps < 1:
        print("--reps must be at least 1", file=sys.stderr)
        return 2

    scenarios = select_scenarios(args.fixture)
    table = BossTable.load(REPO_ROOT / "config" / "bosses.yaml")

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    out_path = Path(args.out) if args.out else REPO_ROOT / "data" / "bench" / f"{stamp}.json"
    md_path = Path(args.md) if args.md else out_path.with_suffix(".md")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    md_path.parent.mkdir(parents=True, exist_ok=True)

    results = asyncio.run(run_bench(specs, scenarios, table, args))
    out_path.write_text(json.dumps(results, indent=2, default=str), encoding="utf-8")
    markdown = render_markdown(results)
    md_path.write_text(markdown, encoding="utf-8")

    print(markdown)
    print(f"json: {out_path}\nmd:   {md_path}", file=sys.stderr)
    return 0 if all(row["passed"] for row in results["rows"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())
