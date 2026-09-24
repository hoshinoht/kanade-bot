"""Deterministic request and observable delivery fingerprints."""

from __future__ import annotations

import re

import pytest

from bot.infrastructure.maintenance.delivery.fingerprints import (
    FINGERPRINT_VERSION,
    observable_fingerprint,
    request_fingerprint,
)
from bot.infrastructure.maintenance.delivery.model import (
    DedupePolicy,
    DeliveryTarget,
    Destination,
    ObservedAttachment,
    ObservedMessage,
    SendFile,
    SendPayload,
    SendPlan,
)


def _plan(
    *,
    destination: Destination | None = None,
    content: str | None = "hello",
    embeds: tuple[dict, ...] = (),
    files: tuple[SendFile, ...] = (),
    users: tuple[str, ...] = (),
    roles: tuple[str, ...] = (),
    everyone: bool = False,
    replied_user: bool = False,
    reference: str | None = None,
) -> SendPlan:
    return SendPlan(
        "chat_reply",
        destination or Destination.channel("1", "2"),
        SendPayload(
            content,
            embeds,
            files,
            users,
            roles,
            everyone,
            replied_user,
            reference,
        ),
        DedupePolicy.operation(),
    )


def test_request_fingerprint_is_v1_and_canonicalizes_allowed_ids():
    left = _plan(users=("22", "11", "22"), roles=("44", "33"))
    right = _plan(users=("11", "22"), roles=("33", "44"))

    fingerprint = request_fingerprint(left)

    assert FINGERPRINT_VERSION == 1
    assert re.fullmatch(r"[0-9a-f]{64}", fingerprint)
    assert request_fingerprint(right) == fingerprint
    assert left.payload.allowed_user_ids == ("11", "22")
    assert left.payload.allowed_role_ids == ("33", "44")


def test_v1_request_and_observable_fingerprints_have_stable_vectors():
    plan = _plan(content="hello")
    destination = Destination.channel("1", "2")

    assert (
        request_fingerprint(plan)
        == "6d1e887b1ec563a3d48ddb9039ed60d4a52d2b100e047549c68391b2d91006a1"
    )
    assert (
        observable_fingerprint(destination, ObservedMessage(content="hello"))
        == "ddc60afbdeccc7e4fcc3f584a42265f225ddb6c143a43f1d855e8b98addb147e"
    )


def test_request_fingerprint_preserves_ordered_embeds_fields_and_files():
    first = _plan(
        embeds=(
            {"title": "one", "fields": [{"name": "a"}, {"name": "b"}]},
            {"title": "two"},
        ),
        files=(SendFile("first.txt", b"first"), SendFile("second.txt", b"second")),
    )
    reversed_embeds = _plan(
        embeds=(
            {"title": "two"},
            {"title": "one", "fields": [{"name": "a"}, {"name": "b"}]},
        ),
        files=(SendFile("first.txt", b"first"), SendFile("second.txt", b"second")),
    )
    reversed_fields = _plan(
        embeds=({"title": "one", "fields": [{"name": "b"}, {"name": "a"}]}, {"title": "two"}),
        files=(SendFile("first.txt", b"first"), SendFile("second.txt", b"second")),
    )
    reversed_files = _plan(
        embeds=(
            {"title": "one", "fields": [{"name": "a"}, {"name": "b"}]},
            {"title": "two"},
        ),
        files=(SendFile("second.txt", b"second"), SendFile("first.txt", b"first")),
    )

    expected = request_fingerprint(first)
    assert request_fingerprint(reversed_embeds) != expected
    assert request_fingerprint(reversed_fields) != expected
    assert request_fingerprint(reversed_files) != expected
    assert request_fingerprint(
        _plan(files=(SendFile("first.txt", b"other"),))
    ) != request_fingerprint(_plan(files=(SendFile("first.txt", b"first"),)))
    assert request_fingerprint(
        _plan(files=(SendFile("other.txt", b"first"),))
    ) != request_fingerprint(_plan(files=(SendFile("first.txt", b"first"),)))


