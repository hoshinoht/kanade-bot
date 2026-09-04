"""Boss table and token parsing.

Chat and slash-command input names bosses as ``nstar``, ``HFA``, ``xkalos``,
``hcarl``.  A token is a single-letter difficulty prefix (``e/n/h/c/x``) followed
by a boss alias; the canonical form the bot stores is the uppercased prefix plus
the boss's short name, e.g. ``HStar``, ``HFA``, ``XKalos``, ``NCarling``.

The difficulty may also be spelled out as a word in front of the boss --
``Hard Baldrix``, ``Extreme Kalos`` -- which is how members say it out loud and
how the chatbot repeats it back.  Such a pair is folded into the prefixed form
before anything is resolved, so both spellings meet the same two refusals.

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
#: Every image of a boss is looked up as ``<key>.<ext>`` over these, in this
#: order -- portraits, their small renders, and the entry artwork alike. The one
#: exception is a portrait named outright by ``portrait:`` in ``bosses.yaml``.
PORTRAIT_SUFFIXES = (".png", ".webp", ".jpg", ".jpeg")
#: The small render of a portrait, under ``PORTRAIT_DIR``: the 64px files the
#: portal drew before the full-size art arrived. Still the right picture for a
#: 26px badge, where the large one is a download and then a blur.
PORTRAIT_ICON_DIR = "icon"

#: Where the entry artwork lives, relative to ``bosses.yaml``. A different
#: picture from a portrait and a different shape -- the wide splash the game
#: shows behind a boss's entry prompt -- so it gets a directory of its own
#: rather than a second naming convention inside ``portraits/``.
ENTRY_ART_DIR = "artwork/entry"


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

    def _fold_difficulty_words(self, tokens: list[str]) -> list[str]:
        """``["Hard", "Baldrix"]`` -> ``["hBaldrix"]``, left to right.

        Members say the difficulty out loud, and the chatbot says it back to
        them: live, "schedule a Hard Baldrix run tonight" was refused as a bare
        boss name, so the bot asked which difficulty it should be after being
        told. A word is folded only when a boss alias actually follows it, which
        leaves a stray "hard" an unknown boss rather than a silent prefix.
        """
        letters = {word.lower(): letter for letter, word in self.difficulties.items()}
        out: list[str] = []
        index = 0
        while index < len(tokens):
            token = tokens[index]
            letter = letters.get(token.lower())
            nxt = tokens[index + 1] if index + 1 < len(tokens) else None
            if letter and nxt is not None and _normalise(nxt) in self.aliases:
                # The prefixed form, not the resolved boss: `parse_token` still
                # has to reject "Chaos Seren", exactly as it rejects `cseren`.
                out.append(letter + nxt)
                index += 2
                continue
            out.append(token)
            index += 1
        return out

    def parse(self, text: str) -> list[str]:
        """Parse a comma/space separated list of tokens, preserving order.

        Every bad token is reported at once so the user can fix them in one go.
        """
        tokens = [t for t in _SPLIT_RE.split(text or "") if t]
        if not tokens:
            raise BossParseError("no bosses given")
        tokens = self._fold_difficulty_words(tokens)
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

    def names_in(self, text: str) -> list[str]:
        """The bosses a loose sentence names, by short name, in the order said.

        Difficulty-agnostic and forgiving where :meth:`parse` is neither, because
        the question is a different one: not "which run did they mean" but "did
        they name a boss at all". So ``hard jupiter tue``, ``hjup`` and
        ``jupiter`` all name Jupiter, and a weekday, a stray word or a run id
        names nothing.

        What it exists for: a search that finds no weekly timing has to tell a
        query that named a boss with none (say so, and stop) from one that named
        no boss at all (ask which boss they meant). Falling back to the day token
        in the first case listed three other parties' Tuesday nights back to a
        member who had asked about Jupiter.
        """
        out: list[str] = []
        for token in _SPLIT_RE.split(text or ""):
            key = _normalise(token)
            if not key:
                continue
            short = self.aliases.get(key)
            if short is None and key[:1] in self.difficulties:
                short = self.aliases.get(key[1:])
            if short is not None and short not in out:
                out.append(short)
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

    @staticmethod
    def _named_file(directory: Path, stem: str) -> Path | None:
        """``<stem>.<ext>`` in one directory, over the extensions we accept."""
        for suffix in PORTRAIT_SUFFIXES:
            candidate = directory / f"{stem}{suffix}"
            if candidate.is_file():
                return candidate
        return None

    def portrait_path(self, short: str, size: str = "full") -> Path | None:
        """The portrait file for a boss, or ``None`` when there isn't one.

        Portraits are entirely optional: ``config/portraits/Star.png`` (the
        ``bosses.yaml`` key), or whatever ``portrait:`` names. The filename is
        never taken from user input -- it is resolved from the table -- so this
        cannot be walked out of the config directory.

        Two renders of the same boss, and the *caller* chooses. ``full`` is the
        picture in ``portraits/``; ``icon`` is the small one under
        ``portraits/icon/``, which is what a badge should be fetching now that
        the full file is artwork rather than a thumbnail. An ``icon`` that is
        not there falls back to the full picture, so a boss added today draws
        correctly before anybody has cropped one -- but ``full`` never looks
        inside ``icon/``, so asking for the big one cannot quietly serve the
        small one to something that needed the detail.

        The ``portrait:`` override belongs to the full size alone: it is one
        line naming one file, and there is no second line to name a small one.
        ``icon/`` is filename-by-key, exactly like the entry artwork.
        """
        boss = self.bosses.get(short)
        if boss is None or self.base_dir is None:
            return None
        directory = self.base_dir / PORTRAIT_DIR
        if size == "icon":
            icon = self._named_file(directory / PORTRAIT_ICON_DIR, boss.short)
            if icon is not None:
                return icon
        if boss.portrait:
            candidate = directory / Path(boss.portrait).name
            return candidate if candidate.is_file() else None
        return self._named_file(directory, boss.short)

    def portrait_for(self, canonical: str) -> Path | None:
        """The portrait for a canonical name like ``"HStar"``, at full size.

        Which is what a card in Discord attaches. Nothing reaches the small
        render through here: the portal asks :meth:`portrait_path` for the size
        it wants, and every size it wants is a badge.
        """
        parts = self.split(canonical)
        return self.portrait_path(parts[1].short) if parts else None

    def entry_art_path(self, short: str) -> Path | None:
        """The entry artwork for a boss, or ``None`` when there isn't one.

        The same deal as :meth:`portrait_path` one directory along, and just as
        optional: ``config/artwork/entry/Seren.png``, named after the
        ``bosses.yaml`` key. There is no ``bosses.yaml`` override to go with it
        -- one splash per boss, named by key, is the whole rule -- and the
        filename is still resolved from the table rather than from anything
        typed, so this cannot be walked out of the config directory either.
        """
        boss = self.bosses.get(short)
        if boss is None or self.base_dir is None:
            return None
        return self._named_file(self.base_dir / ENTRY_ART_DIR, boss.short)

    def entry_art_for(self, canonical: str) -> Path | None:
        """The entry artwork for a canonical name like ``"HStar"``."""
        parts = self.split(canonical)
        return self.entry_art_path(parts[1].short) if parts else None

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
