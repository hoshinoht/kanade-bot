"""The canonical ordered function schemas handed to the model."""

from __future__ import annotations


def _tool(name: str, description: str, properties: dict, required: list[str]) -> dict:
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    }


_RUN_QUERY = {
    "type": "string",
    "description": (
        "Which run: its short id from an earlier tool result, or a boss and a day "
        "like 'hstar wednesday' or 'kalos tonight'."
    ),
}


TOOLS: list[dict] = [
    _tool(
        "get_schedule",
        "Runs for a boss week: day, time, bosses, status and how many people have "
        "answered. Call this for any question about what is on. Runs marked 'already "
        "happened' are in the past: never offer one as the next or upcoming run. If "
        "nothing upcoming is left, say so plainly instead of reaching for a past run.",
        {
            "week": {
                "type": "string",
                "enum": ["this", "next"],
                "description": "'this' for the current boss week, 'next' for the one after.",
            },
            "scope": {
                "type": "string",
                "enum": ["all", "channel"],
                "description": (
                    "Use 'channel' only when they explicitly say 'this channel', 'here', "
                    "'our runs' or equivalent. A bare date question such as 'what's for "
                    "tomorrow?' asks about the whole group across all channels: use 'all', "
                    "which is the default. The @mention used to address the bot is not a "
                    "channel qualifier. When answering from 'all', say which channel each "
                    "run is in, or say it is the whole group's schedule."
                ),
            },
            "participant": {
                "type": "string",
                "description": (
                    "Set this only when they explicitly ask 'what's for me', 'my runs', "
                    "'what am I on', 'my schedule', or name one roster member. A bare date "
                    "question such as 'what's for tomorrow?' does not ask about the person "
                    "speaking: omit this field. Never copy the @mention used to address the "
                    "bot; it is not a participant."
                ),
            },
            "day": {
                "type": "string",
                "description": (
                    "Optional date within the selected boss week: 'today', 'tonight', "
                    "'tomorrow', or one weekday. Use it whenever the question names a day; "
                    "omit it only when they ask for the whole boss week. i.e. 'whats this weeks"
                    "schedule' Choose the boss week that contains the requested date. DO NOT"
                    "INCLUDE if query asks for entire week!"
                ),
            },
        },
        ["week"],
    ),
    _tool(
        "get_run",
        "One run in full, including who is on it and what each of them answered.",
        {"query": _RUN_QUERY},
        ["query"],
    ),
    _tool(
        "list_bosses",
        "The bosses this guild runs, with their difficulties. Use it to check a name.",
        {},
        [],
    ),
    _tool(
        "get_boss_strategy",
        "Source-backed local strategy notes for one boss. Use this for boss mechanics, phases, "
        "dangers, and strategy facts; it returns only checked-in guide content and sources.",
        {
            "boss": {
                "type": "string",
                "description": "A boss alias, full name, or canonical token such as 'HFA'.",
            },
            "difficulty": {
                "type": "string",
                "description": "Optional difficulty prefix or full name, such as 'h' or 'Hard'.",
            },
        },
        ["boss"],
    ),
    _tool(
        "get_pending",
        "Proposal cards that are waiting for somebody to react ✅ or ❌.",
        {},
        [],
    ),
    _tool(
        "propose_move",
        "Post a card proposing that ONE dated run moves to a new day and time -- that "
        "night only, leaving the rest of the schedule alone. If they mean the recurring "
        'weekly itself ("change the weekly to 23:30", "we do it on Wednesdays now"), use '
        "propose_change_fixed instead. This does NOT move the run: somebody has to react "
        "✅ on the card first.",
        {
            "run_query": _RUN_QUERY,
            "to_when": {
                "type": "string",
                "description": "The new day and time, e.g. 'wed 21:30' or 'tomorrow 9:45pm'.",
            },
        },
        ["run_query", "to_when"],
    ),
    _tool(
        "propose_add",
        "Post a card proposing a NEW run that is not on the schedule yet. By default it is "
        "a ONE-TIME run that week -- only `weekly` makes it recurring. Never use this to "
        "change a weekly that already exists: it would leave a second one beside it and "
        "the party on neither. Use propose_change_fixed for that. This does NOT create "
        "it: somebody has to react ✅ on the card first.",
        {
            "boss": {
                "type": "string",
                "description": (
                    "One complete difficulty-qualified boss. Use EITHER a canonical token such as "
                    "'XBM', 'HBellona', or 'XKalos', OR words with the difficulty first, such as "
                    "'Extreme Black Mage' or 'Hard Bellona'. Do not combine forms or add a second "
                    "difficulty: 'XBM Hard' is invalid; 'extreme bm' means 'XBM'. A bare boss name "
                    "without a difficulty is refused -- ask which one they mean."
                ),
            },
            "when": {
                "type": "string",
                "description": "The day and time, e.g. 'today 21:30' or 'sat 9pm'.",
            },
            "participants": {
                "type": "string",
                "description": (
                    "Optional. Who the run is for, by name, comma separated. Leave it out "
                    "for just the person asking -- which is the default. When you do fill "
                    "it in, the person asking goes in it too whenever they put themselves "
                    "on the run -- 'for me', 'for us', 'I'll come', 'count me in'. Every "
                    "line you are shown is labelled with who said it, so their name is one "
                    "you can write; the word 'me' works as well. 'Schedule a run for me "
                    "and kanon' is a run for BOTH of them: never leave out the person "
                    "asking for it."
                ),
            },
            "weekly": {
                "type": "boolean",
                "description": (
                    "Optional, default false, meaning ONE run on that day only. Set it true "
                    "ONLY when they explicitly say it repeats -- 'weekly', 'every week', "
                    "'every Tuesday', 'recurring', 'fixed'. A separate sentence about the "
                    "run they just asked for counts as saying it: 'tonight 1900, this is "
                    "fixed', 'make it fixed', 'make it weekly' are all true, even though "
                    "the rest of the line reads one-time. Asking for a run that ALREADY "
                    "exists to repeat -- 'make this weekly', 'make it run every week' -- is "
                    "this argument too, not propose_change_fixed: pass the run's boss and "
                    "the day and time it should keep, and the scheduler folds this week's "
                    "run into the new weekly instead of leaving a duplicate beside it. "
                    "'Schedule a run', 'add a run tonight', 'can we do HStar friday' are "
                    "all one-time: leave it out. If their wording is unclear, do NOT ask "
                    "which they mean -- leave it out. "
                    "One-time is the safe default and the card says which one it is."
                ),
            },
        },
        ["boss", "when"],
    ),
    _tool(
        "propose_cancel",
        "Post a card proposing that ONE dated run is cancelled -- a single night off. "
        'For the recurring weekly baseline ("remove the fixed run", "stop doing this '
        'every week") use propose_remove_fixed instead. This does NOT cancel anything: '
        "somebody has to react ✅ on the card first.",
        {"run_query": _RUN_QUERY},
        ["run_query"],
    ),
    _tool(
        "propose_remove_fixed",
        "Post a card proposing that a RECURRING weekly timing is removed, so the boss "
        "stops being scheduled every week. This is not the same as cancelling one night "
        "-- for a single dated run use propose_cancel. If it is unclear which they mean, "
        'ask: "just this week\'s run, or the weekly one?" This does NOT remove anything: '
        "somebody has to react ✅ on the card first.",
        {
            "query": {
                "type": "string",
                "description": (
                    "Which weekly timing: its short id, or a boss and (if there are "
                    "several) a day, like 'weekly hbellona' or 'bellona tuesday'."
                ),
            }
        },
        ["query"],
    ),
    _tool(
        "propose_change_fixed",
        "Post a card proposing that an EXISTING recurring weekly timing changes: the day "
        "and time it happens every week, who is on it, or both. This is the tool for "
        '"change the weekly to 23:30", "we do the fixed run on Wednesdays now" and "add '
        "Priya to the weekly\". It is NOT for one week's run on its own -- propose_move "
        "does that -- and never reach for propose_add instead: adding another weekly "
        "leaves a duplicate and the party split across the two. This does NOT change "
        "anything: somebody has to react ✅ on the card first.",
        {
            "query": {
                "type": "string",
                "description": (
                    "Which weekly timing: its short id from an earlier tool result, or the "
                    "boss and the day it runs on NOW, like 'hlimbo monday'. A channel can "
                    "have several weekly timings -- even two for the same boss on different "
                    "nights -- so give the boss AND its current day, and the time too if "
                    "that is still not enough. If you cannot tell which one they mean, ask "
                    "them; never pick one yourself."
                ),
            },
            "day": {
                "type": "string",
                "description": (
                    "Optional. The new day of the week it should happen on, e.g. "
                    "'wednesday'. Leave it out when only the time changes."
                ),
            },
            "time": {
                "type": "string",
                "description": (
                    "Optional. The new start time, e.g. '23:30' or '9:30pm'. Leave it out "
                    "when only the day changes."
                ),
            },
            "participants": {
                "type": "string",
                "description": (
                    "Optional. The WHOLE party it should have from now on, by name, comma "
                    "separated -- not only the people joining or leaving. That includes "
                    "the person asking whenever they put themselves on it: 'add me to the "
                    "weekly' means the party it has now plus them, and every line you are "
                    "shown is labelled with who said it. Leave it out to keep the party "
                    "exactly as it is."
                ),
            },
        },
        ["query"],
    ),
    _tool(
        "propose_rsvp",
        "Post a card recording the answer of the person you are talking to for one run. "
        "Only ever for them -- you cannot answer on anybody else's behalf.",
        {
            "run_query": _RUN_QUERY,
            "answer": {
                "type": "string",
                "enum": ["yes", "no"],
                "description": "Whether the person speaking to you can make that run.",
            },
        },
        ["run_query", "answer"],
    ),
]


def tool_names() -> list[str]:
    """Return the tools' canonical presentation order."""
    return [t["function"]["name"] for t in TOOLS]
