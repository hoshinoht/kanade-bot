"""Small helpers shared by the command and event layers."""

from __future__ import annotations

import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from typing import Any

_MENTION_RE = re.compile(r"<@!?(\d+)>")
_ROLE_MENTION_RE = re.compile(r"<@&(\d+)>")
_BARE_ID_RE = re.compile(r"\b(\d{15,25})\b")


def positive_int(raw: str | None, default: int) -> int:
    """A stored number, or ``default`` when the row is absent or nonsense.

    Runtime config is text in a SQLite column, and the row is written by
    validated paths only -- but a hand-edited database must not take the bot
    down, and a limit of zero is worse than the default it replaced. Anything
    unparseable or non-positive falls back rather than raising, because these
    are read on the hot path of every message.
    """
    try:
        value = int(str(raw).strip())
    except (TypeError, ValueError):
        return default
    return value if value >= 1 else default


def positive_float(raw: str | None, default: float) -> float:
    """A stored window in seconds, or ``default``. See :func:`positive_int`."""
    try:
        value = float(str(raw).strip())
    except (TypeError, ValueError):
        return default
    return value if value > 0 else default


def is_bot_admin(
    is_guild_admin: bool,
    is_guild_owner: bool,
    role_ids: Iterable[int],
    admin_role_id: int | None,
) -> bool:
    """Who runs this bot: the configured admin role, the owner, or a server admin.

    ``ADMIN_ROLE_ID`` is the point of it -- a role the guild hands out, which is
    how the guild already thinks about who is in charge. Discord's own
    Administrator permission and the guild owner stay as fallbacks so an unset
    or mistyped role id cannot lock the owner out of their own bot.

    ``is_guild_admin`` has to be passed in because no id implies it: it comes
    from ``member.guild_permissions.administrator``. Kept as a pure predicate so
    the rule is testable without a gateway.
    """
    if is_guild_admin or is_guild_owner:
        return True
    return admin_role_id is not None and int(admin_role_id) in [int(r) for r in role_ids]


def mentions_in(text: str | None) -> tuple[list[str], list[str]]:
    """The user and role mentions actually written in a message: ``(users, roles)``.

    Unlike :func:`parse_mentions`, a bare snowflake does not count. This decides
    who a message is allowed to *notify*, and Discord only notifies what it
    renders as a mention -- a number in a sentence is a number.
    """
    if not text:
        return [], []
    users = list(dict.fromkeys(m.group(1) for m in _MENTION_RE.finditer(text)))
    roles = list(dict.fromkeys(m.group(1) for m in _ROLE_MENTION_RE.finditer(text)))
    return users, roles


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
