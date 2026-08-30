"""Boss table and token parsing.

Chat and slash-command input names bosses as ``nstar``, ``HFA``, ``xkalos``,
``hcarl``.  A token is a single-letter difficulty prefix (``e/n/h/c/x``) followed
by a boss alias; the canonical form the bot stores is the uppercased prefix plus
the boss's short name, e.g. ``HStar``, ``HFA``, ``XKalos``, ``NCarling``.

Two things are rejected rather than guessed:

* a token with no difficulty prefix -- the guild runs Normal and Hard of the same
  boss on different days, so picking one would be wrong;
* a prefix that boss does not have in game (``hkalos``) -- that would schedule a
  run nobody can enter.

Both errors list the forms that would have worked.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

import yaml

_NORMALISE_RE = re.compile(r"[^a-z0-9]+")
_SPLIT_RE = re.compile(r"[,\s/+&]+")

#: Where portraits live, relative to ``bosses.yaml``. The whole ``config/``
#: directory is bind-mounted read-only, so dropping a file in is enough --
#: no rebuild, no restart.
PORTRAIT_DIR = "portraits"
#: Tried in order for ``<short>.<ext>`` when no explicit ``portrait:`` is set.
PORTRAIT_SUFFIXES = (".png", ".webp", ".jpg", ".jpeg")


class BossParseError(ValueError):
    """Raised when one or more tokens cannot be resolved to a canonical boss."""


class BossTableError(ValueError):
    """Raised when ``bosses.yaml`` itself is malformed."""


def _normalise(token: str) -> str:
    return _NORMALISE_RE.sub("", token.strip().lower())


@dataclass(frozen=True)
class Boss:
    """One row of ``bosses.yaml``."""

    short: str
    full: str
    level: int | None
    #: Difficulty prefixes this boss actually has, in table order.
    difficulties: tuple[str, ...]
    aliases: tuple[str, ...]
    #: An explicit portrait filename from ``bosses.yaml``; otherwise the file is
    #: looked up as ``<short>.png`` and friends.
    portrait: str | None = None

    def canonical(self, letter: str) -> str:
        return f"{letter.upper()}{self.short}"


@dataclass(frozen=True)
class BossTable:
    """Immutable boss table loaded from ``config/bosses.yaml``."""

    difficulties: dict[str, str]
    bosses: dict[str, Boss]
    #: normalised alias -> short name
    aliases: dict[str, str]
    #: The directory ``bosses.yaml`` was loaded from; portraits live under it.
    base_dir: Path | None = None

    @classmethod
    def load(cls, path: str | Path) -> BossTable:
        path = Path(path)
        raw = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
        return cls.from_dict(raw, base_dir=path.parent)

    @classmethod
    def from_dict(cls, raw: dict, base_dir: Path | None = None) -> BossTable:
        difficulties = {str(k).lower(): str(v) for k, v in (raw.get("difficulties") or {}).items()}
        if not difficulties:
            raise BossTableError("bosses.yaml has no `difficulties:` map")
        for letter in difficulties:
            if len(letter) != 1 or not letter.isalpha():
                raise BossTableError(f"difficulty prefix {letter!r} must be a single letter")

        raw_bosses = raw.get("bosses") or {}
        if not raw_bosses:
            raise BossTableError("bosses.yaml has no `bosses:` map")

        bosses: dict[str, Boss] = {}
        aliases: dict[str, str] = {}
        for short, spec in raw_bosses.items():
            spec = spec or {}
            short = str(short)
            allowed = tuple(str(d).lower() for d in (spec.get("difficulties") or difficulties))
            unknown = [d for d in allowed if d not in difficulties]
            if unknown:
                raise BossTableError(
                    f"{short} lists difficulty prefix(es) {unknown} that are not in `difficulties:`"
                )
            level = spec.get("level")
            portrait = spec.get("portrait")
            boss = Boss(
                short=short,
                full=str(spec.get("full") or short),
                level=int(level) if level is not None else None,
                difficulties=allowed,
                aliases=tuple(str(a) for a in (spec.get("aliases") or [])),
                portrait=str(portrait) if portrait else None,
            )
            bosses[short] = boss
            for name in (short, boss.full, *boss.aliases):
                key = _normalise(name)
                if not key:
                    continue
                if key in aliases and aliases[key] != short:
                    raise BossTableError(
                        f"alias {name!r} is claimed by both {aliases[key]!r} and {short!r}"
                    )
                aliases[key] = short
        return cls(difficulties=difficulties, bosses=bosses, aliases=aliases, base_dir=base_dir)

    # -- lookups ----------------------------------------------------------
    @property
    def prefixes(self) -> str:
        return "/".join(self.difficulties)

    def valid_forms(self, short: str) -> str:
        """The canonical names this boss actually has, e.g. ``EKalos, NKalos, ...``."""
        boss = self.bosses[short]
        return ", ".join(boss.canonical(letter) for letter in boss.difficulties)

    # -- parsing ----------------------------------------------------------
    def parse_token(self, token: str) -> str:
        """Resolve a single token, e.g. ``ncarling`` -> ``NCarling``."""
        key = _normalise(token)
        if not key:
            raise BossParseError("empty boss token")
        if key in self.aliases:
            raise BossParseError(
                f"`{token}` is missing a difficulty prefix "
                f"({self.prefixes}) - try {self.valid_forms(self.aliases[key])}"
            )
        letter, rest = key[0], key[1:]
        if letter in self.difficulties and rest in self.aliases:
            short = self.aliases[rest]
            boss = self.bosses[short]
            if letter not in boss.difficulties:
                raise BossParseError(
                    f"{boss.full} has no {self.difficulties[letter]} difficulty - "
                    f"did you mean {self.valid_forms(short)}?"
                )
            return boss.canonical(letter)
        raise BossParseError(f"unknown boss `{token}`")

    def parse(self, text: str) -> list[str]:
        """Parse a comma/space separated list of tokens, preserving order.

        Every bad token is reported at once so the user can fix them in one go.
        """
        tokens = [t for t in _SPLIT_RE.split(text or "") if t]
        if not tokens:
            raise BossParseError("no bosses given")
        out: list[str] = []
        problems: list[str] = []
        for token in tokens:
            try:
                canonical = self.parse_token(token)
            except BossParseError as exc:
                problems.append(str(exc))
                continue
            if canonical not in out:
                out.append(canonical)
        if problems:
            raise BossParseError("; ".join(problems))
        return out

    # -- display ----------------------------------------------------------
    def split(self, canonical: str) -> tuple[str, Boss] | None:
        """``"HFA"`` -> ``("h", <Boss FA>)``, or ``None`` if it is not ours."""
        letter, short = canonical[:1].lower(), canonical[1:]
        boss = self.bosses.get(short)
        if letter not in self.difficulties or boss is None:
            return None
        return letter, boss

    def describe(self, canonical: str) -> str:
        """``"HFA"`` -> ``"The First Adversary (Hard, Lv270)"``."""
        parts = self.split(canonical)
        if parts is None:
            return canonical
        letter, boss = parts
        detail = self.difficulties[letter]
        if boss.level is not None:
            detail += f", Lv{boss.level}"
        return f"{boss.full} ({detail})"

    def describe_all(self, canonicals: list[str]) -> str:
        return " · ".join(self.describe(name) for name in canonicals)

    def portrait_path(self, short: str) -> Path | None:
        """The portrait file for a boss, or ``None`` when there isn't one.

        Portraits are entirely optional: ``config/portraits/Star.png`` (the
        ``bosses.yaml`` key), or whatever ``portrait:`` names. The filename is
        never taken from user input -- it is resolved from the table -- so this
        cannot be walked out of the config directory.
        """
        boss = self.bosses.get(short)
        if boss is None or self.base_dir is None:
            return None
        directory = self.base_dir / PORTRAIT_DIR
        if boss.portrait:
            candidate = directory / Path(boss.portrait).name
            return candidate if candidate.is_file() else None
        for suffix in PORTRAIT_SUFFIXES:
            candidate = directory / f"{boss.short}{suffix}"
            if candidate.is_file():
                return candidate
        return None

    def portrait_for(self, canonical: str) -> Path | None:
        """The portrait for a canonical name like ``"HStar"``."""
        parts = self.split(canonical)
        return self.portrait_path(parts[1].short) if parts else None

    def difficulty_name(self, letter: str) -> str:
        """``"h"`` -> ``"Hard"``. Unknown letters come back as themselves."""
        return self.difficulties.get(letter.lower(), letter.upper())

    def ordered(self) -> list[Boss]:
        """Every boss in the order the in-game list uses: by level, then name.

        The portal's boss grid renders this, so adding a boss to
        ``bosses.yaml`` puts it in the right place with no code change.
        """
        return sorted(self.bosses.values(), key=lambda b: (b.level or 0, b.short))

    def detail(self, canonical: str) -> dict | None:
        """The parts of a canonical name, for anything that renders it richly.

        ``"HStar"`` -> full name, the difficulty as a word, the level and the
        prefix letter -- what the difficulty pill and the boss grid need, rather
        than the single pre-formatted string :meth:`describe` returns.
        """
        parts = self.split(canonical)
        if parts is None:
            return None
        letter, boss = parts
        return {
            "token": canonical,
            "short": boss.short,
            "full": boss.full,
            "level": boss.level,
            "letter": letter,
            "difficulty": self.difficulties[letter],
        }
