"""Domain-separated hashes for minimal, content-free adoption evidence."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping

EVIDENCE_HASH_VERSION = 1
_DOMAIN = b"kanade-bot:adoption-source-evidence:v1\0"


def source_evidence_hash(evidence: Mapping[str, object]) -> str:
    """Hash canonical source identity and binding facts, never reconstructed content."""
    canonical = json.dumps(
        evidence,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(_DOMAIN + canonical).hexdigest()
