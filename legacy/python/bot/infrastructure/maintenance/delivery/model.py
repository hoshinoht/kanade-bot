"""Immutable values for one journalled outbound message."""

from __future__ import annotations

import math
import re
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime
from enum import StrEnum
from types import MappingProxyType

from bot.domain.timeutil import from_iso, to_iso

type Identifier = str | int
type FrozenJSON = (
    type(None) | bool | int | float | str | tuple[FrozenJSON, ...] | Mapping[str, FrozenJSON]
)

_SNOWFLAKE = re.compile(r"[1-9][0-9]*\Z")
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")


class DestinationKind(StrEnum):
    CHANNEL = "channel"
    DM = "dm"


class BindingType(StrEnum):
    REMINDER = "reminder"
    DIGEST = "digest"
    DECLINE = "decline"
    CARD = "card"
    DEBUG_CARD = "debug_card"


class DedupeScope(StrEnum):
    NATIVE = "native"
    SOURCE = "source"
    OPERATION = "operation"


class DeliveryOutcomeKind(StrEnum):
    BOUND = "bound"
    SUPPRESSED = "suppressed"
    INDETERMINATE = "indeterminate"


_EFFECT_KIND = re.compile(r"[a-z][a-z0-9_.-]{0,127}\Z")
_MAX_MESSAGE_EMBEDS = 10
_MAX_MESSAGE_FILES = 10


def _identifier(value: Identifier, label: str) -> str:
    if isinstance(value, bool) or not isinstance(value, (str, int)):
        raise ValueError(f"{label} must be an identifier")
    normalized = str(value)
    if not normalized.strip() or normalized != normalized.strip():
        raise ValueError(f"{label} is required")
    return normalized


def _snowflake(value: Identifier, label: str) -> str:
    normalized = _identifier(value, label)
    if not _SNOWFLAKE.fullmatch(normalized):
        raise ValueError(f"{label} must be a canonical nonzero decimal ID")
    return normalized


def _bounded_text(value: str, label: str, limit: int = 512) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be text")
    value = value.strip()
    if not value or len(value) > limit:
        raise ValueError(f"{label} must be nonempty and at most {limit} characters")
    return value


def _freeze_json(value: object) -> FrozenJSON:
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError("embed data must contain finite numbers")
        return value
    if isinstance(value, Mapping):
        if any(not isinstance(key, str) for key in value):
            raise ValueError("embed object keys must be strings")
        return MappingProxyType({key: _freeze_json(item) for key, item in value.items()})
    if isinstance(value, (list, tuple)):
        return tuple(_freeze_json(item) for item in value)
    raise ValueError("embed data must be JSON-compatible")


def thaw_json(value: FrozenJSON) -> object:
    """Return a JSON-encoder-compatible copy of frozen embed data."""
    if isinstance(value, Mapping):
        return {key: thaw_json(item) for key, item in value.items()}
    if isinstance(value, tuple):
        return [thaw_json(item) for item in value]
    return value


@dataclass(frozen=True, slots=True)
class Destination:
    kind: DestinationKind
    guild_id: str | None = None
    channel_id: str | None = None
    recipient_id: str | None = None

    def __post_init__(self) -> None:
        kind = DestinationKind(self.kind)
        object.__setattr__(self, "kind", kind)
        guild = _snowflake(self.guild_id, "guild id") if self.guild_id is not None else None
        channel = _snowflake(self.channel_id, "channel id") if self.channel_id is not None else None
        recipient = (
            _snowflake(self.recipient_id, "recipient id") if self.recipient_id is not None else None
        )
        if kind is DestinationKind.CHANNEL:
            if guild is None or channel is None or recipient is not None:
                raise ValueError("channel destinations require guild and channel IDs only")
        elif guild is not None or recipient is None:
            raise ValueError("DM destinations require a recipient and no guild ID")
        object.__setattr__(self, "guild_id", guild)
        object.__setattr__(self, "channel_id", channel)
        object.__setattr__(self, "recipient_id", recipient)

    @classmethod
    def channel(cls, guild_id: Identifier, channel_id: Identifier) -> Destination:
        return cls(DestinationKind.CHANNEL, guild_id=guild_id, channel_id=channel_id)

    @classmethod
    def dm(cls, recipient_id: Identifier, *, channel_id: Identifier | None = None) -> Destination:
        return cls(
            DestinationKind.DM,
            channel_id=channel_id,
            recipient_id=recipient_id,
        )


@dataclass(frozen=True, slots=True)
class SendFile:
    name: str
    content: bytes

    def __post_init__(self) -> None:
        if not isinstance(self.name, str) or not self.name:
            raise ValueError("file name is required")
        if not isinstance(self.content, bytes):
            raise ValueError("file content must be bytes")


