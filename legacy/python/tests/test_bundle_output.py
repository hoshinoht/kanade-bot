import os
from pathlib import Path

import pytest

from bot.infrastructure.bundle.errors import BundleError
from bot.infrastructure.bundle.output import PrivateOutputApproval, publish


def private_root(tmp_path: Path) -> Path:
    root = tmp_path / "private"
    root.mkdir(mode=0o700, parents=True)
    root.chmod(0o700)
    return root


def test_publish_requires_explicit_private_approval_and_never_overwrites(tmp_path: Path) -> None:
    root = private_root(tmp_path)
    target = root / "bundle.zip"
    approval = PrivateOutputApproval(root=root, approved=True)
    assert publish(target, b"data", approval=approval) == target
    assert target.read_bytes() == b"data"
    assert target.stat().st_mode & 0o777 == 0o600
    with pytest.raises(BundleError, match="output-exists"):
        publish(target, b"new", approval=approval)
    with pytest.raises(BundleError, match="output-not-approved"):
        publish(root / "no.zip", b"data", approval=PrivateOutputApproval(root=root, approved=False))


def test_publish_allows_nonprivate_ancestors_but_not_nonprivate_approved_root(
    tmp_path: Path,
) -> None:
    ancestor = tmp_path / "shared-ancestor"
    ancestor.mkdir(mode=0o755)
    ancestor.chmod(0o755)
    root = private_root(ancestor)
    target = root / "bundle.zip"
    publish(target, b"data", approval=PrivateOutputApproval(root=root, approved=True))
    assert target.read_bytes() == b"data"


def test_publish_no_replace_race_preserves_existing_target(tmp_path: Path, monkeypatch) -> None:
    root = private_root(tmp_path)
    target = root / "bundle.zip"
    approval = PrivateOutputApproval(root=root, approved=True)
    real_link = os.link

    def race(source: str, destination: str, **kwargs) -> None:
        descriptor = os.open(
            destination,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL,
            0o600,
            dir_fd=kwargs["dst_dir_fd"],
        )
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(b"racer")
        real_link(source, destination, **kwargs)

    monkeypatch.setattr(os, "link", race)
    with pytest.raises(BundleError, match="output-exists"):
        publish(target, b"data", approval=approval)
    assert target.read_bytes() == b"racer"
    assert not list(root.glob(".*.tmp-*"))


def test_publish_anchors_after_directory_swap(tmp_path: Path, monkeypatch) -> None:
    root = private_root(tmp_path)
    nested = root / "nested"
    nested.mkdir(mode=0o700)
    nested.chmod(0o700)
    moved = root / "moved"
    outside = private_root(tmp_path / "outside-parent")
    target = nested / "bundle.zip"
    approval = PrivateOutputApproval(root=root, approved=True)
    real_link = os.link

    def swap(source: str, destination: str, **kwargs) -> None:
        nested.rename(moved)
        nested.symlink_to(outside, target_is_directory=True)
        real_link(source, destination, **kwargs)

    monkeypatch.setattr(os, "link", swap)
    assert publish(target, b"data", approval=approval) == target
    assert (moved / "bundle.zip").read_bytes() == b"data"
    assert not (outside / "bundle.zip").exists()
    assert not list(moved.glob(".*.tmp-*"))


def test_publish_rejects_symlink_in_approved_root_or_target_path(tmp_path: Path) -> None:
    root = private_root(tmp_path)
    root_link = tmp_path / "root-link"
    root_link.symlink_to(root, target_is_directory=True)
    with pytest.raises(BundleError, match="unsafe-output-path"):
        publish(
            root_link / "bundle.zip",
            b"data",
            approval=PrivateOutputApproval(root=root_link, approved=True),
        )
    outside = private_root(tmp_path / "outside-parent")
    nested_link = root / "nested-link"
    nested_link.symlink_to(outside, target_is_directory=True)
    with pytest.raises(BundleError, match="unsafe-output-path"):
        publish(
            nested_link / "bundle.zip",
            b"data",
            approval=PrivateOutputApproval(root=root, approved=True),
        )
    with pytest.raises(BundleError, match="unsafe-output-path"):
        publish(
            Path(f"{root}/nested/../bundle.zip"),
            b"data",
            approval=PrivateOutputApproval(root=root, approved=True),
        )
    with pytest.raises(BundleError, match="unsafe-output-path"):
        publish(
            root / "bundle.zip",
            b"data",
            approval=PrivateOutputApproval(root=Path(f"{root}/../private"), approved=True),
        )
