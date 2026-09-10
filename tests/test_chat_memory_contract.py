"""Pure grammar, lifecycle, and rendering contracts for governed memory."""

from __future__ import annotations

from datetime import UTC, datetime, timedelta, tzinfo

import pytest

from bot.chat.memory import (
    MEMORY_ACTIVE_TTL,
    MEMORY_EVENT_RETENTION,
    MEMORY_HELP,
    MEMORY_INACTIVE_RETENTION,
    MEMORY_MAX_RECORDS,
    MEMORY_MAX_RENDERED_CHARS,
    MEMORY_PROPOSAL_TTL,
    MEMORY_RETRIEVAL_LIMIT,
    MEMORY_RETRIEVAL_RETENTION,
    AnswerDetail,
    AnswerFormat,
    CanonicalBoss,
    MemoryLifecycle,
    MemoryParseKind,
    MemoryRecord,
    StrategyEmphasis,
    parse_memory_request,
    rank_memories,
    render_memories,
)
from bot.domain.bosses import BossReference, BossTable


def resolved_boss(table: BossTable) -> CanonicalBoss:
    return CanonicalBoss.from_resolved(table, table.resolve_reference("hbm"))


class BrokenTimezone(tzinfo):
    def utcoffset(self, _value):
        raise ValueError("invalid timezone")

    def dst(self, _value):
        raise ValueError("invalid timezone")


@pytest.mark.parametrize(
    ("text", "slot", "value", "boss"),
    [
        (
            "remember preference: answer_detail=concise",
            "answer_detail",
            AnswerDetail.CONCISE,
            None,
        ),
        (
            " Remember Preference : strategy_emphasis = SURVIVAL ; boss = Black Mage ",
            "strategy_emphasis",
            StrategyEmphasis.SURVIVAL,
            "Black Mage",
        ),
        (
            "remember preference: ANSWER_FORMAT=BuLlEtS",
            "answer_format",
            AnswerFormat.BULLETS,
            None,
        ),
    ],
)
def test_parser_accepts_only_the_typed_forms(text, slot, value, boss):
    result = parse_memory_request(text)

    assert result.kind is MemoryParseKind.VALID
    assert result.preference is not None
    assert result.preference.slot == slot
    assert result.preference.value is value
    assert result.preference.boss == boss


@pytest.mark.parametrize(
    "text",
    [
        "remember preference: answer_detail=concise; answer_format=prose",
        "remember preference: answer_detail=concise; answer_detail=detailed",
        "remember preference: answer_detail=concise; boss=Black Mage",
        "remember preference: strategy_emphasis=survival; boss=",
        "remember preference: strategy_emphasis=survival; boss=" + "x" * 65,
        "remember preference: strategy_emphasis=survival; boss=Black;Mage",
        "remember preference: strategy_emphasis=survival\n; boss=Black Mage",
        "remember preference: answer_detail=verbose",
        "remember preference: answer_detail=concise?",
        "remember preference: answer_detail=concise; boss=`Black Mage`",
        "remember preference: answer_detail=concise; boss=not Black Mage",
    ],
)
def test_malformed_prefixed_text_is_classified_with_deterministic_help(text):
    result = parse_memory_request(text)

    assert result.kind is MemoryParseKind.MALFORMED
    assert result.preference is None
    assert result.help == MEMORY_HELP


@pytest.mark.parametrize(
    "text",
    [
        "what is the answer detail?",
        "please remember preference: answer_detail=concise",
        "do not remember preference: answer_detail=concise",
        "remember when I preferred concise answers",
        "`remember preference: answer_detail=concise`",
    ],
)
def test_non_memory_text_does_not_enter_the_memory_branch(text):
    assert parse_memory_request(text).kind is MemoryParseKind.NOT_MEMORY


def test_contextual_negatives_have_a_pure_caller_flag():
    result = parse_memory_request("remember preference: answer_detail=concise", reply_derived=True)

    assert result.kind is MemoryParseKind.REJECTED
    assert result.reason == "reply_derived"
    assert result.help == MEMORY_HELP


def test_lifecycle_and_retention_contracts_are_fixed():
    assert set(MemoryLifecycle) == {
        MemoryLifecycle.PROPOSED,
        MemoryLifecycle.ACTIVE,
        MemoryLifecycle.SUPERSEDED,
        MemoryLifecycle.REJECTED,
        MemoryLifecycle.REVOKED,
        MemoryLifecycle.EXPIRED,
    }
    assert MEMORY_PROPOSAL_TTL == timedelta(days=7)
    assert MEMORY_ACTIVE_TTL == timedelta(days=180)
    assert MEMORY_INACTIVE_RETENTION == timedelta(days=30)
    assert MEMORY_RETRIEVAL_RETENTION == timedelta(days=30)
    assert MEMORY_EVENT_RETENTION == timedelta(days=365)
    assert MEMORY_RETRIEVAL_LIMIT == 500


