"""Closed portable identities for native delivery targets."""

from datetime import datetime
from typing import Annotated, Literal
from uuid import UUID

from pydantic import Field

from ..common import AwareModel, Snowflake


class ReminderTarget(AwareModel):
    binding_type: Literal["reminder"]
    reminder_id: UUID


class DigestTarget(AwareModel):
    binding_type: Literal["digest"]
    week_start: datetime


class DeclineTarget(AwareModel):
    binding_type: Literal["decline"]
    run_id: UUID
    user_id: Snowflake


class CardTarget(AwareModel):
    binding_type: Literal["card"]
    amendment_id: UUID


class DebugCardTarget(AwareModel):
    binding_type: Literal["debug_card"]
    message_id: Snowflake


NativeBindingRef = Annotated[
    ReminderTarget | DigestTarget | DeclineTarget | CardTarget | DebugCardTarget,
    Field(discriminator="binding_type"),
]
