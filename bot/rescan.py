"""The rescan queue: re-reading channels without blocking the bot.

Re-reading a boss week is one model call per conversation per channel, which on
this Mac is minutes. Doing that inside a slash command or an HTTP request meant
the reminder tick, reactions and every other command waited on Ollama.

So a request *enqueues*. One worker task drains the queue in order -- the model
lock would serialise the calls anyway, and in order each party's cards land
together in its own channel -- while everything else on the loop keeps running.
Callers get a job id back immediately and watch it: the portal polls a fragment,
``bossctl`` polls the API, a slash command says where the cards will appear.

Jobs are cancellable between bursts (never mid-call: an in-flight extraction is
already paid for), and each one is written to ``rescan_jobs`` so the portal can
show what has been run since the last restart.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from collections.abc import Sequence
from dataclasses import dataclass, field
from datetime import datetime
from typing import TYPE_CHECKING, Any

from .extract.window import DEFAULT_WINDOW, clamp_window
from .timeutil import utcnow

if TYPE_CHECKING:  # pragma: no cover
    from .client import BossBot

log = logging.getLogger(__name__)

QUEUED, RUNNING, DONE, FAILED, CANCELLED = "queued", "running", "done", "failed", "cancelled"

#: Finished jobs kept in memory for the page that started them. The database
#: row outlives this; the in-memory copy is what progress polling reads.
KEEP_JOBS = 20


@dataclass
class RescanJob:
    """One request to re-read some channels, and how far through it is."""

    id: str
    channels: list[str]
    window: str
    source: str = "manual"
    automated: bool = False
    requested_by: str | None = None
    #: Display names, parallel to ``channels``, for progress messages.
    names: dict[str, str] = field(default_factory=dict)
    status: str = QUEUED
    created_at: datetime = field(default_factory=utcnow)
    started_at: datetime | None = None
    finished_at: datetime | None = None
    current: str | None = None
    results: list[dict] = field(default_factory=list)
    error: str | None = None
    #: Set by :meth:`RescanWorker.cancel`; read between bursts.
    stop_requested: bool = False

    @property
    def running(self) -> bool:
        return self.status in (QUEUED, RUNNING)

    @property
    def total(self) -> int:
        return len(self.channels)

    @property
    def done(self) -> int:
        return len(self.results)

    @property
    def percent(self) -> int:
        if not self.running:
            return 100
        return int(100 * self.done / self.total) if self.total else 0

    @property
    def elapsed_ms(self) -> int:
        end = self.finished_at or utcnow()
        return int((end - (self.started_at or self.created_at)).total_seconds() * 1000)

    @property
    def short_id(self) -> str:
        return self.id[:8]

    def channel_names(self) -> list[str]:
        return [self.names.get(cid, cid) for cid in self.channels]

    def covers(self, channels: Sequence[str]) -> bool:
        return set(map(str, channels)) <= set(self.channels)


class RescanWorker:
    """Owns the queue and the one task that drains it."""

    def __init__(self, bot: BossBot, keep: int = KEEP_JOBS):
        self.bot = bot
        self.keep = keep
        self._queue: asyncio.Queue[str] = asyncio.Queue()
        self._jobs: dict[str, RescanJob] = {}
        self._order: list[str] = []
        self._task: asyncio.Task | None = None
        self.current: RescanJob | None = None

    # -- lifecycle ---------------------------------------------------------
    async def start(self) -> None:
        if self._task is None or self._task.done():
            self._task = asyncio.create_task(self._drain(), name="rescan-worker")

    async def stop(self) -> None:
        for job in self._jobs.values():
            if job.running:
                job.stop_requested = True
        task, self._task = self._task, None
        if task is not None:
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):  # noqa: B902 - shutdown is best effort
                pass

    @property
    def running(self) -> bool:
        return self._task is not None and not self._task.done()

    # -- the queue ---------------------------------------------------------
    def get(self, job_id: str) -> RescanJob | None:
        return self._jobs.get(job_id)

    def recent(self, limit: int = 10) -> list[RescanJob]:
        return [self._jobs[jid] for jid in reversed(self._order[-limit:])]

    def active(self) -> RescanJob | None:
        """The job running or waiting, newest first; ``None`` when idle."""
        for job_id in reversed(self._order):
            job = self._jobs[job_id]
            if job.running:
                return job
        return None

    def submit(
        self,
        channels: Sequence[str],
        window: str = DEFAULT_WINDOW,
        source: str = "manual",
        automated: bool = False,
        requested_by: int | str | None = None,
        names: dict[str, str] | None = None,
    ) -> RescanJob:
        """Queue a rescan, or hand back the one already covering these channels.

        Asking twice for the same channels is common -- a button pressed again,
        a slash command run while the portal sweep is still going -- and queuing
        both would read everything twice and post two cards. A **queued** job for
        the same channels is replaced (the newer window wins); a **running** one
        is handed back to attach to.
        """
        wanted = [str(c) for c in channels]
        window = clamp_window(window, automated)

        existing = self.active()
        if existing is not None and existing.covers(wanted):
            if existing.status == QUEUED:
                existing.window = window
                existing.names.update(names or {})
                self.bot.repo.update_rescan_job(existing.id, window=window)
                log.info("rescan %s: replaced the queued request", existing.short_id)
            else:
                log.info("rescan %s: attaching to the running job", existing.short_id)
            return existing

        job = RescanJob(
            id=str(uuid.uuid4()),
            channels=wanted,
            window=window,
            source=source,
            automated=automated,
            requested_by=str(requested_by) if requested_by is not None else None,
            names=dict(names or {}),
        )
        self._remember(job)
        self.bot.repo.create_rescan_job(
            job.id,
            channels=job.channels,
            window=job.window,
            source=source,
            automated=automated,
            requested_by=job.requested_by,
            at=job.created_at,
        )
        self._queue.put_nowait(job.id)
        log.info(
            "rescan %s queued: %d channel(s), %s%s",
            job.short_id,
            len(job.channels),
            job.window,
            " (automated)" if automated else "",
        )
        return job

    def cancel(self, job_id: str) -> bool:
        """Ask a job to stop. Returns False when it is already finished."""
        job = self._jobs.get(job_id)
        if job is None or not job.running:
            return False
        job.stop_requested = True
        if job.status == QUEUED:
            # Never started, so it can be retired now rather than when reached.
            self._finish(job, CANCELLED)
        return True

    def _remember(self, job: RescanJob) -> None:
        self._jobs[job.id] = job
        self._order.append(job.id)
        while len(self._order) > self.keep:
            self._jobs.pop(self._order.pop(0), None)

    def _finish(self, job: RescanJob, status: str, error: str | None = None) -> None:
        job.status = status
        job.error = error
        job.current = None
        job.finished_at = utcnow()
        self.bot.repo.update_rescan_job(
            job.id,
            status=status,
            finished_at=job.finished_at,
            results=job.results,
            error=error,
        )

    # -- the loop ----------------------------------------------------------
    async def _drain(self) -> None:
        while True:
            job_id = await self._queue.get()
            job = self._jobs.get(job_id)
            try:
                if job is None or not job.running:
                    continue  # cancelled while it waited, or evicted
                await self._run(job)
            except asyncio.CancelledError:
                if job is not None and job.running:
                    self._finish(job, CANCELLED)
                raise
            except Exception as exc:  # noqa: BLE001 - one bad job must not end the worker
                log.exception("rescan %s failed", getattr(job, "short_id", "?"))
                if job is not None:
                    self._finish(job, FAILED, str(exc) or exc.__class__.__name__)
            finally:
                self._queue.task_done()
                self.current = None

    async def _run(self, job: RescanJob) -> None:
        self.current = job
        job.status = RUNNING
        job.started_at = utcnow()
        self.bot.repo.update_rescan_job(job.id, status=RUNNING, started_at=job.started_at)

        from .api import service

        failure: str | None = None
        for channel_id in job.channels:
            if job.stop_requested:
                break
            job.current = job.names.get(channel_id, channel_id)
            try:
                job.results.append(
                    await service.rescan_one(
                        self.bot,
                        channel_id,
                        window=job.window,
                        automated=job.automated,
                        should_stop=lambda: job.stop_requested,
                    )
                )
            except asyncio.CancelledError:
                raise
            except Exception as exc:  # noqa: BLE001 - the other channels still get read
                # One channel losing its connection to Discord used to abandon
                # every channel queued behind it.
                log.exception("rescan of channel %s failed", channel_id)
                if failure is None:
                    failure = f"{job.current}: {str(exc) or exc.__class__.__name__}"
            self.bot.repo.update_rescan_job(job.id, results=job.results)
        if job.stop_requested:
            self._finish(job, CANCELLED, failure)
        elif failure is not None and not job.results:
            # Every channel failed, so the job did too. One failure among
            # several channels is reported beside the channels that worked.
            self._finish(job, FAILED, failure)
        else:
            self._finish(job, DONE, failure)


def job_view(job: RescanJob) -> dict[str, Any]:
    """A job as JSON, for the API and the portal's progress fragment."""
    return {
        "job_id": job.id,
        "short_id": job.short_id,
        "status": job.status,
        "window": job.window,
        "source": job.source,
        "automated": job.automated,
        "channels": job.channels,
        "channel_names": job.channel_names(),
        "current": job.current,
        "done": job.done,
        "total": job.total,
        "percent": job.percent,
        "elapsed_ms": job.elapsed_ms,
        "running": job.running,
        "error": job.error,
        "results": job.results,
    }


__all__ = [
    "CANCELLED",
    "DONE",
    "FAILED",
    "QUEUED",
    "RUNNING",
    "RescanJob",
    "RescanWorker",
    "job_view",
]
