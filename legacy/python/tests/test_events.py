"""The broadcaster behind the Limits page's live updates.

Its whole job is to be harmless: it is called from inside the model lock and
from the middle of a database write, so the interesting tests are not "does a
subscriber get the message" but "what happens when there is nobody, no loop, or
a browser that has stopped reading".
"""

from __future__ import annotations

import asyncio

import pytest

from bot.infrastructure import events

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.fixture(autouse=True)
def _no_leaked_subscribers():
    """Every test starts and ends with nobody listening."""
    events._subscribers.clear()
    yield
    events._subscribers.clear()


# ---------------------------------------------------------------------------
# the quiet cases, which are the ones that run in production
# ---------------------------------------------------------------------------


def test_notifying_nobody_is_a_no_op():
    events.notify()
    assert events.listeners() == 0


def test_notifying_needs_no_running_event_loop():
    """It is called from constructors and from synchronous mutations."""
    assert asyncio.get_event_loop_policy() is not None
    events.notify()  # no loop running here at all
    events.notify("something-nobody-watches")


def test_a_broken_subscriber_does_not_reach_the_caller():
    """A notification must never be the thing that fails a mutation."""

    class Exploding:
        def put_nowait(self, _item):
            raise RuntimeError("this queue is on fire")

    events._subscribers[events.LIMITS] = {Exploding()}
    events.notify()  # must not raise


# ---------------------------------------------------------------------------
# subscribing
# ---------------------------------------------------------------------------


async def test_a_subscriber_is_told_and_then_forgotten():
    with events.subscribe() as queue:
        assert events.listeners() == 1
        events.notify()
        assert queue.get_nowait() == events.LIMITS
    assert events.listeners() == 0


async def test_the_subscription_is_dropped_even_when_the_block_raises():
    """A disconnected browser leaves through an exception, not a return."""
    with pytest.raises(asyncio.CancelledError), events.subscribe():
        assert events.listeners() == 1
        raise asyncio.CancelledError
    assert events.listeners() == 0


async def test_every_subscriber_hears_the_same_event():
    with events.subscribe() as first, events.subscribe() as second:
        events.notify()
        assert (first.get_nowait(), second.get_nowait()) == (events.LIMITS, events.LIMITS)


async def test_topics_do_not_leak_into_each_other():
    with events.subscribe() as queue:
        events.notify("rescans-or-whatever")
        assert queue.empty()


async def test_a_subscriber_that_stops_reading_is_dropped_not_waited_for():
    """The bound: a slow browser must never back-pressure the model."""
    with events.subscribe() as queue:
        for _ in range(events.QUEUE_DEPTH + 20):
            events.notify()  # never blocks, never raises

        assert queue.qsize() == events.QUEUE_DEPTH
        # ...and it still works once they catch up.
        while not queue.empty():
            queue.get_nowait()
        events.notify()
        assert queue.get_nowait() == events.LIMITS


# ---------------------------------------------------------------------------
# the stream the portal opens
# ---------------------------------------------------------------------------


async def test_the_stream_opens_before_it_has_anything_to_say():
    """`EventSource` reaches `onopen` on the first bytes, not on the first event."""
    from bot.api.routes_web import limits_event_stream

    stream = limits_event_stream()
    try:
        assert await anext(stream) == ": watching limits\n\n"
        assert events.listeners() == 1
    finally:
        await stream.aclose()


async def test_a_nudge_reaches_the_stream_as_an_event():
    from bot.api.routes_web import limits_event_stream

    stream = limits_event_stream()
    try:
        await anext(stream)  # the opening comment
        events.notify()
        assert await anext(stream) == f"event: {events.LIMITS}\ndata: changed\n\n"
    finally:
        await stream.aclose()


async def test_a_burst_of_nudges_is_one_refetch():
    """The page refetches everything on any event, so five would be four wasted."""
    from bot.api.routes_web import limits_event_stream

    stream = limits_event_stream()
    try:
        await anext(stream)
        for _ in range(5):
            events.notify()

        assert await anext(stream) == f"event: {events.LIMITS}\ndata: changed\n\n"
        # ...and nothing is left queued behind it.
        events.notify()
        assert await anext(stream) == f"event: {events.LIMITS}\ndata: changed\n\n"
    finally:
        await stream.aclose()


async def test_a_quiet_stream_is_kept_alive(monkeypatch):
    """`tailscale serve` and most proxies drop a stream that goes silent."""
    from bot.api import routes_web

    monkeypatch.setattr(routes_web, "HEARTBEAT_S", 0.01)
    stream = routes_web.limits_event_stream()
    try:
        await anext(stream)
        assert await anext(stream) == ": keep-alive\n\n"
    finally:
        await stream.aclose()


async def test_closing_the_stream_drops_the_subscription():
    """A closed laptop must not leave a queue nobody will ever read again."""
    from bot.api.routes_web import limits_event_stream

    stream = limits_event_stream()
    await anext(stream)
    assert events.listeners() == 1

    await stream.aclose()

    assert events.listeners() == 0
