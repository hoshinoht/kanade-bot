"""Deployment environment configuration."""

from __future__ import annotations

import re
from datetime import time
from functools import lru_cache
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import Field, field_validator, model_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

from bot.domain.weeks import parse_hhmm, parse_weekday

#: Match trailing ``.env`` comments before validation.
_INLINE_COMMENT_RE = re.compile(r"(?:^|\s)#.*$", re.DOTALL)

#: Secrets are not comment-stripped.
_VERBATIM = frozenset({"discord_token", "admin_token"})


def _int_list(raw: str) -> list[int]:
    return [int(part) for part in raw.replace(";", ",").split(",") if part.strip()]


class Settings(BaseSettings):
    """Read ``.env`` and process environment settings."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
        case_sensitive=False,
    )

    # --- Discord ---------------------------------------------------------
    discord_token: str
    guild_id: int
    #: Watched channels: explicit ids, and/or every text channel under these
    #: categories (resolved per message, so later additions are picked up).
    #: At least one of the two must be set -- see `_require_watched_channels`.
    chat_channel_ids: str = ""
    chat_category_ids: str = ""
    #: Optional. Runs post in their own home channel; this is for guild-wide
    #: posts (the weekly digest) and as a fallback when a home channel is gone.
    post_channel_id: int | None = None
    bossing_role_id: int
    admin_role_id: int | None = None
    #: Extra user ids allowed to use /debug, on top of the guild owner and
    #: ADMIN_ROLE_ID members.
    debug_user_ids: str = ""

    # --- scheduling ------------------------------------------------------
    tz: str = "Asia/Kuala_Lumpur"
    boss_week_reset_weekday: str = "thu"
    boss_week_reset_time: str = "00:00"
    day_of_ping_time: str = "09:00"
    countdown_minutes: str = "60,15"

    # --- storage ---------------------------------------------------------
    db_path: str = "data/bot.sqlite"
    bosses_path: str = "config/bosses.yaml"

    # --- phase 2: the chat extractor -------------------------------------
    ollama_host: str = "http://host.docker.internal:11434"
    ollama_model: str = "gpt-oss:20b"
    #: Seconds to wait for one extraction call. `gpt-oss:20b` on an M4 Pro takes
    #: roughly 10-40 s for a ~3k-token prompt; the first call after a cold start
    #: also pays for loading 13 GB of weights.
    ollama_timeout: float = Field(default=120.0, gt=0)
    #: `think` for gpt-oss-style reasoning models: low/medium/high, or "off".
    ollama_think: str = "low"
    #: Context window handed to the model.
    ollama_num_ctx: int = Field(default=8192, ge=2048)

    #: Master switch for chat extraction. Messages are still logged when off.
    extract_enabled: bool = True
    #: Silence, in seconds, that ends a burst and triggers one LLM call.
    extract_debounce_seconds: float = Field(default=90.0, gt=0)
    #: How many earlier messages of the channel to show the model as context.
    extract_context_messages: int = Field(default=25, ge=0, le=100)
    #: Below this, an extracted amendment is logged but never posted.
    extract_min_confidence: float = Field(default=0.6, ge=0.0, le=1.0)
    #: Pull each watched channel's history for the current boss week on start.
    #: No model call is made -- it only fills `messages`, so a `/rescan` (or a
    #: card's evidence links) still works after the database has been reset.
    backfill_on_start: bool = True

    # --- phase 4: the speech pilot ---------------------------------------
    #: Role permitted to use the chatbot; unset disables it.
    chat_pilot_role_id: int | None = None
    #: Initial role-to-behaviour-plugin assignments as ``ROLE_ID=plugin`` pairs.
    #: They seed SQLite once; the portal owns the mappings after that.
    chat_role_plugins: str = ""
    #: Chatbot channels, independent of extractor channels.
    chat_pilot_channel_ids: str = ""
    #: Chatbot categories; both channel lists empty disables the feature.
    chat_pilot_category_ids: str = ""
    #: Replies per person per window before the bot goes quiet at them.
    #: ``ADMIN_ROLE_ID`` holders are exempt (see :func:`bot.agent.util.is_bot_admin`).
    chat_pilot_rate_count: int = Field(default=4, ge=1)
    chat_pilot_rate_window_s: float = Field(default=300.0, gt=0)
    #: Guild-wide answer limit; administrators are exempt.
    chat_pilot_global_rate_count: int = Field(default=12, ge=1)
    chat_pilot_global_rate_window_s: float = Field(default=900.0, gt=0)
    #: Shared-model wait before normal requests are shed; staff wait the timeout.
    chat_pilot_lock_wait_s: float = Field(default=2.0, ge=0)
    #: Per-turn conversation-history TTL.
    chat_pilot_history_ttl_s: float = Field(default=2700.0, gt=0)
    #: The model that answers. Separate from ``OLLAMA_MODEL`` so the extractor's
    #: model can be changed without silently changing the bot's voice.
    chat_pilot_model: str = "gpt-oss:20b"
    #: Seconds for one whole answer, tool rounds included.
    chat_pilot_timeout: float = Field(default=60.0, gt=0)
    #: Sampling temperature for chatbot replies.
    chat_pilot_temperature: float = Field(default=0.7, ge=0.0, le=2.0)
    #: Stable identity document on the persona bind mount.
    persona_path: str = "config/personas/identities/persona.md"

    # --- phase 3: the portal + `bossctl` ---------------------------------
    #: Empty refuses every non-health API request.
    admin_token: str = ""
    #: Tailscale logins allowed through `tailscale serve`, comma separated.
    allowed_tailscale_logins: str = ""
    #: Trust `Tailscale-User-Login` on requests arriving from the host. Off by
    #: default: without `tailscale serve` in front, the header is just a string
    #: anyone can send. See README "Portal & CLI".
    trust_tailscale_headers: bool = False
    #: Discord user id credited for changes made in the portal. Defaults to the
    #: guild owner when unset.
    portal_actor_id: int | None = None
    #: Compose overrides this to ``0.0.0.0`` inside the container namespace.
    api_host: str = "127.0.0.1"
    api_port: int = 8080

    log_level: str = "INFO"
    #: seconds between reminder-loop ticks
    tick_seconds: int = Field(default=30, ge=5, le=600)

    # --- validation ------------------------------------------------------
    @model_validator(mode="before")
    @classmethod
    def _tidy_env_values(cls, values: Any) -> Any:
        """Strip inline comments and blanks before validation."""
        if not isinstance(values, dict):
            return values
        cleaned: dict[str, Any] = {}
        for key, value in values.items():
            if not isinstance(value, str) or str(key).lower() in _VERBATIM:
                # A comment-only secret value is unset, not a credential.
                if isinstance(value, str) and value.strip().startswith("#"):
                    continue
                cleaned[key] = value
                continue
            text = _INLINE_COMMENT_RE.sub("", value).strip()
            if text == "":
                continue  # let the field default (or "field required") speak
            cleaned[key] = text
        return cleaned

    @field_validator("tz")
    @classmethod
    def _check_tz(cls, value: str) -> str:
        try:
            ZoneInfo(value)
        except (ZoneInfoNotFoundError, ValueError) as exc:
            # ZoneInfoNotFoundError is a KeyError, which pydantic would let
            # escape as a raw traceback instead of a readable config error.
            raise ValueError(f"unknown timezone {value!r}: {exc}") from None
        return value

    @field_validator("boss_week_reset_weekday")
    @classmethod
    def _check_weekday(cls, value: str) -> str:
        parse_weekday(value)
        return value

    @field_validator("boss_week_reset_time", "day_of_ping_time")
    @classmethod
    def _check_time(cls, value: str) -> str:
        parse_hhmm(value)
        return value

    @model_validator(mode="after")
    def _require_watched_channels(self) -> Settings:
        if not self.chat_channel_id_list and not self.chat_category_id_list:
            raise ValueError(
                "set CHAT_CHANNEL_IDS and/or CHAT_CATEGORY_IDS - the bot needs at least "
                "one watched channel, because /fixed add must be run inside one"
            )
        return self

    @field_validator("ollama_think")
    @classmethod
    def _check_think(cls, value: str) -> str:
        key = value.strip().lower()
        if key not in ("low", "medium", "high", "off", "false", "none", ""):
            raise ValueError("OLLAMA_THINK must be low, medium, high or off")
        return key

    @field_validator("countdown_minutes")
    @classmethod
    def _check_countdowns(cls, value: str) -> str:
        minutes = _int_list(value)
        if any(m <= 0 for m in minutes):
            raise ValueError("COUNTDOWN_MINUTES must be positive whole minutes")
        return value

    # --- derived ---------------------------------------------------------
    @property
    def zoneinfo(self) -> ZoneInfo:
        return ZoneInfo(self.tz)

    @property
    def chat_channel_id_list(self) -> list[int]:
        return _int_list(self.chat_channel_ids)

    @property
    def chat_category_id_list(self) -> list[int]:
        return _int_list(self.chat_category_ids)

    @property
    def chat_pilot_channel_id_list(self) -> list[int]:
        return _int_list(self.chat_pilot_channel_ids)

    @property
    def chat_pilot_category_id_list(self) -> list[int]:
        return _int_list(self.chat_pilot_category_ids)

    @property
    def chat_pilot_configured(self) -> bool:
        """Whether both chatbot gates are configured."""
        return self.chat_pilot_role_id is not None and bool(
            self.chat_pilot_channel_id_list or self.chat_pilot_category_id_list
        )

    @property
    def debug_user_id_list(self) -> list[int]:
        return _int_list(self.debug_user_ids)

    @property
    def countdown_minute_list(self) -> list[int]:
        return sorted(set(_int_list(self.countdown_minutes)), reverse=True)

    @property
    def reset_weekday(self) -> int:
        return parse_weekday(self.boss_week_reset_weekday)

    @property
    def reset_time(self) -> time:
        return parse_hhmm(self.boss_week_reset_time)

    @property
    def think(self) -> str | bool | None:
        """Return the Ollama ``think`` argument."""
        level = (self.ollama_think or "").strip().lower()
        if level in ("low", "medium", "high"):
            return level
        if level in ("off", "false", "none", "0"):
            return False
        return None

    @property
    def allowed_login_list(self) -> list[str]:
        return [p.strip().lower() for p in self.allowed_tailscale_logins.split(",") if p.strip()]


@lru_cache(maxsize=1)
def get_settings() -> Settings:
    """Load and cache settings lazily."""
    return Settings()  # type: ignore[call-arg]
