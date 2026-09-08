"""Boss token parsing: `nstar`, `HFA`, `xkalos`, `hcarl` -> canonical names."""

from __future__ import annotations

from collections.abc import Mapping

import pytest

from bot.domain.bosses import BossParseError, BossReference, BossTable, BossTableError


@pytest.mark.parametrize(
    ("token", "expected"),
    [
        ("nstar", "NMaleficStar"),
        ("hstar", "HMaleficStar"),
        ("HMaleficStar", "HMaleficStar"),
        ("hrms", "HMaleficStar"),
        ("nmalefic", "NMaleficStar"),
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
        ("n-star", "NMaleficStar"),
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
        ("cstar", "NMaleficStar, HMaleficStar"),
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
    # The table is deliberately only the eleven current bosses.
    for token in ("hdamien", "hlucid", "xwill", "cgloom", "hvhilla", "nslime"):
        with pytest.raises(BossParseError, match="unknown boss"):
            bosses.parse_token(token)


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("hstar, hfa", ["HMaleficStar", "HFA"]),
        ("hstar hfa", ["HMaleficStar", "HFA"]),
        ("HMaleficStar + HFA", ["HMaleficStar", "HFA"]),
        ("xkalos/hcarl", ["XKalos", "HCarling"]),
        ("nstar, nstar", ["NMaleficStar"]),  # de-duplicated, order preserved
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
        ("hard star hard fa", ["HMaleficStar", "HFA"]),
        ("hard maleficstar", ["HMaleficStar"]),
        ("HBellona, hard star", ["HBellona", "HMaleficStar"]),  # the two spellings mix
    ],
)
def test_a_spelled_out_difficulty_is_folded_into_the_prefix(bosses: BossTable, text, expected):
    assert bosses.parse(text) == expected


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("Hard Black Mage", ["HBM"]),
        ("Extreme Black Mage", ["XBM"]),
        ("Extreme Gatekeeper Kalos", ["XKalos"]),
        ("Normal Gatekeeper Kalos", ["NKalos"]),
        ("Normal Radiant Malefic Star", ["NMaleficStar"]),
        ("Hard Radiant Malefic Star", ["HMaleficStar"]),
        ("Normal The First Adversary", ["NFA"]),
        ("normal first adversary", ["NFA"]),
        ("Hard Black Mage, Normal MaleficStar", ["HBM", "NMaleficStar"]),
    ],
)
def test_a_spelled_out_difficulty_folds_onto_a_spaced_name(bosses: BossTable, text, expected):
    assert bosses.parse(text) == expected


@pytest.mark.parametrize(
    ("text", "expected_valid"),
    [
        ("Normal Black Mage", "HBM, XBM"),  # Black Mage is Hard/Extreme only
        ("Hard Gatekeeper Kalos", "EKalos, NKalos, CKalos, XKalos"),
    ],
)
def test_a_spelled_out_difficulty_on_a_spaced_name_is_still_rejected(
    bosses: BossTable, text, expected_valid
):
    with pytest.raises(BossParseError) as exc:
        bosses.parse(text)
    message = str(exc.value)
    assert "difficulty" in message
    assert expected_valid in message


@pytest.mark.parametrize(
    ("text", "expected_valid"),
    [
        ("black mage", "HBM, XBM"),
        ("radiant malefic star", "NMaleficStar, HMaleficStar"),
        ("gatekeeper kalos", "EKalos, NKalos, CKalos, XKalos"),
        ("the first adversary", "EFA, NFA, HFA, XFA"),
        ("first adversary", "EFA, NFA, HFA, XFA"),
    ],
)
def test_a_spaced_name_without_a_difficulty_names_its_forms(
    bosses: BossTable, text, expected_valid
):
    with pytest.raises(BossParseError) as exc:
        bosses.parse(text)
    message = str(exc.value)
    assert "missing a difficulty prefix" in message
    assert expected_valid in message


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
    assert bosses.describe("NMaleficStar") == "Radiant Malefic Star (Normal, Lv280)"
    assert bosses.describe("HCarling") == "Karling (Hard, Lv275)"
    assert bosses.describe("???") == "???"


