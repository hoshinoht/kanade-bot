"""Versioned, content-free hashes for requested and observed messages."""

from __future__ import annotations

import hashlib
import json

from .model import Destination, ObservedMessage, SendPlan, thaw_json

FINGERPRINT_VERSION = 1


def _sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _json_hash(value: object) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return _sha256(encoded)


def _destination_value(destination: Destination) -> dict[str, str | None]:
    return {
        "kind": destination.kind.value,
        "guild_id": destination.guild_id,
        "channel_id": destination.channel_id,
        "recipient_id": destination.recipient_id,
    }


def request_fingerprint(plan: SendPlan) -> str:
    """Hash the exact v1 request, including ordered embeds and file bytes."""
    payload = plan.payload
    return _json_hash(
        {
            "fingerprint_version": FINGERPRINT_VERSION,
            "destination": _destination_value(plan.destination),
            "content": payload.content,
            "embeds": [thaw_json(embed) for embed in payload.embeds],
            "files": [
                {
                    "name_sha256": _sha256(item.name.encode("utf-8")),
                    "length": len(item.content),
                    "content_sha256": _sha256(item.content),
                }
                for item in payload.files
            ],
            "allowed_user_ids": list(payload.allowed_user_ids),
            "allowed_role_ids": list(payload.allowed_role_ids),
            "allow_everyone": payload.allow_everyone,
            "replied_user": payload.replied_user,
            "reference": payload.reference,
        }
    )


def observable_fingerprint(destination: Destination, observed: ObservedMessage) -> str:
    """Hash only fields recoverable from the accepted remote message."""
    return _json_hash(
        {
            "fingerprint_version": FINGERPRINT_VERSION,
            "destination": _destination_value(destination),
            "content": observed.content,
            "embeds": [thaw_json(embed) for embed in observed.embeds],
            "user_mentions": list(observed.user_mentions),
            "role_mentions": list(observed.role_mentions),
            "everyone_mentioned": observed.everyone_mentioned,
            "replied_user": observed.replied_user,
            "reference": observed.reference,
            "attachments": [
                {
                    "name_sha256": _sha256(item.name.encode("utf-8")),
                    "size": item.size,
                    "url_sha256": _sha256(item.url.encode("utf-8")) if item.url else None,
                    "content_sha256": item.content_sha256,
                }
                for item in observed.attachments
            ],
        }
    )
