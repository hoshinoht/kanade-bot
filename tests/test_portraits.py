"""Boss portraits: resolving the file, serving it, and attaching it (item 5).

Portraits are entirely optional, so most of what matters here is what happens
when there is no file: a monogram in the portal, and a Discord message
identical to the one sent before the feature existed.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from bot import formatting
from bot.bosses import BossTable

from .fake_bot import ADMIN_TOKEN


@pytest.fixture
def table_with_portraits(tmp_path: Path):
    """A boss table whose config directory has portraits for three bosses.

    Between the four of them that is every case the size argument has: Star has
    both renders, Bellona has only the full one, Kalos names its full file with
    ``portrait:`` and has a same-named decoy inside ``icon/``, and Limbo has no
    picture at all.
    """
    (tmp_path / "portraits").mkdir()
    (tmp_path / "portraits" / "Star.png").write_bytes(b"\x89PNG\r\n\x1a\n fake")
    (tmp_path / "portraits" / "Bellona.png").write_bytes(b"\x89PNG\r\n\x1a\n fake")
    (tmp_path / "portraits" / "kalos-art.webp").write_bytes(b"RIFF fake")
    (tmp_path / "portraits" / "icon").mkdir()
    (tmp_path / "portraits" / "icon" / "Star.png").write_bytes(b"\x89PNG\r\n\x1a\n small")
    (tmp_path / "portraits" / "icon" / "kalos-art.webp").write_bytes(b"RIFF small")
    (tmp_path / "bosses.yaml").write_text(
        """
difficulties: {n: Normal, h: Hard, x: Extreme}
bosses:
  Star:
    full: Radiant Malefic Star
    level: 280
    difficulties: [n, h]
    aliases: [star]
  Kalos:
    full: Gatekeeper Kalos
    level: 265
    difficulties: [n, x]
    portrait: kalos-art.webp
    aliases: [kalos]
  Bellona:
    full: Bellona
    level: 285
    difficulties: [n, h]
    aliases: [bellona]
  Limbo:
    full: Limbo
    level: 285
    difficulties: [n, h]
    aliases: [limbo]
""",
        encoding="utf-8",
    )
    return BossTable.load(tmp_path / "bosses.yaml")


# --- resolving --------------------------------------------------------------


def test_a_portrait_is_found_by_the_boss_key(table_with_portraits):
    path = table_with_portraits.portrait_path("Star")
    assert path is not None and path.name == "Star.png"


def test_an_explicit_filename_wins(table_with_portraits):
    assert table_with_portraits.portrait_path("Kalos").name == "kalos-art.webp"


def test_a_boss_with_no_file_has_no_portrait(table_with_portraits):
    assert table_with_portraits.portrait_path("Limbo") is None


def test_an_unknown_boss_has_no_portrait(table_with_portraits):
    assert table_with_portraits.portrait_path("Gollux") is None


def test_a_canonical_name_resolves_to_its_boss_portrait(table_with_portraits):
    """Every difficulty shares one image; the pill carries the difficulty."""
    assert table_with_portraits.portrait_for("HStar").name == "Star.png"
    assert table_with_portraits.portrait_for("NStar").name == "Star.png"
    assert table_with_portraits.portrait_for("nonsense") is None


def test_an_explicit_filename_that_does_not_exist_is_not_invented(tmp_path: Path):
    (tmp_path / "bosses.yaml").write_text(
        "difficulties: {h: Hard}\nbosses:\n  Star: {portrait: missing.png, difficulties: [h],"
        " aliases: [star]}\n",
        encoding="utf-8",
    )
    assert BossTable.load(tmp_path / "bosses.yaml").portrait_path("Star") is None


def test_a_table_built_without_a_directory_has_no_portraits(bosses):
    assert (
        BossTable.from_dict({"difficulties": {"h": "Hard"}, "bosses": {"Star": {}}}).portrait_path(
            "Star"
        )
        is None
    )


# --- two renders, and the caller picks --------------------------------------


def test_the_small_render_comes_out_of_the_icon_directory(table_with_portraits):
    """Same filename, one directory down -- so the directory is what to assert."""
    icon = table_with_portraits.portrait_path("Star", "icon")

    assert icon.name == "Star.png"
    assert icon.parent.name == "icon"
    assert icon.read_bytes().endswith(b"small")


def test_the_full_render_never_looks_inside_icon(table_with_portraits):
    """Asking for the big one cannot quietly serve the small one to something
    that needed the detail -- Discord's card thumbnail, for instance."""
    full = table_with_portraits.portrait_path("Star")

    assert full.parent.name == "portraits"
    assert table_with_portraits.portrait_path("Star", "full") == full


