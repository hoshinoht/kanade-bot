"""The rescan queue: one worker, in order, cancellable, without blocking (item 9)."""

from __future__ import annotations

import asyncio

import pytest

from bot.rescan import CANCELLED, DONE, QUEUED, RUNNING, RescanWorker, job_view

from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL


class Recorder:
    """Stands in for `service.rescan_one`, with a controllable pause."""

    def __init__(self, pause: float = 0.0):
        self.calls: list[str] = []
        self.pause = pause
        self.stops: list = []

    async def __call__(self, bot, channel_id, window="week", automated=False, should_stop=None):
        self.calls.append(str(channel_id))
        self.stops.append(should_stop)
        if self.pause:
            await asyncio.sleep(self.pause)
        return {"channel_id": str(channel_id), "channel_name": f"#{channel_id}", "proposals": 1}


def worker_with(fake_bot, recorder, monkeypatch) -> RescanWorker:
    from bot.api import service

    monkeypatch.setattr(service, "rescan_one", recorder)
    return RescanWorker(fake_bot)


# --- queueing ---------------------------------------------------------------


def test_submitting_returns_immediately_with_a_job(fake_bot):
    worker = RescanWorker(fake_bot)
    job = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL], window="week")
    assert job.status == QUEUED
    assert job.total == 2
    assert job.running is True


def test_the_job_is_written_down_when_it_is_asked_for(fake_bot):
    """So the portal can show it even if the process restarts before it runs."""
    job = RescanWorker(fake_bot).submit([WATCHED_CHANNEL], requested_by=1001)
    row = fake_bot.repo.get_rescan_job(job.id)
    assert row["status"] == "queued"
    assert row["requested_by"] == "1001"
    assert row["channels"] == [str(WATCHED_CHANNEL)]


def test_asking_twice_for_the_same_channels_replaces_the_queued_job(fake_bot):
    """A button pressed twice must not read everything twice and post two cards."""
    worker = RescanWorker(fake_bot)
    first = worker.submit([WATCHED_CHANNEL], window="week")
    second = worker.submit([WATCHED_CHANNEL], window="2weeks")
    assert second is first
    assert first.window == "2weeks", "the newer window wins"


def test_a_narrower_request_attaches_to_a_broader_queued_one(fake_bot):
    worker = RescanWorker(fake_bot)
    everything = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL])
    assert worker.submit([WATCHED_CHANNEL]) is everything


def test_a_broader_request_is_its_own_job(fake_bot):
    worker = RescanWorker(fake_bot)
    one = worker.submit([WATCHED_CHANNEL])
    two = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL])
    assert two is not one


def test_old_jobs_are_evicted_so_the_queue_cannot_grow(fake_bot):
    worker = RescanWorker(fake_bot, keep=2)
    made = [worker.submit([str(i)]) for i in range(4)]
    assert worker.get(made[0].id) is None
    assert worker.get(made[-1].id) is not None


# --- the 48-hour cap on automated rescans -----------------------------------


@pytest.mark.parametrize("asked", ["week", "2weeks"])
def test_an_automated_rescan_is_capped_at_48h(fake_bot, asked):
    """The bot re-reading a fortnight on its own initiative is how it becomes noise."""
    job = RescanWorker(fake_bot).submit([WATCHED_CHANNEL], window=asked, automated=True)
    assert job.window == "48h"


@pytest.mark.parametrize("asked", ["24h", "48h"])
def test_an_automated_rescan_keeps_a_narrower_window(fake_bot, asked):
    job = RescanWorker(fake_bot).submit([WATCHED_CHANNEL], window=asked, automated=True)
    assert job.window == asked


@pytest.mark.parametrize("asked", ["week", "2weeks", "48h", "24h"])
def test_a_person_gets_the_window_they_asked_for(fake_bot, asked):
    job = RescanWorker(fake_bot).submit([WATCHED_CHANNEL], window=asked)
    assert job.window == asked


def test_an_automated_rescan_never_widens_to_last_week():
    from bot.extract.window import should_widen

    assert should_widen("week", 0, automated=False) is True
    assert should_widen("week", 0, automated=True) is False


def test_the_cap_is_enforced_in_the_pipeline_too(fake_bot):
    """Not by convention at the call sites -- a future scheduled sweep must not widen."""
    from bot.extract.window import clamp_window

    assert clamp_window("week", automated=True) == "48h"
    assert clamp_window("week", automated=False) == "week"


# --- draining ---------------------------------------------------------------


