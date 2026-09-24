"""The keyword gate, exercised on the kind of lines a party channel really carries.

The quoted strings below are anonymised scheduling chat and invented banter, in
the register the gate has to cope with: abbreviations, Manglish, and times
written a dozen ways.  The gate is allowed to be generous -- a false positive
costs one local model call -- but it must never drop a message that changes the
schedule, and it must not turn ordinary words into bosses.
"""

from __future__ import annotations

import pytest

from bot.extract.gate import (
    GateResult,
    canonical_bosses,
    evaluate,
    explicit_rsvp,
    find_bosses,
    find_days,
    find_mentions,
    find_times,
    should_extract,
)

ROSTER = ["100000000000000001", "100000000000000002", "100000000000000003"]


def sig(text: str, table) -> set[str]:
    return set(evaluate(text, table, ROSTER).signals)


# ---------------------------------------------------------------------------
# boss tokens
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("@here pls note tmr 1030~11+pm hlimbo+baldrix", ["HLimbo"]),
        ("Tmr 11pm ckalos ya then the other boss", ["CKalos"]),
        (
            "tonight must clear hfa, hlimbo, nbaldrix and hcarling",
            ["HFA", "HLimbo", "NBaldrix", "HCarling"],
        ),
        ("then weds we do xkalos and u 3 the nbaldrix", ["XKalos", "NBaldrix"]),
        ("we doing our nstar and ncarl tonight?", ["NMaleficStar", "NCarling"]),
        ("then alvin and i can duo hlimbo again this week", ["HLimbo"]),
        ("i carry them hstar hfa", ["HMaleficStar", "HFA"]),
        ("wanna do xserene", ["XSeren"]),
        ("exkalos when", ["XKalos"]),
        ("We try duo hkaling boss room", ["HCarling"]),
        ("u free for hstarr later", ["HMaleficStar"]),  # typo -> fuzzy, prefix present
        ("nbald and hlimb later tonight", ["NBaldrix", "HLimbo"]),
        ("ch7 hstar map", ["HMaleficStar"]),
    ],
)
def test_canonical_boss_tokens_from_real_lines(text, expected, bosses):
    assert canonical_bosses(find_bosses(text, bosses)) == expected


@pytest.mark.parametrize(
    ("text", "short"),
    [
        ("limbo cleared already", "Limbo"),
        ("rmc means malefic?", "MaleficStar"),
        ("Join us for hcarling", "Carling"),  # h+carling is canonical, but 'carling' alone too
        ("baldguy first", "Baldrix"),
        ("bladrix pt", "Baldrix"),
    ],
)
def test_bare_aliases_are_found_without_a_difficulty(text, short, bosses):
    hits = find_bosses(text, bosses)
    assert short in [h.short for h in hits]


def test_a_bare_alias_carries_no_invented_difficulty(bosses):
    (hit,) = find_bosses("limbo tonight", bosses)
    assert (hit.short, hit.difficulty, hit.canonical) == ("Limbo", None, None)


def test_a_difficulty_the_boss_does_not_have_is_reported_without_a_canonical(bosses):
    # Kalos has no Hard mode; the gate still sees a boss, but refuses to name a run.
    (hit,) = find_bosses("hkalos tonight?", bosses)
    assert hit.short == "Kalos"
    assert hit.canonical is None


@pytest.mark.parametrize(
    "text",
    [
        "start of the month",
        "starting now",
        "was doing the daily quest line",
        "wah 120k range already ah",
        "let me move my mesos over",
        "botter again sigh",
        "got the union raid also",
        "i go eat first",
        "extra drop rate today ool",
        "my client keep crashing today",
    ],
)
def test_ordinary_words_never_become_bosses(text, bosses):
    assert find_bosses(text, bosses) == []


# ---------------------------------------------------------------------------
# times
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("9pm i reach kk early", ["9pm"]),
        ("Aiyo amend to 9:45pm", ["9:45pm"]),
        ("9:30pm onward ya", ["9:30pm"]),
        ("@here pls note tmr 1030~11+pm hlimbo+baldrix", ["1030~11+pm"]),
        ("@here see u all later at 11", ["at 11"]),
        ("Boss run later at 11 @here", ["at 11"]),
        ("we run at 930pm ba", ["930pm"]),
        ("hi all, note later 930pm & 1030pm timeslots", ["930pm", "1030pm"]),
        ("8~1130", ["8~1130"]),
        ("11pm, 12am i got x2 hfa boss", ["11pm", "12am"]),
        ("ahh need wait me 1010", ["1010"]),
        ("i can do 11 to 1145pm max", ["1145pm"]),
        ("maybe 9:30 to 10pm ish?", ["9:30", "10pm"]),
        ("9+pm", ["9+pm"]),
        ("21:30 lock in ya", ["21:30"]),
    ],
)
def test_time_expressions_from_real_lines(text, expected):
    assert find_times(text) == expected


