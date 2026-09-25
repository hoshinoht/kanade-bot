"""Serialized commit scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

from .. import fixtures

WEEK1 = "2026-08-27T00:00:00+08:00"
WEEK2 = "2026-09-03T00:00:00+08:00"
HOME, ELSEWHERE = "900", "901"
MY, ALVIN, PRIYA, KANON = "1", "2", "3", "4"
MON = "2026-08-31T21:30:00+08:00"
WED = "2026-09-02T21:30:00+08:00"


def _case(case_id: str, steps: list[dict[str, Any]], clock: str = "2026-08-27T01:00:00+08:00"):
    return {
        "case_id": case_id,
        "input": {
            "clock": clock,
            "timezone": fixtures.TIMEZONE,
            "reset_weekday": fixtures.RESET_WEEKDAY,
            "reset_time": fixtures.RESET_TIME,
            "ping_time": "09:00:00",
            "countdowns": [60, 15],
            "members": [MY, ALVIN, PRIYA, KANON],
            "uuid_sequence": [f"00000000-0000-4000-8003-{n:012d}" for n in range(1, 121)],
            "steps": steps,
        },
    }


def run(
    key: str = "a",
    at: str = MON,
    bosses=("HMaleficStar", "HFA"),
    people=(MY, ALVIN, PRIYA),
    status: str = "planned",
    channel: str | None = HOME,
    week: str = WEEK1,
) -> dict[str, Any]:
    return {
        "op": "create_run",
        "run_key": key,
        "week_start": week,
        "at": at,
        "bosses": list(bosses),
        "participants": list(people),
        "status": status,
        "channel_id": channel,
    }


def propose(key: str, kind: str, run_key: str | None = "a", **fields: Any) -> dict[str, Any]:
    step: dict[str, Any] = {
        "op": "propose",
        "amendment_key": key,
        "kind": kind,
        "week_start": WEEK1,
        "run_key": run_key,
        "fixed_key": None,
        "bosses": [],
        "new_datetime": None,
        "participants": [],
        "confidence": 0.9,
        "channel_id": HOME,
        "is_question": False,
        "rsvp": None,
        "day_ref": None,
        "time_ref": None,
        "payload": {},
    }
    step.update(fields)
    return step


def commit(key: str, actor: str = MY, channel: str | None = HOME, remat: bool = False):
    return {
        "op": "commit",
        "amendment_key": key,
        "actor_id": actor,
        "channel_id": channel,
        "rematerialise": remat,
    }


def may(key: str, run_key: str | None, user: str, role: bool = True, admin=False, owner=False):
    return {
        "op": "may_commit",
        "amendment_key": key,
        "run_key": run_key,
        "user_id": user,
        "has_role": role,
        "is_admin": admin,
        "is_owner": owner,
    }


def rsvp(user: str, state: str, key: str = "a") -> dict[str, Any]:
    return {"op": "set_rsvp", "run_key": key, "user_id": user, "state": state}


def fixed(key: str, bosses: list[str], weekday: int, hhmm: str, people: list[str]):
    return {
        "op": "add_fixed",
        "fixed_key": key,
        "owner_id": MY,
        "bosses": bosses,
        "weekday": weekday,
        "time": hhmm,
        "participants": people,
        "channel_id": HOME,
    }


def cases() -> list[dict[str, Any]]:
    return [
        _case(
            "who-may-confirm",
            [
                run(),
                propose("move", "move", new_datetime=WED),
                may("move", "a", ALVIN),
                may("move", "a", "99"),
                may("move", "a", "99", role=False, admin=True),
                may("move", "a", "99", role=False, owner=True),
                may("move", "a", ALVIN, role=False),
                propose("add-named", "add", None, bosses=["NMaleficStar"], participants=[KANON]),
                may("add-named", None, KANON),
                may("add-named", None, "99"),
                propose("add-anyone", "add", None, bosses=["NMaleficStar"]),
                may("add-anyone", None, "99"),
                may("add-anyone", None, "99", role=False),
                propose("fix-anyone", "fix", None, bosses=["NMaleficStar"]),
                may("fix-anyone", None, "99", role=False),
            ],
        ),
        _case(
            "move-reschedules-and-refuses",
            [
                run(),
                rsvp(MY, "yes"),
                rsvp(ALVIN, "yes"),
                rsvp(PRIYA, "yes"),
                propose("no-time", "move", day_ref="wed"),
                commit("no-time"),
                propose("move", "move", new_datetime=WED),
                propose("sibling", "move", new_datetime="2026-09-02T22:30:00+08:00"),
                commit("move"),
                propose("gone", "move", None, new_datetime=WED),
                commit("gone"),
            ],
        ),
        _case(
            "move-into-a-week-its-weekly-already-holds",
            [
                fixed("f", ["HLimbo"], 0, "21:30", [MY, ALVIN]),
                {"op": "rematerialise"},
                {"op": "find_fixed_run", "fixed_key": "f", "week_start": WEEK1, "run_key": "a"},
                propose("across", "move", new_datetime="2026-09-07T21:30:00+08:00"),
                commit("across"),
                propose("within", "move", new_datetime="2026-09-01T21:30:00+08:00"),
                commit("within"),
            ],
        ),
        _case(
            "add-creates-a-run-or-refuses",
            [
                propose("no-when", "add", None, bosses=["NMaleficStar"], day_ref="fri"),
                commit("no-when"),
                propose("no-bosses", "add", None, new_datetime=WED),
                commit("no-bosses"),
                propose(
                    "add",
                    "add",
                    None,
                    bosses=["NMaleficStar", "NCarling"],
                    new_datetime="2026-09-04T21:00:00+08:00",
                    participants=[KANON, PRIYA],
                ),
                propose("same-bosses", "add", None, bosses=["NCarling", "NMaleficStar"]),
                propose(
                    "other-channel",
                    "add",
                    None,
                    bosses=["NMaleficStar", "NCarling"],
                    channel_id=ELSEWHERE,
                ),
                commit("add", KANON),
                propose(
                    "nobody",
                    "add",
                    None,
                    bosses=["HLimbo"],
                    new_datetime="2026-09-03T21:00:00+08:00",
                ),
                commit("nobody", ALVIN, channel=None),
            ],
        ),
        _case(
            "cancel-and-own-time",
            [
                run(),
                run("b", "2026-09-01T22:00:00+08:00", ("HCarling",), (MY, KANON)),
                propose("cancel", "cancel"),
                commit("cancel"),
                propose("otot", "otot", "b"),
                commit("otot"),
                propose("gone", "cancel", None),
                commit("gone"),
            ],
        ),
        _case(
            "stand-ins",
            [
                run(status="confirmed"),
                rsvp(MY, "yes"),
                rsvp(ALVIN, "yes"),
                rsvp(PRIYA, "yes"),
                propose("swap", "sub", payload={"remove": [PRIYA], "add": [KANON]}),
                commit("swap"),
                propose("noop", "sub", payload={"remove": ["99"], "add": [MY]}),
                commit("noop"),
                run("b", "2026-09-01T22:00:00+08:00", ("HCarling",), (KANON,)),
                propose("empty", "sub", "b", payload={"remove": [KANON], "add": []}),
                commit("empty"),
                propose("out", "sub", "b", payload={"remove": [], "add": [ALVIN]}),
                commit("out"),
            ],
        ),
        _case(
            "split-a-run",
            [
                run("a", MON, ("HMaleficStar", "HFA", "HCarling")),
                propose(
                    "split",
                    "split",
                    bosses=["HCarling"],
                    new_datetime="2026-09-01T22:00:00+08:00",
                    payload={"bosses": ["HCarling", "XKalos"], "participants": [ALVIN]},
                ),
                commit("split"),
                propose("none-left", "split", bosses=["XKalos"]),
                commit("none-left"),
                run("b", "2026-09-01T22:00:00+08:00", ("NBaldrix",), (MY, KANON)),
                propose("all", "split", "b", bosses=["NBaldrix"], new_datetime=WED),
                commit("all"),
                run("c", "2026-09-01T23:00:00+08:00", ("NKalos", "NLimbo"), (MY,)),
                propose("keep-time", "split", "c", bosses=["NLimbo"]),
                commit("keep-time"),
            ],
        ),
        _case(
            "carded-answers",
            [
                run(),
                rsvp(MY, "yes"),
                rsvp(ALVIN, "yes"),
                propose("yes", "rsvp", rsvp="yes", participants=[PRIYA]),
                commit("yes"),
                propose("no", "rsvp", rsvp="no", participants=[ALVIN]),
                commit("no"),
                propose("outsider", "rsvp", rsvp="yes", participants=[KANON]),
                commit("outsider"),
                propose("blank", "rsvp", participants=[MY]),
                commit("blank"),
                propose("nobody", "rsvp", rsvp="maybe"),
                commit("nobody"),
            ],
        ),
        _case(
            "weekly-timings-from-chat",
            [
                run("one-off", "2026-09-01T22:30:00+08:00", ("HLimbo", "NBaldrix"), (MY, KANON)),
                propose("no-slot", "fix", None, bosses=["HLimbo", "NBaldrix"]),
                commit("no-slot"),
                propose(
                    "fix",
                    "fix",
                    None,
                    bosses=["HLimbo", "NBaldrix"],
                    participants=[KANON],
                    payload={"weekday": 1, "time": "22:30"},
                ),
                commit("fix", MY, remat=True),
                {
                    "op": "add_fixed",
                    "fixed_key": "w",
                    "owner_id": MY,
                    "bosses": ["NKalos"],
                    "weekday": 4,
                    "time": "21:00",
                    "participants": [MY, ALVIN],
                    "channel_id": HOME,
                },
                {"op": "rematerialise"},
                propose(
                    "edit",
                    "fix",
                    None,
                    fixed_key="w",
                    bosses=["NKalos"],
                    participants=[MY, ALVIN],
                    payload={
                        "op": "edit",
                        "weekday": 5,
                        "time": "20:00",
                        "participants": [MY, PRIYA],
                    },
                ),
                commit("edit", MY, remat=True),
                propose("edit-nothing", "fix", None, fixed_key="w", payload={"op": "edit"}),
                commit("edit-nothing"),
                propose("remove", "fix", None, fixed_key="w", payload={"op": "remove"}),
                commit("remove"),
                propose("remove-again", "fix", None, fixed_key="w", payload={"op": "remove"}),
                commit("remove-again"),
            ],
        ),
        _case(
            "supersede-reject-and-expire",
            [
                run(),
                propose("old", "move", new_datetime=WED),
                propose("away", "move", new_datetime=WED, channel_id=ELSEWHERE),
                {
                    "op": "supersede",
                    "run_key": "a",
                    "channel_id": None,
                    "bosses": [],
                    "keep_key": None,
                    "from_channel": ELSEWHERE,
                },
                propose("new", "otot"),
                {
                    "op": "supersede",
                    "run_key": "a",
                    "channel_id": None,
                    "bosses": [],
                    "keep_key": "new",
                    "from_channel": HOME,
                },
                propose("add1", "add", None, bosses=["NLimbo"]),
                propose("add2", "add", None, bosses=["NLimbo", "NKalos"]),
                {
                    "op": "supersede",
                    "run_key": None,
                    "channel_id": HOME,
                    "bosses": ["NLimbo"],
                    "keep_key": None,
                    "from_channel": None,
                },
                {
                    "op": "supersede",
                    "run_key": None,
                    "channel_id": None,
                    "bosses": [],
                    "keep_key": None,
                    "from_channel": None,
                },
                {"op": "reject", "amendment_key": "new"},
                propose("late", "cancel"),
                {"op": "set_clock", "clock": "2026-08-28T01:00:00+08:00"},
                {"op": "expire_stale"},
                {"op": "set_clock", "clock": "2026-08-28T01:00:01+08:00"},
                {"op": "expire_stale"},
                commit("late"),
            ],
        ),
    ]