@dataclass(frozen=True, slots=True)
class SendPayload:
    content: str | None = None
    embeds: tuple[Mapping[str, FrozenJSON], ...] = ()
    files: tuple[SendFile, ...] = ()
    allowed_user_ids: tuple[str, ...] = ()
    allowed_role_ids: tuple[str, ...] = ()
    allow_everyone: bool = False
    replied_user: bool = False
    reference: str | None = None

    def __post_init__(self) -> None:
        if self.content is not None and not isinstance(self.content, str):
            raise ValueError("message content must be text or null")
        embeds: list[Mapping[str, FrozenJSON]] = []
        for embed in self.embeds:
            frozen = _freeze_json(embed)
            if not isinstance(frozen, Mapping):
                raise ValueError("each embed must be an object")
            embeds.append(frozen)
        files = tuple(self.files)
        if any(not isinstance(item, SendFile) for item in files):
            raise ValueError("files must be SendFile values")
        if not self.content and not embeds and not files:
            raise ValueError("message payload requires content, an embed, or a file")
        if len(embeds) > _MAX_MESSAGE_EMBEDS:
            raise ValueError("message payload supports at most 10 embeds")
        if len(files) > _MAX_MESSAGE_FILES:
            raise ValueError("message payload supports at most 10 files")
        users = tuple(
            sorted({_snowflake(item, "allowed user id") for item in self.allowed_user_ids})
        )
        roles = tuple(
            sorted({_snowflake(item, "allowed role id") for item in self.allowed_role_ids})
        )
        if not isinstance(self.allow_everyone, bool) or not isinstance(self.replied_user, bool):
            raise ValueError("mention flags must be booleans")
        reference = _snowflake(self.reference, "message reference") if self.reference else None
        object.__setattr__(self, "embeds", tuple(embeds))
        object.__setattr__(self, "files", files)
        object.__setattr__(self, "allowed_user_ids", users)
        object.__setattr__(self, "allowed_role_ids", roles)
        object.__setattr__(self, "reference", reference)


@dataclass(frozen=True, slots=True)
class DeliveryTarget:
    binding_type: BindingType
    key_primary: str | None = None
    key_secondary: str | None = None
    debug_run_id: str | None = None
    debug_kind: str | None = None

    def __post_init__(self) -> None:
        binding_type = BindingType(self.binding_type)
        object.__setattr__(self, "binding_type", binding_type)
        primary = _identifier(self.key_primary, "target primary key") if self.key_primary else None
        secondary = (
            _identifier(self.key_secondary, "target secondary key") if self.key_secondary else None
        )
        run_id = _identifier(self.debug_run_id, "debug run id") if self.debug_run_id else None
        kind = _bounded_text(self.debug_kind, "debug kind", 128) if self.debug_kind else None

        if binding_type is BindingType.REMINDER:
            valid = primary is not None and secondary is run_id is kind is None
        elif binding_type is BindingType.DIGEST:
            if primary is not None:
                try:
                    primary = to_iso(from_iso(primary))
                except (TypeError, ValueError) as exc:
                    raise ValueError("digest target must be an ISO week start") from exc
            valid = primary is not None and secondary is run_id is kind is None
        elif binding_type is BindingType.DECLINE:
            if secondary is not None:
                secondary = _snowflake(secondary, "decline user id")
            valid = primary is not None and secondary is not None and run_id is kind is None
        elif binding_type is BindingType.CARD:
            valid = primary is not None and secondary is run_id is kind is None
        else:
            valid = primary is secondary is None and run_id is not None and kind is not None
        if not valid:
            raise ValueError(f"invalid {binding_type.value} target identity")
        object.__setattr__(self, "key_primary", primary)
        object.__setattr__(self, "key_secondary", secondary)
        object.__setattr__(self, "debug_run_id", run_id)
        object.__setattr__(self, "debug_kind", kind)

    @classmethod
    def reminder(cls, reminder_id: Identifier) -> DeliveryTarget:
        return cls(BindingType.REMINDER, key_primary=_identifier(reminder_id, "reminder id"))

    @classmethod
    def digest(cls, week_start: datetime | str) -> DeliveryTarget:
        value = to_iso(week_start) if isinstance(week_start, datetime) else week_start
        return cls(BindingType.DIGEST, key_primary=value)

    @classmethod
    def decline(cls, run_id: Identifier, user_id: Identifier) -> DeliveryTarget:
        return cls(
            BindingType.DECLINE,
            key_primary=_identifier(run_id, "run id"),
            key_secondary=_identifier(user_id, "user id"),
        )

    @classmethod
    def card(cls, amendment_id: Identifier) -> DeliveryTarget:
        return cls(BindingType.CARD, key_primary=_identifier(amendment_id, "amendment id"))

    @classmethod
    def debug_card(cls, run_id: Identifier, kind: str) -> DeliveryTarget:
        return cls(
            BindingType.DEBUG_CARD, debug_run_id=_identifier(run_id, "run id"), debug_kind=kind
        )

    @property
    def claim_key(self) -> tuple[str, str, str]:
        if self.binding_type is BindingType.DEBUG_CARD:
            raise ValueError("debug-card target identity is created after transport receipt")
        return (self.binding_type.value, self.key_primary or "", self.key_secondary or "")

    @property
    def order_key(self) -> tuple[str, ...]:
        return (
            self.binding_type.value,
            self.key_primary or "",
            self.key_secondary or "",
            self.debug_run_id or "",
            self.debug_kind or "",
        )


