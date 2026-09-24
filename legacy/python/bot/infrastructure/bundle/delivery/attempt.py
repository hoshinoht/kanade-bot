"""Portable grouped delivery attempts and retirements."""

from typing import Annotated, Literal

from pydantic import ConfigDict, Field

from ..common import AwareModel, Snowflake
from .refs import NativeBindingRef

DeliveryKind = Literal["reminder", "digest", "decline", "card", "debug_card"]
RetiredDeliveryKind = DeliveryKind | Literal["pre_journal_attestation"]
RetirementActor = Annotated[str, Field(pattern=r"^(?:[1-9][0-9]{0,19}|admin|system)$")]


class BoundDeliveryAttempt(AwareModel):
    state: Literal["bound"]
    delivery_kind: DeliveryKind
    channel_id: Snowflake
    message_id: Snowflake
    targets: Annotated[list[NativeBindingRef], Field(min_length=1)]


class RetiredDeliveryAttempt(AwareModel):
    model_config = ConfigDict(
        extra="forbid",
        strict=True,
        json_schema_extra={
            "allOf": [
                {
                    "if": {"properties": {"delivery_kind": {"const": "pre_journal_attestation"}}},
                    "then": {"properties": {"targets": {"maxItems": 0}}},
                }
            ]
        },
    )
    state: Literal["retired"]
    delivery_kind: RetiredDeliveryKind
    channel_id: Snowflake | None = None
    message_id: Snowflake | None = None
    targets: list[NativeBindingRef] = Field(default_factory=list)
    retired_by: RetirementActor
    retired_reason: Annotated[str, Field(min_length=1, max_length=1000)]


DeliveryAttempt = Annotated[
    BoundDeliveryAttempt | RetiredDeliveryAttempt,
    Field(discriminator="state"),
]
