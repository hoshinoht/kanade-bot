"""Loading ``tests/fixtures/extract/*.json`` and scoring a real extraction against it.

The fixtures are the guild's own chat, anonymised: names are single letters, ids
are fake snowflakes, the wording is untouched.  Each one carries the channel's
runs and fixed timings at the time, the messages, and the amendments a correct
extraction produces -- after :mod:`bot.extract.resolve` has turned the model's
literal ``day_ref``/``time_ref`` into a datetime.

Scoring is deliberately strict: a fixture passes only when every expected
amendment is matched *and* nothing extra was invented.  Loosening an expectation
to make a run go green would hide exactly the failure this is here to catch.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from bot.domain.weeks import parse_hhmm, parse_weekday
from bot.extract import prompt as prompt_mod
from bot.extract.pipeline import Plan, plan_burst
from bot.extract.schema import Amendment

FIXTURE_DIR = Path(__file__).resolve().parent / "fixtures" / "extract"

#: Kinds whose commit path uses the boss list (it names the run being changed).
SCORED_BOSSES = frozenset({"move", "add", "cancel", "otot", "split", "fix"})
#: Kinds whose commit path uses the resolved datetime.
SCORED_TIME = frozenset({"move", "add", "split", "fix"})


def fixture_paths() -> list[Path]:
    return sorted(FIXTURE_DIR.glob("*.json"))


def _local(text: str, tz: ZoneInfo) -> datetime:
    return datetime.fromisoformat(text).replace(tzinfo=tz)


@dataclass
class Scenario:
    """One fixture, expanded into the shapes the extractor actually works with."""

    name: str
    note: str
    tz: ZoneInfo
    channel: str
    roster: list[dict]
    runs: list[dict]
    fixed_runs: list[dict]
    context: list[prompt_mod.Msg]
    burst: list[prompt_mod.Msg]
    expected: list[dict]
    #: display name -> user id, for reading expectations written by name
    ids: dict[str, str] = field(default_factory=dict)

    @property
    def anchor(self) -> datetime:
        return self.burst[-1].created_at

    def context_for(self, table) -> prompt_mod.PromptContext:
        return prompt_mod.PromptContext(
            tz=self.tz,
            table=table,
            burst=self.burst,
            context=self.context,
            runs=self.runs,
            fixed_runs=self.fixed_runs,
            roster=self.roster,
            channel_name=self.channel,
        )


def load(path: Path) -> Scenario:
    raw = json.loads(path.read_text(encoding="utf-8"))
    tz = ZoneInfo(raw["tz"])
    ids = {member["name"]: member["id"] for member in raw["roster"]}
    roster = [
        {"user_id": m["id"], "display_name": m["name"], "nickname": None, "aliases": []}
        for m in raw["roster"]
    ]

    def people(names) -> list[str]:
        return [ids[n] for n in names]

    def message(row) -> prompt_mod.Msg:
        return prompt_mod.Msg(
            id=str(row["id"]),
            author_id=ids[row["author"]],
            author_name=row["author"],
            created_at=_local(row["at"], tz),
            content=row["text"],
        )

    runs = [
        {
            "id": run["id"],
            "fixed_run_id": run.get("fixed"),
            "channel_id": raw["channel"],
            "bosses": run["bosses"],
            "datetime": _local(run["at"], tz),
            "participants": people(run["participants"]),
            "status": run.get("status", "planned"),
            "source": "fixed",
        }
        for run in raw["runs"]
    ]
    fixed_runs = [
        {
            "id": fixed["id"],
            "owner_id": people(fixed["participants"])[0],
            "channel_id": raw["channel"],
            "bosses": fixed["bosses"],
            "weekday": parse_weekday(fixed["weekday"]),
            "time": parse_hhmm(fixed["time"]).strftime("%H:%M"),
            "participants": people(fixed["participants"]),
            "note": None,
        }
        for fixed in raw["fixed_runs"]
    ]
    return Scenario(
        name=raw["name"],
        note=raw.get("note", ""),
        tz=tz,
        channel=raw["channel"],
        roster=roster,
        runs=runs,
        fixed_runs=fixed_runs,
        context=[message(row) for row in raw.get("context", [])],
        burst=[message(row) for row in raw["messages"]],
        expected=raw["expected"],
        ids=ids,
    )


# ---------------------------------------------------------------------------
# scoring
# ---------------------------------------------------------------------------


@dataclass
class Actual:
    """One extracted amendment after the deterministic stages have run."""

    amendment: Amendment
    at: datetime | None
    day: object | None
    run_id: str | None

    def describe(self, tz: ZoneInfo) -> str:
        when = (
            self.at.astimezone(tz).strftime("%Y-%m-%d %H:%M")
            if self.at
            else (str(self.day) if self.day else "TBD")
        )
        parts = [
            self.amendment.kind,
            "+".join(self.amendment.bosses) or "-",
            when,
        ]
        if self.amendment.rsvp:
            parts.append(f"rsvp={self.amendment.rsvp}")
        if self.amendment.is_question:
            parts.append("question")
        return " · ".join(parts) + f" ({self.amendment.confidence:.2f})"


def realise(extraction, scenario: Scenario, min_confidence: float = 0.0, table=None) -> Plan:
    """Run the deterministic stages the live pipeline runs, using the same code.

    Deliberately :func:`bot.extract.pipeline.plan_burst` rather than a copy of
    it: merge -> resolve -> match, the confidence floor, the split of a `sub`
    across the runs it spans, and the dropping of an amendment with no run to
    apply to. A fixture then measures what the bot would actually have posted.
    """
    messages = list(scenario.context) + list(scenario.burst)
    return plan_burst(
        extraction,
        anchor=scenario.anchor,
        tz=scenario.tz,
        channel_runs=scenario.runs,
        burst_order=[m.id for m in messages],
        author_ids={m.id: m.author_id for m in messages},
        min_confidence=min_confidence,
        boss_table=table,
        burst_messages=scenario.burst,
        # A fixture measures what the bot would have posted *at the time*. Left
        # to the wall clock, `plan_burst` would correctly drop every one of
        # these as "already passed" the day after it was recorded, and the
        # suite would score the calendar rather than the model.
        now=scenario.anchor,
    )


def to_actual(entry, scenario: Scenario) -> Actual:
    return Actual(
        amendment=entry.amendment,
        at=entry.resolved.at,
        day=entry.resolved.day,
        run_id=entry.run["id"] if entry.run else None,
    )


def _matches(expected: dict, actual: Actual, scenario: Scenario) -> bool:
    if expected["kind"] != actual.amendment.kind:
        return False
    # Each kind is scored on the fields the pipeline actually consumes for it
    # (see SCORED_BOSSES / SCORED_TIME). A `cancel` is identified by its run, so
    # the day/time it happens to echo back is not part of being right; an `rsvp`
    # is a person and a yes/no, and which run it lands on comes from `match_run`.
    if actual.amendment.kind in SCORED_BOSSES:
        if set(expected["bosses"]) != set(actual.amendment.bosses):
            return False
    if actual.amendment.kind in SCORED_TIME:
        want_at = expected.get("resolved_local_datetime")
        have_at = (
            actual.at.astimezone(scenario.tz).strftime("%Y-%m-%d %H:%M") if actual.at else None
        )
        if want_at != have_at:
            return False
    if "resolved_local_day" in expected and actual.amendment.kind in SCORED_TIME:
        have_day = str(actual.day) if actual.day else None
        if expected["resolved_local_day"] != have_day:
            return False
    if expected.get("rsvp") is not None and expected["rsvp"] != actual.amendment.rsvp:
        return False
    if expected.get("is_question", False) != actual.amendment.is_question:
        return False
    wanted_people = {scenario.ids[n] for n in expected.get("participants", [])}
    if wanted_people and not wanted_people <= set(actual.amendment.participants):
        return False
    return True


@dataclass
class Score:
    scenario: Scenario
    actuals: list[Actual]
    matched: list[tuple[dict, Actual]] = field(default_factory=list)
    missing: list[dict] = field(default_factory=list)
    extra: list[Actual] = field(default_factory=list)
    #: Extracted but never posted (below the floor, or nothing to apply it to).
    dropped: list[Actual] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return not self.missing and not self.extra

    def reason(self) -> str:
        if self.passed:
            return f"{len(self.matched)}/{len(self.scenario.expected)} as expected"
        bits = []
        if self.missing:
            bits.append(
                "missing "
                + "; ".join(f"{e['kind']} {'+'.join(e['bosses']) or '-'}" for e in self.missing)
            )
        if self.extra:
            bits.append("extra " + "; ".join(a.describe(self.scenario.tz) for a in self.extra))
        if self.dropped:
            bits.append("dropped " + "; ".join(a.describe(self.scenario.tz) for a in self.dropped))
        return " | ".join(bits)


def score(scenario: Scenario, extraction, min_confidence: float = 0.0, table=None) -> Score:
    """Match expected against actual, one-to-one, ignoring order."""
    plan = realise(extraction, scenario, min_confidence, table)
    actuals = [to_actual(entry, scenario) for entry in plan.planned]
    result = Score(
        scenario=scenario,
        actuals=actuals,
        dropped=[to_actual(entry, scenario) for entry in plan.dropped],
    )
    unclaimed = list(actuals)
    for expected in scenario.expected:
        found = next((a for a in unclaimed if _matches(expected, a, scenario)), None)
        if found is None:
            result.missing.append(expected)
        else:
            unclaimed.remove(found)
            result.matched.append((expected, found))
    result.extra = unclaimed
    return result
