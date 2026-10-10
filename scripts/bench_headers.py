"""Benchmark header rewriting with small/fast models.

Tests whether local models can reliably apply a persona to code-generated
schedule headers (daily pings, countdowns) without hallucinating extra content.

    python3 scripts/bench_headers.py --reps 3

Models run one at a time: pull → test → stop → next.  AFM (``fm respond``)
runs first since it needs no Ollama model slot.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# ── models ──────────────────────────────────────────────────────────────────

OLLAMA_MODELS = [
    "gemma4:e2b",
    "gemma4:e4b",
    "qwen3:1.7b",
    "qwen3:4b",
    "phi4-mini",
]

# Models known to support thinking (send think=false to suppress reasoning).
THINKING_MODELS = {"qwen3:1.7b", "qwen3:4b"}

# ── test headers ────────────────────────────────────────────────────────────

TEST_HEADERS = [
    "📅 Today — Mon 21 Sep",
    "⏰ HMaleficStar in 1h (23:00) — @Jonas lau @huihui @hoshi",
    "📅 Next Week — Sun 27 Sep",
]

# Expected tokens per header for quality scoring.
EXPECTED_DATES = ["Mon 21 Sep", "23:00", "Sun 27 Sep"]
EXPECTED_EMOJIS = ["📅", "⏰", "📅"]
# Only header index 1 has mentions.
EXPECTED_MENTIONS = [[], ["@Jonas lau", "@huihui", "@hoshi"], []]

# Boss/run/time words that should NOT appear in a header rewrite.
HALLUCINATION_WORDS = re.compile(
    r"\b(boss|run|card|schedule|slot|party|bossing|weekly|reset"
    r"|HLimb|HBald|HKalos|NMalefic|NCarling|XKalos"
    r"|hlimb|hbald|hkalos|nmalefic|ncarling|xkalos)\b",
    re.IGNORECASE,
)


# ── quality scoring ────────────────────────────────────────────────────────


@dataclass
class Quality:
    date_ok: bool = False
    emoji_ok: bool = False
    mentions_ok: bool = True  # default True for headers without mentions
    hallucination: bool = False
    deviation: bool = False  # multi-line, too long, wrong language, etc.
    note: str = ""

    @property
    def passed(self) -> bool:
        return self.date_ok and self.emoji_ok and self.mentions_ok and not self.hallucination and not self.deviation

    def grade(self) -> str:
        if self.passed:
            return "✅"
        parts = []
        if not self.date_ok:
            parts.append("date")
        if not self.emoji_ok:
            parts.append("emoji")
        if not self.mentions_ok:
            parts.append("mentions")
        if self.hallucination:
            parts.append("halluc")
        if self.deviation:
            parts.append("deviate")
        return "❌ " + "+".join(parts)


def score_result(output: str, header_idx: int) -> Quality:
    q = Quality()
    # Date check.
    q.date_ok = EXPECTED_DATES[header_idx] in output
    # Emoji check.
    q.emoji_ok = EXPECTED_EMOJIS[header_idx] in output
    # Mention check (only for header 1).
    expected = EXPECTED_MENTIONS[header_idx]
    if expected:
        q.mentions_ok = all(m in output for m in expected)
    # Hallucination check.
    if HALLUCINATION_WORDS.search(output):
        q.hallucination = True
        q.note = f"halluc: {HALLUCINATION_WORDS.search(output).group()}"
    # Deviation check: multi-line or excessively long.
    lines = [l for l in output.strip().splitlines() if l.strip()]
    if len(lines) > 1:
        q.deviation = True
        q.note = f"multi-line ({len(lines)} lines)"
    elif len(output) > 200:
        q.deviation = True
        q.note = f"too long ({len(output)} chars)"
    return q


# ── result container ───────────────────────────────────────────────────────


@dataclass
class Result:
    model: str
    header: str
    header_idx: int
    rep: int
    output: str
    elapsed_s: float
    quality: Quality
    error: str | None = None


# ── model runners ──────────────────────────────────────────────────────────


def load_system_prompt() -> str:
    path = REPO_ROOT / "config" / "personas" / "personas" / "kanade" / "default-compact.md"
    return path.read_text(encoding="utf-8").strip()


def query_fm(prompt: str, system_prompt: str) -> str:
    cmd = ["fm", "respond", "--instructions", system_prompt, "--model", "system", prompt]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=30, check=True)
        return res.stdout.strip()
    except subprocess.CalledProcessError as e:
        return f"Error: {e.stderr.strip()}"
    except subprocess.TimeoutExpired:
        return "Error: timeout (30s)"


def query_ollama(model: str, prompt: str, system_prompt: str, *, think: bool | None = None) -> str:
    url = "http://127.0.0.1:11434/api/chat"
    data: dict = {
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": prompt},
        ],
        "stream": False,
        "options": {"temperature": 0.0},
    }
    if think is not None:
        data["think"] = think
    req = urllib.request.Request(
        url,
        data=json.dumps(data).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as response:  # noqa: S310
            res = json.loads(response.read().decode())
            return res["message"]["content"].strip()
    except Exception as e:  # noqa: BLE001
        return f"Error: {e}"


def ollama_pull(model: str) -> bool:
    """Pull a model, returning True on success."""
    print(f"  pulling {model}...", file=sys.stderr, end=" ", flush=True)
    try:
        subprocess.run(
            ["ollama", "pull", model],
            capture_output=True,
            text=True,
            timeout=600,
            check=True,
        )
        print("ok", file=sys.stderr)
        return True
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
        print(f"FAILED: {e}", file=sys.stderr)
        return False


def ollama_stop(model: str) -> None:
    subprocess.run(["ollama", "stop", model], capture_output=True, timeout=120, check=False)


# ── benchmark loop ─────────────────────────────────────────────────────────


def run_model(
    model_label: str,
    query_fn,
    reps: int,
    system_prompt: str,
) -> list[Result]:
    results: list[Result] = []
    for rep in range(1, reps + 1):
        for idx, header in enumerate(TEST_HEADERS):
            start = time.monotonic()
            output = query_fn(header, system_prompt)
            elapsed = time.monotonic() - start
            quality = score_result(output, idx)
            r = Result(
                model=model_label,
                header=header,
                header_idx=idx,
                rep=rep,
                output=output,
                elapsed_s=elapsed,
                quality=quality,
                error=output if output.startswith("Error:") else None,
            )
            results.append(r)
            mark = quality.grade()
            print(
                f"  rep{rep} h{idx} {elapsed:5.2f}s {mark:20s} | {output[:100]}",
                file=sys.stderr,
            )
    return results


# ── markdown report ────────────────────────────────────────────────────────


def render_markdown(all_results: dict[str, list[Result]], reps: int) -> str:
    lines = [
        "# Header Summarisation Benchmark",
        "",
        f"- run: {datetime.now().astimezone().isoformat(timespec='seconds')}",
        f"- reps: {reps}",
        f"- persona: `config/personas/personas/kanade/default-compact.md`",
        "",
    ]

    # Summary table.
    models = list(all_results.keys())
    lines += [
        "## Summary",
        "",
        "| Model | Pass Rate | Mean Time | Date✓ | Emoji✓ | Mentions✓ | Halluc? | Deviate? |",
        "| --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for model in models:
        rs = all_results[model]
        total = len(rs)
        passed = sum(1 for r in rs if r.quality.passed)
        times = [r.elapsed_s for r in rs if not r.error]
        mean_t = f"{sum(times) / len(times):.2f}s" if times else "-"
        date_ok = sum(1 for r in rs if r.quality.date_ok)
        emoji_ok = sum(1 for r in rs if r.quality.emoji_ok)
        mentions_ok = sum(1 for r in rs if r.quality.mentions_ok)
        halluc = sum(1 for r in rs if r.quality.hallucination)
        deviate = sum(1 for r in rs if r.quality.deviation)
        lines.append(
            f"| `{model}` | {passed}/{total} | {mean_t} "
            f"| {date_ok}/{total} | {emoji_ok}/{total} | {mentions_ok}/{total} "
            f"| {halluc} | {deviate} |"
        )

    # Per-model detail.
    for model in models:
        rs = all_results[model]
        lines += ["", f"## {model}", ""]
        for r in rs:
            lines += [
                f"**Header {r.header_idx + 1} rep{r.rep}** (`{r.header}`)",
                f"> {r.output}",
                f"",
                f"Time: {r.elapsed_s:.2f}s · Grade: {r.quality.grade()}"
                + (f" · {r.quality.note}" if r.quality.note else ""),
                "",
            ]

    return "\n".join(lines) + "\n"


# ── CLI ─────────────────────────────────────────────────────────────────────


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="python scripts/bench_headers.py",
        description="Benchmark header rewriting with small/fast local models.",
    )
    p.add_argument("--reps", type=int, default=3, help="runs per header (default 3)")
    p.add_argument(
        "--md",
        default="data/bench/header-summarisation-2026.md",
        help="markdown output path",
    )
    p.add_argument("--skip-afm", action="store_true", help="skip Apple Foundation Model")
    p.add_argument(
        "--model",
        action="append",
        default=[],
        help="only test these Ollama models (repeatable; default all)",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    system_prompt = load_system_prompt()
    print(f"Loaded system prompt ({len(system_prompt)} chars). {args.reps} reps.", file=sys.stderr)

    ollama_models = args.model if args.model else OLLAMA_MODELS
    all_results: dict[str, list[Result]] = {}

    # ── AFM ──
    if not args.skip_afm:
        print("\n=== AFM (fm respond) ===", file=sys.stderr)
        all_results["afm"] = run_model("afm", query_fm, args.reps, system_prompt)

    # ── Ollama models, one at a time ──
    for model in ollama_models:
        print(f"\n=== {model} ===", file=sys.stderr)
        if not ollama_pull(model):
            print(f"  SKIPPED (pull failed)", file=sys.stderr)
            continue

        think = False if model in THINKING_MODELS else None

        def query(prompt, sp, *, _m=model, _t=think):
            return query_ollama(_m, prompt, sp, think=_t)

        all_results[model] = run_model(model, query, args.reps, system_prompt)
        ollama_stop(model)
        print(f"  stopped {model}", file=sys.stderr)

    # ── report ──
    md_path = Path(args.md)
    md_path.parent.mkdir(parents=True, exist_ok=True)
    markdown = render_markdown(all_results, args.reps)
    md_path.write_text(markdown, encoding="utf-8")
    print(markdown)
    print(f"\nmd: {md_path}", file=sys.stderr)

    total = sum(len(rs) for rs in all_results.values())
    passed = sum(1 for rs in all_results.values() for r in rs if r.quality.passed)
    return 0 if passed == total else 1


if __name__ == "__main__":
    raise SystemExit(main())
