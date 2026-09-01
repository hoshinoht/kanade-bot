"""Boss entry artwork: resolving the file, serving it, and veiling the cards.

The splash MapleStory shows behind a boss's entry prompt, laid behind the Week
page's run cards. Optional in exactly the way portraits are -- the directory is
git-ignored game art, so a fresh clone has none of it -- which makes "no file"
the case worth most of this module: no element at all, on either surface, rather
than something holding space for a picture that is not there.

The other half is the lead-boss rule. A run with two bosses is one card and one
picture, and the picture is the *first* boss's: the one the run is named after,
which is the same choice `bot.formatting.lead_portrait` makes for a card in
Discord.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from bot.api import service
from bot.api.app import STATIC_DIR
from bot.bosses import BossTable

PAGE_CSS = (STATIC_DIR / "portal.css").read_text(encoding="utf-8")

#: The stylesheet with its prose taken out, the same way test_portal_tables
#: reads it: every rule in that file is explained above itself, so a comment
#: that *mentions* `mask-image: var(--art-mask)` reads as a declaration to any
#: scan over the raw text.
CSS_RULES = re.sub(r"/\*.*?\*/", "", PAGE_CSS, flags=re.DOTALL)


@pytest.fixture
def table_with_entry_art(tmp_path: Path):
    """A boss table with entry artwork for three of its four bosses.

    Its own files rather than the repository's `config/artwork/entry/`: that
    directory is git-ignored, so whether a developer has dropped the real art in
    must not decide what the suite asserts. Same reason the `bosses` fixture
    loads the shipped yaml out of an empty directory.

    Kalos is the boss with none -- and carries a `portrait:` override whose file
    *is* in here, because the two lookups must not share a filename. BM is here
    only to be a third boss on a run, which is one more than a card can wear.
    """
    art = tmp_path / "artwork" / "entry"
    art.mkdir(parents=True)
    (art / "Star.png").write_bytes(b"\x89PNG\r\n\x1a\n fake")
    (art / "FA.webp").write_bytes(b"RIFF fake")
    (art / "BM.png").write_bytes(b"\x89PNG\r\n\x1a\n fake")
    (art / "kalos-art.webp").write_bytes(b"RIFF fake")
    (tmp_path / "bosses.yaml").write_text(
        """
difficulties: {n: Normal, h: Hard, x: Extreme}
bosses:
  Star:
    full: Radiant Malefic Star
    level: 280
    difficulties: [n, h]
    aliases: [star]
  FA:
    full: The First Adversary
    level: 270
    difficulties: [h]
    aliases: [fa]
  BM:
    full: Black Mage
    level: 275
    difficulties: [h]
    aliases: [bm]
  Kalos:
    full: Gatekeeper Kalos
    level: 265
    difficulties: [n, x]
    portrait: kalos-art.webp
    aliases: [kalos]
""",
        encoding="utf-8",
    )
    return BossTable.load(tmp_path / "bosses.yaml")


def sheet_card_for(body: str, short: str) -> str:
    """One run's full article, out of whichever sheet holds it."""
    start = body.rindex("<article", 0, body.index(f'id="run-{short}"'))
    return body[start : body.index("</article>", start)]


def board_card_for(body: str, short: str) -> str:
    """The same run's compact card off the board above.

    Cut to the board first: the now strip's "what's next" tile opens a sheet
    with the same `data-dialog`, and it is written further up the page.
    """
    board = body[body.index('class="board"') : body.index('id="days"')]
    anchor = board.index(f'data-dialog="sheet-{short}"')
    start = board.rindex('<a class="runcard', 0, anchor)
    return board[start : board.index("</a>", start)]


# --- resolving --------------------------------------------------------------


def test_the_art_is_found_by_the_boss_key(table_with_entry_art):
    path = table_with_entry_art.entry_art_path("Star")
    assert path is not None and path.name == "Star.png"


def test_the_extension_fallback_is_the_portraits_one(table_with_entry_art):
    """One rule for "an image of a boss", so a .webp lands the same way here."""
    assert table_with_entry_art.entry_art_path("FA").name == "FA.webp"


def test_a_boss_with_no_file_has_no_art(table_with_entry_art):
    assert table_with_entry_art.entry_art_path("Kalos") is None