def test_ranking_filters_lifecycle_expiry_and_prefers_specific_then_newest(bosses):
    now = datetime(2026, 9, 10, tzinfo=UTC)
    future = now + timedelta(days=1)
    boss = resolved_boss(bosses)
    records = [
        MemoryRecord(
            "strategy_emphasis",
            "mechanics",
            accepted_at=now - timedelta(days=1),
            memory_id="general-old",
            expires_at=future,
        ),
        MemoryRecord(
            "strategy_emphasis",
            "survival",
            boss=boss,
            accepted_at=now - timedelta(days=30),
            memory_id="specific",
            expires_at=future,
        ),
        MemoryRecord(
            "answer_format",
            "steps",
            lifecycle=MemoryLifecycle.REVOKED,
            accepted_at=now,
            memory_id="revoked",
            expires_at=future,
        ),
        MemoryRecord(
            "strategy_emphasis",
            "survival",
            expires_at=now,
            accepted_at=now,
            memory_id="expired",
        ),
    ]

    ranked = rank_memories(records, boss=boss, now=now)

    assert [record.memory_id for record in ranked] == ["specific"]


def test_missing_naive_mixed_and_invalid_expiry_fail_closed():
    now = datetime(2026, 9, 10, tzinfo=UTC)
    future = now + timedelta(days=1)
    missing = MemoryRecord("answer_detail", "concise", memory_id="missing")
    naive = MemoryRecord(
        "answer_detail",
        "concise",
        expires_at=future.replace(tzinfo=None),
        memory_id="naive",
    )
    invalid = MemoryRecord(
        "answer_detail",
        "concise",
        expires_at=datetime(2026, 9, 11, tzinfo=BrokenTimezone()),
        memory_id="invalid",
    )
    eligible = MemoryRecord("answer_detail", "concise", expires_at=future, memory_id="eligible")

    assert rank_memories([missing], now=now) == []
    assert rank_memories([naive], now=now) == []
    assert rank_memories([invalid], now=now) == []
    assert rank_memories([eligible], now=now.replace(tzinfo=None)) == []
    assert rank_memories([eligible], now=now) == [eligible]
    assert render_memories([missing], now=now) == ""
    assert render_memories([naive], now=now) == ""


def test_canonical_boss_requires_a_resolved_reference_and_typed_scope(bosses):
    now = datetime(2026, 9, 10, tzinfo=UTC)
    future = now + timedelta(days=1)
    boss = resolved_boss(bosses)

    assert boss.value == "HBM"
    with pytest.raises(TypeError):
        CanonicalBoss("HBlackMage")
    with pytest.raises(ValueError):
        CanonicalBoss.from_resolved(bosses, BossReference("UnknownBoss", "h"))
    with pytest.raises(ValueError):
        CanonicalBoss.from_resolved(bosses, BossReference("BM", "n"))
    assert CanonicalBoss.from_storage(bosses, "HBM").value == "HBM"
    with pytest.raises(ValueError):
        CanonicalBoss.from_storage(bosses, "hzzz")
    with pytest.raises(ValueError):
        CanonicalBoss.from_storage(bosses, "NBM")
    with pytest.raises(TypeError):
        MemoryRecord(
            "strategy_emphasis",
            "survival",
            boss="HBlackMage",
            expires_at=future,
        )
    with pytest.raises(TypeError):
        MemoryRecord.from_mapping(
            {
                "slot": "strategy_emphasis",
                "value": "survival",
                "boss": "HBlackMage",
                "expires_at": future,
            }
        )


def test_renderer_is_bounded_and_excludes_untyped_content(bosses):
    now = datetime.now(UTC)
    future = now + timedelta(days=1)
    records = [
        MemoryRecord(
            "strategy_emphasis",
            "mechanics",
            boss=resolved_boss(bosses),
            accepted_at=now,
            memory_id="do-not-render-this-id",
            expires_at=future,
        ),
        MemoryRecord(
            "answer_detail",
            "concise",
            accepted_at=now,
            memory_id="also-private",
            expires_at=future,
        ),
    ]

    rendered = render_memories(records)

    assert len(rendered) <= MEMORY_MAX_RENDERED_CHARS
    assert rendered.count("\n- ") <= MEMORY_MAX_RECORDS - 1
    assert "strategy_emphasis=mechanics; boss=HBM" in rendered
    assert "do-not-render-this-id" not in rendered
    assert "also-private" not in rendered
