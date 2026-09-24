"""Explicit non-secret configuration and catalog section models."""

from datetime import datetime
from typing import Literal

from pydantic import Field, field_validator

from .common import AwareModel, Snowflake

RUNTIME_CONFIG_KEYS = frozenset(
    {
        "day_of_ping_time",
        "countdown_minutes",
        "paused",
        "extract_enabled",
        "quiet_mode",
        "chat_mode",
        "persona",
        "chat_role_plugins",
        "chat_selectable_plugins",
        "chat_pilot_rate_count",
        "chat_pilot_rate_window_s",
        "chat_pilot_global_rate_count",
        "chat_pilot_global_rate_window_s",
        "extract_model",
        "extract_reasoning",
        "chat_pilot_model",
        "chat_pilot_think",
        "last_materialised_week",
        "last_digest_week",
    }
)


class Config(AwareModel):
    timezone: str
    boss_week_reset_weekday: Literal["thu"] = "thu"
    boss_week_reset_time: str = Field(pattern=r"^([01][0-9]|2[0-3]):[0-5][0-9]$")
    runtime: dict[str, str] = Field(
        json_schema_extra={"propertyNames": {"enum": sorted(RUNTIME_CONFIG_KEYS)}}
    )

    @field_validator("runtime")
    @classmethod
    def _allowlisted(cls, value: dict[str, str]) -> dict[str, str]:
        unknown = set(value) - RUNTIME_CONFIG_KEYS
        if unknown:
            raise ValueError(f"runtime config keys not exportable: {sorted(unknown)}")
        return value


class RateOverride(AwareModel):
    user_id: Snowflake
    count: int = Field(ge=1)
    window_s: float = Field(gt=0)
    updated_at: datetime


class CatalogBoss(AwareModel):
    token: str
    label: str


class Catalog(AwareModel):
    revision: str
    bosses: list[CatalogBoss]
    historic_tokens: list[str] = Field(default_factory=list)