def test_a_portrait_override_is_not_borrowed_for_the_art(table_with_entry_art):
    """`kalos-art.webp` is sitting in the artwork directory and is still not it.

    A portrait may be named in `bosses.yaml`; entry artwork is named by boss key
    and nothing else. Sharing the override would mean a portrait filename could
    quietly decide which splash a run card wears.
    """
    assert table_with_entry_art.portrait_path("Kalos") is None  # no portraits/ here
    assert table_with_entry_art.entry_art_path("Kalos") is None


def test_an_unknown_boss_has_no_art(table_with_entry_art):
    assert table_with_entry_art.entry_art_path("Gollux") is None


def test_a_canonical_name_resolves_to_its_bosss_art(table_with_entry_art):
    """Every difficulty shares one splash, exactly as they share one portrait."""
    assert table_with_entry_art.entry_art_for("HStar").name == "Star.png"
    assert table_with_entry_art.entry_art_for("NStar").name == "Star.png"
    assert table_with_entry_art.entry_art_for("nonsense") is None


def test_a_table_built_without_a_directory_has_no_art():
    table = BossTable.from_dict({"difficulties": {"h": "Hard"}, "bosses": {"Star": {}}})
    assert table.entry_art_path("Star") is None


def test_the_real_table_ships_the_directory_and_its_readme():
    """The `bosses` fixture is deliberately art-free, so ask the real config."""
    from .conftest import REPO_ROOT

    assert (REPO_ROOT / "config" / "artwork" / "entry" / "README.md").is_file()


# --- serving ----------------------------------------------------------------


def test_the_art_is_served_off_the_config_directory(client, fake_bot, table_with_entry_art):
    fake_bot.bosses = table_with_entry_art
    served = client.get("/static/entry/Star")

    assert served.status_code == 200
    assert served.content.startswith(b"\x89PNG")
    # A replaced file should show up on a reload, not in a week.
    assert "max-age" in served.headers["cache-control"]


def test_art_does_not_need_a_session(client, fake_bot, table_with_entry_art):
    """Like the stylesheet and the portraits: a picture, and the browser caches it."""
    fake_bot.bosses = table_with_entry_art
    assert client.get("/static/entry/Star").status_code == 200


def test_an_unknown_boss_is_a_404_not_a_traversal(client, fake_bot, table_with_entry_art):
    fake_bot.bosses = table_with_entry_art

    assert client.get("/static/entry/Kalos").status_code == 404
    # The name is looked up in the boss table, so this is simply not a boss --
    # the filename on disk never comes from the URL.
    assert client.get("/static/entry/..%2F..%2Fbosses.yaml").status_code == 404
    assert client.get("/static/entry/..%2Fportraits%2FStar.png").status_code == 404


# --- which artwork, and in what order ---------------------------------------


def test_a_run_lists_its_bosses_artwork_lead_first(fake_bot, table_with_entry_art):
    """Order is the run's own, so the compact card -- which takes the first --
    shows the boss the run is named after."""
    fake_bot.bosses = table_with_entry_art

    assert service.run_entry_art(fake_bot, ["HStar", "HFA"]) == [
        "/static/entry/Star",
        "/static/entry/FA",
    ]
    assert service.run_entry_art(fake_bot, ["HFA", "HStar"]) == [
        "/static/entry/FA",
        "/static/entry/Star",
    ]


def test_a_boss_with_no_file_is_absent_rather_than_a_gap(fake_bot, table_with_entry_art):
    """Two bosses of which one has artwork make a one-layer card, not a
    half-empty two-layer one -- and the layer is that boss's wherever it sat."""
    fake_bot.bosses = table_with_entry_art

    assert service.run_entry_art(fake_bot, ["XKalos"]) == []
    assert service.run_entry_art(fake_bot, ["XKalos", "HStar"]) == ["/static/entry/Star"]
    assert service.run_entry_art(fake_bot, ["HStar", "XKalos"]) == ["/static/entry/Star"]
    assert service.run_entry_art(fake_bot, []) == []
    assert service.run_entry_art(fake_bot, ["HGollux"]) == []


def test_a_third_boss_is_not_represented(fake_bot, table_with_entry_art):
    """The sheet splits one edge between two corners; a third has nowhere to be."""
    fake_bot.bosses = table_with_entry_art

    art = service.run_entry_art(fake_bot, ["HStar", "HFA", "HBM"])

    assert art == ["/static/entry/Star", "/static/entry/FA"]
    assert len(art) == service.MAX_ENTRY_ART


