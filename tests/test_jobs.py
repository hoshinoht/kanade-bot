"""The background-job registry the portal polls (item 9)."""

from __future__ import annotations

import asyncio

import pytest

from bot.api.jobs import Job, JobRegistry


def run(coro):
    return asyncio.run(coro)


def test_a_new_job_starts_empty_and_running():
    job = JobRegistry().create("rescan", ["#a", "#b"])
    assert job.running is True
    assert (job.total, job.done, job.percent) == (2, 0, 0)


def test_progress_tracks_the_results_recorded():
    registry = JobRegistry()

    async def scenario():
        job = registry.create("rescan", ["#a", "#b"])

        async def work(j):
            j.current = "#a"
            j.results.append({"channel_name": "#a"})
            assert j.percent == 50
            j.current = "#b"
            j.results.append({"channel_name": "#b"})

        registry.start(job, work)
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        return job

    job = run(scenario())
    assert job.done == 2
    assert job.running is False
    assert job.percent == 100
    assert job.current is None


def test_a_failing_job_records_the_reason_and_finishes():
    registry = JobRegistry()

    async def scenario():
        job = registry.create("rescan", ["#a"])

        async def work(_):
            raise RuntimeError("ollama is not answering")

        registry.start(job, work)
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        return job

    job = run(scenario())
    assert job.error == "ollama is not answering"
    assert job.running is False


def test_only_one_job_of_a_kind_is_active_at_a_time():
    registry = JobRegistry()
    first = registry.create("rescan", ["#a"])
    assert registry.active("rescan") is first
    first.finished_at = first.started_at
    assert registry.active("rescan") is None


def test_active_is_scoped_to_the_kind():
    registry = JobRegistry()
    registry.create("rescan", ["#a"])
    assert registry.active("digest") is None
    assert registry.active() is not None


def test_old_jobs_are_evicted_so_the_registry_cannot_grow():
    registry = JobRegistry(keep=3)
    made = [registry.create("rescan", ["#a"]) for _ in range(5)]
    assert registry.get(made[0].id) is None
    assert registry.get(made[-1].id) is not None


def test_an_unknown_job_is_simply_missing():
    assert JobRegistry().get("nope") is None


def test_elapsed_is_measured_to_the_finish_not_to_now():
    from bot.timeutil import utcnow

    job = Job(id="x", kind="rescan", steps=["#a"], started_at=utcnow())
    job.finished_at = job.started_at
    assert job.elapsed_ms == 0


@pytest.mark.parametrize("steps", [[], ["#a"]])
def test_percent_never_divides_by_zero(steps):
    assert JobRegistry().create("rescan", steps).percent == 0


def test_shutdown_cancels_whatever_is_still_running():
    registry = JobRegistry()

    async def scenario():
        job = registry.create("rescan", ["#a"])

        async def work(_):
            await asyncio.sleep(30)

        registry.start(job, work)
        await asyncio.sleep(0)
        await registry.shutdown()
        await asyncio.sleep(0)
        return job

    job = run(scenario())
    assert job.cancelled is True
