"""Replay digest vectors through the real ``BossBot.post_week_digest``."""

from __future__ import annotations

from typing import Any

from bot.agent.client import CFG_LAST_DIGEST, CFG_LAST_WEEK

from .. import host_replay
from ..host_replay import Session, at
from . import validation

CONFIG_KEYS = {"last_digest_week": CFG_LAST_DIGEST, "last_materialised_week": CFG_LAST_WEEK}


def _handlers(session: Session) -> dict[str, host_replay.Handler]:
    repo, host = session.repo, session.host

    def add_fixed(step: dict) -> str:
        session.fixed[step["fixed_key"]] = repo.add_fixed_run(
            step["owner_id"],
            step["bosses"],
            step["weekday"],
            step["time"],
            step["participants"],
            channel_id=step["channel_id"],
        )
        return session.fixed[step["fixed_key"]]

    def materialise_weeks(step: dict) -> None:
        host.materialise_weeks()
        session.register_materialised()

    def post(step: dict) -> str | None:
        message = host_replay.run_async(host.post_week_digest(session.clock.now))
        return None if message is None else str(message.id)

    def set_post_channel(step: dict) -> str | None:
        value = step["channel_id"]
        host.settings.post_channel_id = int(value) if value is not None else None
        return value

    def set_config(step: dict) -> str:
        repo.set_config(CONFIG_KEYS[step["key"]], step["value"])
        return step["value"]

    def set_weekly_digest(step: dict) -> None:
        repo.set_weekly_digest(
            at(step["week_start"]), step["channel_id"], step["message_id"], at=session.clock.now
        )

    def retire_before(step: dict) -> int:
        return repo.retire_weekly_digests_before(at(step["week_start"]), at=session.clock.now)

    return {
        "add_fixed": add_fixed,
        "materialise_weeks": materialise_weeks,
        "post_week_digest": post,
        "set_post_channel": set_post_channel,
        "set_config": set_config,
        "set_weekly_digest": set_weekly_digest,
        "retire_weekly_digests_before": retire_before,
    }


def _state(session: Session) -> dict[str, Any]:
    rows = session.repo._conn.execute(  # noqa: SLF001 - no public digest-log reader exists.
        "SELECT week_start FROM weekly_digests ORDER BY week_start"
    ).fetchall()
    digests = []
    for row in rows:
        found = session.repo.get_weekly_digest(at(row["week_start"]))
        digests.append(
            {
                "week_start": found["week_start"].isoformat(),
                "channel_id": found["channel_id"],
                "message_id": found["message_id"],
                "posted_at": found["posted_at"].isoformat(),
                "retired_at": found["retired_at"].isoformat() if found["retired_at"] else None,
            }
        )
    return {
        "weekly_digests": digests,
        "config": {name: session.repo.get_config(key) for name, key in CONFIG_KEYS.items()},
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    return host_replay.replay(case, validation.validate_input_case, _handlers, _state)