def test_describe_all(bosses: BossTable):
    assert bosses.describe_all(["HMaleficStar", "HFA"]) == (
        "Radiant Malefic Star (Hard, Lv280) · The First Adversary (Hard, Lv270)"
    )


def test_the_shipped_table_is_exactly_the_eleven_current_bosses(bosses: BossTable):
    assert set(bosses.bosses) == {
        "Lotus",
        "Seren",
        "Kalos",
        "FA",
        "Carling",
        "BM",
        "MaleficStar",
        "Bellona",
        "Limbo",
        "Baldrix",
        "Jupiter",
    }
    assert set(bosses.difficulties) == {"e", "n", "h", "c", "x"}


def test_guide_colours_are_loaded_from_the_nested_guide_map(bosses: BossTable):
    assert bosses.bosses["Lotus"].guide_colour == 0x39BFFF
    assert bosses.bosses["Jupiter"].guide_colour == 0x21A38F


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("Jupiter", BossReference("Jupiter", None)),
        ("Chosen Seren", BossReference("Seren", None)),
        ("HJupiter", BossReference("Jupiter", "h")),
        ("Hard Jupiter", BossReference("Jupiter", "h")),
        ("hard hjup", BossReference("Jupiter", "h")),
        ("please explain Hard Jupiter tonight", BossReference("Jupiter", "h")),
    ],
)
def test_free_form_reference_resolution(bosses: BossTable, raw: str, expected: BossReference):
    assert bosses.resolve_reference(raw) == expected


@pytest.mark.parametrize(
    ("raw", "match"),
    [
        ("Jupiter Limbo", "multiple bosses"),
        ("Hard NJupiter", "conflicting difficulties"),
        ("Chaos Jupiter", "has no Chaos difficulty"),
        ("help with Chaos Jupiter mechanics", "has no Chaos difficulty"),
        ("Normal Jupiter or Hard Jupiter?", "conflicting difficulties"),
        ("not a boss", "no boss found"),
    ],
)
def test_free_form_reference_resolution_refuses_ambiguity(bosses: BossTable, raw: str, match: str):
    with pytest.raises(BossParseError, match=match):
        bosses.resolve_reference(raw)


def test_unknown_guide_keys_and_colours_are_rejected():
    base = {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficulties": ["n"]}}}
    for guide, match in (({"unknown": 1}, "unknown key"), ({"colour": 0x1000000}, "0xFFFFFF")):
        raw = {**base, "bosses": {"Foo": {"difficulties": ["n"], "guide": guide}}}
        with pytest.raises(BossTableError, match=match):
            BossTable.from_dict(raw)


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


@pytest.mark.parametrize(
    "catalog",
    [
        """\
difficulties: {n: Normal}
difficulties: {h: Hard}
bosses: {Foo: {}}
""",
        """\
difficulties: {n: Normal}
bosses:
  Foo:
    full: Foo
    full: Other Foo
""",
    ],
)
def test_yaml_duplicate_keys_are_rejected_at_every_mapping_level(tmp_path, catalog):
    path = tmp_path / "bosses.yaml"
    path.write_text(catalog, encoding="utf-8")

    with pytest.raises(BossTableError, match="duplicate key") as exc:
        BossTable.load(path)
    assert path.name in str(exc.value)


def test_malformed_yaml_is_a_file_specific_table_error(tmp_path):
    path = tmp_path / "bosses.yaml"
    path.write_text("difficulties: [n: Normal\nbosses: {}", encoding="utf-8")

    with pytest.raises(BossTableError, match="cannot parse boss catalog") as exc:
        BossTable.load(path)
    assert path.name in str(exc.value)


@pytest.mark.parametrize(
    ("raw", "match"),
    [
        ([], "root must be a map"),
        ({"difficulties": [], "bosses": {"Foo": {}}}, "difficulties"),
        ({"difficulties": {"n": "Normal"}, "bosses": []}, "bosses"),
        ({"difficulties": {"n": "Normal"}, "bosses": {"Foo": "nope"}}, "Foo must be a map"),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficulties": "n"}}},
            "difficulties must be a non-empty list",
        ),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"aliases": "foo"}}},
            "aliases must be a list",
        ),
        ({"difficulties": {"n": 1}, "bosses": {"Foo": {}}}, "non-empty name"),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"full": ["Foo"]}}},
            "full must be a non-empty string",
        ),
    ],
)
def test_table_rejects_wrong_root_spec_list_and_scalar_types(raw, match):
    with pytest.raises(BossTableError, match=match):
        BossTable.from_dict(raw)


