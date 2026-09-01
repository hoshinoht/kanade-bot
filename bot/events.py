"""Telling an open page that something changed, without it having to ask.

The Limits page used to poll, which is the wrong shape for what it shows: the
interesting moments -- the model being taken, a budget being spent, a rescan
starting -- are instants, and a timer either misses them or asks for nothing
most of the time. So the places that *cause* those moments say so, and whoever
is watching refetches.

Deliberately tiny, and deliberately not a message bus. An event carries no
payload beyond its topic, because the browser answers it by refetching the whole
fragment: "something changed" and "these four things changed" produce the same
request, and the second is a schema to keep in step for no gain.

Three properties the rest of the bot depends on:

* :func:`notify` is **synchronous and never blocks**. It is called from the
  middle of taking a lock and from inside a database write; a subscriber that
  cannot keep up is dropped a message, never waited for. A browser on a train
  must not be able to slow the model down.
* It is **safe with no running event loop**. Unit tests construct a pilot and
  take a lock without an ``asyncio`` loop anywhere, and ``Queue.put_nowait``
  does not need one -- but with no subscribers there is nothing to put, which is
  the case that actually runs in those tests.
* It **never raises**. Nothing here is worth failing a mutation over.

It lives at the top of the package rather than under :mod:`bot.api` so
:mod:`bot.modellock` can import it: the lock is held by the extractor and the
chatbot, neither of which should have to know the HTTP layer exists.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
from collections.abc import Iterator

log = logging.getLogger(__name__)

__all__ = ["LIMITS", "listeners", "notify", "subscribe"]

#: The one topic. Coarse on purpose -- see the module docstring.
LIMITS = "limits"

#: How many un-read events one subscriber may have waiting. Past this the
#: oldest news is already stale: the page refetches everything on any single
#: event, so eight pending "something changed" notices and one mean the same
#: thing, and the ninth is dropped rather than queued.
QUEUE_DEPTH = 8

#: topic -> the queues currently listening to it. Empty between requests, which
#: is the state the bot spends almost all its life in.
_subscribers: dict[str, set[asyncio.Queue[str]]] = {}


def notify(topic: str = LIMITS) -> None:
    """Tell every subscriber that ``topic`` changed. Never blocks, never raises.

    A no-op when nobody is watching, which is the common case: nothing is
    allocated and no loop is touched, so a nudge from deep inside the model lock
    costs a dictionary lookup.
    """
    for queue in list(_subscribers.get(topic, ())):
        try:
            queue.put_nowait(topic)
        except asyncio.QueueFull:
            # A page that is not keeping up. Dropping is correct here: it will
            # refetch the current state on the next event it does receive.
            log.debug("events: a %s subscriber is full; dropping the nudge", topic)
        except Exception:  # noqa: BLE001 - a notification must never fail a mutation
            log.debug("events: could not notify a %s subscriber", topic, exc_info=True)


@contextlib.contextmanager
def subscribe(topic: str = LIMITS) -> Iterator[asyncio.Queue[str]]:
    """Listen to ``topic`` for the duration of the block.

    A context manager because the one caller is a streaming response, and a
    subscription that outlived its connection would be a queue nobody ever
    reads -- filling up, being dropped from, and never collected. The ``finally``
    runs on a clean end and on the cancellation a disconnect produces alike.
    """
    queue: asyncio.Queue[str] = asyncio.Queue(maxsize=QUEUE_DEPTH)
    _subscribers.setdefault(topic, set()).add(queue)
    try:
        yield queue
    finally:
        watching = _subscribers.get(topic)
        if watching is not None:
            watching.discard(queue)
            if not watching:
                _subscribers.pop(topic, None)


def listeners(topic: str = LIMITS) -> int:
    """How many subscribers ``topic`` has. For the tests and the odd log line."""
    return len(_subscribers.get(topic, ()))
