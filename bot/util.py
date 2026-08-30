"""Small helpers shared by the command and event layers."""

from __future__ import annotations

import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from typing import Any

_MENTION_RE = re.compile(r"<@!?(\d+)>")
_BARE_ID_RE = re.compile(r"\b(\d{15,25})\b")


def mention(user_id: int | str) -> str:
    return f"<@{user_id}>"


def mentions(user_ids: list[str]) -> str:
    return " ".join(mention(uid) for uid in user_ids)


def parse_mentions(text: str | None) -> list[str]:
    """Pull user ids out of a free-text field of ``@mentions``.

    Accepts ``<@123>``/``<@!123>`` (what Discord actually sends) and bare
    snowflakes, preserving order and dropping duplicates.
    """
    if not text:
        return []
    out: list[str] = []
    for match in _MENTION_RE.finditer(text):
        if match.group(1) not in out:
            out.append(match.group(1))
    stripped = _MENTION_RE.sub(" ", text)
    for match in _BARE_ID_RE.finditer(stripped):
        if match.group(1) not in out:
            out.append(match.group(1))
    return out


def can_modify_run(
    run: dict, user_id: int | str, is_admin: bool = False, owner_id: str | None = None
) -> bool:
    """A run may be changed by its participants, its owner, or an admin.

    ``owner_id`` is the owner of the fixed run this run was materialised from,
    looked up by the caller (it is not stored on the run itself).
    """
    if is_admin:
        return True
    user_id = str(user_id)
    return user_id in run["participants"] or (owner_id is not None and user_id == str(owner_id))


def can_modify_fixed(fixed: dict, user_id: int | str, is_admin: bool = False) -> bool:
    """A fixed run may be changed by its owner, its participants, or an admin."""
    if is_admin:
        return True
    user_id = str(user_id)
    return user_id == fixed["owner_id"] or user_id in fixed["participants"]


def roster_rows(members: Iterable[Any]) -> list[tuple[str, str, str | None, bool]]:
    """Rows for :meth:`bot.db.Repo.sync_roster` from the bossing role's members.

    Bot accounts are dropped: a webhook or another bot holding the role must
    never become a run participant or get pinged.
    """
    return [
        (str(m.id), m.display_name, m.nick, True) for m in members if not getattr(m, "bot", False)
    ]


_TOKEN_SPLIT_RE = re.compile(r"[,\s]+")


@dataclass
class NameResolution:
    """Outcome of turning a free-text participants field into user ids."""

    ids: list[str] = field(default_factory=list)
    #: tokens that matched nobody on the roster
    unknown: list[str] = field(default_factory=list)
    #: token -> the display names it could have meant
    ambiguous: dict[str, list[str]] = field(default_factory=dict)

    @property
    def ok(self) -> bool:
        return not self.unknown and not self.ambiguous


def _candidate_names(member: dict) -> list[str]:
    names = [member.get("display_name") or "", member.get("nickname") or ""]
    names.extend(member.get("aliases") or [])
    return [n for n in names if n]


def match_roster(token: str, roster: Sequence[dict]) -> list[dict]:
    """Members whose name, nickname or `/nick` alias matches ``token``.

    Tried in order of decreasing confidence -- exact, then prefix, then
    substring -- and the first tier that hits anything wins. Display names carry
    guild decoration ("kanon [AZUR]", "Alvin tan"), so requiring an exact match
    would reject the names people actually type.
    """
    low = token.strip().lower()
    if not low:
        return []
    tests = (
        lambda c: c.lower() == low,
        lambda c: c.lower().startswith(low),
        lambda c: low in c.lower(),
    )
    for test in tests:
        hits: list[dict] = []
        for member in roster:
            if any(test(name) for name in _candidate_names(member)) and member not in hits:
                hits.append(member)
        if hits:
            return hits
    return []


def resolve_participant_text(text: str | None, roster: Sequence[dict]) -> NameResolution:
    """Turn a participants field into user ids, preserving order.

    Accepts ``<@id>`` mentions and bare snowflakes (what Discord's picker and a
    copied id give you) and, for anything left over, plain names typed by hand.
    """
    result = NameResolution()
    if not text:
        return result

    def add(uid: str) -> None:
        if uid not in result.ids:
            result.ids.append(uid)

    for match in _MENTION_RE.finditer(text):
        add(match.group(1))
    remaining = _MENTION_RE.sub(" ", text)
    for match in _BARE_ID_RE.finditer(remaining):
        add(match.group(1))
    remaining = _BARE_ID_RE.sub(" ", remaining)

    for raw_token in _TOKEN_SPLIT_RE.split(remaining):
        token = raw_token.strip().lstrip("@").strip()
        if not token:
            continue
        matches = match_roster(token, roster)
        if not matches:
            if token not in result.unknown:
                result.unknown.append(token)
        elif len(matches) > 1:
            result.ambiguous[token] = [m.get("display_name") or m["user_id"] for m in matches]
        else:
            add(str(matches[0]["user_id"]))
    return result
