from __future__ import annotations

import contextlib
from datetime import datetime, time
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest

from bot.bosses import BossTable
from bot.db import Repo

TZ = ZoneInfo("Asia/Kuala_Lumpur")
RESET_WEEKDAY = 3  # Thursday
RESET_TIME = time(0, 0)
PING_TIME = time(9, 0)
COUNTDOWNS = [60, 15]

REPO_ROOT = Path(__file__).resolve().parent.parent


def kl(year: int, month: int, day: int, hour: int = 0, minute: int = 0) -> datetime:
    """An aware datetime in the guild timezone."""
    return datetime(year, month, day, hour, minute, tzinfo=TZ)


@pytest.fixture
def repo() -> Repo:
    r = Repo(":memory:")
    yield r
    r.close()


@pytest.fixture(scope="session")
def bosses(tmp_path_factory: pytest.TempPathFactory) -> BossTable:
    """The shipped boss table, read from a config directory that has no portraits.

    Portrait images are git-ignored, so whether a developer has dropped them
    into `config/portraits/` must not change what the suite asserts. Loading a
    copy of the yaml from a directory with no images makes "this boss has no
    portrait" true by construction; the portrait-present path is covered
    deterministically by `table_with_portraits` in test_portraits.py.
    """
    directory = tmp_path_factory.mktemp("config")
    path = directory / "bosses.yaml"
    path.write_bytes((REPO_ROOT / "config" / "bosses.yaml").read_bytes())
    return BossTable.load(path)


@pytest.fixture(autouse=True)
def model_lock(monkeypatch):
    """A fresh :data:`bot.modellock.MODEL_LOCK` per test, in every namespace.

    The host's one model is guarded by one process-global lock, which makes it
    the one piece of state the suite genuinely shares: a test that leaves it held
    -- a cancelled generation, an event loop closed mid-answer -- would leave
    every later chat test shedding its question instead of answering it, and the
    failure would land nowhere near the cause.

    Autouse rather than opt-in for that reason, and also because `asyncio.Lock`
    binds itself to the first event loop it ever has to *wait* on: contend for it
    in one test and the next test's loop is the wrong one. Every module that
    imported the name is patched, because two features holding two different
    locks would serialise nothing while looking exactly like this.

    Requested by name to get the lock itself, which is how a test holds the model
    against the code under test.

    The holder label is reset with it: it is the same shared state seen from the
    portal's end, and a stale "chat #777… has had it for 4000 seconds" would be
    reported by :func:`bot.modellock.holder` in every test that followed.
    """
    import asyncio

    from bot import modellock
    from bot.chat import agent
    from bot.extract import llm

    lock = asyncio.Lock()
    for module in (modellock, agent, llm):
        monkeypatch.setattr(module, "MODEL_LOCK", lock)
    monkeypatch.setattr(modellock, "_HOLDER", None)
    monkeypatch.setattr(modellock, "_SINCE", 0.0)
    return lock


# ---------------------------------------------------------------------------
# phase 3: the portal + API
# ---------------------------------------------------------------------------


@pytest.fixture
def fake_bot(repo: Repo, bosses: BossTable):
    """A stand-in client with an in-memory database; see `tests/fake_bot.py`."""
    from .fake_bot import FakeBot

    return FakeBot(repo, bosses)


@pytest.fixture
def app(fake_bot):
    from bot.api import create_app

    return create_app(fake_bot)


@pytest.fixture
def client(app, fake_bot):
    """An unauthenticated test client -- add the header or cookie per test.

    The rescan worker is started inside the client's event loop, so a queued
    rescan actually runs, exactly as it does in the bot.
    """
    import asyncio

    from fastapi.testclient import TestClient

    with TestClient(app) as test_client:
        test_client.portal.call(fake_bot.rescans.start)
        try:
            yield test_client
        finally:
            with contextlib.suppress(asyncio.CancelledError, RuntimeError):
                test_client.portal.call(fake_bot.rescans.stop)


