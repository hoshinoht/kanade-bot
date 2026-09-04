"""UUID identifiers, and the short forms people actually type.

Scheduling rows are keyed by uuid4 rather than autoincrement integers, so ids
are stable across exports, rebuilds and any future merge of two databases, and
can never be guessed by counting.

A full uuid is unusable in chat, so everything displayed uses the first eight hex
characters (``#a1b2c3d4``) and every command that takes an id accepts any unique
prefix. Autocomplete means people rarely type one at all.
"""

from __future__ import annotations

import uuid
from collections.abc import Iterable

#: Shortest prefix accepted from a user. Four hex chars is 65k values -- ample
#: for one guild's runs, and short enough to type from a phone.
MIN_PREFIX = 4
SHORT_LENGTH = 8


class IdError(ValueError):
    """Base class for id-resolution problems."""


class IdTooShort(IdError):
    pass


class IdNotFound(IdError):
    pass


class IdAmbiguous(IdError):
    """More than one row starts with the given prefix."""

    def __init__(self, prefix: str, candidates: list[str]):
        self.prefix = prefix
        self.candidates = candidates
        super().__init__(f"`{prefix}` matches {len(candidates)} rows")


def new_id() -> str:
    """A fresh uuid4, as its canonical dashed string."""
    return str(uuid.uuid4())


def canonical(value: str) -> str:
    """Hex-only, lowercase form used for comparison (dashes and `#` stripped)."""
    return str(value).strip().lstrip("#").replace("-", "").lower()


def short_id(value: str) -> str:
    """The display form: first eight hex characters, e.g. ``a1b2c3d4``."""
    return canonical(value)[:SHORT_LENGTH]


def tag(value: str) -> str:
    """The display form with its `#`, e.g. ``#a1b2c3d4``."""
    return f"#{short_id(value)}"


def resolve_id(text: str, candidates: Iterable[str]) -> str:
    """Resolve a full uuid or a unique prefix to exactly one candidate id.

    Case-insensitive; a leading ``#`` and any dashes are ignored, so everything
    the bot itself prints can be pasted straight back in.
    """
    prefix = canonical(text)
    if len(prefix) < MIN_PREFIX:
        raise IdTooShort(f"`{text}` is too short - give at least {MIN_PREFIX} characters")
    ids = list(candidates)
    for candidate in ids:
        if canonical(candidate) == prefix:
            return candidate
    matches = [c for c in ids if canonical(c).startswith(prefix)]
    if not matches:
        raise IdNotFound(f"nothing matches `{text}`")
    if len(matches) > 1:
        raise IdAmbiguous(text, matches)
    return matches[0]
