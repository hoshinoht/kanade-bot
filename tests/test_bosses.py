"""Boss token parsing: `nstar`, `HFA`, `xkalos`, `hcarl` -> canonical names."""

from __future__ import annotations

import pytest

from bot.bosses import BossParseError, BossTable, BossTableError


@pytest.mark.parametrize(
    ("token", "expected"),
    [
        ("nstar", "NStar"),
        ("HStar", "HStar"),
        ("hrms", "HStar"),
        ("nmalefic", "NStar"),
        ("hfa", "HFA"),
        ("HFA", "HFA"),
        ("eadversary", "EFA"),
        ("xkalos", "XKalos"),
        ("ekalos", "EKalos"),
        ("cgatekeeper", "CKalos"),
        ("hcarl", "HCarling"),
        ("ncarling", "NCarling"),
        ("hkarling", "HCarling"),
        ("nkaling", "NCarling"),
        ("hbaldrix", "HBaldrix"),
        ("hbaldguy", "HBaldrix"),  # the guild's channel names use this
        ("nlimbo", "NLimbo"),
        ("njup", "NJupiter"),
        ("hjupiter", "HJupiter"),
        ("hbm", "HBM"),
        ("xblackmage", "XBM"),
        ("nseren", "NSeren"),
        ("xchosenseren", "XSeren"),
        ("n-star", "NStar"),
        ("  Xkalos  ", "XKalos"),
    ],
)
def test_parse_token(bosses: BossTable, token, expected):
    assert bosses.parse_token(token) == expected


def test_bare_boss_name_demands_a_difficulty(bosses: BossTable):
    with pytest.raises(BossParseError) as exc:
        bosses.parse_token("kalos")
    message = str(exc.value)
    assert "missing a difficulty prefix" in message
    # The error lists only the forms this boss actually has.
    assert "EKalos, NKalos, CKalos, XKalos" in message
    assert "HKalos" not in message


@pytest.mark.parametrize(
    ("token", "expected_valid"),
    [
        ("hkalos", "EKalos, NKalos, CKalos, XKalos"),  # Kalos has no Hard
        ("nbm", "HBM, XBM"),  # Black Mage is Hard/Extreme only
        ("cstar", "NStar, HStar"),
        ("xlimbo", "NLimbo, HLimbo"),
    ],
)
def test_a_difficulty_the_boss_does_not_have_is_rejected(bosses: BossTable, token, expected_valid):
    with pytest.raises(BossParseError) as exc:
        bosses.parse_token(token)
    message = str(exc.value)
    assert "difficulty" in message
    assert expected_valid in message


def test_cseren_is_read_as_chaos_seren_and_corrected(bosses: BossTable):
    # "cseren" reads as Chosen Seren to a human, but parses as Chaos + Seren.
    # Seren has no Chaos difficulty, so it is rejected with the real forms.
    with pytest.raises(BossParseError) as exc:
        bosses.parse_token("cseren")
    message = str(exc.value)
    assert "Chosen Seren has no Chaos difficulty" in message
    assert "NSeren, HSeren, XSeren" in message


def test_unknown_boss_is_reported(bosses: BossTable):
    with pytest.raises(BossParseError, match="unknown boss `hzzz`"):
        bosses.parse_token("hzzz")


def test_bosses_the_guild_does_not_run_are_not_in_the_table(bosses: BossTable):
    # The table is deliberately only the ten current bosses (DESIGN.md §10).
    for token in ("nlotus", "hdamien", "hlucid", "xwill", "cgloom", "hvhilla", "nslime"):
        with pytest.raises(BossParseError, match="unknown boss"):
            bosses.parse_token(token)


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("hstar, hfa", ["HStar", "HFA"]),
        ("hstar hfa", ["HStar", "HFA"]),
        ("HStar + HFA", ["HStar", "HFA"]),
        ("xkalos/hcarl", ["XKalos", "HCarling"]),
        ("nstar, nstar", ["NStar"]),  # de-duplicated, order preserved
    ],
)
def test_parse_list(bosses: BossTable, text, expected):
    assert bosses.parse(text) == expected


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("Hard Baldrix", ["HBaldrix"]),  # the live case: the member had said Hard
        ("Extreme Kalos", ["XKalos"]),
        ("HARD baldrix", ["HBaldrix"]),
        ("hard bald", ["HBaldrix"]),  # a word in front of an alias, not just the short
        ("Easy Carling", ["ECarling"]),
        ("Chaos Kalos", ["CKalos"]),
        ("hard star hard fa", ["HStar", "HFA"]),
        ("HBellona, hard star", ["HBellona", "HStar"]),  # the two spellings mix
    ],
)
def test_a_spelled_out_difficulty_is_folded_into_the_prefix(bosses: BossTable, text, expected):
    assert bosses.parse(text) == expected


