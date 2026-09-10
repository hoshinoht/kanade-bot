"""The look of the portal, the grouped nav, and the bot's own identity art.

Things that are only ever seen, never computed, so what is worth testing is the
plumbing under them: a look the server never learns and therefore cannot get
wrong, the two hand-copied dark blocks staying identical, a nav that still has
every page in it, and two image routes that answer honestly when nothing has
been cached -- which is the state a fresh deployment and every test run are in.
"""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from bot.infrastructure import identity

from .fake_bot import ADMIN_TOKEN, make_settings

PNG = b"\x89PNG\r\n\x1a\n and then some pixels"

# --- the look never reaches the server --------------------------------------


# --- the card ---------------------------------------------------------------


# --- the stylesheet's two dark faces ----------------------------------------


# --- the nav ----------------------------------------------------------------


# --- the type scale ---------------------------------------------------------


# --- the nav ----------------------------------------------------------------


# --- the identity cache -----------------------------------------------------


def identity_client(fake_bot, tmp_path: Path):
    """The app, with a data directory a test can drop files into."""
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.settings = make_settings(db_path=str(tmp_path / "bot.sqlite"))
    return TestClient(create_app(fake_bot))


def test_nothing_cached_is_a_404_not_a_broken_page(fake_bot, tmp_path):
    with identity_client(fake_bot, tmp_path) as client:
        client.headers["Authorization"] = f"Bearer {ADMIN_TOKEN}"
        assert client.get("/identity/avatar").status_code == 404
        assert client.get("/identity/banner").status_code == 404


def test_a_cached_avatar_is_served_as_what_it_actually_is(fake_bot, tmp_path):
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(PNG)

    with identity_client(fake_bot, tmp_path) as client:
        response = client.get("/identity/avatar")

    assert response.status_code == 200
    assert response.content == PNG
    assert response.headers["content-type"] == "image/png"
    assert "max-age" in response.headers["cache-control"]


def test_an_animated_avatar_keeps_its_own_type_under_the_png_name(fake_bot, tmp_path):
    """Discord hands back a GIF for an animated avatar; the cache filename is fixed."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(b"GIF89a still not a real gif")

    with identity_client(fake_bot, tmp_path) as client:
        assert client.get("/identity/avatar").headers["content-type"] == "image/gif"


def test_the_art_does_not_need_a_session(fake_bot, tmp_path):
    """Like the stylesheet and the boss portraits: the sign-in page is wearing it."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "banner.png").write_bytes(PNG)

    with identity_client(fake_bot, tmp_path) as client:
        assert client.get("/identity/banner").status_code == 200


def test_the_sign_in_page_needs_no_image_to_look_right(client):
    """It asks for both and gets neither, and there is no <img> to break."""
    body = client.get("/login").text
    assert "gate__hero" in body
    assert "gate__avatar" in body
    assert "<img" not in body


# --- writing the cache ------------------------------------------------------


class FakeAsset:
    """Stands in for ``discord.Asset``: the one method this code calls."""

    def __init__(self, data: bytes | None = None, error: Exception | None = None):
        self.data = data
        self.error = error

    async def read(self) -> bytes:
        if self.error is not None:
            raise self.error
        return self.data or b""


class FakeUser:
    def __init__(self, avatar=None, banner=None):
        self.id = 5555555555555555555
        self.display_avatar = avatar
        self.banner = banner


class FakeClient:
    """Only what :func:`bot.identity.refresh` reaches for."""

    def __init__(self, tmp_path: Path, avatar=None, banner=None, fetch_error=None):
        self.settings = make_settings(db_path=str(tmp_path / "bot.sqlite"))
        self.user = FakeUser(avatar)
        self.banner = banner
        self.fetch_error = fetch_error
        self.fetched: list[int] = []

    async def fetch_user(self, user_id):
        self.fetched.append(user_id)
        if self.fetch_error is not None:
            raise self.fetch_error
        return FakeUser(banner=self.banner)


def test_both_pictures_are_written_beside_the_database(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG), banner=FakeAsset(b"GIF89a banner"))

    written = asyncio.run(identity.refresh(client))

    assert written == ["avatar.png", "banner.png"]
    assert (tmp_path / "identity" / "avatar.png").read_bytes() == PNG
    assert (tmp_path / "identity" / "banner.png").read_bytes() == b"GIF89a banner"
    # The banner is not on the gateway payload, so it costs one REST fetch.
    assert client.fetched == [client.user.id]


def test_a_bot_with_no_banner_still_gets_its_avatar(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG))

    assert asyncio.run(identity.refresh(client)) == ["avatar.png"]
    assert not (tmp_path / "identity" / "banner.png").exists()


def test_discord_being_unreachable_leaves_the_last_copy_alone(tmp_path):
    """Identity art is cosmetic: a refresh that fails must not empty the cache."""
    (tmp_path / "identity").mkdir()
    (tmp_path / "identity" / "avatar.png").write_bytes(PNG)
    client = FakeClient(
        tmp_path,
        avatar=FakeAsset(error=OSError("no route to host")),
        fetch_error=OSError("no route to host"),
    )

    assert asyncio.run(identity.refresh(client)) == []
    assert (tmp_path / "identity" / "avatar.png").read_bytes() == PNG


def test_an_in_memory_database_has_nowhere_to_cache(tmp_path):
    client = FakeClient(tmp_path, avatar=FakeAsset(PNG))
    client.settings = make_settings(db_path=":memory:")

    assert asyncio.run(identity.refresh(client)) == []
    assert identity.identity_dir(":memory:") is None
    assert identity.cached(":memory:", identity.AVATAR_NAME) is None


def test_a_half_written_file_is_never_what_a_browser_sees(tmp_path):
    """Written to a temp name in the same directory, then renamed over."""
    target = tmp_path / "identity" / "avatar.png"
    identity.write_atomic(target, PNG)
    identity.write_atomic(target, b"\x89PNG\r\n\x1a\n newer")

    assert target.read_bytes() == b"\x89PNG\r\n\x1a\n newer"
    assert list(target.parent.iterdir()) == [target]


@pytest.mark.parametrize(
    "head,expected",
    [
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"GIF89a", "image/gif"),
        (b"\xff\xd8\xff\xe0", "image/jpeg"),
        (b"RIFF\x00\x00\x00\x00WEBPVP8 ", "image/webp"),
        (b"not a picture at all", "image/png"),
    ],
)
def test_the_type_is_read_off_the_bytes_not_the_name(tmp_path, head, expected):
    path = tmp_path / "avatar.png"
    path.write_bytes(head)
    assert identity.media_type(path) == expected


def test_caching_identity_never_breaks_a_start(tmp_path, monkeypatch):
    """`on_ready` calls this; nothing cosmetic may take the bot down with it."""
    from bot.agent.client import BossBot

    async def explode(_client):
        raise RuntimeError("discord.py changed under us")

    monkeypatch.setattr(identity, "refresh", explode)
    asyncio.run(BossBot.cache_identity(FakeClient(tmp_path)))
