"""Typed values for operator resolution of unprovable runtime delivery attempts."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import StrEnum

from ..delivery.model import Destination, ObservedMessage, _snowflake

#: Discord creation time may precede the locally recorded intent by host clock skew.
EVIDENCE_CLOCK_SKEW = timedelta(seconds=60)


class RecoveryError(RuntimeError):
    """A runtime attempt cannot be resolved safely."""


class RecoveryEvidenceError(RecoveryError):
    """The observation does not prove this attempt's actual send."""


class RecoveryDispositionError(RecoveryError):
    """No no-replay native disposition exists for this attempt."""


class RetirementClass(StrEnum):
    """Why an operator retires an attempt without evidence of its outcome."""

    OPERATOR_NO_REPLAY = "operator_no_replay"
    #: Only for retained v15 personal-memory attempts; their native rows are gone.
    FEATURE_REMOVED = "feature_removed"


class AttemptFamily(StrEnum):
    REMINDER = "reminder"
    DIGEST = "digest"
    DECLINE = "decline"
    CARD = "card"
    OPERATION = "operation"
    MEMORY = "memory"


@dataclass(frozen=True, slots=True)
class RecoveryObservation:
    """One fetched Discord message; constructing it is not proof of authorship."""

    destination: Destination
    message_id: str
    author_id: str
    created_at: datetime
    observed: ObservedMessage
    unsupported_components: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.destination, Destination) or not isinstance(
            self.observed, ObservedMessage
        ):
            raise ValueError("observation requires a destination and observed message")
        if (
            not isinstance(self.created_at, datetime)
            or self.created_at.tzinfo is None
            or self.created_at.utcoffset() is None
        ):
            raise ValueError("observation creation time must be timezone-aware")
        if not isinstance(self.unsupported_components, tuple) or any(
            not isinstance(item, str) or not item.strip() for item in self.unsupported_components
        ):
            raise ValueError("unsupported message components must be named explicitly")
        object.__setattr__(self, "message_id", _snowflake(self.message_id, "message id"))
        object.__setattr__(self, "author_id", _snowflake(self.author_id, "message author id"))


@dataclass(frozen=True, slots=True)
class TargetClaim:
    binding_type: str
    key_primary: str
    key_secondary: str
    released: bool


@dataclass(frozen=True, slots=True)
class AttemptBlocker:
    """Identities only; payloads and observed content are never reported."""

    attempt_id: str
    origin: str
    effect_kind: str
    family: str
    state: str
    destination_kind: str
    guild_id: str | None
    channel_id: str | None
    recipient_id: str | None
    intended_at: str
    operation_id: str
    operation_lease: str | None
    targets: tuple[TargetClaim, ...]


@dataclass(frozen=True, slots=True)
class OrphanLease:
    operation_id: str
    operation_kind: str
    orphaned_at: str | None


@dataclass(frozen=True, slots=True)
class BlockerReport:
    mode: str
    blocker_code: str | None
    state_revision: int
    adoption_state: str
    adoption_id: str | None
    pending_adoption_sources: int
    attempts: tuple[AttemptBlocker, ...]
    orphan_leases: tuple[OrphanLease, ...]
    #: Proposed amendments whose card send was retired unproven. They never
    #: repost; the portal inbox resolves them, and pending proposals block export.
    retired_card_proposals: tuple[str, ...]
    live_leases: int
    #: False while any persistent blocker remains. Clear does not reopen:
    #: BLOCKED/FROZEN -> OPEN still requires the explicit resume controller.
    persistent_blockers_clear: bool
