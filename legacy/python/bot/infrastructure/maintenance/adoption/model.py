"""Typed local records produced by the offline adoption scan."""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum


class SourceFamily(StrEnum):
    REMINDERS = "reminders"
    AMENDMENTS = "amendments"
    WEEKLY_DIGESTS = "weekly_digests"
    DECLINE_NOTICES = "decline_notices"
    DEBUG_MESSAGES = "debug_messages"


class SourceReason(StrEnum):
    CANDIDATE_MESSAGE = "candidate_message"
    FUTURE_UNSENT = "future_unsent"
    DUE_UNBOUND = "due_unbound"
    PARTIAL_BINDING = "partial_binding"
    TERMINAL_UNBOUND = "terminal_unbound"
    PROPOSED_UNBOUND = "proposed_unbound"
    DIGEST_MARKER_GAP = "digest_marker_gap"
    DIGEST_RETIRED_MISMATCH = "digest_retired_mismatch"
    DEBUG_MISSING_CHANNEL = "debug_missing_channel"
    JOURNAL_CLAIM_CONFLICT = "journal_claim_conflict"
    INCONSISTENT_SOURCE = "inconsistent_source"


class ResolutionState(StrEnum):
    PENDING = "pending"
    PROVEN_UNSENT = "proven_unsent"
    TERMINAL_NO_REPLAY = "terminal_no_replay"


class AdoptionConflictError(RuntimeError):
    """A previously seeded source changed or disappeared during pending adoption."""


@dataclass(frozen=True, slots=True, order=True)
class SourceKey:
    family: str
    primary: str
    secondary: str = ""


@dataclass(frozen=True, slots=True)
class AdoptionSource:
    key: SourceKey
    reason: SourceReason
    resolution_state: ResolutionState
    evidence_hash: str
    legacy_channel_id: str | None = None
    legacy_message_id: str | None = None
    resolution_actor: str | None = None
    resolution_reason: str | None = None


@dataclass(frozen=True, slots=True)
class CandidateGroup:
    family: str
    channel_id: str
    message_id: str
    targets: tuple[SourceKey, ...]


@dataclass(frozen=True, slots=True)
class AdoptionSeedReport:
    adoption_id: str
    pinned_at: str
    boss_week_start: str
    classified_sources: int
    inserted_sources: int
    unchanged_sources: int
    candidate_groups: tuple[CandidateGroup, ...]