# --- the two surfaces -------------------------------------------------------


def test_a_two_boss_run_splits_the_sheets_edge_between_them(
    auth, fake_bot, table_with_entry_art, seeded
):
    """The seed's HStar + HFA. Two layers, and each says which corner it takes --
    the server names it, because how many layers there are is a fact about the
    run and a stylesheet cannot count."""
    fake_bot.bosses = table_with_entry_art
    short = service.short_id(seeded["run_star"])

    card = sheet_card_for(auth.get("/").text, short)

    assert card.count('class="run__art') == 2
    assert 'class="run__art run__art--lead"' in card
    assert 'class="run__art run__art--second"' in card
    # The lead is written first, and the lead is the run's first boss.
    assert card.index("/static/entry/Star") < card.index("/static/entry/FA")
    # Decorative, hence the empty alt: both bosses are named, in full, on top.
    assert card.count('alt=""') == 2


def test_the_compact_card_takes_the_lead_bosss_art_and_only_that(
    auth, fake_bot, table_with_entry_art, seeded
):
    """A card this size has room for one picture, so it is the one the run is
    named after -- and it is the *first* boss, not whichever sorts first."""
    fake_bot.bosses = table_with_entry_art
    short = service.short_id(seeded["run_star"])

    compact = board_card_for(auth.get("/").text, short)
    assert compact.count('class="runcard__art"') == 1
    assert 'src="/static/entry/Star"' in compact
    assert "/static/entry/FA" not in compact

    fake_bot.repo.set_run_bosses(seeded["run_star"], ["HFA", "HStar"])
    compact = board_card_for(auth.get("/").text, short)
    assert 'src="/static/entry/FA"' in compact
    assert "/static/entry/Star" not in compact


def test_one_boss_gets_one_layer_and_no_corner_to_share(
    auth, fake_bot, table_with_entry_art, seeded
):
    """Nothing to split the edge with, so the layer is the plain side vignette --
    which is what the bare class draws, hence no modifier on it at all."""
    fake_bot.bosses = table_with_entry_art
    fake_bot.repo.set_run_bosses(seeded["run_star"], ["HStar"])
    short = service.short_id(seeded["run_star"])

    card = sheet_card_for(auth.get("/").text, short)

    assert card.count('class="run__art') == 1
    assert "run__art--" not in card


def test_a_run_with_no_art_carries_no_veil_at_all(auth, fake_bot, table_with_entry_art, seeded):
    """No layer, and nothing holding space for one: the card it was before."""
    fake_bot.bosses = table_with_entry_art
    short = service.short_id(seeded["run_kalos"])
    body = auth.get("/").text

    assert "run__art" not in sheet_card_for(body, short)
    assert "runcard__art" not in board_card_for(body, short)


def test_a_clone_with_no_artwork_renders_the_week_exactly_as_before(auth, seeded):
    """The `bosses` fixture has no artwork directory, which is the shipped case."""
    body = auth.get("/").text

    assert "run__art" not in body
    assert "runcard__art" not in body


def test_the_veil_survives_an_htmx_swap(auth, fake_bot, table_with_entry_art, seeded):
    """Every action replaces `closest .run` whole, so the layer has to be inside
    the article -- anything outside it would vanish on the first button press."""
    fake_bot.bosses = table_with_entry_art

    response = auth.post(
        f"/runs/{seeded['run_star']}/cancel", data={"next": "/"}, headers={"HX-Request": "true"}
    )

    assert response.text.strip().startswith('<article class="run run--cancelled"')
    assert 'src="/static/entry/Star"' in response.text


# --- the stylesheet ---------------------------------------------------------


def rule_from(selector: str) -> str:
    """One rule's body: from where its selector starts to the brace that closes
    it. Nothing in these rules nests, so the first `}` is the end of one.

    Sliced from the comment-stripped text, so a rule's own explanation cannot
    hand a scan the property names it talks about."""
    start = CSS_RULES.index(selector)
    return CSS_RULES[start : CSS_RULES.index("}", start)]


#: Both surfaces share one rule, so most of what follows is about this block.
VEIL = rule_from(".run__art,")


