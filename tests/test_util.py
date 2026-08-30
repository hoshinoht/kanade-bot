"""Mention parsing and the "who may change this run" rules."""

from __future__ import annotations

from bot.util import can_modify_fixed, can_modify_run, mention, parse_mentions

RUN = {"id": 1, "participants": ["10", "20"], "status": "planned"}
FIXED = {"id": 1, "owner_id": "10", "participants": ["10", "20"]}


def test_parse_mentions_handles_both_mention_forms():
    assert parse_mentions("<@10> <@!20>") == ["10", "20"]


def test_parse_mentions_accepts_bare_snowflakes():
    assert parse_mentions("123456789012345678") == ["123456789012345678"]


def test_parse_mentions_dedupes_and_keeps_order():
    assert parse_mentions("<@20> <@10> <@20>") == ["20", "10"]


def test_parse_mentions_ignores_prose_and_short_numbers():
    assert parse_mentions("me and 3 friends") == []
    assert parse_mentions(None) == []
    assert parse_mentions("") == []


def test_mention_format():
    assert mention(10) == "<@10>"


def test_participants_may_change_their_run():
    assert can_modify_run(RUN, "10")
    assert can_modify_run(RUN, 20)


def test_outsiders_may_not():
    assert not can_modify_run(RUN, "99")


def test_the_owner_and_admins_may():
    assert can_modify_run(RUN, "99", is_admin=True)
    assert can_modify_run({"participants": []}, "5", owner_id="5")


def test_fixed_run_permissions():
    assert can_modify_fixed(FIXED, "10")  # owner
    assert can_modify_fixed(FIXED, "20")  # participant
    assert not can_modify_fixed(FIXED, "99")
    assert can_modify_fixed(FIXED, "99", is_admin=True)
