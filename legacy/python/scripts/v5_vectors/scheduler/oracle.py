"""Shared replay support for the reminder and mutation vector families."""

from __future__ import annotations

import logging
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from datetime import datetime
from typing import Any

from bot.infrastructure import db
from bot.infrastructure.db import Repo

from . import replay, validation


class Clock:
    """The pinned aware instant every patched ``utcnow`` alias returns."""

    def __init__(self, value: str):
        self.now = datetime.fromisoformat(value)

    def __call__(self) -> datetime:
        return self.now


class Ids:
    """The deterministic UUID source; exhaustion is remembered even if v4 swallows it."""

    def __init__(self, ids: Sequence[str]):
        self._sequence = iter(ids)
        self.exhausted = False

    def __call__(self) -> str:
        try:
            return next(self._sequence)
        except StopIteration:
            self.exhausted = True
            raise


@contextmanager
def seams(clock: Clock, ids: Ids, modules: Sequence[Any]) -> Iterator[None]:
    """Patch ``db.new_id`` and every listed module's ``utcnow`` for one replay."""
    patched = [(db, "new_id", db.new_id)] + [
        (module, "utcnow", module.utcnow) for module in (db, *modules)
    ]
    db.new_id = ids
    for module, _, _ in patched[1:]:
        module.utcnow = clock
    try:
        yield
    finally:
        for module, name, original in patched:
            setattr(module, name, original)


def _exhausted(case_id: str, step_index: int) -> validation.ContractError:
    return validation.ContractError(
        f"{case_id}: uuid_sequence exhausted during oracle replay at step {step_index}; "
        "any prior writes were ephemeral and discarded"
    )


def run_step(
    case_id: str,
    step_index: int,
    step: dict[str, Any],
    ids: Ids,
    errors: dict[str, type[Exception]],
    call: Callable[[], Any],
) -> dict[str, Any]:
    """Run one oracle call; only the step's declared exact error class is captured."""
    expected = step.get("error_type")
    try:
        value = call()
    except StopIteration:
        raise _exhausted(case_id, step_index) from None
    except tuple(errors.values()) as exc:
        if ids.exhausted:
            raise _exhausted(case_id, step_index) from None
        if expected is None or type(exc) is not errors[expected]:
            raise
        return {"error": {"type": expected, "message": str(exc)}}
    if ids.exhausted:
        raise _exhausted(case_id, step_index)
    if expected is not None:
        raise AssertionError(f"{case_id} step {step_index} expected {expected} but succeeded")
    return {"value": value}


def run_id(case_id: str, refs: dict[str, str], key: str) -> str:
    try:
        return refs[key]
    except KeyError:
        raise validation.ContractError(
            f"{case_id}: the oracle did not produce run key {key!r}"
        ) from None


def reminder_row(repo: Repo, refs: dict[str, str], case_id: str, key: str, kind: str) -> dict:
    rows = [row for row in repo.list_reminders(run_id(case_id, refs, key)) if row["kind"] == kind]
    if not rows:
        raise validation.ContractError(f"{case_id}: run {key!r} has no {kind!r} reminder")
    return rows[0]


def snapshot(repo: Repo, side_effects: list[dict[str, Any]]) -> dict[str, Any]:
    """The scheduler snapshot plus reminder message IDs, ordered by (run, fire_at, kind)."""
    state = replay._snapshot(repo)  # noqa: SLF001 - one shared normalisation for every family.
    state["reminders"] = [
        {
            "id": row["id"],
            "run_id": row["run_id"],
            "kind": row["kind"],
            "fire_at": row["fire_at"].isoformat(),
            "sent_at": row["sent_at"].isoformat() if row["sent_at"] else None,
            "message_id": row["message_id"],
        }
        for run in repo.list_runs()
        for row in sorted(
            repo.list_reminders(run["id"]), key=lambda item: (item["fire_at"], item["kind"])
        )
    ]
    state["side_effects"] = side_effects
    return state


@contextmanager
def quiet_logs() -> Iterator[None]:
    """Mute v4's own warnings (e.g. swallowed send failures) while replaying.

    Their consequences are captured as recorded intent outcomes instead.
    """
    previous = logging.root.manager.disable
    logging.disable(logging.CRITICAL)
    try:
        yield
    finally:
        logging.disable(previous)
