"""Request and response shapes for ``/api``.

Response models double as the contract ``bossctl`` reads: FastAPI filters every
handler's return value down to the fields declared here, so a view growing an
extra key never silently changes the JSON.  Requests are strict -- an unknown
field is a typo worth reporting, not something to ignore.
"""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field

Week = Literal["this", "next"]
Answer = Literal["yes", "no", "maybe"]


class Strict(BaseModel):
    """Request bodies: reject anything not declared."""

    model_config = ConfigDict(extra="forbid")


class ErrorOut(BaseModel):
    error: str


# --- schedule ---------------------------------------------------------------


class MonogramOut(BaseModel):
    """The stand-in badge shown when a boss has no portrait file."""

    text: str
    hue: int


class BossOut(BaseModel):
    token: str
    short: str
    full: str
    level: int | None
    #: The difficulty prefix (``H``) and its in-game word (``HARD``).
    letter: str
    difficulty: str
    label: str
    #: ``/static/portraits/<short>`` when `config/portraits/` has a file.
    portrait: str | None = None
    monogram: MonogramOut | None = None


class ParticipantOut(BaseModel):
    id: str
    name: str
    rsvp: Answer | None = None


class RunOut(BaseModel):
    id: str
    short_id: str
    bosses: list[str]
    boss_detail: list[BossOut]
    datetime: str
    local_date: str
    local_day: str
    local_time: str
    weekday: int
    status: str
    status_label: str
    source: str
    fixed_run_id: str | None
    channel_id: str | None
    channel_name: str | None
    week_start: str
    participants: list[ParticipantOut]
    yes: int
    no: int
    unanswered: int


class DayOut(BaseModel):
    heading: str
    runs: list[RunOut]


class ScheduleOut(BaseModel):
    week: str
    week_start: str
    week_label: str
    timezone: str
    show_past: bool
    #: How many `done`/`cancelled` runs were left out, so the reader is told.
    hidden: int
    days: list[DayOut]
    runs: list[RunOut]
    count: int


# --- fixed runs -------------------------------------------------------------


class FixedOut(BaseModel):
    id: str
    short_id: str
    owner_id: str
    owner_name: str
    channel_id: str | None
    channel_name: str | None
    channel_watched: bool
    bosses: list[str]
    boss_detail: list[BossOut]
    weekday: int
    weekday_name: str
    time: str
    participants: list[ParticipantOut]
    note: str | None


class FixedCreate(Strict):
    bosses: str = Field(description="e.g. `hstar, hfa` - each needs a difficulty prefix")
    day: str = Field(description="`mon`..`sun`, or 0-6 with Monday as 0")
    time: str = Field(description="HH:MM in the guild timezone")
    participants: list[str] = Field(min_length=1, description="Discord user ids")
    channel_id: str = Field(description="the run's home channel; must be watched")
    owner_id: str | None = None
    note: str | None = None


class FixedUpdate(Strict):
    bosses: str | None = None
    day: str | None = None
    time: str | None = None
    participants: list[str] | None = None
    channel_id: str | None = None
    note: str | None = None


class DeletedOut(BaseModel):
    id: str
    short_id: str
    cancelled_runs: int


# --- run actions ------------------------------------------------------------


class AmendIn(Strict):
    to: str = Field(description="e.g. `wed 21:30`, `tomorrow 9:45pm`")


class RsvpIn(Strict):
    user_id: str
    answer: Answer


#: `at_risk` is derived from the answers people give, and `done` is also set
#: automatically once a run's night has passed; neither is set by hand here.
RunStatus = Literal["planned", "confirmed", "otot", "done", "cancelled"]


class StatusIn(Strict):
    status: RunStatus


# --- the inbox --------------------------------------------------------------


class EvidenceOut(BaseModel):
    id: str
    missing: bool
    author_id: str | None = None
    author_name: str | None = None
    created_at: str | None = None
    local_time: str | None = None
    content: str | None = None
    url: str | None = None


class AmendmentOut(BaseModel):
    id: str
    short_id: str
    kind: str
    kind_label: str
    status: str
    bosses: list[str]
    boss_detail: list[BossOut] = []
    run_id: str | None
    run: RunOut | None
    new_datetime: str | None
    when: str
    day_ref: str | None
    time_ref: str | None
    confidence: float | None
    is_question: bool
    summary: str | None
    rsvp: str | None
    payload: dict[str, Any]
    channel_id: str | None
    channel_name: str | None
    created_at: str
    week_start: str
    participants: list[ParticipantOut]
    card_url: str | None
    evidence: list[EvidenceOut] = []


class ApproveIn(Strict):
    actor_id: str | None = Field(
        default=None,
        description="who to credit; defaults to PORTAL_ACTOR_ID, else the guild owner",
    )


class ApproveOut(BaseModel):
    id: str
    short_id: str
    kind: str
    applied: bool
    actor_id: str
    run_id: str | None
    fixed_run_id: str | None
    created_run_ids: list[str]
    superseded: list[str]


