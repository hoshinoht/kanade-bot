"""Deployment environment configuration."""

from __future__ import annotations

import os
import re
from collections.abc import Callable
from datetime import time
from functools import lru_cache
from typing import Any
from urllib.parse import urlsplit
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import Field, PrivateAttr, field_validator, model_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

from bot.domain.weeks import parse_hhmm, parse_weekday

_INLINE_COMMENT_RE = re.compile(r"(?:^|\s)#.*$", re.DOTALL)

#: Secrets are not comment-stripped.
_VERBATIM = frozenset({"discord_token", "admin_token"})

#: Settings of the removed personal-memory feature: tolerated so an existing
#: ``.env`` still starts, and reported once by the bot at startup.
_REMOVED_PREFIXES = ("chat_memory_",)

#: Removed direct-Ollama settings and the setting that replaced each one.
REPLACED_SETTINGS = {
    "OLLAMA_HOST": "KANATA_BASE_URL",
    "OLLAMA_MODEL": "EXTRACT_MODEL",
    "OLLAMA_THINK": "EXTRACT_REASONING",
    "OLLAMA_TIMEOUT": "KANATA_TIMEOUT",
    "OLLAMA_NUM_CTX": "MODEL_CONTEXT_TOKENS",
}

_REASONING_WORDS = ("low", "medium", "high", "off", "false", "none", "")

#: Runtime config rows selecting the models. The environment seeds them on first
#: run; the rows win afterwards and are applied onto the shared settings object.
MODEL_ALIAS_KEYS = ("extract_model", "chat_pilot_model")
MODEL_REASONING_KEYS = ("extract_reasoning", "chat_pilot_think")
MODEL_CONFIG_KEYS = (*MODEL_ALIAS_KEYS, *MODEL_REASONING_KEYS)
#: Reasoning levels offered for selection; ``chat_pilot_think`` also accepts ``""`` (inherit).
REASONING_CHOICES = ("off", "low", "medium", "high")


def normalize_reasoning(value: Any, *, inherit: bool = False) -> str:
    """A stored reasoning level: low/medium/high/off, or ``""`` where inheriting is allowed."""
    key = str(value if value is not None else "").strip().lower()
    if key in ("false", "none"):
        key = "off"
    if key in REASONING_CHOICES or (inherit and key == ""):
        return key
    raise ValueError("reasoning must be low, medium, high or off")


