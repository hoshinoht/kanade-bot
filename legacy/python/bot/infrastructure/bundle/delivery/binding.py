"""Authoritative current-week native delivery bindings."""

from datetime import datetime
from uuid import UUID

from ..common import AwareModel, Snowflake


class ReminderBinding(AwareModel):
    id: UUID
    run_id: UUID
    fire_at: datetime
    kind: str
    sent_at: datetime | None = None
    message_id: Snowflake | None = None


class DigestBinding(AwareModel):
    week_start: datetime
    channel_id: Snowflake
    message_id: Snowflake
    posted_at: datetime
    retired_at: datetime | None = None


class DeclineBinding(AwareModel):
    run_id: UUID
    user_id: Snowflake
    channel_id: Snowflake | None = None
    message_id: Snowflake | None = None
    notified_at: datetime


class CardBinding(AwareModel):
    amendment_id: UUID
    channel_id: Snowflake
    message_id: Snowflake


class DebugCardBinding(AwareModel):
    run_id: UUID
    channel_id: Snowflake | None = None
    message_id: Snowflake
    kind: str
    created_at: datetime
