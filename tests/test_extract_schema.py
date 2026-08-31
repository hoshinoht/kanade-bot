"""The structured-output model, and the sloppiness it has to survive.

Ollama's ``format=`` constrains the model to the schema, so most of these are
belt-and-braces -- but a 20B model does emit ``"<@123>"`` where a bare id
belongs, ``82`` where ``0.82`` does, and ``"null"`` where ``null`` does.
"""

from __future__ import annotations

import json

import pytest
from pydantic import ValidationError

from bot.extract.llm import parse_response
from bot.extract.schema import KINDS, Amendment, Extraction, json_schema


def test_a_minimal_amendment_only_needs_a_kind():
    amendment = Amendment(kind="move")
    assert amendment.bosses == [] and amendment.day_ref is None
    assert amendment.confidence == 0.0 and amendment.is_question is False


@pytest.mark.parametrize("kind", KINDS)
def test_every_documented_kind_validates(kind):
    assert Amendment(kind=kind).kind == kind


def test_an_unknown_kind_is_rejected():
    with pytest.raises(ValidationError):
        Amendment(kind="teleport")


def test_mentions_are_reduced_to_bare_ids():
    amendment = Amendment(kind="rsvp", participants=["<@123>", "<@!456>", "789"])
    assert amendment.participants == ["123", "456", "789"]


def test_a_bare_string_is_accepted_where_a_list_belongs():
    amendment = Amendment(kind="move", bosses="HStar, HFA", evidence_message_ids="101")
    assert amendment.bosses == ["HStar", "HFA"]
    assert amendment.evidence_message_ids == ["101"]


def test_duplicates_are_dropped_preserving_order():
    assert Amendment(kind="move", bosses=["HFA", "HStar", "HFA"]).bosses == ["HFA", "HStar"]


@pytest.mark.parametrize("nullish", ["", "null", "none", "N/A", "TBD", "-", "?"])
def test_the_words_a_model_uses_for_nothing_become_none(nullish):
    amendment = Amendment(kind="move", day_ref=nullish, time_ref=nullish, rsvp=nullish)
    assert amendment.day_ref is None and amendment.time_ref is None and amendment.rsvp is None


@pytest.mark.parametrize(
    ("given", "expected"),
    [(0.82, 0.82), ("0.9", 0.9), (82, 0.82), (5, 0.05), (-1, 0.0), ("lots", 0.0), (None, 0.0)],
)
def test_confidence_is_coerced_into_zero_to_one(given, expected):
    assert Amendment(kind="move", confidence=given).confidence == pytest.approx(expected)


@pytest.mark.parametrize(("given", "expected"), [("true", True), ("no", False), (True, True)])
def test_is_question_accepts_a_string(given, expected):
    assert Amendment(kind="move", is_question=given).is_question is expected


def test_rsvp_is_lowercased():
    assert Amendment(kind="rsvp", rsvp="YES").rsvp == "yes"


def test_extra_fields_the_model_invents_are_ignored():
    amendment = Amendment(kind="move", reason="because", certainty=1)
    assert amendment.kind == "move"


def test_a_single_amendment_object_is_accepted_where_a_list_belongs():
    extraction = Extraction(amendments={"kind": "move"}, summary=None)
    assert len(extraction.amendments) == 1 and extraction.summary == ""


def test_an_empty_extraction_is_valid():
    assert Extraction().amendments == []


# ---------------------------------------------------------------------------
# the schema handed to Ollama
# ---------------------------------------------------------------------------


def test_every_property_is_required_so_the_shape_never_varies():
    schema = json_schema()
    amendment = schema["$defs"]["Amendment"]
    assert set(amendment["required"]) == set(amendment["properties"])
    assert set(schema["required"]) == {"amendments", "summary"}


def test_the_schema_is_json_serialisable():
    # It goes over the wire to Ollama as JSON; anything exotic would fail there.
    assert json.loads(json.dumps(json_schema()))["title"] == "Extraction"


def test_the_kind_enum_in_the_schema_matches_the_documented_kinds():
    assert tuple(json_schema()["$defs"]["Amendment"]["properties"]["kind"]["enum"]) == KINDS


# ---------------------------------------------------------------------------
# parsing a raw response
# ---------------------------------------------------------------------------


def test_a_real_response_parses():
    raw = (
        '{"amendments":[{"kind":"move","bosses":["HStar","HFA"],"day_ref":"wed",'
        '"time_ref":null,"participants":["<@1>"],"rsvp":null,"is_question":true,'
        '"confidence":0.8,"evidence_message_ids":["101"],"target_run_hint":"#a1a1a1a1"}],'
        '"summary":"proposed for wed"}'
    )
    extraction = parse_response(raw)
    (amendment,) = extraction.amendments
    assert amendment.kind == "move"
    assert amendment.participants == ["1"]
    assert amendment.target_run_hint == "#a1a1a1a1"


@pytest.mark.parametrize(
    ("raw", "message"),
    [("", "empty"), ("not json", "not JSON"), ("[1, 2]", "JSON object")],
)
def test_junk_responses_raise_something_the_caller_can_report(raw, message):
    with pytest.raises(ValueError, match=message):
        parse_response(raw)