@dataclass(frozen=True, slots=True)
class DedupePolicy:
    scope: DedupeScope
    guild_id: str | None = None
    channel_id: str | None = None
    source_id: str | None = None
    semantic_slot: str | None = None
    effect_ordinal: int | None = None

    def __post_init__(self) -> None:
        scope = DedupeScope(self.scope)
        object.__setattr__(self, "scope", scope)
        guild = _snowflake(self.guild_id, "source guild id") if self.guild_id is not None else None
        channel = (
            _snowflake(self.channel_id, "source channel id")
            if self.channel_id is not None
            else None
        )
        source = (
            _snowflake(self.source_id, "source message id") if self.source_id is not None else None
        )
        slot = (
            _bounded_text(self.semantic_slot, "semantic slot", 128) if self.semantic_slot else None
        )
        ordinal = self.effect_ordinal
        if ordinal is not None and (
            isinstance(ordinal, bool) or not isinstance(ordinal, int) or ordinal < 0
        ):
            raise ValueError("effect ordinal must be a nonnegative integer")
        if scope is DedupeScope.NATIVE:
            valid = guild is channel is source is slot is None and ordinal is None
        elif scope is DedupeScope.SOURCE:
            valid = (
                guild is not None
                and channel is not None
                and source is not None
                and slot is not None
                and ordinal is None
            )
        else:
            valid = guild is channel is source is slot is None
        if not valid:
            raise ValueError(f"invalid {scope.value} dedupe key")
        object.__setattr__(self, "guild_id", guild)
        object.__setattr__(self, "channel_id", channel)
        object.__setattr__(self, "source_id", source)
        object.__setattr__(self, "semantic_slot", slot)

    @classmethod
    def native(cls) -> DedupePolicy:
        return cls(DedupeScope.NATIVE)

    @classmethod
    def source(
        cls, guild_id: Identifier, channel_id: Identifier, source_id: Identifier, semantic_slot: str
    ) -> DedupePolicy:
        return cls(
            DedupeScope.SOURCE,
            guild_id=_snowflake(guild_id, "source guild id"),
            channel_id=_snowflake(channel_id, "source channel id"),
            source_id=_snowflake(source_id, "source message id"),
            semantic_slot=semantic_slot,
        )

    @classmethod
    def operation(cls, effect_ordinal: int | None = None) -> DedupePolicy:
        return cls(DedupeScope.OPERATION, effect_ordinal=effect_ordinal)


@dataclass(frozen=True, slots=True)
class SendPlan:
    effect_kind: str
    destination: Destination
    payload: SendPayload
    dedupe: DedupePolicy
    targets: tuple[DeliveryTarget, ...] = ()

    def __post_init__(self) -> None:
        effect_kind = _bounded_text(self.effect_kind, "effect kind", 128)
        if not _EFFECT_KIND.fullmatch(effect_kind):
            raise ValueError("effect kind must be a lowercase machine identifier")
        if not isinstance(self.destination, Destination) or not isinstance(
            self.payload, SendPayload
        ):
            raise ValueError("send plan requires a destination and payload")
        if not isinstance(self.dedupe, DedupePolicy):
            raise ValueError("send plan requires a dedupe policy")
        targets = tuple(self.targets)
        if any(not isinstance(target, DeliveryTarget) for target in targets):
            raise ValueError("send plan targets must be DeliveryTarget values")
        identities = [target.order_key for target in targets]
        if len(identities) != len(set(identities)):
            raise ValueError("send plan contains duplicate native targets")
        if len(targets) > 1:
            families = {target.binding_type for target in targets}
            if len(families) != 1 or not families <= {BindingType.REMINDER, BindingType.CARD}:
                raise ValueError(
                    "only homogeneous reminder or card groups may have multiple targets"
                )
        if self.dedupe.scope is DedupeScope.NATIVE:
            if not targets or any(
                target.binding_type is BindingType.DEBUG_CARD for target in targets
            ):
                raise ValueError("native dedupe requires stable native target identities")
        object.__setattr__(self, "effect_kind", effect_kind)
        object.__setattr__(self, "targets", targets)


