"""Work the portal starts but does not wait for.

A rescan of every party channel is one model call per conversation per channel,
which on this Mac is minutes. A browser cannot hold a form post open that long,
so the portal starts a job, gets an id back, and polls a fragment.

Deliberately in memory and deliberately small: a job is progress for a page that
is open right now. If the bot restarts mid-rescan the job is gone, which is the
right answer -- the messages are already stored, and re-running it is cheap.
Anything that must survive a restart is a row in SQLite, not a job.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from collections import OrderedDict
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

from ..timeutil import utcnow

log = logging.getLogger(__name__)

#: How many finished jobs to keep for the page that started them.
KEEP_JOBS = 20


@dataclass
class Job:
    """One long-running portal action, and how far through it is."""

    id: str
    kind: str
    steps: list[str]
    started_at: datetime
    label: str = ""
    finished_at: datetime | None = None
    current: str | None = None
    results: list[dict] = field(default_factory=list)
    error: str | None = None
    cancelled: bool = False

    @property
    def total(self) -> int:
        return len(self.steps)

    @property
    def done(self) -> int:
        return len(self.results)

    @property
    def running(self) -> bool:
        return self.finished_at is None

    @property
    def percent(self) -> int:
        return 100 if not self.running else int(100 * self.done / self.total) if self.total else 0

    @property
    def elapsed_ms(self) -> int:
        end = self.finished_at or utcnow()
        return int((end - self.started_at).total_seconds() * 1000)

    def short_id(self) -> str:
        return self.id[:8]


class JobRegistry:
    """The jobs one bot process has run, newest last, oldest evicted."""

    def __init__(self, keep: int = KEEP_JOBS):
        self._jobs: OrderedDict[str, Job] = OrderedDict()
        self._tasks: dict[str, asyncio.Task] = {}
        self.keep = keep

    def create(self, kind: str, steps: list[str], label: str = "") -> Job:
        job = Job(
            id=str(uuid.uuid4()), kind=kind, steps=list(steps), started_at=utcnow(), label=label
        )
        self._jobs[job.id] = job
        while len(self._jobs) > self.keep:
            oldest, _ = self._jobs.popitem(last=False)
            self._tasks.pop(oldest, None)
        return job

    def get(self, job_id: str) -> Job | None:
        return self._jobs.get(job_id)

    def active(self, kind: str | None = None) -> Job | None:
        """The job still running, if there is one. Used to refuse a second."""
        for job in reversed(self._jobs.values()):
            if job.running and (kind is None or job.kind == kind):
                return job
        return None

    def start(self, job: Job, work: Callable[[Job], Awaitable[Any]]) -> Job:
        """Run ``work`` on the bot's loop, recording failure on the job itself."""

        async def runner() -> None:
            try:
                await work(job)
            except asyncio.CancelledError:
                job.cancelled = True
                raise
            except Exception as exc:  # noqa: BLE001 - a job must never kill the loop
                log.exception("job %s (%s) failed", job.short_id(), job.kind)
                job.error = str(exc) or exc.__class__.__name__
            finally:
                job.current = None
                job.finished_at = utcnow()

        self._tasks[job.id] = asyncio.create_task(runner(), name=f"job-{job.kind}-{job.short_id()}")
        return job

    async def shutdown(self) -> None:
        for task in list(self._tasks.values()):
            task.cancel()
        self._tasks.clear()


__all__ = ["KEEP_JOBS", "Job", "JobRegistry"]
