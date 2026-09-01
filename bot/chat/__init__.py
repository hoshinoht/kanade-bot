"""The speech pilot: a mention-gated, persona-driven chatbot over the schedule.

Four rules shape everything in here, and each has a module:

* it answers only when spoken to, in a channel it was told about, by somebody
  holding the chat role (:mod:`bot.chat.gate`, :mod:`bot.chat.ratelimit`);
* it never changes the schedule -- a write tool posts the same ✅/❌ proposal
  card the extractor posts, and a human ratifies it (:mod:`bot.chat.tools`);
* the model's arguments are untrusted input, re-validated against the service
  layer before anything is written (:mod:`bot.chat.tools`);
* the voice is a file on the data volume, not code (:mod:`bot.chat.persona`).

It also speaks once without being spoken to: when a card it posted is ❌'d by
the member who asked for it, it asks what that should have been instead
(:mod:`bot.chat.followup`). The answer comes back through the ordinary gate.

:mod:`bot.chat.agent` ties them together and is the only module the client
knows about.
"""

from __future__ import annotations

from .agent import ChatPilot

__all__ = ["ChatPilot"]
