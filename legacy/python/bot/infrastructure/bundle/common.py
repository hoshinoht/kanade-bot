"""Shared strict primitives for the portable bundle contract."""

from datetime import datetime
from typing import Annotated, Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


Snowflake = Annotated[str, Field(pattern=r"^[1-9][0-9]{0,19}$")]
Identifier = Annotated[str, Field(min_length=1, max_length=200)]
UtcDateTime = Annotated[datetime, Field()]
Weekday = Annotated[int, Field(ge=0, le=6)]


class UuidRecord(StrictModel):
    id: UUID


class BossToken(StrictModel):
    token: Annotated[str, Field(pattern=r"^[A-Za-z][A-Za-z0-9_-]{0,127}$")]


RunStatus = Literal["planned", "confirmed", "at_risk", "otot", "done", "cancelled"]
RsvpState = Literal["yes", "no", "maybe"]
AmendmentStatus = Literal["proposed", "confirmed", "rejected", "expired", "superseded", "withdrawn"]


def aware(value: datetime) -> datetime:
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("datetime must be timezone-aware")
    return value


class AwareModel(StrictModel):
    @field_validator("*", mode="after")
    @classmethod
    def _aware_datetimes(cls, value: object) -> object:
        if isinstance(value, datetime):
            return aware(value)
        return value
