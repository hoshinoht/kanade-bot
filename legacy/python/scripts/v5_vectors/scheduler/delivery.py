"""Journal seams and intent capture shared by the dispatch and digest families.

The delivery journal and maintenance coordinator stamp rows with
``datetime.now(UTC)`` and ``uuid.uuid4()`` aliases of their own. Both are
patched per replay: the clock to the pinned instant (it becomes reminder
``sent_at`` and digest ``posted_at``), and the UUIDs to an internal counter
whose values (attempt/operation/instance IDs) never appear in any output.
"""

from __future__ import annotations

import re
import uuid as uuid_module
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime
from types import SimpleNamespace
from typing import Any

from bot.infrastructure import db
from bot.infrastructure.maintenance import coordinator
from bot.infrastructure.maintenance.delivery import journal

from . import validation
from .oracle import Clock


def _pinned_datetime(clock: Clock) -> type[datetime]:
    class Pinned(datetime):
        @classmethod
        def now(cls, tz: Any = None) -> datetime:  # type: ignore[override]
            return clock.now.astimezone(tz)

    return Pinned


@contextmanager
def journal_seams(clock: Clock) -> Iterator[None]:
    counter = iter(range(1, 1_000_000))
    ids = SimpleNamespace(uuid4=lambda: uuid_module.UUID(int=next(counter)))
    pinned = _pinned_datetime(clock)
    patched = [
        (journal, "datetime", pinned),
        (journal, "uuid", ids),
        (coordinator, "datetime", pinned),
        (coordinator, "uuid", ids),
        (db, "uuid", ids),
    ]
    originals = [(module, name, getattr(module, name)) for module, name, _ in patched]
    for module, name, value in patched:
        setattr(module, name, value)
    try:
        yield
    finally:
        for module, name, original in originals:
            setattr(module, name, original)


def record_plans(repo: Any, sink: list[dict[str, Any]]) -> None:
    """Record every ``SendPlan`` this replay's journal executes, with its outcome."""
    original = repo.delivery.execute

    async def execute(plan: Any, transport: Any) -> Any:
        entry = intent(plan)
        sink.append(entry)
        try:
            outcome = await original(plan, transport)
        except Exception as exc:
            entry["outcome"] = f"raised:{type(exc).__name__}"
            raise
        entry["outcome"] = outcome.kind.value
        return outcome

    repo.delivery.execute = execute


def intent(plan: Any) -> dict[str, Any]:
    payload = plan.payload
    entry: dict[str, Any] = {
        "effect_kind": plan.effect_kind,
        "channel_id": plan.destination.channel_id,
        "dedupe_scope": plan.dedupe.scope.value,
        "targets": [
            {
                "binding_type": target.binding_type.value,
                "key_primary": target.key_primary,
                "key_secondary": target.key_secondary,
            }
            for target in plan.targets
        ],
        "mentions": list(payload.allowed_user_ids),
        "role_mentions": list(payload.allowed_role_ids),
        "mention_everyone": payload.allow_everyone,
    }
    if plan.effect_kind == "digest":
        entry["digest"] = digest_inclusion(payload.embeds)
    return entry


_SUMMARY = re.compile(
    r"\*\*(?P<cleared>\d+)/(?P<live>\d+) Cleared\*\* · (?P=live) run\(s\) across "
    r"(?P<days>\d+) day\(s\)(?: · \*\*(?P<unsettled>\d+)\*\* still unconfirmed ⚠️)?"
    r"(?: · \*\*(?P<at_risk>\d+)\*\* at risk ❗)?"
)
_TAG = re.compile(r"`#([0-9a-f]{8})`")


def digest_inclusion(embeds: Any) -> dict[str, Any]:
    """Read which runs v4's own ``digest_card`` included, and its counts.

    Only structure is kept: per day field, the short run IDs in order, plus the
    summary numbers. The summary must match v4's exact pattern or generation
    fails, so wording drift cannot silently change the frozen counts.
    """
    if len(embeds) != 1:
        raise validation.ContractError(f"digest payload has {len(embeds)} embeds, expected 1")
    embed = embeds[0]
    fields = embed.get("fields", [])
    description = embed.get("description", "")
    if not fields:
        return {"days": [], "cleared": 0, "live": 0, "unsettled": 0, "at_risk": 0}
    match = _SUMMARY.fullmatch(description)
    if match is None:
        raise validation.ContractError(f"unrecognised digest summary {description!r}")
    days = [{"runs": _TAG.findall(field["value"])} for field in fields]
    if int(match["days"]) != len(days):
        raise validation.ContractError("digest summary day count disagrees with its fields")
    return {
        "days": days,
        "cleared": int(match["cleared"]),
        "live": int(match["live"]),
        "unsettled": int(match["unsettled"] or 0),
        "at_risk": int(match["at_risk"] or 0),
    }