class RejectOut(BaseModel):
    id: str
    short_id: str
    status: str


# --- extraction log ---------------------------------------------------------


class ExtractionOut(BaseModel):
    id: str
    short_id: str
    at: str
    local_time: str
    model: str
    latency_ms: int | None
    message_ids: list[str]
    amendment_count: int


class ExtractionDetailOut(ExtractionOut):
    prompt: str
    raw_response: str
    messages: list[EvidenceOut]
    amendments: list[AmendmentOut]


# --- members, reminders -----------------------------------------------------


class MemberOut(BaseModel):
    user_id: str
    display_name: str
    nickname: str | None
    aliases: list[str]
    has_role: bool
    updated_at: str
    runs_this_week: int


class NickIn(Strict):
    alias: str


class NickOut(BaseModel):
    user_id: str
    name: str
    aliases: list[str]


class ReminderOut(BaseModel):
    id: str
    short_id: str
    run_id: str
    run_short_id: str
    kind: str
    fire_at: str
    local_fire_at: str
    sent_at: str | None
    message_id: str | None
    url: str | None
    bosses: list[str]
    boss_detail: list[BossOut] = []
    run_local: str | None
    status: str | None


# --- config and actions -----------------------------------------------------


class ConfigOut(BaseModel):
    day_of_ping_time: str
    countdown_minutes: str
    paused: bool
    extract_enabled: bool
    timezone: str
    reset: str
    model: str
    ollama_host: str
    min_confidence: float
    post_channel_id: str | None
    guild_id: str
    watched_channels: list[str]
    watched_categories: list[str]


class ConfigIn(Strict):
    """Only the runtime-editable keys; everything else is a redeploy."""

    day_of_ping_time: str | None = None
    countdown_minutes: str | None = None
    paused: bool | None = None
    extract_enabled: bool | None = None


class DigestIn(Strict):
    channel_id: str | None = None
    week: Week = "this"


class DigestOut(BaseModel):
    posted: bool
    week: str
    channel_id: str | None
    message_id: str
    url: str | None


class RescanIn(Strict):
    #: Empty (or absent) means every watched channel.
    channels: list[str] = []
    window: Literal["week", "2weeks", "48h", "24h"] = "week"
    post: bool = True


class ProposedOut(BaseModel):
    kind: str
    bosses: list[str]
    confidence: float
    run_id: str | None


class ChannelRescanOut(BaseModel):
    """What re-reading one channel did."""

    channel_id: str
    channel_name: str
    asked: bool
    window: str
    since: str
    #: True when the week held no scheduling chat and the search widened once.
    widened: bool = False
    backfilled: int = 0
    stored: int = 0
    gated: int = 0
    bursts: int = 0
    extracted: int = 0
    proposals: int = 0
    dropped: int = 0
    stale: int = 0
    elapsed_ms: int = 0
    error: str | None = None
    summary: str = ""
    proposed: list[ProposedOut] = []


class RescanOut(BaseModel):
    window: str
    channels: list[ChannelRescanOut]
    asked: bool
    widened: bool = False
    backfilled: int = 0
    gated: int = 0
    bursts: int = 0
    extracted: int = 0
    proposals: int = 0
    dropped: int = 0
    stale: int = 0
    elapsed_ms: int = 0
    errors: list[str] = []
    proposed: list[ProposedOut] = []


class RescanTargetOut(BaseModel):
    id: str
    name: str
    has_runs: bool


class PingIn(Strict):
    run_id: str
    kind: str = "day_of"


class PingOut(BaseModel):
    run_id: str
    kind: str
    channel_id: str | None
    message_id: str
    url: str | None


class AccessOut(BaseModel):
    """What the bot may actually do in one channel."""

    id: str
    name: str
    watched: bool
    is_digest_channel: bool
    view: bool
    send: bool
    history: bool
    embed: bool
    react: bool
    #: True when the bot is not connected, so nothing could be checked.
    unknown: bool


class HealthOut(BaseModel):
    status: str


__all__ = [
    "AccessOut",
    "AmendIn",
    "AmendmentOut",
    "ApproveIn",
    "ApproveOut",
    "BossOut",
    "ConfigIn",
    "ConfigOut",
    "DayOut",
    "DeletedOut",
    "DigestIn",
    "DigestOut",
    "ErrorOut",
    "EvidenceOut",
    "ExtractionDetailOut",
    "ExtractionOut",
    "FixedCreate",
    "FixedOut",
    "FixedUpdate",
    "HealthOut",
    "MemberOut",
    "MonogramOut",
    "NickIn",
    "NickOut",
    "ParticipantOut",
    "PingIn",
    "PingOut",
    "ProposedOut",
    "RejectOut",
    "ReminderOut",
    "ChannelRescanOut",
    "RescanIn",
    "RescanOut",
    "RescanTargetOut",
    "RsvpIn",
    "RunStatus",
    "StatusIn",
    "RunOut",
    "ScheduleOut",
]
