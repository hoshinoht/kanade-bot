"""Faithful retained operational history models."""

from datetime import datetime
from typing import Literal
from uuid import UUID

from pydantic import Field

from .common import AwareModel, Identifier, Snowflake


class Message(AwareModel):
    id: Snowflake
    channel_id: Snowflake
    author_id: Snowflake
    created_at: datetime
    content: str
    processed_at: datetime | None = None


class Extraction(AwareModel):
    id: UUID
    at: datetime
    model: str
    prompt: str
    raw_response: str
    latency_ms: int | None = Field(default=None, ge=0)
    message_ids: list[Snowflake]
    amendment_ids: list[UUID]


class StrategyTrace(AwareModel):
    boss: str
    difficulty: str
    path: str
    researched_as_of: str
    meta_hash: str
    document_hash: str
    source_count: int = Field(ge=0)


class ToolTrace(AwareModel):
    name: str
    round: int = Field(ge=0)
    arguments: str
    output: str
    ms: int = Field(ge=0)
    outcome: str
    created: list[UUID]
    posted: list[UUID]
    strategy: StrategyTrace | None = None


class ModelRound(AwareModel):
    round: int = Field(ge=1)
    content: str | None = None
    thinking: str | None = None
    requested_tools: list[str]


class ChatInteraction(AwareModel):
    id: UUID
    at: datetime
    channel_id: Snowflake | None = None
    message_id: Snowflake | None = None
    author_id: Snowflake | None = None
    model: str
    question: str
    reply: str
    outcome: Literal["answered", "failed"]
    error: str | None = None
    rounds: int = Field(ge=0)
    latency_ms: int | None = Field(default=None, ge=0)
    model_ms: int | None = Field(default=None, ge=0)
    tools_ms: int | None = Field(default=None, ge=0)
    prompt_tokens: int | None = Field(default=None, ge=0)
    completion_tokens: int | None = Field(default=None, ge=0)
    tool_calls: list[ToolTrace]
    model_rounds: list[ModelRound]


class Audit(AwareModel):
    id: UUID
    at: datetime
    surface: Identifier
    actor: str
    action: str
    subject: str | None = None
    detail: str


class RescanJob(AwareModel):
    id: UUID
    channels: list[Snowflake]
    window: str
    source: Identifier
    automated: bool
    requested_by: Snowflake | None = None
    status: Literal["queued", "running", "done", "failed", "cancelled"]
    created_at: datetime
    started_at: datetime | None = None
    finished_at: datetime | None = None
    results: list["RescanChannelResult"]
    error: str | None = None


class RescanProposal(AwareModel):
    kind: Literal["move", "add", "cancel", "otot", "split", "sub", "rsvp", "fix"]
    bosses: list[str]
    confidence: float = Field(ge=0, le=1)
    run_id: UUID | None = None


class RescanChannelResult(AwareModel):
    channel_id: Snowflake
    channel_name: str
    asked: bool
    window: str
    since: datetime
    widened: bool
    backfilled: int = Field(ge=0)
    stored: int = Field(ge=0)
    gated: int = Field(ge=0)
    bursts: int = Field(ge=0)
    extracted: int = Field(ge=0)
    proposals: int = Field(ge=0)
    dropped: int = Field(ge=0)
    stale: int = Field(ge=0)
    elapsed_ms: int = Field(ge=0)
    cancelled: bool
    error: str | None = None
    summary: str
    proposed: list[RescanProposal]


class History(AwareModel):
    messages: list[Message]
    extractions: list[Extraction]
    chat_interactions: list[ChatInteraction]
    audit: list[Audit]
    rescan_jobs: list[RescanJob]