@pytest.mark.parametrize(
    "text",
    [
        "cc9 entrance",
        "cc6 later",
        "ch7 hstar map",
        "290",  # a level; 2:90 is not a time
        "$200",
        "87218573-alvin tan",
        "91234567",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "Ytd grind whole day 26 lv le",
        "2019 was a year",
    ],
)
def test_non_times_are_not_read_as_clock_expressions(text):
    assert find_times(text) == []


# ---------------------------------------------------------------------------
# days, mentions
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("today i kenot leh we do tues instead?", ["today", "tues"]),
        ("can change to wed?", ["wed"]),
        ("This Sunday can anot?", ["sunday"]),
        ("Tmr 11pm ckalos ya", ["tmr"]),
        ("we doing our nstar and ncarl tonight?", ["tonight"]),
        ("u free on tue/wed night?", ["tue", "wed"]),
        ("mon and tuesday suddenly got things on", ["mon", "tuesday"]),
        ("So Mon/tue we otot do the hcarl", ["mon", "tue"]),
    ],
)
def test_day_words_from_real_lines(text, expected):
    assert find_days(text) == expected


def test_now_and_later_are_not_day_words():
    # They are real, but "cc6 later" is half the corpus; see SOON_WORDS.
    assert find_days("cc6 later") == []
    assert find_days("i am free now") == []


def test_mentions_are_filtered_to_the_roster():
    text = "<@100000000000000001> <@999999999999999999> come"
    assert find_mentions(text, ROSTER) == ["100000000000000001"]


# ---------------------------------------------------------------------------
# the gate verdict
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "text",
    [
        "@Priya @Alvin tan we doing our nstar and ncarl tonight?",
        "Aiyo amend to 9:45pm",
        "sorry mon and tuesday suddenly got things on can change to wed? If not find temp",
        "This Sunday can anot?",
        "today kenot sry 🥲",
        "Wed i done with boss so 9:30pm onwards can run le",
        "So Mon/tue we otot do the hcarl",
        "@here pls note tmr 1030~11+pm hlimbo+baldrix",
        "HLimbo+Nbaldrix we just lockin Tue night 1030pm onwards as the default time?",
        "today i kenot leh we do tues instead?",
        "Boss run later at 11 @here",
        "Tmr 11pm ckalos ya then the other boss",
        "can i suggest we shift our hstar run to sunday pls",
        "930 can postpone to 11 anot boss ask me go makan ltr 😭",
        "we do HMaleficStar when ah? Tuesday evening?",
        "9:30pm onward ya",
        "i can do 11 to 1145pm max",
        "hi all can do monday?",
    ],
)
def test_scheduling_messages_wake_the_model(text, bosses):
    assert evaluate(text, bosses, ROSTER).strong, sig(text, bosses)


@pytest.mark.parametrize(
    "text",
    [
        "botter again sigh",
        "ur pet looks silly",
        "Built different",
        "oops just saw this",
        "let me move my mesos over",
        "i go eat first",
        "wah 120k range already ah",
        "https://tenor.com/view/cat-typing-gif-12345678",
        "91234567",
        "Image",
        "ps was afk LOL",
        "Hoshino will carry us",
        "",
    ],
)
def test_banter_never_wakes_the_model(text, bosses):
    assert not evaluate(text, bosses, ROSTER).strong, sig(text, bosses)


@pytest.mark.parametrize("text", ["Can", "Ok", "kenot", "okie", "sure", "ya"])
def test_bare_answers_are_a_hit_but_not_a_trigger(text, bosses):
    result = evaluate(text, bosses, ROSTER)
    assert result.hit and not result.strong


def test_answers_are_extracted_only_when_the_channel_was_scheduling(bosses):
    answers = [evaluate("Can", bosses, ROSTER), evaluate("Ok", bosses, ROSTER)]
    assert not should_extract(answers, context_is_scheduling=False)
    assert should_extract(answers, context_is_scheduling=True)


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("Can", "yes"),
        ("I think should be okay for wed", "yes"),
        ("today kenot sry", "no"),
        ("Today can ah?", None),
        ("930 can postpone to 11 anot", None),
        ("I can take", None),
    ],
)
def test_only_explicit_attendance_replies_are_deterministic(text, expected):
    assert explicit_rsvp(text) == expected


def test_one_strong_message_carries_the_whole_burst(bosses):
    burst = [
        evaluate("Ok", bosses, ROSTER),
        evaluate("Aiyo amend to 9:45pm", bosses, ROSTER),
    ]
    assert should_extract(burst)


def test_an_empty_burst_is_never_extracted():
    assert not should_extract([])
    assert not should_extract([GateResult()], context_is_scheduling=True)
