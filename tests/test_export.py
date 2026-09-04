"""`python -m bot.export` -- the pure parts.

No Discord connection and no real export: these cover argument parsing, the
default output path, and turning a message object into a JSONL record.
"""

from __future__ import annotations

import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace

import discord
import pytest

from bot.export import (
    AccessDenied,
    _export_channel,
    attachment_label,
    build_parser,
    default_out_path,
    message_record,
    parse_when,
    slugify,
)
from bot.infrastructure.watch import origin_ids

from .conftest import TZ, kl

# -- since / until parsing ---------------------------------------------------


def test_a_bare_date_means_midnight_in_the_guild_timezone():
    assert parse_when("2026-06-01", TZ) == kl(2026, 6, 1)


def test_an_iso_timestamp_without_a_zone_is_read_as_guild_time():
    assert parse_when("2026-06-01T21:30", TZ) == kl(2026, 6, 1, 21, 30)


def test_an_explicit_offset_is_respected():
    assert parse_when("2026-06-01T13:30:00+00:00", TZ) == kl(2026, 6, 1, 21, 30)


def test_the_result_is_always_utc():
    assert parse_when("2026-06-01", TZ).tzinfo is UTC


@pytest.mark.parametrize("text", ["yesterday", "01/06/2026", "", "nonsense"])
def test_unparseable_dates_are_rejected(text):
    with pytest.raises(ValueError, match="YYYY-MM-DD"):
        parse_when(text, TZ)


# -- output paths ------------------------------------------------------------


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("hstar-alvin-kanon", "hstar-alvin-kanon"),
        ("Boss Planning", "boss-planning"),
        ("🎮 gaming chat", "gaming-chat"),
        ("", "channel"),
    ],
)
def test_slugify(name, expected):
    assert slugify(name) == expected


def test_default_out_path_is_one_file_per_channel_dated_in_guild_time():
    path = default_out_path("hstar-alvin-kanon", kl(2026, 6, 1), TZ)
    assert path == Path("data/exports/hstar-alvin-kanon-2026-06-01.jsonl")


def test_the_stamp_uses_guild_local_date_not_utc():
    # 2026-06-01 00:30 +08 is still 2026-05-31 in UTC; the filename must say June.
    assert default_out_path("c", kl(2026, 6, 1, 0, 30), TZ).name == "c-2026-06-01.jsonl"


def test_out_dir_is_overridable(tmp_path):
    path = default_out_path("c", kl(2026, 6, 1), TZ, out_dir=tmp_path)
    assert path.parent == tmp_path


# -- attachments -------------------------------------------------------------


@pytest.mark.parametrize(
    ("content_type", "filename", "expected"),
    [
        ("image/png", "shot.png", "[image] shot.png"),
        ("image/jpeg", "ring.jpg", "[image] ring.jpg"),
        ("text/plain", "log.txt", "[file] log.txt"),
        ("application/pdf", "a.pdf", "[file] a.pdf"),
        (None, "unknown.bin", "[file] unknown.bin"),
    ],
)
def test_attachment_label_never_implies_a_download(content_type, filename, expected):
    assert attachment_label(content_type, filename) == expected


# -- records -----------------------------------------------------------------


def user(uid, name, bot=False):
    return SimpleNamespace(id=uid, display_name=name, bot=bot)


def make_message(**over):
    base = dict(
        id=555,
        channel=SimpleNamespace(id=900, category_id=200, parent=None),
        author=user(1, "kanon"),
        created_at=datetime(2026, 8, 29, 3, 54, tzinfo=UTC),
        content="we doing our nstar and ncarl tonight?",
        mentions=[user(2, "Alvin"), user(3, "Priya")],
        reference=None,
        attachments=[],
        reactions=[],
        edited_at=None,
    )
    base.update(over)
    return SimpleNamespace(**base)


def test_a_plain_message_round_trips_to_json():
    record = message_record(make_message(), "hstar-alvin-kanon")
    assert json.loads(json.dumps(record)) == record
    assert record["id"] == "555"
    assert record["channel_id"] == "900"
    assert record["channel_name"] == "hstar-alvin-kanon"
    assert record["thread_id"] is None
    assert record["author_id"] == "1"
    assert record["author_name"] == "kanon"
    assert record["author_bot"] is False
    assert record["created_at"] == "2026-08-29T03:54:00+00:00"
    assert record["content"] == "we doing our nstar and ncarl tonight?"
    assert record["mentions"] == ["2", "3"]
    assert record["reply_to"] is None
    assert record["reactions"] == {}
    assert record["attachments"] == []
    assert "edited_at" not in record


def test_ids_are_strings_so_snowflakes_survive_json():
    record = message_record(make_message(id=100000000000000010), "c")
    assert record["id"] == "100000000000000010"