@pytest.mark.parametrize(
    ("text", "expected_valid"),
    [
        ("Hard Kalos", "EKalos, NKalos, CKalos, XKalos"),
        ("Chaos Seren", "NSeren, HSeren, XSeren"),
    ],
)
def test_a_spelled_out_difficulty_the_boss_lacks_is_still_rejected(
    bosses: BossTable, text, expected_valid
):
    """Folding produces the prefixed token; `parse_token` judges it as it always did."""
    with pytest.raises(BossParseError) as exc:
        bosses.parse(text)
    message = str(exc.value)
    assert "difficulty" in message
    assert expected_valid in message


def test_a_difficulty_word_with_no_boss_after_it_is_not_a_prefix(bosses: BossTable):
    with pytest.raises(BossParseError, match="unknown boss `hard`"):
        bosses.parse("hard")


def test_a_bare_name_is_still_refused_now_the_word_form_exists(bosses: BossTable):
    with pytest.raises(BossParseError, match="missing a difficulty prefix"):
        bosses.parse("baldrix")


def test_parse_reports_every_bad_token_at_once(bosses: BossTable):
    with pytest.raises(BossParseError) as exc:
        bosses.parse("hstar, kalos, hzzz, hkalos")
    message = str(exc.value)
    assert "missing a difficulty prefix" in message
    assert "unknown boss `hzzz`" in message
    assert "no Hard difficulty" in message


def test_parse_rejects_empty_input(bosses: BossTable):
    with pytest.raises(BossParseError, match="no bosses given"):
        bosses.parse("   ")


def test_describe_expands_to_the_full_in_game_name(bosses: BossTable):
    assert bosses.describe("HFA") == "The First Adversary (Hard, Lv270)"
    assert bosses.describe("XKalos") == "Gatekeeper Kalos (Extreme, Lv265)"
    assert bosses.describe("NStar") == "Radiant Malefic Star (Normal, Lv280)"
    assert bosses.describe("HCarling") == "Carling (Hard, Lv275)"
    assert bosses.describe("???") == "???"


def test_describe_all(bosses: BossTable):
    assert bosses.describe_all(["HStar", "HFA"]) == (
        "Radiant Malefic Star (Hard, Lv280) · The First Adversary (Hard, Lv270)"
    )


def test_the_shipped_table_is_exactly_the_ten_current_bosses(bosses: BossTable):
    assert set(bosses.bosses) == {
        "Seren",
        "Kalos",
        "FA",
        "Carling",
        "BM",
        "Star",
        "Bellona",
        "Limbo",
        "Baldrix",
        "Jupiter",
    }
    assert set(bosses.difficulties) == {"e", "n", "h", "c", "x"}


def test_duplicate_aliases_are_rejected_at_load():
    with pytest.raises(BossTableError, match="claimed by both"):
        BossTable.from_dict(
            {
                "difficulties": {"n": "Normal"},
                "bosses": {"Foo": {"aliases": ["x"]}, "Bar": {"aliases": ["x"]}},
            }
        )


def test_a_boss_cannot_allow_a_difficulty_that_does_not_exist():
    with pytest.raises(BossTableError, match="not in `difficulties:`"):
        BossTable.from_dict(
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficulties": ["z"]}}}
        )


def test_table_needs_difficulties_and_bosses():
    with pytest.raises(BossTableError, match="difficulties"):
        BossTable.from_dict({"bosses": {"Foo": {}}})
    with pytest.raises(BossTableError, match="bosses"):
        BossTable.from_dict({"difficulties": {"n": "Normal"}})