def test_an_icon_that_is_not_there_falls_back_to_the_full_picture(table_with_portraits):
    """A boss added today draws correctly before anybody has cropped one -- and
    a fresh clone, where `icon/` does not exist at all, draws the same."""
    fallen_back = table_with_portraits.portrait_path("Bellona", "icon")

    assert fallen_back.name == "Bellona.png"
    assert fallen_back.parent.name == "portraits"


def test_an_override_is_never_looked_for_inside_icon(table_with_portraits):
    """`icon/` is filename-by-key, like the entry artwork: the one line in
    bosses.yaml names the full file and there is no second line naming a small
    one. `icon/kalos-art.webp` is sitting right there and is still not it --
    what comes back is the override, reached through the ordinary fallback."""
    icon = table_with_portraits.portrait_path("Kalos", "icon")

    assert icon.name == "kalos-art.webp"
    assert icon.parent.name == "portraits"


def test_a_boss_with_neither_render_still_has_nothing(table_with_portraits):
    assert table_with_portraits.portrait_path("Limbo", "icon") is None
    assert table_with_portraits.portrait_path("Gollux", "icon") is None


# --- the shipped table ------------------------------------------------------


def test_the_real_table_knows_where_portraits_would_live():
    """The `bosses` fixture is deliberately portrait-free, so ask the real config."""
    from .conftest import REPO_ROOT

    config = REPO_ROOT / "config"
    assert BossTable.load(config / "bosses.yaml").base_dir == config
    assert (config / "portraits" / "README.md").is_file()


# --- the monogram fallback --------------------------------------------------


def test_the_monogram_uses_the_first_letters_of_the_name():
    from bot.api.service import monogram

    assert monogram("Radiant Malefic Star")["text"] == "RM"
    assert monogram("Limbo")["text"] == "L"
    assert monogram("The First Adversary")["text"] == "TF"


def test_the_monogram_colour_is_stable_for_a_name():
    from bot.api.service import monogram

    assert monogram("Limbo")["hue"] == monogram("Limbo")["hue"]
    assert 0 <= monogram("Limbo")["hue"] < 360


def test_a_nameless_boss_still_gets_a_badge():
    from bot.api.service import monogram

    assert monogram("")["text"] == "?"


# --- the portal -------------------------------------------------------------


def test_the_portal_shows_a_monogram_when_there_is_no_portrait(auth, seeded):
    body = auth.get("/").text
    assert "portrait--mono" in body
    assert "--mono-hue:" in body


def test_the_portal_links_the_portrait_when_there_is_one(fake_bot, table_with_portraits, seeded):
    """And links the *small* one: every portrait either surface draws is a badge
    -- 26px beside a boss's name, 38px in the boss grid -- so none of them wants
    the full art, which is a splash now rather than a thumbnail."""
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.bosses = table_with_portraits
    fake_bot.repo.set_run_bosses(seeded["run_star"], ["HStar"])
    with TestClient(create_app(fake_bot)) as client:
        client.headers["Authorization"] = f"Bearer {ADMIN_TOKEN}"
        assert 'src="/static/portraits/Star?size=icon"' in client.get("/").text
        served = client.get("/static/portraits/Star", params={"size": "icon"})
        assert served.status_code == 200
        assert served.content.endswith(b"small")
        assert "max-age" in served.headers["cache-control"]