@pytest.mark.parametrize(
    ("raw", "match"),
    [
        ({"difficulties": {"n": " "}, "bosses": {"Foo": {}}}, "non-empty name"),
        ({"difficulties": {"nn": "Normal"}, "bosses": {"Foo": {}}}, "single letter"),
        ({"difficulties": {"n": "Normal"}, "bosses": {"": {}}}, "short name"),
        ({"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"full": " "}}}, "full"),
        ({"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"level": True}}}, "positive integer"),
        ({"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"level": 0}}}, "positive integer"),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficulties": [""]}}},
            "non-empty strings",
        ),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficulties": ["n", "N"]}}},
            "duplicates",
        ),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"aliases": [""]}}},
            "non-empty strings",
        ),
        (
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"portrait": "nested/foo.png"}}},
            "basename",
        ),
    ],
)
def test_table_validates_catalog_values(raw, match):
    with pytest.raises(BossTableError, match=match):
        BossTable.from_dict(raw)


def test_unknown_root_and_boss_keys_are_rejected():
    with pytest.raises(BossTableError, match="unknown root key"):
        BossTable.from_dict({"difficulties": {"n": "Normal"}, "bosses": {"Foo": {}}, "typo": 1})
    with pytest.raises(BossTableError, match="Foo has unknown key"):
        BossTable.from_dict(
            {"difficulties": {"n": "Normal"}, "bosses": {"Foo": {"difficutlies": ["n"]}}}
        )


def test_loaded_root_shape_error_names_the_catalog_file(tmp_path):
    path = tmp_path / "bosses.yaml"
    path.write_text("- not a catalog", encoding="utf-8")

    with pytest.raises(BossTableError, match="invalid boss catalog") as exc:
        BossTable.load(path)
    assert path.name in str(exc.value)


def test_omitted_difficulties_default_to_the_catalog_and_table_maps_are_immutable():
    table = BossTable.from_dict(
        {"difficulties": {"n": "Normal", "h": "Hard"}, "bosses": {"Foo": {}}}
    )

    assert table.bosses["Foo"].difficulties == ("n", "h")
    for mapping, key, value in (
        (table.difficulties, "x", "Extreme"),
        (table.bosses, "Bar", table.bosses["Foo"]),
        (table.aliases, "bar", "Foo"),
    ):
        assert isinstance(mapping, Mapping)
        with pytest.raises(TypeError):
            mapping[key] = value


# -- names_in: "did they say a boss at all" ----------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("jupiter", ["Jupiter"]),
        ("hjupiter", ["Jupiter"]),
        ("hard jupiter tue", ["Jupiter"]),
        ("jup", ["Jupiter"]),
        ("hlimb", ["Limbo"]),
        ("chosen seren", ["Seren"]),
        ("hstar and xkalos", ["MaleficStar", "Kalos"]),
        ("hstar hstar", ["MaleficStar"]),
        ("black mage", ["BM"]),
        ("Hard Black Mage tonight", ["BM"]),
        ("gatekeeper kalos", ["Kalos"]),
        ("the first adversary", ["FA"]),
        ("first adversary", ["FA"]),
        ("radiant malefic star", ["MaleficStar"]),
    ],
)
def test_a_loose_sentence_gives_up_the_bosses_it_names(bosses: BossTable, text, expected):
    assert bosses.names_in(text) == expected


@pytest.mark.parametrize("text", ["", "the weekly run", "monday", "tuesday 23:00", "152fa345"])
def test_anything_that_is_not_a_boss_names_none(bosses: BossTable, text):
    assert bosses.names_in(text) == []


def test_it_does_not_care_about_difficulty_the_way_parse_does(bosses: BossTable):
    """`cseren` is refused by `parse`; here it still names the boss they said."""
    with pytest.raises(BossParseError):
        bosses.parse("cseren")
    assert bosses.names_in("cseren") == ["Seren"]
