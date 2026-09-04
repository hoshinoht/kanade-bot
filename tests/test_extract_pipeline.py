"""The burst pipeline: what gets planned, what gets posted, what gets applied.

The model itself is stubbed -- :mod:`tests.test_extract_fixtures` is where the
real one is measured -- so these tests pin the deterministic behaviour around it:
merging, resolving, matching, the confidence floor, the rsvp shortcut, and the
one-card-per-burst rule.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field

import pytest

from bot.agent import formatting
from bot.extract import gate
from bot.extract.llm import ExtractionCall
from bot.extract.pipeline import Pipeline, plan_burst, urgent
from bot.extract.schema import Amendment, Extraction
from bot.infrastructure.config import Settings
from bot.infrastructure.db import Repo

from .conftest import PING_TIME, TZ, kl

CHANNEL = "900"
MY, ALVIN, PRIYA, KANON = "1", "2", "3", "4"
WEEK = kl(2026, 8, 27)
ANCHOR = kl(2026, 8, 30, 13, 7)


def amendment(kind="move", **kwargs) -> Amendment:
    return Amendment(kind=kind, **kwargs)


# ---------------------------------------------------------------------------
# plan_burst -- pure
# ---------------------------------------------------------------------------


@pytest.fixture
def runs(repo: Repo) -> list[dict]:
    ids = [
        repo.create_run(
            WEEK,
            ["HStar", "HFA"],
            kl(2026, 8, 31, 21, 30),
            [MY, ALVIN, PRIYA],
            channel_id=CHANNEL,
        ),
        repo.create_run(
            WEEK,
            ["HCarling", "XKalos"],
            kl(2026, 9, 1, 23, 0),
            [MY, PRIYA, KANON],
            channel_id=CHANNEL,
        ),
    ]
    return [repo.get_run(i) for i in ids]


@pytest.fixture(autouse=True)
def _as_of_the_conversation(monkeypatch):
    """Run this file's fixtures as of the evening they describe.

    The pipeline drops amendments that are already in the past, judged against
    the real now. These fixtures are dated, so without a frozen clock they stop
    proposing anything the moment the date rolls over -- which is the rule
    working, not the pipeline breaking. `tests/test_rescan_window.py` is where
    that rule itself is tested, with explicit times.
    """
    from datetime import timedelta

    from bot.extract import pipeline as pipeline_module

    monkeypatch.setattr(pipeline_module, "utcnow", lambda: ANCHOR + timedelta(minutes=1))


def plan(extraction, runs, **kwargs):
    kwargs.setdefault("anchor", ANCHOR)
    kwargs.setdefault("tz", TZ)
    # Pin "now" to the conversation, not the wall clock: these fixtures are
    # dated, and `plan_burst` drops anything already in the past. Tests that are
    # *about* that rule pass their own `now` (see tests/test_rescan_window.py).
    kwargs.setdefault("now", ANCHOR)
    return plan_burst(extraction, channel_runs=runs, **kwargs)


def test_a_move_is_resolved_and_matched(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(bosses=["HStar", "HFA"], day_ref="wed", time_ref="9:30pm", confidence=0.9)
            ]
        ),
        runs,
    )
    (entry,) = result.planned
    assert entry.resolved.at == kl(2026, 9, 2, 21, 30)
    assert entry.run["id"] == runs[0]["id"]


def test_a_day_with_no_time_takes_the_time_the_run_already_has(runs):
    """Nobody proposed changing the time, so it is not TBD -- it is 21:30."""
    result = plan(
        Extraction(amendments=[amendment(bosses=["HStar"], day_ref="wed", confidence=0.9)]),
        runs,
    )
    (entry,) = result.planned
    assert entry.resolved.day == kl(2026, 9, 2).date()
    assert entry.resolved.at == kl(2026, 9, 2, 21, 30)
    # A stated change with a concrete instant is a decision, not a question.
    assert not entry.needs_answer


def test_a_day_with_no_time_that_was_asked_stays_a_question(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(bosses=["HStar"], day_ref="wed", confidence=0.9, is_question=True)
            ]
        ),
        runs,
    )
    (entry,) = result.planned
    assert entry.resolved.at == kl(2026, 9, 2, 21, 30)
    assert entry.needs_answer


def test_anything_below_the_confidence_floor_is_dropped(runs):
    result = plan(
        Extraction(amendments=[amendment(bosses=["HStar"], day_ref="wed", confidence=0.4)]),
        runs,
        min_confidence=0.6,
    )
    assert not result.planned and len(result.dropped) == 1


def test_a_move_that_matches_no_run_is_dropped(runs):
    result = plan(
        Extraction(amendments=[amendment(bosses=["NStar"], day_ref="wed", confidence=0.9)]),
        runs,
    )
    assert not result.planned
    assert "no run matched" in result.dropped[0].match_reason


def test_an_add_that_matches_no_run_is_kept_it_creates_one(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "add", bosses=["NStar"], day_ref="tonight", time_ref="9:45pm", confidence=0.9
                )
            ]
        ),
        runs,
    )
    (entry,) = result.planned
    assert entry.run is None and entry.resolved.at == kl(2026, 8, 30, 21, 45)


def test_a_fix_becomes_a_weekday_and_a_clock_time(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "fix", bosses=["HLimbo"], day_ref="tue", time_ref="1030pm", confidence=0.9
                )
            ]
        ),
        runs,
    )
    assert result.planned[0].payload == {"weekday": 1, "time": "22:30"}


def test_a_fix_with_no_time_carries_no_payload_so_the_commit_refuses(runs):
    result = plan(
        Extraction(amendments=[amendment("fix", bosses=["HLimbo"], day_ref="tue", confidence=0.9)]),
        runs,
    )
    assert result.planned[0].payload == {}


def test_a_split_payload_only_names_bosses_the_run_actually_has(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment("split", bosses=["XKalos", "NBaldrix"], day_ref="wed", confidence=0.9)
            ]
        ),
        runs,
    )
    assert result.planned[0].payload["bosses"] == ["XKalos"]


def test_a_sub_marks_the_person_asking_as_the_one_dropping_out(runs):
    result = plan(
        Extraction(
            amendments=[amendment("sub", bosses=["HStar"], participants=[MY], confidence=0.9)]
        ),
        runs,
    )
    assert result.planned[0].payload == {"remove": [MY], "add": []}


def test_rsvps_and_proposals_are_separated(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(bosses=["HStar"], day_ref="wed", time_ref="9:30pm", confidence=0.9),
                amendment(
                    "rsvp", bosses=["HStar"], participants=[PRIYA], rsvp="yes", confidence=0.9
                ),
            ]
        ),
        runs,
    )
    assert len(result.proposals) == 1 and len(result.rsvps) == 1


def test_the_author_of_the_evidence_is_used_for_matching_when_nobody_is_named(runs):
    # KANON is only on the HCarling/XKalos run, and no bosses were named.
    result = plan(
        Extraction(
            amendments=[amendment(day_ref="wed", evidence_message_ids=["7"], confidence=0.9)]
        ),
        runs,
        author_ids={"7": KANON},
    )
    assert result.planned[0].run["id"] == runs[1]["id"]


# ---------------------------------------------------------------------------
# the urgency rule
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "text",
    [
        "@here pls note tmr 1030~11+pm hlimbo+baldrix",
        "<@1> <@2> we doing hstar tonight?",
        "@here see u all later at 11",
    ],
)
def test_a_ping_with_a_boss_or_a_time_does_not_wait_for_the_debounce(text, bosses):
    assert urgent(gate.evaluate(text, bosses, [MY, ALVIN]))


@pytest.mark.parametrize(
    "text",
    [
        "Aiyo amend to 9:45pm",  # no mention: it can wait for the burst to settle
        "<@1> give me your paynow",  # a mention, but nothing scheduled
        "@here i abit lazy to list the FB le",
    ],
)
def test_everything_else_waits(text, bosses):
    assert not urgent(gate.evaluate(text, bosses, [MY, ALVIN]))


# ---------------------------------------------------------------------------
# the pipeline against a stand-in bot
# ---------------------------------------------------------------------------


@dataclass
class FakeMessage:
    id: int
    content: str
    embed: object | None = None


@dataclass
class FakeChannel:
    id: int = int(CHANNEL)
    name: str = "party-hstar"
    sent: list = field(default_factory=list)

    async def send(self, content, **kwargs):
        message = FakeMessage(id=5000 + len(self.sent), content=content)
        self.sent.append((content, kwargs))
        return message


class FakeBot:
    """Just the surface of :class:`bot.client.BossBot` that the pipeline touches."""

    def __init__(self, repo: Repo, bosses, settings: Settings):
        self.repo = repo
        self.bosses = bosses
        self.settings = settings
        self.tz = TZ
        self.paused = False
        self.channel = FakeChannel()
        self.declines: list[tuple] = []
        self.retractions: list[tuple] = []
        self.posted: list = []
        self.annotated: list[tuple] = []

    def get_channel(self, _id):
        return self.channel

    async def post_channel(self, _channel_id=None):
        return self.channel

    async def _post(self, channel, card, mention_users=None):
        self.posted.append((card, mention_users))
        return await channel.send(card.content)

    async def annotate_message(self, channel_id, message_id, notice):
        self.annotated.append((str(message_id), notice))

    async def notify_decline(self, run, user_id, name, channel_id=None, reference_id=None):
        self.declines.append((run["id"], str(user_id), name))

    async def retract_decline(self, run, user_id):
        self.retractions.append((run["id"], str(user_id)))


class StubExtractor:
    """Returns a canned extraction instead of calling a 13 GB model."""

    def __init__(self, extraction: Extraction | None, error: str | None = None):
        self.extraction = extraction
        self.error = error
        self.calls: list[list[dict]] = []

    async def extract(self, messages):
        self.calls.append(messages)
        return ExtractionCall(
            prompt="\n".join(m["content"] for m in messages),
            raw="{}",
            latency_ms=42,
            extraction=self.extraction,
            error=self.error,
        )

    async def close(self):
        pass


@pytest.fixture
def settings() -> Settings:
    return Settings(
        discord_token="x",
        guild_id=1,
        bossing_role_id=1,
        chat_channel_ids=CHANNEL,
        extract_min_confidence=0.6,
    )


@pytest.fixture
def wired(repo: Repo, bosses, settings, runs):
    for uid, name in ((MY, "MY"), (ALVIN, "Alvin"), (PRIYA, "Priya"), (KANON, "kanon")):
        repo.upsert_member(uid, name, None, True)
    return FakeBot(repo, bosses, settings)


def store(repo: Repo, message_id: str, author: str, when, text: str) -> dict:
    repo.record_message(message_id, CHANNEL, author, when, text)
    return repo.get_message(message_id)


def test_a_proposal_becomes_one_row_and_one_card(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "mon cannot, change to wed 9:30pm?")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(
                    bosses=["HStar", "HFA"],
                    day_ref="wed",
                    time_ref="9:30pm",
                    confidence=0.9,
                    evidence_message_ids=["101"],
                    participants=[MY],
                )
            ],
            summary="proposed for wed",
        )
    )
    pipeline = Pipeline(wired, extractor=stub)
    plan = asyncio.run(pipeline.extract(CHANNEL, rows))

    assert len(plan.proposals) == 1
    (stored,) = repo.list_amendments(status="proposed")
    assert stored["kind"] == "move"
    assert stored["new_datetime"] == kl(2026, 9, 2, 21, 30)
    assert stored["day_ref"] == "wed" and stored["time_ref"] == "9:30pm"
    assert stored["channel_id"] == CHANNEL
    assert stored["evidence_msg_ids"] == ["101"]
    # One card, and the reaction path can find its way back from it.
    assert len(wired.channel.sent) == 1
    assert repo.amendments_by_message(5000)[0]["id"] == stored["id"]
    # The burst is not re-read next time.
    assert repo.get_message("101")["processed_at"] is not None


def test_one_card_covers_every_amendment_in_the_burst(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "mon and tue cannot this week")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment("cancel", bosses=["HStar", "HFA"], confidence=0.9),
                amendment("cancel", bosses=["HCarling", "XKalos"], confidence=0.9),
            ]
        )
    )
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    assert len(repo.list_amendments(status="proposed")) == 2
    assert len(wired.channel.sent) == 1
    assert len(repo.amendments_by_message(5000)) == 2


def test_an_rsvp_is_applied_straight_away_with_no_card(wired, repo: Repo, runs):
    rows = [store(repo, "101", PRIYA, ANCHOR, "can")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(
                    "rsvp",
                    bosses=["HStar", "HFA"],
                    participants=[PRIYA],
                    rsvp="yes",
                    confidence=0.9,
                )
            ]
        )
    )
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    assert repo.get_rsvps(runs[0]["id"]) == {PRIYA: "yes"}
    assert repo.list_amendments() == []
    assert wired.channel.sent == []
    assert wired.retractions == [(runs[0]["id"], PRIYA)]


def test_a_decline_from_chat_tells_the_rest_of_the_party(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "today kenot sry")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(
                    "rsvp", bosses=["HStar", "HFA"], participants=[MY], rsvp="no", confidence=0.9
                )
            ]
        )
    )
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    assert repo.get_rsvps(runs[0]["id"]) == {MY: "no"}
    assert repo.get_run(runs[0]["id"])["status"] == "at_risk"
    assert wired.declines == [(runs[0]["id"], MY, "MY")]


def test_an_rsvp_from_someone_not_on_the_run_is_ignored(wired, repo: Repo, runs):
    rows = [store(repo, "101", KANON, ANCHOR, "can")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(
                    "rsvp",
                    bosses=["HStar", "HFA"],
                    participants=[KANON],
                    rsvp="yes",
                    confidence=0.9,
                )
            ]
        )
    )
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))
    assert repo.get_rsvps(runs[0]["id"]) == {}


def test_a_low_confidence_amendment_is_logged_and_never_posted(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "maybe wed?")]
    stub = StubExtractor(
        Extraction(amendments=[amendment(bosses=["HStar"], day_ref="wed", confidence=0.3)])
    )
    plan = asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    assert plan.planned == [] and len(plan.dropped) == 1
    assert repo.list_amendments() == []
    assert wired.channel.sent == []


def test_a_model_failure_is_logged_and_changes_nothing(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "change to wed?")]
    stub = StubExtractor(None, error="connection refused")
    plan = asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    assert plan.error == "connection refused"
    assert repo.list_amendments() == []
    assert repo.get_run(runs[0]["id"])["datetime"] == runs[0]["datetime"]
    # The call is still logged: the extraction log is the prompt-tuning tool.
    assert len(repo.recent_extractions()) == 1


def test_every_call_lands_in_the_extraction_log(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "change to wed 9:30pm?")]
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(bosses=["HStar"], day_ref="wed", time_ref="9:30pm", confidence=0.9)
            ]
        )
    )
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    (logged,) = repo.recent_extractions()
    assert logged["message_ids"] == ["101"]
    assert len(logged["amendment_ids"]) == 1
    assert "NEW MESSAGES" in logged["prompt"]


def test_the_prompt_carries_this_channels_runs_and_roster(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "change to wed?")]
    stub = StubExtractor(Extraction())
    asyncio.run(Pipeline(wired, extractor=stub).extract(CHANNEL, rows))

    (system, user) = stub.calls[0]
    assert "HStar + HFA" in user["content"]
    assert "<@1> = MY" in user["content"]
    assert "[101]" in user["content"]
    assert "evidence" in system["content"].lower()


def test_nothing_is_posted_when_the_model_finds_nothing(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "cch7 hstar map")]
    plan = asyncio.run(
        Pipeline(wired, extractor=StubExtractor(Extraction())).extract(CHANNEL, rows)
    )

    assert plan.planned == []
    assert wired.channel.sent == []
    assert repo.get_message("101")["processed_at"] is not None


def test_rescan_can_be_asked_not_to_post(wired, repo: Repo, runs):
    store(repo, "101", MY, ANCHOR, "change to wed 9:30pm?")
    stub = StubExtractor(
        Extraction(
            amendments=[
                amendment(bosses=["HStar"], day_ref="wed", time_ref="9:30pm", confidence=0.9)
            ]
        )
    )
    plan = asyncio.run(Pipeline(wired, extractor=stub).rescan(CHANNEL, hours=24, post=False))

    assert len(plan.proposals) == 1
    assert wired.channel.sent == []  # /debug extract must never spam the channel
    # ...and a dry run writes nothing, so the live pass still sees the messages.
    assert repo.list_amendments() == []
    assert repo.get_message("101")["processed_at"] is None


def test_rescan_ignores_processed_at(wired, repo: Repo, runs):
    store(repo, "101", MY, ANCHOR, "change to wed 9:30pm?")
    repo.mark_messages_processed(["101"])
    stub = StubExtractor(Extraction())
    asyncio.run(Pipeline(wired, extractor=stub).rescan(CHANNEL, hours=24, post=False))
    assert stub.calls, "a rescan re-reads messages the live pass already handled"


def test_rescan_skips_people_without_the_bossing_role(wired, repo: Repo, runs):
    repo.upsert_member("99", "stranger", None, False)
    store(repo, "101", "99", ANCHOR, "change to wed 9:30pm?")
    stub = StubExtractor(Extraction())
    assert (
        asyncio.run(Pipeline(wired, extractor=stub).rescan(CHANNEL, hours=24, post=False)) is None
    )
    assert not stub.calls


# ---------------------------------------------------------------------------
# superseding an older card
# ---------------------------------------------------------------------------


def move_extraction(time_ref="9:30pm"):
    return Extraction(
        amendments=[
            amendment(
                bosses=["HStar", "HFA"],
                day_ref="wed",
                time_ref=time_ref,
                confidence=0.9,
                evidence_message_ids=["101"],
            )
        ]
    )


def test_a_second_card_for_the_same_run_retires_the_first(wired, repo: Repo, runs):
    """An urgent flush and the later debounce flush must not leave two live cards."""
    rows = [store(repo, "101", MY, ANCHOR, "change to wed 9:30pm?")]
    pipeline = Pipeline(wired, extractor=StubExtractor(move_extraction("9:30pm")))
    asyncio.run(pipeline.extract(CHANNEL, rows))
    (first,) = repo.list_amendments(status="proposed")

    rows = [store(repo, "102", MY, ANCHOR, "actually make it 10pm")]
    pipeline = Pipeline(wired, extractor=StubExtractor(move_extraction("10pm")))
    asyncio.run(pipeline.extract(CHANNEL, rows))

    assert repo.get_amendment(first["id"])["status"] == "superseded"
    live = repo.list_amendments(status="proposed")
    assert len(live) == 1 and live[0]["new_datetime"] == kl(2026, 9, 2, 22, 0)
    # The stale card says so in the channel rather than sitting there pressable.
    assert wired.annotated == [("5000", formatting.SUPERSEDED_NOTICE)]


def test_a_second_card_for_a_new_run_is_keyed_on_its_bosses(wired, repo: Repo, runs):
    def add_extraction(time_ref):
        return Extraction(
            amendments=[
                amendment(
                    "add",
                    bosses=["NStar", "NCarling"],
                    day_ref="tonight",
                    time_ref=time_ref,
                    confidence=0.9,
                    evidence_message_ids=["101"],
                )
            ]
        )

    rows = [store(repo, "101", MY, ANCHOR, "we do nstar and ncarl tonight 9pm?")]
    asyncio.run(
        Pipeline(wired, extractor=StubExtractor(add_extraction("9pm"))).extract(CHANNEL, rows)
    )
    (first,) = repo.list_amendments(status="proposed")

    rows = [store(repo, "102", MY, ANCHOR, "amend to 9:45pm")]
    asyncio.run(
        Pipeline(wired, extractor=StubExtractor(add_extraction("9:45pm"))).extract(CHANNEL, rows)
    )

    assert repo.get_amendment(first["id"])["status"] == "superseded"
    assert len(repo.list_amendments(status="proposed")) == 1


def test_a_card_about_a_different_run_is_left_alone(wired, repo: Repo, runs):
    rows = [store(repo, "101", MY, ANCHOR, "tue run to wed?")]
    other = Extraction(
        amendments=[
            amendment(
                bosses=["HCarling", "XKalos"],
                day_ref="wed",
                time_ref="11pm",
                confidence=0.9,
                evidence_message_ids=["101"],
            )
        ]
    )
    asyncio.run(Pipeline(wired, extractor=StubExtractor(other)).extract(CHANNEL, rows))
    (untouched,) = repo.list_amendments(status="proposed")

    rows = [store(repo, "102", MY, ANCHOR, "mon run to wed 9:30pm?")]
    asyncio.run(Pipeline(wired, extractor=StubExtractor(move_extraction())).extract(CHANNEL, rows))

    assert repo.get_amendment(untouched["id"])["status"] == "proposed"
    assert len(repo.list_amendments(status="proposed")) == 2
    assert wired.annotated == []


def test_a_dry_run_supersedes_nothing(wired, repo: Repo, runs):
    store(repo, "101", MY, ANCHOR, "change to wed 9:30pm?")
    asyncio.run(
        Pipeline(wired, extractor=StubExtractor(move_extraction())).extract(
            CHANNEL, [repo.get_message("101")]
        )
    )
    (live,) = repo.list_amendments(status="proposed")

    asyncio.run(
        Pipeline(wired, extractor=StubExtractor(move_extraction("10pm"))).rescan(
            CHANNEL, hours=24, post=False
        )
    )
    assert repo.get_amendment(live["id"])["status"] == "proposed"


# ---------------------------------------------------------------------------
# a burst that proposes a run and then settles its time
# ---------------------------------------------------------------------------


def test_a_later_time_change_folds_into_the_new_run_it_is_about(runs):
    """ "we doing our nstar tonight?" -> "9pm" -> "amend to 9:45pm" is ONE new run."""
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "add",
                    bosses=["NStar", "NCarling"],
                    day_ref="tonight",
                    time_ref="9pm",
                    confidence=0.9,
                    evidence_message_ids=["401"],
                ),
                amendment(
                    "move",
                    bosses=["NStar", "NCarling"],
                    time_ref="9:45pm",
                    confidence=0.9,
                    evidence_message_ids=["403"],
                ),
            ]
        ),
        runs,
        burst_order=["401", "402", "403"],
    )
    (entry,) = result.planned
    assert entry.kind == "add"
    assert entry.resolved.at == kl(2026, 8, 30, 21, 45)
    assert entry.amendment.evidence_message_ids == ["401", "403"]


def test_a_move_about_a_run_that_does_exist_is_never_folded(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "add",
                    bosses=["NStar"],
                    day_ref="tonight",
                    time_ref="9pm",
                    confidence=0.9,
                    evidence_message_ids=["401"],
                ),
                amendment(
                    "move",
                    bosses=["HStar", "HFA"],
                    day_ref="wed",
                    time_ref="9:30pm",
                    confidence=0.9,
                    evidence_message_ids=["403"],
                ),
            ]
        ),
        runs,
        burst_order=["401", "403"],
    )
    assert {e.kind for e in result.planned} == {"add", "move"}
    add = next(e for e in result.planned if e.kind == "add")
    assert add.resolved.at == kl(2026, 8, 30, 21, 0)  # untouched by the move


def test_an_earlier_move_is_not_folded_into_a_later_add(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "move",
                    bosses=["NStar"],
                    time_ref="9:45pm",
                    confidence=0.9,
                    evidence_message_ids=["401"],
                ),
                amendment(
                    "add",
                    bosses=["NStar"],
                    day_ref="tonight",
                    time_ref="9pm",
                    confidence=0.9,
                    evidence_message_ids=["403"],
                ),
            ]
        ),
        runs,
        burst_order=["401", "403"],
    )
    # The move has nothing to apply to and is dropped; the add keeps its own time.
    add = next(e for e in result.planned if e.kind == "add")
    assert add.resolved.at == kl(2026, 8, 30, 21, 0)


# ---------------------------------------------------------------------------
# a sub that spans two runs
# ---------------------------------------------------------------------------


def test_a_sub_across_two_runs_becomes_one_candidate_each(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "sub",
                    bosses=["HStar", "HFA", "HCarling", "XKalos"],
                    participants=[MY],
                    is_question=True,
                    confidence=0.8,
                )
            ]
        ),
        runs,
    )
    assert len(result.planned) == 2
    assert [e.amendment.bosses for e in result.planned] == [
        ["HStar", "HFA"],
        ["HCarling", "XKalos"],
    ]
    assert [e.run["id"] for e in result.planned] == [runs[0]["id"], runs[1]["id"]]
    assert all(e.payload == {"remove": [MY], "add": []} for e in result.planned)


def test_a_sub_about_one_run_stays_one_candidate(runs):
    result = plan(
        Extraction(
            amendments=[
                amendment("sub", bosses=["HStar", "HFA"], participants=[MY], confidence=0.8)
            ]
        ),
        runs,
    )
    assert len(result.planned) == 1 and result.planned[0].run["id"] == runs[0]["id"]


def test_a_sub_only_spans_the_runs_the_author_is_on(runs):
    # KANON is on the HCarling/XKalos run only, so asking for a temp across both
    # boss sets is still only about their own run.
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "sub",
                    bosses=["HStar", "HFA", "HCarling", "XKalos"],
                    participants=[KANON],
                    confidence=0.8,
                    evidence_message_ids=["7"],
                )
            ]
        ),
        runs,
        author_ids={"7": KANON},
    )
    assert len(result.planned) == 1
    assert result.planned[0].run["id"] == runs[1]["id"]


def test_a_cancel_naming_two_runs_worth_of_bosses_cancels_both(runs):
    """DESIGN.md §8: "mon and tuesday suddenly got things on" is about both.

    The model is asked for one amendment per run and usually gives it. When it
    does not, the four bosses used to be forced onto whichever run scored
    highest -- and half the sentence was thrown away without a word.
    """
    result = plan(
        Extraction(
            amendments=[
                amendment("cancel", bosses=["HStar", "HFA", "HCarling", "XKalos"], confidence=0.9)
            ]
        ),
        runs,
    )
    assert [e.run["id"] for e in result.planned] == [runs[0]["id"], runs[1]["id"]]
    # Each line names only its own run's bosses, not all four.
    assert [list(e.amendment.bosses) for e in result.planned] == [
        list(runs[0]["bosses"]),
        list(runs[1]["bosses"]),
    ]


def test_a_kind_with_no_boss_evidence_is_dropped_rather_than_guessed(runs):
    """Two runs match a bare "change to wed?" equally well; picking one moves a night."""
    # MY is on both runs, so the author's own name cannot pick one either.
    result = plan(
        Extraction(
            amendments=[
                amendment(
                    "move",
                    bosses=[],
                    day_ref="wed",
                    confidence=0.9,
                    evidence_message_ids=["7"],
                )
            ]
        ),
        runs,
        author_ids={"7": MY},
    )
    assert result.planned == []
    assert any("ambiguous" in e.match_reason for e in result.dropped)


def test_the_card_names_who_still_has_to_answer(runs):
    amendments = [
        {
            "kind": "move",
            "bosses": ["HStar", "HFA"],
            "run_id": runs[0]["id"],
            "participants": [MY],
            "is_question": True,
            "new_datetime": None,
        }
    ]
    waiting = Pipeline._unanswered(amendments, {runs[0]["id"]: runs[0]})
    assert waiting == [ALVIN, PRIYA]


def test_a_settled_proposal_names_nobody():
    amendments = [
        {"is_question": False, "new_datetime": kl(2026, 9, 2, 21, 30), "participants": []}
    ]
    assert Pipeline._unanswered(amendments, {}) == []


def test_ping_time_is_read_from_the_bot_not_hardcoded():
    assert PING_TIME.hour == 9  # guards the conftest values these tests rely on
