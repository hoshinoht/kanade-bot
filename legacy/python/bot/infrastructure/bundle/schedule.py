"""Schedule and roster section models."""

from datetime import datetime
from typing import Literal
from uuid import UUID

from pydantic import Field, model_validator

from .common import (
    AmendmentStatus,
    AwareModel,
    Identifier,
    RsvpState,
    RunStatus,
    Snowflake,
    Weekday,
)


class Member(AwareModel):
    user_id: Snowflake
    display_name: str
    nickname: str | None = None
    aliases: list[str] = Field(default_factory=list)
    has_role: bool
    ping_level: Literal["essential", "all", "off"]
    reply_style: str | None = None
    updated_at: datetime


class FixedRun(AwareModel):
    id: UUID
    owner_id: Snowflake
    channel_id: Snowflake | None = None
    bosses: list[str]
    weekday: Weekday
    time: str = Field(pattern=r"^([01][0-9]|2[0-3]):[0-5][0-9]$")
    participants: list[Snowflake]
    note: str | None = None
    created_at: datetime


class Run(AwareModel):
    id: UUID
    fixed_run_id: UUID | None = None
    channel_id: Snowflake | None = None
    week_start: datetime
    bosses: list[str]
    datetime: datetime
    participants: list[Snowflake]
    status: RunStatus
    source: Identifier
    created_at: datetime


class Rsvp(AwareModel):
    run_id: UUID
    user_id: Snowflake
    state: RsvpState
    source: Identifier
    at: datetime


class AmendmentPayload(AwareModel):
    """Closed union of v4 `fix`, `split`, `sub`, and annotation envelopes."""

    weekday: Weekday | None = None
    time: str | None = Field(default=None, pattern=r"^([01][0-9]|2[0-3]):[0-5][0-9]$")
    bosses: list[str] | None = None
    participants: list[Snowflake] | None = None
    remove: list[Snowflake] | None = None
    add: list[Snowflake] | None = None
    also_mentioned: list[
        Literal["move", "add", "cancel", "otot", "split", "sub", "rsvp", "fix"]
    ] = Field(default_factory=list)
    op: Literal["remove", "edit"] | None = None
    fixed_run_id: UUID | None = None
    weekly_when: str | None = None


class Amendment(AwareModel):
    id: UUID
    week_start: datetime
    kind: Literal["move", "add", "cancel", "otot", "split", "sub", "rsvp", "fix"]
    bosses: list[str]
    run_id: UUID | None = None
    new_datetime: datetime | None = None
    participants: list[Snowflake]
    status: AmendmentStatus
    confidence: float | None = Field(default=None, ge=0, le=1)
    evidence_msg_ids: list[Snowflake]
    proposal_message_id: Snowflake | None = None
    created_at: datetime
    channel_id: Snowflake | None = None
    is_question: bool
    rsvp: RsvpState | None = None
    day_ref: str | None = None
    time_ref: str | None = None
    summary: str | None = None
    payload: AmendmentPayload = Field(default_factory=AmendmentPayload)

    @model_validator(mode="after")
    def _payload_matches_kind(self) -> "Amendment":
        allowed = {
            "fix": {
                "weekday",
                "time",
                "participants",
                "op",
                "fixed_run_id",
                "weekly_when",
                "also_mentioned",
            },
            "split": {"bosses", "participants", "also_mentioned"},
            "sub": {"remove", "add", "also_mentioned"},
        }.get(self.kind, {"also_mentioned"})
        present = {
            name
            for name in (
                "weekday",
                "time",
                "bosses",
                "participants",
                "remove",
                "add",
                "op",
                "fixed_run_id",
                "weekly_when",
            )
            if getattr(self.payload, name) is not None
        }
        if not present <= allowed:
            raise ValueError(
                f"payload fields {sorted(present - allowed)} are invalid for {self.kind}"
            )
        if self.kind == "fix":
            if self.payload.op is None:
                if not present <= {"weekday", "time"}:
                    raise ValueError("extractor fix payload permits only weekday/time")
            elif self.payload.fixed_run_id is None or self.payload.weekly_when is None:
                raise ValueError("chat fix payload requires fixed_run_id and weekly_when")
        return self


class Schedule(AwareModel):
    members: list[Member]
    fixed_runs: list[FixedRun]
    runs: list[Run]
    rsvps: list[Rsvp]
    amendments: list[Amendment]
