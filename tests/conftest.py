from __future__ import annotations

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
def bosses() -> BossTable:
    return BossTable.load(REPO_ROOT / "config" / "bosses.yaml")