def test_a_reply_records_the_message_it_answers():
    message = make_message(reference=SimpleNamespace(message_id=444))
    assert message_record(message, "c")["reply_to"] == "444"


def test_an_edit_is_recorded():
    edited = datetime(2026, 8, 29, 4, 0, tzinfo=UTC)
    record = message_record(make_message(edited_at=edited), "c")
    assert record["edited_at"] == "2026-08-29T04:00:00+00:00"


def test_reactions_are_taken_from_the_caller():
    record = message_record(make_message(), "c", {"✅": [2, 3], "❌": [4]})
    assert record["reactions"] == {"✅": [2, 3], "❌": [4]}


def test_a_bot_author_is_flagged_rather_than_dropped():
    # The export keeps them so a fixture shows what the bot itself said.
    assert (
        message_record(make_message(author=user(9, "YuukiSakuna", bot=True)), "c")["author_bot"]
        is True
    )


def test_attachments_are_listed_by_kind():
    message = make_message(
        attachments=[
            SimpleNamespace(content_type="image/png", filename="shot.png"),
            SimpleNamespace(content_type="application/zip", filename="logs.zip"),
        ]
    )
    assert message_record(message, "c")["attachments"] == ["[image] shot.png", "[file] logs.zip"]


def test_empty_content_is_an_empty_string_not_none():
    assert message_record(make_message(content=None), "c")["content"] == ""


# -- threads -----------------------------------------------------------------


def test_a_thread_message_is_filed_under_its_parent_channel():
    thread = SimpleNamespace(id=777, category_id=None, parent=SimpleNamespace(id=900))
    record = message_record(make_message(channel=thread), "hstar-alvin-kanon")
    assert record["channel_id"] == "900"
    assert record["thread_id"] == "777"


def test_origin_ids_for_a_plain_channel():
    assert origin_ids(SimpleNamespace(id=900, parent=None)) == (900, None)


def test_origin_ids_for_a_thread():
    assert origin_ids(SimpleNamespace(id=777, parent=SimpleNamespace(id=900))) == (900, 777)


# -- CLI ---------------------------------------------------------------------


def test_channel_and_category_are_repeatable():
    args = build_parser().parse_args(["--channel", "1", "--channel", "2", "--category", "3"])
    assert args.channel == [1, 2]
    assert args.category == [3]


def test_no_targets_means_every_watched_channel():
    args = build_parser().parse_args([])
    assert args.channel == [] and args.category == []
    assert args.since is None and args.until is None and args.out is None


def test_the_documented_invocation_parses():
    args = build_parser().parse_args(["--category", "100000000000000010", "--since", "2026-06-01"])
    assert args.category == [100000000000000010]
    assert parse_when(args.since, TZ) == kl(2026, 6, 1)


def test_a_non_numeric_id_is_rejected():
    with pytest.raises(SystemExit):
        build_parser().parse_args(["--channel", "general"])


# -- resilience --------------------------------------------------------------


class _FakeHistory:
    """Stands in for `channel.history(...)`, optionally refusing access."""

    def __init__(self, messages, forbidden=False):
        self._messages = messages
        self._forbidden = forbidden

    def __call__(self, **_kwargs):
        return self

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._forbidden:
            raise discord.Forbidden(SimpleNamespace(status=403, reason="Forbidden"), "no access")
        if not self._messages:
            raise StopAsyncIteration
        return self._messages.pop(0)


def fake_channel(name, messages, forbidden=False):
    channel = SimpleNamespace(id=900, name=name, parent=None, threads=[])
    channel.history = _FakeHistory(list(messages), forbidden)
    channel.archived_threads = lambda **_k: _FakeHistory([])
    return channel


def test_an_unreadable_channel_raises_access_denied(tmp_path, repo):
    channel = fake_channel("private", [], forbidden=True)
    path = tmp_path / "out.jsonl"
    with pytest.raises(AccessDenied):
        asyncio.run(_export_channel(channel, kl(2026, 6, 1), None, repo, path))


def test_a_readable_channel_writes_one_json_object_per_line(tmp_path, repo):
    messages = [make_message(id=1), make_message(id=2)]
    channel = fake_channel("hstar-alvin-kanon", messages)
    path = tmp_path / "out.jsonl"

    count = asyncio.run(_export_channel(channel, kl(2026, 6, 1), None, repo, path))

    assert count == 2
    lines = path.read_text(encoding="utf-8").strip().split("\n")
    assert [json.loads(line)["id"] for line in lines] == ["1", "2"]


def test_exported_messages_are_upserted_for_a_later_rescan(tmp_path, repo):
    channel = fake_channel("c", [make_message(id=1)])
    asyncio.run(_export_channel(channel, kl(2026, 6, 1), None, repo, tmp_path / "o.jsonl"))
    stored = repo._conn.execute("SELECT id, channel_id FROM messages").fetchall()
    assert [tuple(r) for r in stored] == [("1", "900")]
