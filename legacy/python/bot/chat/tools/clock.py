"""Access the facade-owned wall-clock seam without binding it in child modules."""

from __future__ import annotations

from datetime import datetime


def utcnow() -> datetime:
    """Return the current facade clock, preserving ``tools.utcnow`` monkeypatches."""
    # Imported only at call time to avoid the package initialisation cycle and to
    # observe a test's replacement of ``bot.chat.tools.utcnow``.
    from . import utcnow as facade_utcnow

    return facade_utcnow()
