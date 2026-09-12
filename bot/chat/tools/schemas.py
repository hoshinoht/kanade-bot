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
        "Runs for a calendar or boss week: day, time, bosses, status and RSVP count. "
        "Use for schedule questions; never offer past runs as next or upcoming.",
        {
            "week": {
                "type": "string",
                "enum": ["this", "next", "this_boss", "next_boss", "auto"],
                "description": (
                    "Use 'this' or 'next' for calendar Monday-Sunday weeks. Use 'this_boss' "
                    "or 'next_boss' only when the member explicitly says boss week. Use 'auto' "
                    "for a bare weekday or today, tonight, or tomorrow."
                ),
            },
            "scope": {
                "type": "string",
                "enum": ["all", "channel"],
                "description": (
                    "Use 'channel' only for explicit 'this channel'/'here'/'our runs'. "
                    "Bare dates ask the whole group: use 'all' (default). The bot @mention "
                    "is not a qualifier. When answering from 'all', say each run's channel."
                ),
            },
            "participant": {
                "type": "string",
                "description": (
                    "Set only for explicit 'for me'/'my runs' or a named member. Bare dates "
                    "are not personal: omit it. Never copy the bot @mention."
                ),
            },
            "day": {
                "type": "string",
                "description": (
                    "Optional 'today'/'tonight'/'tomorrow'/weekday. A bare weekday means the "
                    "next upcoming occurrence; a period-qualified weekday means that weekday "
                    "inside the requested calendar or boss week. Omit for whole-week asks."
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
        "dangers, and strategy facts; it returns only checked-in guide content.",
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
        "list_fixed",
        "Recurring weekly timings: boss, weekday, time and party. Use it to see weeklies "
        "before changing one, instead of guessing from the schedule.",
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
        "Propose a NEW run. Default: ONE-TIME. Set `weekly` true for explicit recurrence, "
        "including 'set up a recurring run every Friday'; recurring does not imply an existing "
        "weekly. Never use this to change a weekly that already exists; use "
        "propose_change_fixed. Needs ✅.",
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
                    "Optional comma-separated names, 'me', or Discord mentions (`<@123>`). "
                    "Mentions are exact: pass them as written; never ask for names/tags. Omit "
                    "for just the asker. For 'for me' or 'for us', include the asker too. Turns "
                    "are labelled with who said it."
                ),
            },
            "weekly": {
                "type": "boolean",
                "description": (
                    "Optional, default false = one run that day only. True ONLY for explicit "
                    "repeats ('weekly'/'every week'/'every Tuesday'/'recurring'/'fixed', even as "
                    "a second sentence). Existing run asked to repeat uses this too (scheduler "
                    "folds it into a weekly, no duplicate). Unclear wording: leave it out."
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
        "Propose changes to an identified EXISTING weekly: day, time, party, or both. Not for "
        "one dated run (use propose_move) or 'set up/create a recurring run' (use propose_add "
        "with `weekly` true). When changing one, never reach for propose_add: that creates a "
        "duplicate. Needs ✅.",
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
                    "Optional WHOLE party, not only the people joining or leaving: names, 'me', "
                    "or Discord mentions (`<@123>`). Mentions are exact: pass them as written; "
                    "never ask for names/tags. Include the person asking for 'add me to the "
                    "weekly'; turns are labelled with who said it. Omit to keep the party."
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
