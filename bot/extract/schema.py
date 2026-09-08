"""The structured output the model is constrained to (DESIGN.md §2.2).

Ollama takes a JSON schema in ``format=`` and converts it to a grammar, so the
model physically cannot emit anything else.  That removes "did it return JSON"
as a failure mode but not "did it return *sensible* JSON", so every field is
still validated here and coerced where a small model predictably slips (a bare
string where a list belongs, ``"null"`` for ``null``, a percentage for a 0-1
confidence).

The model never computes a date.  ``day_ref``/``time_ref`` carry the literal
expression it saw -- ``"weds"``, ``"9:30pm"`` -- and :mod:`bot.extract.resolve`
turns those into an instant.  Anything the model is unsure about is ``null``,
which becomes a ``TBD`` on the card rather than a guess.
"""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator

#: The amendment kinds from DESIGN.md §1, in the order the prompt lists them.
KINDS = ("move", "add", "cancel", "split", "otot", "sub", "rsvp", "fix")
RSVP_VALUES = ("yes", "no", "maybe")

Kind = Literal["move", "add", "cancel", "split", "otot", "sub", "rsvp", "fix"]
Rsvp = Literal["yes", "no", "maybe"]

#: Strings a small model reaches for when it means "nothing here".
_NULLISH = frozenset({"", "null", "none", "n/a", "na", "unknown", "tbd", "-", "?"})


def _clean(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return None if text.lower() in _NULLISH else text


class Amendment(BaseModel):
    """One proposed change, exactly as the model saw it in the conversation."""

    model_config = ConfigDict(extra="ignore")

    kind: Kind = Field(description="what kind of change this is")
    bosses: list[str] = Field(
        default_factory=list,
        description="canonical boss names from the boss table, e.g. HMaleficStar, XKalos",
    )
    day_ref: str | None = Field(
        default=None, description="the day exactly as written: weds, tmr, tonight, 2026-09-02"
    )
    time_ref: str | None = Field(
        default=None, description="the time exactly as written: 9:30pm, 930, 1030~11+pm"
    )
    participants: list[str] = Field(
        default_factory=list, description="discord user ids of the people this is about"
    )
    rsvp: Rsvp | None = Field(default=None, description="for kind=rsvp: yes, no or maybe")
    is_question: bool = Field(default=False, description="true if this was asked, not decided")
    confidence: float = Field(default=0.0, ge=0.0, le=1.0)
    evidence_message_ids: list[str] = Field(
        default_factory=list, description="the [msg_id]s this came from"
    )
    target_run_hint: str | None = Field(
        default=None, description="the short run id from the RUNS list, if one clearly matches"
    )

    # -- coercions ---------------------------------------------------------
    @field_validator("bosses", "participants", "evidence_message_ids", mode="before")
    @classmethod
    def _as_list(cls, value: Any) -> list[str]:
        """Accept a bare string, a comma-separated string, or a list."""
        if value is None:
            return []
        if isinstance(value, str):
            value = [part for part in value.replace(";", ",").split(",")]
        if not isinstance(value, (list, tuple, set)):
            value = [value]
        out: list[str] = []
        for item in value:
            text = _clean(item)
            if text is None:
                continue
            # `<@123>` -> `123`; the prompt shows mentions in that form.
            text = text.strip("<>@!")
            if text and text not in out:
                out.append(text)
        return out

    @field_validator("day_ref", "time_ref", "target_run_hint", mode="before")
    @classmethod
    def _as_optional_text(cls, value: Any) -> str | None:
        return _clean(value)

    @field_validator("rsvp", mode="before")
    @classmethod
    def _as_optional_rsvp(cls, value: Any) -> str | None:
        text = _clean(value)
        return text.lower() if text else None

    @field_validator("kind", mode="before")
    @classmethod
    def _as_kind(cls, value: Any) -> Any:
        text = _clean(value)
        return text.lower() if text else value

    @field_validator("is_question", mode="before")
    @classmethod
    def _as_bool(cls, value: Any) -> Any:
        if isinstance(value, str):
            return value.strip().lower() in ("true", "yes", "1", "y")
        return value

    @field_validator("confidence", mode="before")
    @classmethod
    def _as_confidence(cls, value: Any) -> float:
        """Accept ``0.82``, ``"0.82"``, ``82`` (a percentage) and nonsense."""
        try:
            number = float(value)
        except (TypeError, ValueError):
            return 0.0
        if number > 1.0:
            number = number / 100.0
        return min(max(number, 0.0), 1.0)


class Extraction(BaseModel):
    """Everything the model found in one burst of messages."""

    model_config = ConfigDict(extra="ignore")

    amendments: list[Amendment] = Field(default_factory=list)
    summary: str = Field(default="", description="one line a human can read")

    @field_validator("amendments", mode="before")
    @classmethod
    def _as_amendment_list(cls, value: Any) -> Any:
        if value is None:
            return []
        if isinstance(value, dict):
            return [value]
        return value

    @field_validator("summary", mode="before")
    @classmethod
    def _as_summary(cls, value: Any) -> str:
        return _clean(value) or ""


def _require_everything(node: Any) -> Any:
    """Mark every object property required, recursively.

    Ollama turns the schema into a grammar; optional properties make that grammar
    bigger and let a small model quietly omit the field that mattered.  Every
    field has a default on the Python side, so an omission is survivable -- but
    asking for all of them keeps the output shape constant, which is what the
    prompt's worked examples show.
    """
    if isinstance(node, dict):
        if node.get("type") == "object" and isinstance(node.get("properties"), dict):
            node["required"] = list(node["properties"])
        return {key: _require_everything(value) for key, value in node.items()}
    if isinstance(node, list):
        return [_require_everything(item) for item in node]
    return node


def json_schema() -> dict:
    """The schema handed to Ollama's ``format=``."""
    return _require_everything(Extraction.model_json_schema())


__all__ = ["KINDS", "RSVP_VALUES", "Amendment", "Extraction", "Kind", "Rsvp", "json_schema"]
