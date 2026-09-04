"""One model, one call at a time, whoever the caller is.

``gpt-oss:20b`` is 13 GB on a 24 GB host, so exactly one copy is resident and the
machine can push exactly one generation through it at a time. Two callers who
both start one do not get half the speed each -- they get two calls that each
take about as long as both together, which is how a 60-second chat answer, an
extraction and a rescan all manage to time out while the host is busy the whole
time. Live, that was two channels asking the chatbot something while a burst
extraction was running: nobody got an answer and the machine did all the work.

Hence a lock that belongs to no feature. The extractor (:mod:`bot.extract.llm`),
the chatbot (:mod:`bot.chat.agent`) and anything added later hold the *same*
:data:`MODEL_LOCK`, and each holds it for a whole interaction rather than for one
HTTP round trip -- a chat answer that released between its tool rounds would let
an extraction slot in and push the answer past its own timeout.

It lives in its own module rather than inside either feature because a lock
imported from the extractor is a lock the next reader will reasonably assume only
extractions take.

``asyncio.Lock`` wakes its waiters in the order they arrived, and that ordering is
the whole of the priority scheme here: a caller willing to queue gets the model
the moment the current call finishes, and a caller who would rather be turned
away says so by giving :func:`acquire_within` a short deadline.

Every holder says who it is (:func:`held`, :func:`acquire_within`), because "the
model is busy" is the answer to half the questions asked about this bot and *what
it is busy with* is the other half. :func:`holder` is what the portal reads.
"""

from __future__ import annotations

import asyncio
import time
from contextlib import asynccontextmanager
from typing import Any

from . import events

__all__ = [
    "EXTRACTOR",
    "FOLLOWUP",
    "MODEL_LOCK",
    "acquire_within",
    "chat_label",
    "held",
    "holder",
    "release",
]

#: Held across a whole model interaction, guild-wide. See the module docstring.
MODEL_LOCK = asyncio.Lock()

#: The labels a holder goes by. Named rather than written out at each site so
#: the portal can ask "is an extraction running?" without matching a string
#: somebody may reword.
EXTRACTOR = "extractor"
FOLLOWUP = "followup"
#: A chat answer names its channel, since two of them are the common case and
#: "chat" alone would not say which conversation the host is spending itself on.
CHAT_PREFIX = "chat #"

#: Who holds it, and since when (``time.monotonic``). ``None`` means either
#: nobody has it or whoever does took the lock directly without saying -- the
#: two are told apart by :data:`MODEL_LOCK` ``.locked()``, which is the truth.
_HOLDER: str | None = None
_SINCE: float = 0.0


def chat_label(channel_id: Any) -> str:
    """The holder label for one channel's answer."""
    return f"{CHAT_PREFIX}{channel_id}"


def _took(label: str) -> None:
    global _HOLDER, _SINCE
    _HOLDER, _SINCE = label, time.monotonic()
    # The two instants the Limits page exists to show. Free when nobody is
    # watching, and incapable of blocking whoever is taking the lock.
    events.notify()


def _gave_back() -> None:
    global _HOLDER, _SINCE
    _HOLDER, _SINCE = None, 0.0
    events.notify()


def holder() -> dict[str, Any]:
    """Who has the model and for how long -- the portal's view of the lock.

    ``busy`` is read from the lock itself rather than from the label, so a
    holder that took :data:`MODEL_LOCK` directly still reads as busy with an
    unnamed holder. Reporting "idle" for a lock that is plainly held would be
    the one wrong answer here.
    """
    return {
        "busy": MODEL_LOCK.locked(),
        "holder": _HOLDER,
        "held_for_s": round(time.monotonic() - _SINCE, 3) if _HOLDER is not None else 0.0,
    }


@asynccontextmanager
async def held(label: str):
    """Hold :data:`MODEL_LOCK` for the block, under ``label``.

    The ordinary way to take it: waits however long it takes, and both the
    release and the label's clearing happen on the way out whatever the block
    did. Callers who would rather be turned away than wait want
    :func:`acquire_within` instead.
    """
    await MODEL_LOCK.acquire()
    _took(label)
    try:
        yield
    finally:
        _gave_back()
        MODEL_LOCK.release()


async def acquire_within(wait_s: float, label: str) -> bool:
    """Take :data:`MODEL_LOCK` under ``label`` if it comes free within ``wait_s``.

    Returns whether it was taken, so a caller who would rather shed a request
    than queue it can say how long "now" is. On a timeout **nothing is held** and
    the caller owes no release: ``wait_for`` cancels the acquisition, and a
    cancelled waiter leaves the queue rather than quietly taking the lock later.
    On success the caller owes exactly one :func:`release` -- which is the only
    correct way to give it back, since a bare ``MODEL_LOCK.release()`` would free
    the model while leaving the portal saying somebody still has it.

    ``wait_s`` of zero is not a useful argument -- ``asyncio.wait_for`` cancels a
    non-positive timeout's coroutine before it has run at all, so a free lock
    would still report as busy. Callers who want "only if it is free right now"
    should read :data:`MODEL_LOCK` ``.locked()`` in a stretch with no ``await``
    in it.
    """
    try:
        await asyncio.wait_for(MODEL_LOCK.acquire(), timeout=wait_s)
    except TimeoutError:
        return False
    _took(label)
    return True


def release() -> None:
    """Give back a lock taken with :func:`acquire_within`, label and all."""
    _gave_back()
    MODEL_LOCK.release()
