import hashlib
import io
import json
import stat
import zipfile
from pathlib import Path

import pytest

from bot.infrastructure.bundle import ArchiveLimits, Bundle, decode, encode
from bot.infrastructure.bundle.errors import BundleError

FIXTURE = Path(__file__).parent / "fixtures" / "bundle" / "example.json"


def bundle() -> Bundle:
    return Bundle.model_validate_json(FIXTURE.read_bytes())


def rewrite(
    data: bytes, replace: dict[str, bytes] | None = None, extra: zipfile.ZipInfo | None = None
) -> bytes:
    replace = replace or {}
    source = zipfile.ZipFile(io.BytesIO(data))
    output = io.BytesIO()
    with source, zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as target:
        for info in source.infolist():
            target.writestr(info.filename, replace.get(info.filename, source.read(info.filename)))
        if extra:
            target.writestr(extra, b"x")
    return output.getvalue()


def test_archive_round_trip_is_repeatably_identical() -> None:
    first = encode(bundle())
    assert first == encode(bundle())
    assert decode(first) == bundle()


def test_archive_rejects_corrupt_checksum_and_wrong_version() -> None:
    encoded = encode(bundle())
    with pytest.raises(BundleError, match="section-checksum-mismatch"):
        decode(rewrite(encoded, {"schedule.json": b"{}"}))
    manifest = json.loads(zipfile.ZipFile(io.BytesIO(encoded)).read("manifest.json"))
    manifest["archive_version"] = 2
    with pytest.raises(BundleError, match="invalid-manifest"):
        decode(rewrite(encoded, {"manifest.json": json.dumps(manifest).encode()}))


@pytest.mark.parametrize("name", ["../memory.json", "/memory.json", "memory\\x.json"])
def test_archive_rejects_unknown_or_unsafe_members(name: str) -> None:
    extra = zipfile.ZipInfo(name)
    extra.compress_type = zipfile.ZIP_STORED
    with pytest.raises(BundleError, match="unsafe-member"):
        decode(rewrite(encode(bundle()), extra=extra), limits=ArchiveLimits(max_members=8))


def test_archive_rejects_duplicate_symlink_compression_and_expansion() -> None:
    duplicate = zipfile.ZipInfo("schedule.json")
    duplicate.compress_type = zipfile.ZIP_STORED
    with pytest.raises(BundleError, match="unsafe-member"):
        decode(rewrite(encode(bundle()), extra=duplicate), limits=ArchiveLimits(max_members=8))
    link = zipfile.ZipInfo("link.json")
    link.compress_type = zipfile.ZIP_STORED
    link.external_attr = (stat.S_IFLNK | 0o777) << 16
    with pytest.raises(BundleError, match="unsupported-member"):
        decode(rewrite(encode(bundle()), extra=link), limits=ArchiveLimits(max_members=8))
    with pytest.raises(BundleError, match="archive-too-large"):
        decode(encode(bundle()), limits=ArchiveLimits(max_input_bytes=1))
    with pytest.raises(BundleError, match="member-too-large"):
        decode(encode(bundle()), limits=ArchiveLimits(max_member_bytes=1))
    compressed = io.BytesIO()
    with (
        zipfile.ZipFile(io.BytesIO(encode(bundle()))) as source,
        zipfile.ZipFile(compressed, "w", compression=zipfile.ZIP_DEFLATED) as target,
    ):
        for info in source.infolist():
            target.writestr(info.filename, source.read(info.filename))
    with pytest.raises(BundleError, match="unsupported-member"):
        decode(compressed.getvalue())


def test_archive_rejects_duplicate_json_keys_and_nonfinite_numbers() -> None:
    encoded = encode(bundle())
    with pytest.raises(BundleError, match="invalid-json"):
        decode(rewrite(encoded, {"manifest.json": b'{"archive_version":1,"archive_version":1}'}))
    malformed = b'{"runtime":{"countdown_minutes":NaN}}'
    manifest = json.loads(zipfile.ZipFile(io.BytesIO(encoded)).read("manifest.json"))
    manifest["sections"]["config.json"] = {
        "sha256": hashlib.sha256(malformed).hexdigest(),
        "size": len(malformed),
    }
    with pytest.raises(BundleError, match="invalid-json"):
        decode(
            rewrite(
                encoded,
                {"config.json": malformed, "manifest.json": json.dumps(manifest).encode()},
            )
        )
