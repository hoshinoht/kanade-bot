"""Pure destination-context validation for decoded portable bundles."""

from collections.abc import Iterable
from dataclasses import dataclass

from .composition import Bundle
from .errors import BundleError


@dataclass(frozen=True)
class PreflightContext:
    guild_id: str
    catalog_revision: str
    catalog_tokens: frozenset[str]
    timezone: str
    reset_time: str


@dataclass(frozen=True)
class PreflightReport:
    counts: dict[str, int]


def preflight(bundle: Bundle, context: PreflightContext) -> PreflightReport:
    """Check destination compatibility and retained references without persistence."""
    if bundle.manifest.guild_id != context.guild_id:
        raise BundleError("guild-mismatch")
    if bundle.catalog.revision != context.catalog_revision:
        raise BundleError("catalog-mismatch")
    if (
        bundle.config.timezone != context.timezone
        or bundle.config.boss_week_reset_time != context.reset_time
    ):
        raise BundleError("reset-config-mismatch")
    _unique("member", (row.user_id for row in bundle.schedule.members))
    _unique("fixed-run", (row.id for row in bundle.schedule.fixed_runs))
    _unique("run", (row.id for row in bundle.schedule.runs))
    _unique("amendment", (row.id for row in bundle.schedule.amendments))
    _unique("message", (row.id for row in bundle.history.messages))
    _unique("extraction", (row.id for row in bundle.history.extractions))
    _unique("chat-interaction", (row.id for row in bundle.history.chat_interactions))
    _unique("audit", (row.id for row in bundle.history.audit))
    _unique("rescan-job", (row.id for row in bundle.history.rescan_jobs))
    _unique("rate-override", (row.user_id for row in bundle.rate_overrides))
    _unique("rsvp", ((row.run_id, row.user_id) for row in bundle.schedule.rsvps))
    _unique("reminder", (row.id for row in bundle.delivery.reminders))
    _unique("digest", (row.week_start for row in bundle.delivery.digests))
    _unique("decline", ((row.run_id, row.user_id) for row in bundle.delivery.declines))
    _unique("card", ((row.amendment_id, row.message_id) for row in bundle.delivery.cards))
    _unique("debug-card", (row.message_id for row in bundle.delivery.debug_cards))
    members = {row.user_id for row in bundle.schedule.members}
    fixed = {row.id for row in bundle.schedule.fixed_runs}
    runs = {row.id for row in bundle.schedule.runs}
    runs_by_id = {row.id: row for row in bundle.schedule.runs}
    amendments = {row.id for row in bundle.schedule.amendments}
    messages = {row.id for row in bundle.history.messages}
    for row in bundle.schedule.fixed_runs:
        _require(row.owner_id in members and set(row.participants) <= members, "member-reference")
        _bosses(row.bosses, active=True, bundle=bundle, context=context)
    for row in bundle.schedule.runs:
        _require(
            row.fixed_run_id is None
            or row.fixed_run_id in fixed
            or row.status in {"done", "cancelled"},
            "fixed-run-reference",
        )
        _require(set(row.participants) <= members, "member-reference")
        _bosses(
            row.bosses,
            active=row.status not in {"done", "cancelled"},
            bundle=bundle,
            context=context,
        )
    for row in bundle.schedule.rsvps:
        _require(row.run_id in runs and row.user_id in members, "rsvp-reference")
        _require(
            row.user_id in runs_by_id[row.run_id].participants,
            "rsvp-participant-reference",
        )
    for row in bundle.schedule.amendments:
        _require(row.run_id is None or row.run_id in runs, "amendment-reference")
        _require(set(row.participants) <= members, "member-reference")
        _bosses(row.bosses, active=row.status == "proposed", bundle=bundle, context=context)
        _require(set(row.evidence_msg_ids) <= messages, "amendment-evidence-reference")
    for row in bundle.history.extractions:
        _require(
            set(row.message_ids) <= messages and set(row.amendment_ids) <= amendments,
            "extraction-reference",
        )
    for row in bundle.delivery.reminders + bundle.delivery.declines + bundle.delivery.debug_cards:
        _require(row.run_id in runs, "delivery-run-reference")
    for row in bundle.delivery.cards:
        _require(row.amendment_id in amendments, "delivery-amendment-reference")
    for decline in bundle.delivery.declines:
        _require(
            decline.user_id in runs_by_id[decline.run_id].participants,
            "decline-participant-reference",
        )
    return PreflightReport(
        counts={
            "members": len(members),
            "fixed_runs": len(fixed),
            "runs": len(runs),
            "amendments": len(amendments),
            "messages": len(messages),
        },
    )


def _unique(kind: str, values: Iterable[object]) -> None:
    values = list(values)
    if len(values) != len(set(values)):
        raise BundleError(f"duplicate-{kind}")


def _require(condition: bool, code: str) -> None:
    if not condition:
        raise BundleError(code)


def _bosses(tokens: list[str], *, active: bool, bundle: Bundle, context: PreflightContext) -> None:
    allowed = (
        context.catalog_tokens
        if active
        else context.catalog_tokens | set(bundle.catalog.historic_tokens)
    )
    if not set(tokens) <= allowed:
        raise BundleError("unknown-boss")