@pytest.fixture
def auth(client):
    """A client that carries the bearer token on every request."""
    from .fake_bot import ADMIN_TOKEN

    client.headers["Authorization"] = f"Bearer {ADMIN_TOKEN}"
    return client


# ---------------------------------------------------------------------------
# phase 4: the speech pilot
# ---------------------------------------------------------------------------


@pytest.fixture
def chat_bot(repo: Repo, bosses: BossTable):
    """A stand-in client configured for the chatbot; see `tests/chat_support.py`."""
    from .chat_support import build_bot

    return build_bot(repo, bosses)


@pytest.fixture
def chat_seeded(chat_bot):
    """`chat_bot` with two parties and a materialised week; returns their ids."""
    from .chat_support import seed

    return seed(chat_bot)


@pytest.fixture
def seeded(fake_bot):
    """A guild with two parties, a week materialised, and one open proposal.

    Returned as a dict of ids so a test can say what it means rather than
    re-deriving them from list order.
    """
    from bot.materialise import materialise_week
    from bot.weeks import current_week_start

    from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

    repo = fake_bot.repo
    for user_id, name, nick in (
        (1001, "Alvin tan", None),
        (1002, "kanon [AZUR]", "kanon"),
        (1003, "Priya", None),
    ):
        repo.upsert_member(user_id, name, nick, True)
    repo.upsert_member(1009, "NotABosser", None, False)

    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    star = repo.add_fixed_run(
        1001, ["HStar", "HFA"], 0, "21:30", ["1001", "1002"], channel_id=WATCHED_CHANNEL
    )
    kalos = repo.add_fixed_run(
        1002, ["XKalos"], 1, "23:00", ["1002", "1003"], channel_id=OTHER_CHANNEL
    )
    materialise_week(repo, ws, TZ, PING_TIME, COUNTDOWNS, now=ws)
    runs = repo.list_runs(week_start=ws)
    star_run = next(r for r in runs if "HStar" in r["bosses"])
    kalos_run = next(r for r in runs if "XKalos" in r["bosses"])
    repo.set_rsvp(star_run["id"], 1001, "yes")

    repo.record_message(
        800000000000000001, WATCHED_CHANNEL, 1002, kl(2026, 8, 30, 11, 50), "can change to wed?"
    )
    amendment = repo.create_amendment(
        week_start=ws,
        kind="move",
        bosses=["HStar", "HFA"],
        run_id=star_run["id"],
        new_datetime=kl(2026, 9, 2, 21, 30),
        participants=["1002"],
        confidence=0.82,
        evidence_msg_ids=["800000000000000001"],
        channel_id=WATCHED_CHANNEL,
        day_ref="wed",
        summary="move to wednesday",
    )
    repo.set_amendment_proposal_message(amendment, 900000000000000001)
    extraction = repo.log_extraction(
        model="gpt-oss:20b",
        prompt="you are an extractor...",
        raw_response='{"amendments": []}',
        latency_ms=1234,
        message_ids=["800000000000000001"],
        amendment_ids=[amendment],
    )
    interaction = repo.log_chat_interaction(
        model="qwen3:32b",
        question="when is star this week?",
        reply="Star is Monday 21:30 — you and Alvin are on it.",
        outcome="answered",
        rounds=2,
        channel_id=WATCHED_CHANNEL,
        message_id=800000000000000002,
        author_id=1002,
        latency_ms=8400,
        model_ms=8100,
        tools_ms=120,
        prompt_tokens=3120,
        completion_tokens=64,
        tool_calls=[
            {
                "name": "get_schedule",
                "arguments": "week='this'",
                "ms": 120,
                "outcome": "ok",
                "created": [],
            }
        ],
    )
    return {
        "week_start": ws,
        "fixed_star": star,
        "fixed_kalos": kalos,
        "run_star": star_run["id"],
        "run_kalos": kalos_run["id"],
        "amendment": amendment,
        "extraction": extraction,
        "interaction": interaction,
    }