def drain(worker, job, tries: int = 200):
    async def scenario():
        await worker.start()
        for _ in range(tries):
            if not job.running:
                break
            await asyncio.sleep(0.005)
        await worker.stop()

    asyncio.run(scenario())
    return job


def test_channels_are_read_one_at_a_time_in_order(fake_bot, monkeypatch):
    recorder = Recorder()
    worker = worker_with(fake_bot, recorder, monkeypatch)
    job = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL])
    drain(worker, job)
    assert recorder.calls == [str(WATCHED_CHANNEL), str(OTHER_CHANNEL)]
    assert job.status == DONE
    assert job.done == 2
    assert job.percent == 100


def test_two_jobs_run_one_after_the_other(fake_bot, monkeypatch):
    recorder = Recorder()
    worker = worker_with(fake_bot, recorder, monkeypatch)
    first = worker.submit([WATCHED_CHANNEL])
    second = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL])

    async def scenario():
        await worker.start()
        for _ in range(200):
            if not first.running and not second.running:
                break
            await asyncio.sleep(0.005)
        await worker.stop()

    asyncio.run(scenario())
    assert first.status == DONE and second.status == DONE
    assert recorder.calls == [str(WATCHED_CHANNEL), str(WATCHED_CHANNEL), str(OTHER_CHANNEL)]


def test_the_results_are_written_to_the_database(fake_bot, monkeypatch):
    worker = worker_with(fake_bot, Recorder(), monkeypatch)
    job = drain(worker, worker.submit([WATCHED_CHANNEL]))
    row = fake_bot.repo.get_rescan_job(job.id)
    assert row["status"] == DONE
    assert row["finished_at"] is not None
    assert len(row["results"]) == 1


def test_one_failing_channel_does_not_end_the_worker(fake_bot, monkeypatch):
    from bot.api import service

    async def boom(bot, channel_id, **kwargs):
        raise RuntimeError("ollama is not answering")

    monkeypatch.setattr(service, "rescan_one", boom)
    worker = RescanWorker(fake_bot)
    job = drain(worker, worker.submit([WATCHED_CHANNEL]))
    assert job.status == "failed"
    assert "ollama is not answering" in job.error
    assert fake_bot.repo.get_rescan_job(job.id)["error"]


# --- cancelling -------------------------------------------------------------


def test_a_queued_job_is_cancelled_outright(fake_bot):
    worker = RescanWorker(fake_bot)
    job = worker.submit([WATCHED_CHANNEL])
    assert worker.cancel(job.id) is True
    assert job.status == CANCELLED
    assert fake_bot.repo.get_rescan_job(job.id)["status"] == CANCELLED


def test_cancelling_stops_before_the_next_channel(fake_bot, monkeypatch):
    """Cooperative: a model call already in flight is left to finish."""
    recorder = Recorder(pause=0.02)
    worker = worker_with(fake_bot, recorder, monkeypatch)
    job = worker.submit([WATCHED_CHANNEL, OTHER_CHANNEL])

    async def scenario():
        await worker.start()
        while job.status != RUNNING:
            await asyncio.sleep(0.002)
        worker.cancel(job.id)
        for _ in range(200):
            if not job.running:
                break
            await asyncio.sleep(0.005)
        await worker.stop()

    asyncio.run(scenario())
    assert job.status == CANCELLED
    assert recorder.calls == [str(WATCHED_CHANNEL)], "the second channel was never started"


def test_the_pipeline_is_given_a_way_to_stop_between_bursts(fake_bot, monkeypatch):
    recorder = Recorder()
    worker = worker_with(fake_bot, recorder, monkeypatch)
    drain(worker, worker.submit([WATCHED_CHANNEL]))
    assert callable(recorder.stops[0])
    assert recorder.stops[0]() is False


def test_cancelling_something_finished_reports_false(fake_bot, monkeypatch):
    worker = worker_with(fake_bot, Recorder(), monkeypatch)
    job = drain(worker, worker.submit([WATCHED_CHANNEL]))
    assert worker.cancel(job.id) is False


def test_cancelling_an_unknown_job_reports_false(fake_bot):
    assert RescanWorker(fake_bot).cancel("nope") is False


# --- the view the API and portal render -------------------------------------


def test_the_job_view_carries_what_a_progress_bar_needs(fake_bot):
    worker = RescanWorker(fake_bot)
    job = worker.submit([WATCHED_CHANNEL], names={str(WATCHED_CHANNEL): "#hstar-party"})
    view = job_view(job)
    assert view["channel_names"] == ["#hstar-party"]
    assert view["percent"] == 0
    assert view["running"] is True
    assert view["short_id"] == job.id[:8]
