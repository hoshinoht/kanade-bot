"""Top-level portable v1 composition and deterministic serialization."""

import json
from datetime import datetime, timedelta
from datetime import time as wall_time
from typing import Literal
from zoneinfo import ZoneInfo

from pydantic import model_validator

from .common import AwareModel, Snowflake
from .config import Catalog, Config, RateOverride
from .delivery import DeliveryLedger
from .history import History
from .schedule import Schedule


class Manifest(AwareModel):
    format_version: Literal[1]
    guild_id: Snowflake
    exported_at: datetime
    memory_preflight_counts: dict[str, int]


class Bundle(AwareModel):
    manifest: Manifest
    schedule: Schedule
    config: Config
    history: History
    catalog: Catalog
    delivery: DeliveryLedger
    rate_overrides: list[RateOverride]

    @model_validator(mode="after")
    def _no_unresolved_delivery_or_proposals(self) -> "Bundle":
        pending = {"proposed"}
        if any(row.status in pending for row in self.schedule.amendments):
            raise ValueError("pending amendments block export")
        if any(row.status in {"queued", "running"} for row in self.history.rescan_jobs):
            raise ValueError("queued or running rescans block export")
        runs = {row.id: row for row in self.schedule.runs}
        amendments = {row.id: row for row in self.schedule.amendments}
        current = self.delivery.current_week_start
        timezone = ZoneInfo(self.config.timezone)
        pinned = self.delivery.pinned_at.astimezone(timezone)
        reset_hour, reset_minute = map(int, self.config.boss_week_reset_time.split(":"))
        reset = datetime.combine(pinned.date(), wall_time(reset_hour, reset_minute), timezone)
        if pinned < reset:
            reset -= timedelta(days=1)
        expected_start = reset - timedelta(days=(reset.weekday() - 3) % 7)
        if current != expected_start:
            raise ValueError("current-boss-week start must match pinned Thursday reset boundary")
        bound_run_ids = [
            row.run_id
            for row in self.delivery.reminders + self.delivery.declines + self.delivery.debug_cards
        ]
        if any(
            run_id not in runs or runs[run_id].week_start != current for run_id in bound_run_ids
        ):
            raise ValueError("delivery bindings must reference current-boss-week runs")
        if any(row.week_start != current for row in self.delivery.digests):
            raise ValueError("delivery digest bindings must be current boss week")
        if any(
            row.amendment_id not in amendments or amendments[row.amendment_id].week_start != current
            for row in self.delivery.cards
        ):
            raise ValueError("delivery card bindings must reference current-boss-week amendments")
        if any(
            amendment.proposal_message_id != card.message_id
            or amendment.channel_id != card.channel_id
            for card in self.delivery.cards
            for amendment in [amendments[card.amendment_id]]
        ):
            raise ValueError("delivery card binding must match amendment proposal identity")
        return self


def canonical_json(bundle: Bundle) -> bytes:
    """Stable UTF-8 JSON for checked-in vectors, not an archive codec."""
    return json.dumps(
        bundle.model_dump(mode="json"), ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
