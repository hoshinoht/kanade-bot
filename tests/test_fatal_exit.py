"""Fatal config errors pause before exiting, but stay interruptible.

Under `restart: unless-stopped` an immediate non-zero exit becomes a tight loop
that hammers Discord's gateway with rejected IDENTIFYs, so the process lingers
first -- while still stopping at once when Docker sends SIGTERM.
"""

from __future__ import annotations

import asyncio
import time

import pytest

from bot.__main__ import FATAL_EXIT_DELAY, _fatal


@pytest.mark.parametrize("code", [2, 3, 4])
def test_the_exit_code_is_preserved(code):
    assert asyncio.run(_fatal(code, asyncio.Event(), delay=0.01)) == code


def test_it_waits_out_the_delay_when_nothing_interrupts():
    started = time.monotonic()
    asyncio.run(_fatal(3, asyncio.Event(), delay=0.05))
    assert time.monotonic() - started >= 0.05


def test_a_shutdown_signal_cuts_the_wait_short():
    async def scenario():
        stop = asyncio.Event()
        started = time.monotonic()
        task = asyncio.create_task(_fatal(4, stop, delay=30))
        await asyncio.sleep(0)
        stop.set()  # what the SIGTERM handler does
        code = await task
        return code, time.monotonic() - started

    code, elapsed = asyncio.run(scenario())
    assert code == 4
    assert elapsed < 1  # not the full 30s


def test_it_still_works_without_a_stop_event():
    assert asyncio.run(_fatal(2, None, delay=0.01)) == 2


def test_the_shipped_delay_outlasts_a_docker_restart_loop():
    # Long enough that a crash-looping container retries about once a minute,
    # not several times a second.
    assert FATAL_EXIT_DELAY >= 30
