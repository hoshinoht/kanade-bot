"""Chat extraction: gate -> prompt -> local LLM -> deterministic resolve -> card.

The model is never the authority (DESIGN.md §2).  Everything it emits is a
*literal expression it saw* ("weds", "9:30pm"); turning that into a datetime, a
run and a schedule change is done here in Python, and the result is posted as a
card that a participant has to ✅ before anything is written to the schedule.

Modules, in pipeline order:

``gate``      deterministic keyword gate + boss-token finder (no LLM)
``prompt``    renders the burst, the channel's runs and the roster into a prompt
``schema``    the Pydantic model that `format=` constrains the model to
``llm``       one guarded, serialised call to Ollama
``resolve``   ``day_ref``/``time_ref`` -> an aware datetime (never the model's job)
``merge``     one burst's per-message pieces -> one candidate per run
``match``     amendment -> the run it is about
``pipeline``  per-channel buffering, and the whole flow on flush
``commit``    ✅/❌ on a proposal card -> the schedule change
"""

from __future__ import annotations

__all__ = [
    "commit",
    "gate",
    "llm",
    "match",
    "merge",
    "pipeline",
    "prompt",
    "resolve",
    "schema",
]