def removed_settings_notice(names: tuple[str, ...]) -> str:
    """One line naming each ignored setting and what, if anything, replaced it."""
    return ", ".join(
        f"{name} (use {REPLACED_SETTINGS[name]})"
        if name in REPLACED_SETTINGS
        else f"{name} (personal memory was removed)"
        for name in names
    )


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

    tz: str = "Asia/Kuala_Lumpur"
    boss_week_reset_weekday: str = "thu"
    boss_week_reset_time: str = "00:00"
    day_of_ping_time: str = "01:00"
    countdown_minutes: str = "60"

    db_path: str = "data/bot.sqlite"
    #: Shared mode-0700 local root for persistent repository owner lockfiles.
    db_owner_lock_dir: str | None = None
    bosses_path: str = "boss/bosses.yaml"
    #: Strict local strategy documents required when the chat pilot is enabled.
    boss_knowledge_path: str = "boss/knowledge"

    #: Kanata inference gateway (OpenAI-compatible); every model call goes here.
    kanata_base_url: str = "https://sumi.kanata.hoshinoht.dev"
    #: Operator file holding the gateway bearer key; never the key itself.
    kanata_api_key_file: str = ""
    #: Seconds to wait for one extraction call.
    kanata_timeout: float = Field(default=120.0, gt=0)
    #: Gateway model alias for extraction. No default: required while extraction is on.
    extract_model: str = ""
    #: Extraction reasoning effort: low/medium/high, or "off" (sent as "none").
    extract_reasoning: str = "off"
    #: Client-side prompt budget only; must match the host model's num_ctx.
    model_context_tokens: int = Field(default=8192, ge=2048)

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
    #: Gateway model alias that answers. Separate from ``EXTRACT_MODEL`` so the
    #: extractor's model can change without silently changing the bot's voice.
    #: No default: required while the chatbot is configured.
    chat_pilot_model: str = ""
    #: Seconds for one whole answer, tool rounds included.
    chat_pilot_timeout: float = Field(default=60.0, gt=0)
    #: Sampling temperature for chatbot replies.
    chat_pilot_temperature: float = Field(default=0.7, ge=0.0, le=2.0)
    #: Reasoning effort for the chat pilot. Empty falls back to ``EXTRACT_REASONING``
    #: so the extractor can stay fast while speech reasons harder.
    chat_pilot_think: str = ""

    #: Deprecated seed identity path; manifest deployments resolve its basename.
    persona_path: str = "config/personas/identities/persona.md"
    #: Legacy-only staging path. Manifest bundles carry their own baseline staging.
    staging_path: str = "config/personas/behaviours/staging.yaml"
    #: Per-profile staging overrides beside each voice. Filename must match
    #: behaviours/<profile>.md. Partial files inherit from default.
    staging_profiles_dir: str = "config/personas/behaviours/staging"

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

    _removed_settings = PrivateAttr(default=())

    @model_validator(mode="wrap")
    @classmethod
    def _note_removed_settings(cls, values: Any, handler: Any) -> Settings:
        keys = set(os.environ)
        if isinstance(values, dict):
            keys.update(str(key) for key in values)
        removed = tuple(
            sorted(
                {
                    key.upper()
                    for key in keys
                    if key.lower().startswith(_REMOVED_PREFIXES) or key.upper() in REPLACED_SETTINGS
                }
            )
        )
        settings = handler(values)
        settings._removed_settings = removed
        return settings

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

    @field_validator("extract_reasoning")
    @classmethod
    def _check_reasoning(cls, value: str) -> str:
        key = value.strip().lower()
        if key not in _REASONING_WORDS:
            raise ValueError("EXTRACT_REASONING must be low, medium, high or off")
        return key

    @field_validator("chat_pilot_think")
    @classmethod
    def _check_chat_think(cls, value: str) -> str:
        key = value.strip().lower()
        if key not in _REASONING_WORDS:
            raise ValueError("CHAT_PILOT_THINK must be low, medium, high or off")
        return key

    @field_validator("kanata_base_url")
    @classmethod
    def _check_base_url(cls, value: str) -> str:
        parsed = urlsplit(value.strip())
        # TLS only; credentials in the URL would end up in logs.
        if (
            parsed.scheme != "https"
            or not parsed.hostname
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError("KANATA_BASE_URL must be an https:// URL without credentials")
        base = value.strip().rstrip("/")
        # OpenAI SDKs take ".../v1"; the client appends /v1 itself.
        return base.removesuffix("/v1") if parsed.path.rstrip("/").endswith("/v1") else base

    @field_validator("extract_model", "chat_pilot_model", "kanata_api_key_file")
    @classmethod
    def _strip(cls, value: str) -> str:
        return value.strip()

    @field_validator("countdown_minutes")
    @classmethod
    def _check_countdowns(cls, value: str) -> str:
        minutes = _int_list(value)
        if any(m <= 0 for m in minutes):
            raise ValueError("COUNTDOWN_MINUTES must be positive whole minutes")
        return value

    @property
    def removed_settings(self) -> tuple[str, ...]:
        """Ignored environment names left over from removed features."""
        return self._removed_settings

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
    def reasoning_effort(self) -> str:
        """Extraction ``reasoning_effort``; ``"none"`` disables thinking."""
        level = self.extract_reasoning
        return level if level in ("low", "medium", "high") else "none"

    @property
    def chat_reasoning_effort(self) -> str:
        """Chat ``reasoning_effort``, falling back to :attr:`reasoning_effort`."""
        level = self.chat_pilot_think
        if level in ("low", "medium", "high"):
            return level
        if level in ("off", "false", "none"):
            return "none"
        return self.reasoning_effort

    def apply_runtime_models(self, get: Callable[[str], str | None]) -> None:
        """Overlay stored model selections; unreadable rows keep the current value."""
        for key in MODEL_ALIAS_KEYS:
            value = get(key)
            if value is not None and value.strip():
                setattr(self, key, value.strip())
        for key in MODEL_REASONING_KEYS:
            value = get(key)
            if value is None:
                continue
            try:
                setattr(self, key, normalize_reasoning(value, inherit=key == "chat_pilot_think"))
            except ValueError:
                continue

    def model_setting_errors(self, extract_on: bool | None = None) -> list[str]:
        """Missing model settings for enabled features.

        ``extract_on`` defaults to ``EXTRACT_ENABLED``; pass the effective
        runtime flag to check what the extractor will actually do.
        """
        from bot.infrastructure.llm.client import key_file_error

        extract_on = self.extract_enabled if extract_on is None else extract_on
        errors = []
        if extract_on and not self.extract_model:
            errors.append(
                "EXTRACT_MODEL is not set and no extraction model is selected "
                "(required while extraction is on)"
            )
        if self.chat_pilot_configured and not self.chat_pilot_model:
            errors.append(
                "CHAT_PILOT_MODEL is not set and no chat model is selected "
                "(required while the chatbot is configured)"
            )
        if extract_on or self.chat_pilot_configured:
            problem = key_file_error(self.kanata_api_key_file)
            if problem is not None:
                errors.append(problem)
        return errors

    @property
    def allowed_login_list(self) -> list[str]:
        return [p.strip().lower() for p in self.allowed_tailscale_logins.split(",") if p.strip()]


@lru_cache(maxsize=1)
def get_settings() -> Settings:
    """Load and cache settings lazily."""
    return Settings()  # type: ignore[call-arg]
