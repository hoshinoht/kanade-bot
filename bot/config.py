"""Environment configuration.

Only *deployment* values live here.  Values the guild may want to change at
runtime (day-of ping time, countdown offsets, pause flag) are seeded from these
on first run and then live in the ``config`` table -- see :mod:`bot.db`.
"""

from __future__ import annotations

import re
from datetime import time
from functools import lru_cache
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import Field, field_validator, model_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

from .weeks import parse_hhmm, parse_weekday

#: ``KEY=value   # note`` -- python-dotenv does not reliably strip these, and it
#: hands the whole comment through as the value when ``value`` is blank.
_INLINE_COMMENT_RE = re.compile(r"(?:^|\s)#.*$", re.DOTALL)

#: Never touched by comment stripping: a secret is whatever the user pasted.
_VERBATIM = frozenset({"discord_token", "admin_token"})


def _int_list(raw: str) -> list[int]:
    return [int(part) for part in raw.replace(";", ",").split(",") if part.strip()]


class Settings(BaseSettings):
    """Reads ``.env`` / the process environment.

    Comma-separated values are declared as plain strings because
    pydantic-settings JSON-decodes complex fields; the parsed forms are exposed
    as properties instead.
    """

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
    #: Context window handed to the model. The prompt is built to stay well
    #: under this; a bigger window costs RAM for no benefit here.
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
    #: The role that may talk to the chatbot. Unset means nobody, which is one
    #: of the two ways the feature stays off. Deliberately *not* named
    #: ``CHAT_ROLE_ID``: ``CHAT_CHANNEL_IDS`` above already means "the channels
    #: the extractor reads", and a second, unrelated ``CHAT_*`` gate reading
    #: from a neighbouring line would be read as its companion.
    chat_pilot_role_id: int | None = None
    #: Channels the chatbot answers in, comma separated. Independent of the
    #: watched-channel list: the pilot runs in its own channel, and a watched
    #: party channel must not start answering chat.
    chat_pilot_channel_ids: str = ""
    #: Categories whose text channels the chatbot answers in, comma separated.
    #: Resolved per message, exactly as ``CHAT_CATEGORY_IDS`` is for the
    #: extractor, so a channel added to the category later is picked up without
    #: a restart. Both lists empty = feature off.
    chat_pilot_category_ids: str = ""
    #: Replies per person per window before the bot goes quiet at them.
    #: ``ADMIN_ROLE_ID`` holders are exempt (see :func:`bot.util.is_bot_admin`).
    chat_pilot_rate_count: int = Field(default=4, ge=1)
    chat_pilot_rate_window_s: float = Field(default=300.0, gt=0)
    #: The model that answers. Separate from ``OLLAMA_MODEL`` so the extractor's
    #: model can be changed without silently changing the bot's voice.
    chat_pilot_model: str = "gpt-oss:20b"
    #: Seconds for one whole answer, tool rounds included.
    chat_pilot_timeout: float = Field(default=60.0, gt=0)
    #: Sampling temperature for the chatbot. Deliberately *not* the extractor's
    #: ``temperature: 0``: that one is reading a schedule out of chat, where the
    #: only good answer is the literal one, while this is holding a conversation
    #: and a greedy decode makes it read like a form letter. Warmth is safe here
    #: because every write it drafts is a card a human still has to ✅.
    #: ``top_p`` is deliberately left at the model's own default rather than
    #: pinned here -- two knobs to tune this is one more than anybody will.
    chat_pilot_temperature: float = Field(default=0.7, ge=0.0, le=2.0)
    #: The persona document, loaded verbatim into the system prompt. It is
    #: deployment flavour text rather than code, so it lives on the data volume
    #: beside the database; compose overrides this to ``/app/data/persona.md``.
    #: A missing file falls back to the tracked ``persona.example.md``.
    persona_path: str = "data/persona.md"

    # --- phase 3: the portal + `bossctl` ---------------------------------
    #: Bearer token for the HTTP API. Empty means the API starts but refuses
    #: every non-health request, so a half-configured deployment fails loudly
    #: instead of quietly serving the schedule to whoever can reach the port.
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
    #: Interface the API binds. ``127.0.0.1`` is right when the bot runs
    #: natively on the host. **Inside the container it must be ``0.0.0.0``** --
    #: compose sets it -- because the loopback of a container namespace is not
    #: the host's; the "loopback only" guarantee comes from the compose port
    #: mapping ``127.0.0.1:8080:8080``, not from this value.
    api_host: str = "127.0.0.1"
    api_port: int = 8080

    log_level: str = "INFO"
    #: seconds between reminder-loop ticks
    tick_seconds: int = Field(default=30, ge=5, le=600)

    # --- validation ------------------------------------------------------
    @model_validator(mode="before")
    @classmethod
    def _tidy_env_values(cls, values: Any) -> Any:
        """Strip inline ``# comments`` and drop blanks so defaults apply.

        Hand-written ``.env`` files routinely carry trailing comments, and a
        blank-but-commented line like ``ADMIN_ROLE_ID=   # optional`` otherwise
        arrives as the comment text and fails to parse as an int.
        """
        if not isinstance(values, dict):
            return values
        cleaned: dict[str, Any] = {}
        for key, value in values.items():
            if not isinstance(value, str) or str(key).lower() in _VERBATIM:
                # A secret is whatever was pasted, `#` included -- but a line
                # left as `ADMIN_TOKEN=   # generate with ...` is a comment, not
                # a token, and must read as "unset" rather than authenticate.
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
        """Both gates are set, so the chatbot could answer somebody.

        Either half missing leaves it off: with no role nobody may speak to it,
        and with neither a channel nor a category there is nowhere it listens.
        """
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
        """The value the ollama client wants.

        A level passes through; ``off`` becomes an explicit ``False``. Omitting
        the key is *not* the same as ``False``: a model that reasons by default
        (Gemma 4) then puts its whole answer into ``message.thinking`` and
        leaves ``content`` empty -- 500+ s for nothing, measured. Anything else
        (unset, a typo) omits the key and lets the model do its default.
        """
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
    """Load settings once.

    Deliberately *not* called at import time: importing :mod:`bot` must work in
    tests and in ``python -c "import bot"`` without a populated environment.
    """
    return Settings()  # type: ignore[call-arg]
