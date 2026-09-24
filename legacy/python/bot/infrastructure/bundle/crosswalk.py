"""Machine-checkable inventory disposition for portable bundle v1."""

import json
from pathlib import Path

INVENTORY_PATH = Path(__file__).parents[5] / "docs" / "v5" / "inventory.json"
TABLE_CROSSWALK = {
    "schema_version": "derive",
    "maintenance_state": "exclude",
    "maintenance_leases": "exclude",
    "adoption_sources": "exclude",
    "delivery_attempts": "carry",
    "delivery_attempt_targets": "carry",
    "members": "carry",
    "fixed_runs": "carry",
    "amendments": "carry",
    "runs": "carry",
    "rsvps": "carry",
    "reminders": "carry",
    "weekly_digests": "carry",
    "messages": "carry",
    "extractions": "carry",
    "debug_messages": "carry",
    "decline_notices": "carry",
    "config": "carry",
    "rescan_jobs": "carry",
    "chat_interactions": "carry",
    "audit": "carry",
    "chat_rate_limits": "carry",
}
RUNTIME_CONFIG_CROSSWALK = {
    "day_of_ping_time": "carry",
    "countdown_minutes": "carry",
    "paused": "carry",
    "extract_enabled": "carry",
    "quiet_mode": "carry",
    "chat_mode": "carry",
    "persona": "carry",
    "chat_role_plugins": "carry",
    "chat_selectable_plugins": "carry",
    "chat_pilot_rate_count": "carry",
    "chat_pilot_rate_window_s": "carry",
    "chat_pilot_global_rate_count": "carry",
    "chat_pilot_global_rate_window_s": "carry",
    "extract_model": "carry",
    "extract_reasoning": "carry",
    "chat_pilot_model": "carry",
    "chat_pilot_think": "carry",
    "last_materialised_week": "carry",
    "last_digest_week": "carry",
    "last_backup_day": "derive",
    "heartbeat": "derive",
}
RATIONALE = {
    "schema_version": "derive: portable format_version is independent of SQLite DDL",
    "maintenance_state": "exclude: local control-plane state is never portable",
    "maintenance_leases": "exclude: live owner token hashes are process-local authority",
    "adoption_sources": "exclude: source-resolution evidence is local control-plane state",
    "delivery_attempts": "carry: retained delivery ledger prevents notification replay",
    "delivery_attempt_targets": "carry: native target claims preserve grouped delivery identity",
    "last_backup_day": "derive: host backup retention marker, never portable behavior",
    "heartbeat": "derive: process liveness marker, never portable behavior",
}


def assert_crosswalk_complete(path: Path = INVENTORY_PATH) -> None:
    inventory = json.loads(path.read_text())
    tables = {item["name"] for item in inventory["tables"]}
    runtime = {item["id"] for item in inventory["runtime_config"]["items"]}
    if tables != set(TABLE_CROSSWALK):
        raise AssertionError(f"table crosswalk mismatch: {tables ^ set(TABLE_CROSSWALK)}")
    if runtime != set(RUNTIME_CONFIG_CROSSWALK):
        raise AssertionError(
            f"runtime config crosswalk mismatch: {runtime ^ set(RUNTIME_CONFIG_CROSSWALK)}"
        )
