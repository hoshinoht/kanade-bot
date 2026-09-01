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
    #: ``/static/portraits/<short>?size=icon`` when `config/portraits/` has a
    #: file. The small render, because every portrait either surface draws is a
    #: badge; drop the query for the full picture, which is what Discord
    #: attaches to a card.
    portrait: str | None = None
    monogram: MonogramOut | None = None


class ParticipantOut(BaseModel):
    id: str
    name: str
    rsvp: Answer | None = None


class RunCardOut(BaseModel):
    """One reminder message a run has produced, or still owes.

    ``state`` is `posted` (``url`` opens it in Discord), `queued` (nothing said
    yet) or `skipped` -- retired without a message, which is what a sleeping
    host or a cancelled run leaves behind.
    """

    kind: str
    label: str
    state: Literal["posted", "queued", "skipped"]
    fire_at: str
    local_fire_at: str
    sent_at: str | None = None
    local_sent_at: str | None = None
    message_id: str | None = None
    url: str | None = None


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
    maybe: int = 0
    unanswered: int
    #: The run's reminder messages, oldest first, so a caller can jump straight
    #: to the card in Discord. `bossctl` can grow the same view from here.
    cards: list[RunCardOut] = []
    roster_change: RosterChangeOut | None = None


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
    #: `clear` removes the answer instead of recording one -- the correction for
    #: a ✅ somebody left by accident, which must leave them *unanswered*.
    answer: Answer | Literal["clear"]


#: `at_risk` is derived from the answers people give, and `done` is also set
#: automatically once a run's night has passed; neither is set by hand here.
RunStatus = Literal["planned", "confirmed", "otot", "done", "cancelled"]


class StatusIn(Strict):
    status: RunStatus


class SwapIn(Strict):
    """Who comes off this week's run, and who takes their place."""

    remove: list[str] = []
    add: list[str] = []


class RosterMemberOut(BaseModel):
    id: str
    name: str


class RosterChangeOut(BaseModel):
    """How this week's party differs from the fixed timing behind it."""

    out: list[RosterMemberOut] = []
    in_: list[RosterMemberOut] = Field(default=[], alias="in")
    changed: bool = False

    model_config = ConfigDict(populate_by_name=True)


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
    #: The run's current time, set only for a move -- the "old" side of the arrow.
    from_when: str | None = None
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


# --- chat interactions ------------------------------------------------------


class ChatInteractionOut(BaseModel):
    id: str
    short_id: str
    at: str
    local_time: str
    author_id: str | None
    author_name: str
    channel_id: str | None
    channel_name: str | None
    model: str
    #: answered | failed.
    outcome: str
    rounds: int
    latency_ms: int | None
    tool_names: list[str]
    tool_count: int
    #: Null where the model reported no usage, which is not the same as zero.
    prompt_tokens: int | None
    completion_tokens: int | None
    url: str | None


class ChatCardOut(BaseModel):
    """A proposal card one tool call raised."""

    id: str
    short_id: str
    kind: str | None = None
    kind_label: str | None = None
    status: str | None = None
    card_url: str | None = None


class ChatToolCallOut(BaseModel):
    name: str
    #: As the DEBUG log renders them, truncated to ~200 characters.
    arguments: str
    ms: int | None
    outcome: str
    ok: bool
    created: list[ChatCardOut]


class ChatInteractionDetailOut(ChatInteractionOut):
    question: str
    reply: str
    error: str | None
    model_ms: int | None
    tools_ms: int | None
    message_id: str | None
    tool_calls: list[ChatToolCallOut]


class ChatModelStatsOut(BaseModel):
    model: str
    count: int
    answered: int
    failed: int
    prompt_tokens: int
    completion_tokens: int
    avg_latency_ms: int | None
    p95_latency_ms: int | None


class ChatSummaryOut(BaseModel):
    models: list[ChatModelStatsOut]
    count: int
    answered: int
    failed: int
    prompt_tokens: int
    completion_tokens: int


# --- the audit trail --------------------------------------------------------


class AuditOut(BaseModel):
    """One recorded change: when, from where, who, and what."""

    id: str
    at: str
    local_time: str
    #: portal | cli | discord | chat | card | system.
    surface: str
    #: A tailnet login, a Discord user id, an OS username, or `token`.
    actor: str
    action: str
    #: The run, card, fixed timing or config key that changed.
    subject: str | None
    short_subject: str | None
    detail: str


# --- members, reminders -----------------------------------------------------


class MemberOut(BaseModel):
    user_id: str
    display_name: str
    nickname: str | None
    aliases: list[str]
    has_role: bool
    #: How much this member wants to be @mentioned: essential | all | off.
    ping_level: str
    updated_at: str
    runs_this_week: int


class MemberUpdate(Strict):
    """A member's own settings. Only the mention level is editable."""

    ping_level: str | None = None


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
    quiet_mode: bool
    chat_mode: bool
    chat_pilot_rate_count: int = 4
    chat_pilot_rate_window_s: float = 300.0
    chat_pilot_global_rate_count: int = 12
    chat_pilot_global_rate_window_s: float = 900.0
    #: The chatbot needs a role and a channel before `chat_mode` means anything.
    chat_configured: bool = False
    chat_channels: list[str] = []
    chat_categories: list[str] = []
    chat_model: str = ""
    timezone: str
    reset: str
    model: str
    ollama_host: str
    min_confidence: float
    post_channel_id: str | None
    guild_id: str
    watched_channels: list[str]
    watched_categories: list[str]
    #: Watched channels where the bot lacks Manage Messages, by name. While one
    #: is listed, a ✅ and a ❌ from the same person both stick in that channel.
    missing_manage_messages: list[str] = []