def test_the_two_sizes_are_two_urls_serving_two_files(fake_bot, table_with_portraits):
    """A query rather than a header, so the browser caches them apart: one URL
    for both would show whichever was fetched first at both sizes."""
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.bosses = table_with_portraits
    with TestClient(create_app(fake_bot)) as client:
        full = client.get("/static/portraits/Star")
        icon = client.get("/static/portraits/Star", params={"size": "icon"})

        assert full.content.endswith(b"fake")
        assert icon.content.endswith(b"small")
        # No query and `?size=full` are the same request; the portal writes the
        # short form, and nothing has to normalise anything.
        assert client.get("/static/portraits/Star", params={"size": "full"}).content == full.content


def test_a_size_that_is_not_one_of_the_two_is_refused(fake_bot, table_with_portraits):
    """Sanitised by being declared: FastAPI checks the word before we see it."""
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.bosses = table_with_portraits
    with TestClient(create_app(fake_bot)) as client:
        assert client.get("/static/portraits/Star", params={"size": "../icon"}).status_code == 422
        assert client.get("/static/portraits/Star", params={"size": "64"}).status_code == 422


def test_an_unknown_portrait_is_a_404_not_a_traversal(fake_bot, table_with_portraits):
    from fastapi.testclient import TestClient

    from bot.api import create_app

    fake_bot.bosses = table_with_portraits
    with TestClient(create_app(fake_bot)) as client:
        assert client.get("/static/portraits/Limbo").status_code == 404
        # The name is looked up in the table, so this is simply not a boss.
        assert client.get("/static/portraits/..%2F..%2Fbosses.yaml").status_code == 404


def test_portraits_do_not_need_a_session(client, fake_bot, table_with_portraits):
    """Like the stylesheet: a picture of a boss, and the browser has to cache it."""
    fake_bot.bosses = table_with_portraits
    assert client.get("/static/portraits/Star").status_code == 200


# --- Discord ----------------------------------------------------------------


def run_row(bosses, at):
    return {
        "id": "r1",
        "bosses": bosses,
        "datetime": at,
        "participants": ["1"],
        "status": "planned",
        "channel_id": "900",
    }


def test_a_card_carries_the_first_bosss_portrait(table_with_portraits):
    from .conftest import TZ, kl

    run = run_row(["HStar", "HLimbo"], kl(2026, 9, 2, 21, 30))
    card = formatting.day_of_card([run], TZ, {}, table=table_with_portraits)
    assert card.thumbnail_path.name == "Star.png"


def test_a_countdown_carries_it_too(table_with_portraits):
    from .conftest import TZ, kl

    run = run_row(["NKalos"], kl(2026, 9, 2, 23, 0))
    card = formatting.countdown_card(run, 60, TZ, {}, table=table_with_portraits)
    assert card.thumbnail_path.name == "kalos-art.webp"


def test_no_portrait_means_no_attachment(bosses):
    from .conftest import TZ, kl

    run = run_row(["HStar"], kl(2026, 9, 2, 21, 30))
    card = formatting.day_of_card([run], TZ, {}, table=bosses)
    assert card.thumbnail_path is None


def test_a_card_with_no_table_asks_for_nothing():
    from .conftest import TZ, kl

    assert formatting.lead_portrait(["HStar"], None) is None
    run = run_row(["HStar"], kl(2026, 9, 2, 21, 30))
    assert formatting.day_of_card([run], TZ, {}).thumbnail_path is None


def test_the_embed_points_at_the_attachment(table_with_portraits):
    from bot.client import BossBot

    card = formatting.Card(content="hi", thumbnail_path=table_with_portraits.portrait_path("Star"))
    embed = BossBot._embed(card)
    assert embed.thumbnail.url == "attachment://Star.png"


def test_an_embedless_card_stays_embedless():
    assert BossBotEmbed(formatting.Card(content="hi")) is None


def BossBotEmbed(card):
    from bot.client import BossBot

    return BossBot._embed(card)


def test_a_thumbnail_alone_is_enough_to_warrant_an_embed(table_with_portraits):
    card = formatting.Card(content="hi", thumbnail_path=table_with_portraits.portrait_path("Star"))
    assert card.has_embed is True


def test_a_missing_file_is_survived_not_raised(tmp_path):
    from bot.client import BossBot

    card = formatting.Card(content="hi", thumbnail_path=tmp_path / "gone.png")
    assert BossBot._attachment(card) is None
