"""Settings parsing, including hand-written `.env` files with inline comments."""

from __future__ import annotations

from datetime import time

import pytest
from pydantic import ValidationError

from bot.config import Settings

REQUIRED = {
    "discord_token": "token",
    "guild_id": 1,
    "bossing_role_id": 3,
    "chat_channel_ids": "10",
}


def make(**overrides) -> Settings:
    # _env_file=None keeps the developer's real .env out of the tests.
    return Settings(_env_file=None, **{**REQUIRED, **overrides})


def test_defaults_match_the_design():
    settings = make()
    assert settings.tz == "Asia/Kuala_Lumpur"
    assert settings.reset_weekday == 3  # Thursday
    assert settings.reset_time == time(0, 0)
    assert settings.countdown_minute_list == [60, 15]
    assert settings.admin_role_id is None
    # Runs post in their own home channel, so the guild-wide channel is optional.
    assert settings.post_channel_id is None


def test_comma_lists_are_parsed_not_json_decoded():
    settings = make(chat_channel_ids="10, 20,30", countdown_minutes="15;60;15")
    assert settings.chat_channel_id_list == [10, 20, 30]
    assert settings.countdown_minute_list == [60, 15]  # sorted, de-duplicated


def test_inline_comments_are_stripped():
    settings = make(guild_id="123   # right-click server -> Copy Server ID")
    assert settings.guild_id == 123


def test_a_blank_value_with_only_a_comment_falls_back_to_the_default():
    # `ADMIN_ROLE_ID=            # optional: role allowed to amend any run`
    settings = make(admin_role_id="            # optional: role allowed to amend any run")
    assert settings.admin_role_id is None


def test_a_blank_optional_value_falls_back_to_the_default():
    assert make(countdown_minutes="").countdown_minute_list == [60, 15]


def test_secrets_are_never_comment_stripped():
    # A token is whatever the user pasted, even if it somehow contains a hash.
    assert make(discord_token="abc#def").discord_token == "abc#def"


def test_a_missing_required_value_says_so_plainly():
    with pytest.raises(ValidationError, match="bossing_role_id"):
        Settings(_env_file=None, discord_token="t", guild_id=1, chat_channel_ids="10")


def test_at_least_one_watched_channel_is_required():
    with pytest.raises(ValidationError, match="CHAT_CHANNEL_IDS"):
        make(chat_channel_ids="", chat_category_ids="")


def test_either_watched_channels_or_categories_will_do():
    assert make(chat_channel_ids="", chat_category_ids="7").chat_category_id_list == [7]
    assert make(chat_channel_ids="7", chat_category_ids="").chat_channel_id_list == [7]


def test_watched_channels_and_categories_are_both_parsed():
    settings = make(chat_channel_ids="1,2", chat_category_ids="3")
    assert settings.chat_channel_id_list == [1, 2]
    assert settings.chat_category_id_list == [3]


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("tz", "Mars/Olympus"),
        ("boss_week_reset_weekday", "caturday"),
        ("boss_week_reset_time", "9pm"),
        ("day_of_ping_time", "25:00"),
        ("countdown_minutes", "-5"),
    ],
)
def test_bad_values_are_rejected_at_startup(field, value):
    with pytest.raises(ValidationError):
        make(**{field: value})


def test_zoneinfo_and_derived_helpers():
    settings = make(tz="UTC", boss_week_reset_weekday="mon", boss_week_reset_time="12:30")
    assert settings.zoneinfo.key == "UTC"
    assert settings.reset_weekday == 0
    assert settings.reset_time == time(12, 30)