def test_the_veil_costs_the_card_no_height():
    """The whole reason it is a backdrop and not a banner: a run card's height is
    what the board has least of, and a picture must not buy any of it."""
    assert "position: absolute" in VEIL
    assert "inset: 0" in VEIL
    # Not a grid item, not a flow box, and not something a click can land on.
    assert "pointer-events: none" in VEIL


def test_the_veil_is_behind_the_words_and_in_front_of_the_card():
    """Two halves of one fact, and the second is the easy one to lose: a negative
    layer inside a card that does not isolate sinks behind the card's own
    background and is simply never seen."""
    assert "z-index: -1" in VEIL

    for selector in (".run {", "  .runcard {"):
        rule = rule_from(selector)
        assert "isolation: isolate" in rule, selector
        assert "position: relative" in rule, selector


def test_the_veil_is_masked_away_from_everything_anybody_reads():
    """The invariant the treatment exists for. Masked in both spellings, and
    never left unset: `mask-image: var(--art-mask)` with nothing behind it
    computes to no mask, which is the wash laid flat over every word."""
    assert "--art-mask: linear-gradient" in VEIL  # the fallback shape, and solo's
    assert "-webkit-mask-image: var(--art-mask)" in VEIL
    # Twice: the prefixed spelling above, and the plain one an engine new enough
    # to ignore it reads instead.
    assert VEIL.count("mask-image: var(--art-mask)") == 2


def test_each_mask_is_one_gradient_written_once():
    """One diagonal per layer -- `mask-composite` was inconsistent enough across
    engines to be worth not relying on, and every shape here is reachable with a
    single gradient anyway."""
    assert "mask-composite" not in CSS_RULES

    gradients = re.findall(r"--art-mask: (linear-gradient\([^;]+\));", CSS_RULES)
    assert len(gradients) == 3  # solo, the lead's corner, the second's
    assert len(set(gradients)) == 3  # and none of them typed twice


def test_two_bosses_take_opposite_corners_of_the_same_edge():
    """Which is the whole idea of the split: they meet in a seam on the right."""
    lead = rule_from(".runcard__art,\n.run__art--lead {")
    second = rule_from(".run__art--second {")

    assert "205deg" in lead  # down-and-left, so opaque at the top right
    assert "335deg" in second  # up-and-left, so opaque at the bottom right
    # The compact card wears the lead's shape too: one picture, one corner.
    assert ".runcard__art," in lead


def test_one_rule_serves_both_surfaces_and_takes_each_cards_corners():
    """`border-radius: inherit` is what lets it: the sheet's card squares its top
    under the title bar and a phone rounds it again, and the veil follows both
    without a rule of its own -- so there is no override to keep in step."""
    assert "border-radius: inherit" in VEIL
    assert ".runsheet__panel > .run > .run__art" not in CSS_RULES


def test_the_tuning_knobs_are_declared_once_and_read_from_there():
    """They will be moved from screenshots, so they are three numbers in one
    place rather than the same numbers typed into several rules."""
    root = CSS_RULES[CSS_RULES.index(":root {") : CSS_RULES.index("[data-colorway=")]

    for knob in ("--art-veil", "--art-crop-sheet", "--art-crop-card"):
        assert root.count(f"{knob}:") == 1, knob
        assert CSS_RULES.count(f"var({knob})") == 1, knob


def test_each_surfaces_crop_opens_below_the_name_plate():
    """Most of these splashes bake a name plate into the top of the frame, so a
    window that opened at the top of the picture would show a caption. The two
    windows are different slices, so they clear the same band at different
    depths -- which is why there are two numbers rather than one."""
    root = CSS_RULES[CSS_RULES.index(":root {") : CSS_RULES.index("[data-colorway=")]
    depth = {
        knob: int(re.search(rf"{knob}: (\d+)%", root).group(1))
        for knob in ("--art-crop-sheet", "--art-crop-card")
    }

    assert all(value > 15 for value in depth.values()), depth
    # The compact card's window is the taller slice, so it starts further down.
    assert depth["--art-crop-card"] > depth["--art-crop-sheet"]

    # Each surface reads its own. `.runcard__art {` opens the shared base rule
    # as well as its own, so this asks for the one that sets a crop rather than
    # for whichever comes first.
    assert re.search(r"\.run__art \{[^}]*--art-crop-sheet", CSS_RULES)
    assert re.search(r"\.runcard__art \{[^}]*--art-crop-card", CSS_RULES)