def test_request_fingerprint_covers_destination_content_mentions_and_reference():
    base = _plan(users=("11",), roles=("22",), reference="33")
    fingerprint = request_fingerprint(base)

    assert (
        request_fingerprint(
            _plan(
                destination=Destination.channel("1", "3"),
                users=("11",),
                roles=("22",),
                reference="33",
            )
        )
        != fingerprint
    )
    assert (
        request_fingerprint(
            _plan(content="different", users=("11",), roles=("22",), reference="33")
        )
        != fingerprint
    )
    assert request_fingerprint(_plan(users=("12",), roles=("22",), reference="33")) != fingerprint
    assert request_fingerprint(_plan(users=("11",), roles=("23",), reference="33")) != fingerprint
    assert (
        request_fingerprint(_plan(users=("11",), roles=("22",), everyone=True, reference="33"))
        != fingerprint
    )
    assert (
        request_fingerprint(_plan(users=("11",), roles=("22",), replied_user=True, reference="33"))
        != fingerprint
    )
    assert request_fingerprint(_plan(users=("11",), roles=("22",), reference="34")) != fingerprint


def test_payload_embeds_are_deeply_immutable():
    field = {"name": "first"}
    embed = {"fields": [field]}
    plan = _plan(embeds=(embed,))
    expected = request_fingerprint(plan)
    field["name"] = "changed"
    embed["title"] = "changed"

    assert request_fingerprint(plan) == expected
    with pytest.raises(TypeError):
        plan.payload.embeds[0]["title"] = "mutation"  # type: ignore[index]
    with pytest.raises(TypeError):
        plan.payload.embeds[0]["fields"][0]["name"] = "mutation"  # type: ignore[index]


def test_observable_fingerprint_uses_actual_mentions_attachments_and_ordered_content():
    destination = Destination.channel("1", "2")
    original = ObservedMessage(
        "posted",
        ({"title": "card", "fields": [{"name": "a"}, {"name": "b"}]},),
        user_mentions=("22", "11"),
        role_mentions=("44",),
        everyone_mentioned=False,
        replied_user=True,
        reference="33",
        attachments=(ObservedAttachment("image.png", 12, "https://cdn.invalid/a"),),
    )
    canonical_mentions = ObservedMessage(
        "posted",
        ({"title": "card", "fields": [{"name": "a"}, {"name": "b"}]},),
        user_mentions=("11", "22"),
        role_mentions=("44",),
        everyone_mentioned=False,
        replied_user=True,
        reference="33",
        attachments=(ObservedAttachment("image.png", 12, "https://cdn.invalid/a"),),
    )
    baseline = observable_fingerprint(destination, original)

    assert re.fullmatch(r"[0-9a-f]{64}", baseline)
    assert observable_fingerprint(destination, canonical_mentions) == baseline
    assert (
        observable_fingerprint(
            destination,
            ObservedMessage(
                original.content,
                original.embeds,
                user_mentions=("11", "23"),
                role_mentions=original.role_mentions,
                everyone_mentioned=False,
                replied_user=True,
                reference="33",
                attachments=original.attachments,
            ),
        )
        != baseline
    )
    assert (
        observable_fingerprint(
            destination,
            ObservedMessage(
                original.content,
                original.embeds,
                original.user_mentions,
                original.role_mentions,
                everyone_mentioned=False,
                replied_user=True,
                reference="34",
                attachments=original.attachments,
            ),
        )
        != baseline
    )
    assert (
        observable_fingerprint(
            destination,
            ObservedMessage(
                "different", original.embeds, original.user_mentions, original.role_mentions
            ),
        )
        != baseline
    )
    assert (
        observable_fingerprint(
            destination,
            ObservedMessage(
                original.content,
                original.embeds,
                original.user_mentions,
                original.role_mentions,
                everyone_mentioned=True,
                replied_user=True,
                reference="33",
                attachments=original.attachments,
            ),
        )
        != baseline
    )
    assert (
        observable_fingerprint(
            destination,
            ObservedMessage(
                original.content,
                original.embeds,
                original.user_mentions,
                original.role_mentions,
                everyone_mentioned=False,
                replied_user=True,
                reference="33",
                attachments=(ObservedAttachment("image.png", 13, "https://cdn.invalid/a"),),
            ),
        )
        != baseline
    )


def test_native_dedupe_value_uses_sorted_typed_targets():
    left = SendPlan(
        "reminder",
        Destination.channel("1", "2"),
        SendPayload("same"),
        DedupePolicy.native(),
        (DeliveryTarget.reminder("reminder-b"), DeliveryTarget.reminder("reminder-a")),
    )
    right = SendPlan(
        "reminder",
        Destination.channel("1", "2"),
        SendPayload("same"),
        DedupePolicy.native(),
        (DeliveryTarget.reminder("reminder-a"), DeliveryTarget.reminder("reminder-b")),
    )

    assert left.targets != right.targets
    assert sorted(target.claim_key for target in left.targets) == sorted(
        target.claim_key for target in right.targets
    )
