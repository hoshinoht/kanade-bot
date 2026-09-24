"""Deterministic, bounded v1 ZIP envelope codec with no filesystem extraction."""

import hashlib
import io
import json
import stat
import zipfile
from dataclasses import dataclass
from typing import Annotated, Literal

from pydantic import Field

from .common import StrictModel
from .composition import Bundle, Manifest
from .errors import BundleError

SECTION_NAMES = (
    "schedule.json",
    "config.json",
    "history.json",
    "catalog.json",
    "delivery.json",
    "rate_overrides.json",
)


@dataclass(frozen=True)
class ArchiveLimits:
    max_input_bytes: int = 8 * 1024 * 1024
    max_members: int = 7
    max_member_bytes: int = 8 * 1024 * 1024
    max_total_bytes: int = 24 * 1024 * 1024


DEFAULT_LIMITS = ArchiveLimits()


class SectionMetadata(StrictModel):
    sha256: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
    size: int = Field(ge=0)


class ArchiveManifest(StrictModel):
    archive_version: Literal[1]
    bundle_manifest: Manifest
    sections: dict[Literal[*SECTION_NAMES], SectionMetadata]


def _canonical(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode(
        "utf-8"
    )


def _loads(data: bytes) -> object:
    try:
        return json.loads(
            data.decode("utf-8", "strict"),
            parse_constant=lambda _: (_ for _ in ()).throw(ValueError()),
            object_pairs_hook=_unique_object,
        )
    except (UnicodeDecodeError, ValueError, json.JSONDecodeError) as error:
        raise BundleError("invalid-json") from error


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate key")
        value[key] = item
    return value


def encode(bundle: Bundle) -> bytes:
    """Encode a stable ZIP_STORED archive; envelope metadata never checks itself."""
    payloads = {
        "schedule.json": _canonical(bundle.schedule.model_dump(mode="json")),
        "config.json": _canonical(bundle.config.model_dump(mode="json")),
        "history.json": _canonical(bundle.history.model_dump(mode="json")),
        "catalog.json": _canonical(bundle.catalog.model_dump(mode="json")),
        "delivery.json": _canonical(bundle.delivery.model_dump(mode="json")),
        "rate_overrides.json": _canonical(
            [row.model_dump(mode="json") for row in bundle.rate_overrides]
        ),
    }
    envelope = ArchiveManifest(
        archive_version=1,
        bundle_manifest=bundle.manifest,
        sections={
            name: SectionMetadata(sha256=hashlib.sha256(data).hexdigest(), size=len(data))
            for name, data in payloads.items()
        },
    )
    payloads = {"manifest.json": _canonical(envelope.model_dump(mode="json"))} | payloads
    output = io.BytesIO()
    with zipfile.ZipFile(
        output, "w", compression=zipfile.ZIP_STORED, strict_timestamps=True
    ) as archive:
        for name, data in payloads.items():
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED
            info.external_attr = (stat.S_IFREG | 0o600) << 16
            archive.writestr(info, data)
    return output.getvalue()


def decode(data: bytes, *, limits: ArchiveLimits = DEFAULT_LIMITS) -> Bundle:
    """Validate a bounded archive in memory without extracting any member."""
    if len(data) > limits.max_input_bytes:
        raise BundleError("archive-too-large")
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
    except (OSError, zipfile.BadZipFile) as error:
        raise BundleError("invalid-archive") from error
    with archive:
        infos = archive.infolist()
        if len(infos) > limits.max_members:
            raise BundleError("too-many-members")
        names: set[str] = set()
        total = 0
        for info in infos:
            _check_member(info, names, limits)
            total += info.file_size
            if total > limits.max_total_bytes:
                raise BundleError("archive-expanded-too-large")
        expected = {"manifest.json", *SECTION_NAMES}
        if names != expected:
            raise BundleError("unexpected-members")
        payloads = {name: _read_member(archive, name, limits.max_member_bytes) for name in names}
    envelope = _validate_envelope(payloads.pop("manifest.json"))
    _validate_payloads(payloads, envelope)
    try:
        return Bundle.model_validate_json(
            _canonical(
                {
                    "manifest": envelope.bundle_manifest.model_dump(mode="json"),
                    "schedule": _loads(payloads["schedule.json"]),
                    "config": _loads(payloads["config.json"]),
                    "history": _loads(payloads["history.json"]),
                    "catalog": _loads(payloads["catalog.json"]),
                    "delivery": _loads(payloads["delivery.json"]),
                    "rate_overrides": _loads(payloads["rate_overrides.json"]),
                }
            )
        )
    except BundleError:
        raise
    except Exception as error:
        # Pydantic details must not become operator output.
        raise BundleError("invalid-bundle") from error


def _check_member(info: zipfile.ZipInfo, names: set[str], limits: ArchiveLimits) -> None:
    name = info.filename
    if (
        not name
        or name in names
        or "\\" in name
        or name.startswith("/")
        or any(part in {"", ".", ".."} for part in name.split("/"))
    ):
        raise BundleError("unsafe-member")
    mode = info.external_attr >> 16
    if info.is_dir() or stat.S_ISLNK(mode):
        raise BundleError("unsupported-member")
    if info.flag_bits & 0x1 or info.compress_type != zipfile.ZIP_STORED:
        raise BundleError("unsupported-member")
    if info.file_size > limits.max_member_bytes or info.compress_size > limits.max_input_bytes:
        raise BundleError("member-too-large")
    if info.compress_size != info.file_size:
        raise BundleError("mismatched-member-size")
    names.add(name)


def _read_member(archive: zipfile.ZipFile, name: str, maximum: int) -> bytes:
    try:
        with archive.open(name) as stream:
            chunks: list[bytes] = []
            total = 0
            while chunk := stream.read(min(64 * 1024, maximum - total + 1)):
                total += len(chunk)
                if total > maximum:
                    raise BundleError("member-too-large")
                chunks.append(chunk)
            return b"".join(chunks)
    except BundleError:
        raise
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        raise BundleError("invalid-member") from error


def _validate_envelope(data: bytes) -> ArchiveManifest:
    try:
        return ArchiveManifest.model_validate_json(_canonical(_loads(data)))
    except BundleError:
        raise
    except Exception as error:
        raise BundleError("invalid-manifest") from error


def _validate_payloads(payloads: dict[str, bytes], envelope: ArchiveManifest) -> None:
    if set(envelope.sections) != set(SECTION_NAMES):
        raise BundleError("invalid-manifest")
    for name, metadata in envelope.sections.items():
        data = payloads[name]
        if metadata.size != len(data) or metadata.sha256 != hashlib.sha256(data).hexdigest():
            raise BundleError("section-checksum-mismatch")