class ConfigIn(Strict):
    """Only the runtime-editable keys; everything else is a redeploy."""

    day_of_ping_time: str | None = None
    countdown_minutes: str | None = None
    paused: bool | None = None
    extract_enabled: bool | None = None
    quiet_mode: bool | None = None
    chat_mode: bool | None = None
    #: The chatbot's capacity, editable at runtime like the flags above.
    chat_pilot_rate_count: int | None = Field(default=None, ge=1)
    chat_pilot_rate_window_s: float | None = Field(default=None, gt=0)
    chat_pilot_global_rate_count: int | None = Field(default=None, ge=1)
    chat_pilot_global_rate_window_s: float | None = Field(default=None, gt=0)


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
    cancelled: bool = False
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


class RescanJobOut(BaseModel):
    """A queued or running rescan, and how far through it is."""

    job_id: str
    short_id: str
    status: Literal["queued", "running", "done", "failed", "cancelled"]
    window: str
    source: str
    automated: bool
    channels: list[str]
    channel_names: list[str]
    current: str | None = None
    done: int
    total: int
    percent: int
    elapsed_ms: int
    running: bool
    error: str | None = None
    results: list[ChannelRescanOut] = []


class RescanJobDetailOut(RescanJobOut):
    totals: RescanOut


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
    #: Needed to keep ✅/❌ exclusive: the bot removes the opposite reaction.
    manage_messages: bool = True
    #: True when the bot is not connected, so nothing could be checked.
    unknown: bool


# --- capacity: the one model, the two windows, and what is queued -----------


class ModelLockOut(BaseModel):
    """Who has the host's one model, and for how long."""

    busy: bool
    #: ``extractor``, ``followup``, or ``chat #<channel id>``. ``None`` while
    #: idle -- or, with ``busy`` set, for a holder that never said who it was.
    holder: str | None = None
    held_for_s: float


class PoolOut(BaseModel):
    """One sliding window as used-of-total."""

    count: int
    window_s: float
    used: int
    remaining: int


class UserWindowOut(BaseModel):
    """One member who is currently inside their own window."""

    user_id: str
    name: str
    used: int
    remaining: int
    #: The allowance this member is on, which is the guild default unless they
    #: have an override -- so a row can be read without cross-referencing.
    count: int
    window_s: float
    overridden: bool = False


class LimitOverrideOut(BaseModel):
    """One member's own allowance, instead of the guild's."""

    user_id: str
    name: str
    count: int
    window_s: float


class PerUserOut(BaseModel):
    #: The guild default; a row above may be on something else.
    count: int
    window_s: float
    #: Only members mid-window; an empty list means nobody has asked recently.
    windows: list[UserWindowOut] = []
    #: Every member with an override, mid-window or not.
    overrides: list[LimitOverrideOut] = []


class AnsweringOut(BaseModel):
    channel_id: str
    channel_name: str


class RescanQueueOut(BaseModel):
    worker_running: bool
    #: Jobs still waiting. The one being drained has already left the queue.
    queued: int
    job: str | None = None
    channel: str | None = None


class JobsOut(BaseModel):
    answering: list[AnsweringOut] = []
    extracting: bool
    rescan: RescanQueueOut


class PilotOut(BaseModel):
    """One holder of the chat role, and where they stand right now."""

    user_id: str
    name: str
    #: Staff are exempt from every budget, so their allowance is decoration.
    staff: bool = False
    count: int
    window_s: float
    overridden: bool = False
    used: int = 0
    remaining: int = 0
    has_window: bool = False


class LimitsOut(BaseModel):
    """Live capacity: nothing here is stored, and nothing here is spent by asking."""

    model: ModelLockOut
    global_pool: PoolOut
    per_user: PerUserOut
    jobs: JobsOut
    #: Everybody holding ``CHAT_PILOT_ROLE_ID``, read live from the guild.
    #: Empty when the role or the guild cannot be resolved -- which is not the
    #: same fact as "nobody holds it", and the portal says which.
    pilots: list[PilotOut] = []


class LimitResetOut(BaseModel):
    """One member's window or override, cleared. The guild's pool is neither."""

    user_id: str
    name: str


class LimitOverrideIn(Strict):
    """The allowance to put one member on, instead of the guild's."""

    count: int = Field(ge=1, description="answers per window")
    window_s: float = Field(gt=0, description="the window, in seconds")


class HealthOut(BaseModel):
    status: str


__all__ = [
    "AccessOut",
    "AmendIn",
    "AmendmentOut",
    "ApproveIn",
    "ApproveOut",
    "AuditOut",
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
    "AnsweringOut",
    "JobsOut",
    "LimitOverrideIn",
    "LimitOverrideOut",
    "LimitResetOut",
    "LimitsOut",
    "ModelLockOut",
    "PerUserOut",
    "PilotOut",
    "PoolOut",
    "RescanQueueOut",
    "UserWindowOut",
    "MemberOut",
    "MemberUpdate",
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
    "RescanJobDetailOut",
    "RescanJobOut",
    "RescanOut",
    "RescanTargetOut",
    "RsvpIn",
    "RunStatus",
    "RosterChangeOut",
    "RosterMemberOut",
    "StatusIn",
    "SwapIn",
    "RunOut",
    "ScheduleOut",
]
