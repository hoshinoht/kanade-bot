"""Replay dispatch vectors through the real ``BossBot.dispatch_reminders``."""

from __future__ import annotations

from typing import Any

from bot.agent import materialise

from .. import host_replay
from ..host_replay import Session, at
from . import validation


def _handlers(session: Session) -> dict[str, host_replay.Handler]:
    repo, host = session.repo, session.host

    def add_reminder(step: dict) -> str | None:
        return repo.add_reminder(session.run(step["run_key"]), step["kind"], at(step["fire_at"]))

    def due_reminders(step: dict) -> list[dict[str, str]]:
        return [
            {"id": row["id"], "kind": row["kind"]} for row in repo.due_reminders(session.clock.now)
        ]

    def dispatch(step: dict) -> None:
        host_replay.run_async(host.dispatch_reminders(session.clock.now))

    def mark_done(step: dict) -> list[str]:
        return materialise.mark_done(repo, now=session.clock.now)

    return {
        "add_reminder": add_reminder,
        "due_reminders": due_reminders,
        "dispatch_reminders": dispatch,
        "mark_done": mark_done,
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    return host_replay.replay(case, validation.validate_input_case, _handlers, lambda _: {})