@dataclass(frozen=True, slots=True)
class ObservedAttachment:
    name: str
    size: int
    url: str | None = None
    content_sha256: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.name, str) or not self.name:
            raise ValueError("observed attachment name is required")
        if isinstance(self.size, bool) or not isinstance(self.size, int) or self.size < 0:
            raise ValueError("observed attachment size must be a nonnegative integer")
        if self.url is not None and not isinstance(self.url, str):
            raise ValueError("observed attachment URL must be text or null")
        if self.content_sha256 is not None and not _SHA256.fullmatch(self.content_sha256):
            raise ValueError("observed attachment hash must be lowercase SHA-256")


@dataclass(frozen=True, slots=True)
class ObservedMessage:
    content: str | None = None
    embeds: tuple[Mapping[str, FrozenJSON], ...] = ()
    user_mentions: tuple[str, ...] = ()
    role_mentions: tuple[str, ...] = ()
    everyone_mentioned: bool = False
    replied_user: bool = False
    reference: str | None = None
    attachments: tuple[ObservedAttachment, ...] = ()

    def __post_init__(self) -> None:
        if self.content is not None and not isinstance(self.content, str):
            raise ValueError("observed content must be text or null")
        embeds: list[Mapping[str, FrozenJSON]] = []
        for embed in self.embeds:
            frozen = _freeze_json(embed)
            if not isinstance(frozen, Mapping):
                raise ValueError("each observed embed must be an object")
            embeds.append(frozen)
        users = tuple(
            sorted({_snowflake(item, "actual user mention") for item in self.user_mentions})
        )
        roles = tuple(
            sorted({_snowflake(item, "actual role mention") for item in self.role_mentions})
        )
        if not isinstance(self.everyone_mentioned, bool) or not isinstance(self.replied_user, bool):
            raise ValueError("observed mention flags must be booleans")
        reference = (
            _snowflake(self.reference, "observed message reference") if self.reference else None
        )
        attachments = tuple(self.attachments)
        if any(not isinstance(item, ObservedAttachment) for item in attachments):
            raise ValueError("observed attachments must be ObservedAttachment values")
        object.__setattr__(self, "embeds", tuple(embeds))
        object.__setattr__(self, "user_mentions", users)
        object.__setattr__(self, "role_mentions", roles)
        object.__setattr__(self, "reference", reference)
        object.__setattr__(self, "attachments", attachments)


@dataclass(frozen=True, slots=True)
class DeliveryReceipt:
    destination: Destination
    message_id: str
    observed: ObservedMessage

    def __post_init__(self) -> None:
        if not isinstance(self.destination, Destination) or not isinstance(
            self.observed, ObservedMessage
        ):
            raise ValueError(
                "delivery receipt requires an actual destination and observable message"
            )
        object.__setattr__(self, "message_id", _snowflake(self.message_id, "message id"))


@dataclass(frozen=True, slots=True)
class DeliveryOutcome:
    kind: DeliveryOutcomeKind
    attempt_id: str
    effect_ordinal: int
    state: str
    receipt: DeliveryReceipt | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "kind", DeliveryOutcomeKind(self.kind))
        if not isinstance(self.attempt_id, str) or not self.attempt_id:
            raise ValueError("attempt id is required")
        if (
            isinstance(self.effect_ordinal, bool)
            or not isinstance(self.effect_ordinal, int)
            or self.effect_ordinal < 0
        ):
            raise ValueError("effect ordinal must be nonnegative")
        if self.state not in {"intent", "indeterminate", "bound", "retired"}:
            raise ValueError("invalid delivery outcome state")
        if self.kind is DeliveryOutcomeKind.BOUND and (
            self.receipt is None or self.state != "bound"
        ):
            raise ValueError("bound outcomes require a bound state and receipt")
        if self.kind is DeliveryOutcomeKind.INDETERMINATE and (
            self.receipt is not None or self.state != "indeterminate"
        ):
            raise ValueError("indeterminate outcomes require no receipt and indeterminate state")
        if self.kind is DeliveryOutcomeKind.SUPPRESSED and self.receipt is not None:
            raise ValueError("suppressed outcomes cannot carry a receipt")
