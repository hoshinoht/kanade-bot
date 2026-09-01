"""The per-person sliding window.

Driven by a fake clock rather than by sleeping: the window is five minutes in
production, and a test that waits it out is a test nobody runs.
"""

from __future__ import annotations

from bot.chat.ratelimit import RateLimiter


class Clock:
    """A monotonic clock a test can wind forward."""

    def __init__(self, now: float = 1000.0):
        self.now = now

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


def test_it_allows_exactly_count_answers():
    limiter = RateLimiter(3, 300, Clock())
    assert [limiter.allow(1002) for _ in range(4)] == [True, True, True, False]


def test_the_window_slides_rather_than_resetting():
    """Two of three expire while the third is still live, so two more get through."""
    clock = Clock()
    limiter = RateLimiter(3, 300, clock)
    limiter.allow(1002)
    limiter.allow(1002)
    clock.advance(200)
    limiter.allow(1002)
    assert limiter.allow(1002) is False

    clock.advance(101)  # the first two are now 301 s old, the third only 101 s
    assert limiter.allow(1002) is True
    assert limiter.allow(1002) is True
    assert limiter.allow(1002) is False


def test_a_full_window_clears_completely():
    clock = Clock()
    limiter = RateLimiter(2, 300, clock)
    limiter.allow(1002)
    limiter.allow(1002)
    assert limiter.allow(1002) is False
    clock.advance(301)
    assert limiter.allow(1002) is True


def test_windows_are_per_person():
    limiter = RateLimiter(1, 300, Clock())
    assert limiter.allow(1001) is True
    assert limiter.allow(1002) is True
    assert limiter.allow(1001) is False


def test_ids_compare_as_strings():
    """An int from Discord and a str from the database are the same person."""
    limiter = RateLimiter(1, 300, Clock())
    assert limiter.allow(1002) is True
    assert limiter.allow("1002") is False


def test_an_exempt_caller_is_never_limited_and_never_recorded():
    limiter = RateLimiter(1, 300, Clock())
    for _ in range(10):
        assert limiter.allow(1002, exempt=True) is True
    # Their exempt calls did not fill up the window they would otherwise have.
    assert limiter.remaining(1002) == 1
    assert limiter.allow(1002) is True


def test_remaining_counts_down_and_recovers():
    clock = Clock()
    limiter = RateLimiter(2, 300, clock)
    assert limiter.remaining(1002) == 2
    limiter.allow(1002)
    assert limiter.remaining(1002) == 1
    limiter.allow(1002)
    assert limiter.remaining(1002) == 0
    clock.advance(301)
    assert limiter.remaining(1002) == 2


def test_retry_after_is_zero_while_there_is_room():
    """Including for somebody the limiter has never heard of."""
    limiter = RateLimiter(2, 300, Clock())
    assert limiter.retry_after(1002) == 0.0
    limiter.allow(1002)
    assert limiter.retry_after(1002) == 0.0


def test_retry_after_counts_from_the_oldest_live_hit():
    """The oldest is the one that expires first, so it is the one to wait for."""
    clock = Clock()
    limiter = RateLimiter(2, 300, clock)
    limiter.allow(1002)
    clock.advance(100)
    limiter.allow(1002)

    # Full: the first hit is 100 s old, so its slot frees in 200 s.
    assert limiter.retry_after(1002) == 200
    clock.advance(150)
    assert limiter.retry_after(1002) == 50


def test_retry_after_goes_back_to_zero_once_the_window_rolls():
    clock = Clock()
    limiter = RateLimiter(1, 300, clock)
    limiter.allow(1002)
    assert limiter.retry_after(1002) == 300
    clock.advance(301)
    assert limiter.retry_after(1002) == 0.0


def test_asking_when_to_come_back_does_not_cost_an_answer():
    limiter = RateLimiter(1, 300, Clock())
    for _ in range(5):
        limiter.retry_after(1002)
    assert limiter.remaining(1002) == 1


def test_reset_forgets_one_person_or_everybody():
    limiter = RateLimiter(1, 300, Clock())
    limiter.allow(1001)
    limiter.allow(1002)
    limiter.reset(1001)
    assert limiter.allow(1001) is True
    assert limiter.allow(1002) is False
    limiter.reset()
    assert limiter.allow(1002) is True


def test_settings_drive_the_defaults(chat_bot):
    from bot.chat.agent import ChatPilot

    pilot = ChatPilot(chat_bot, client=object())
    assert pilot.limiter.count == chat_bot.settings.chat_pilot_rate_count
    assert pilot.limiter.window == chat_bot.settings.chat_pilot_rate_window_s
    assert pilot.global_limiter.count == chat_bot.settings.chat_pilot_global_rate_count
    assert pilot.global_limiter.window == chat_bot.settings.chat_pilot_global_rate_window_s
