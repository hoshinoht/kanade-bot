"""Building the prompt (DESIGN.md §2.2, "Prompt input").

Everything the model needs and nothing else: the boss table, *this channel's*
runs and fixed timings, the roster members who actually turn up in the burst,
and the messages themselves with stable ``[msg_id]`` prefixes so the model can
cite evidence.  Per DESIGN.md §1 chat is interpreted against the channel's own
runs, which is what keeps the prompt small as the guild grows.

Times are rendered in the guild timezone with the weekday spelled out, because
the model's whole job on the time axis is to *quote the expression it saw* --
it never has to work out what "weds" means, so it only needs enough calendar
context to tell "weds" from "today".
"""

from __future__ import annotations

import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any
from zoneinfo import ZoneInfo

from bot.domain.ids import short_id
from bot.domain.weeks import WEEKDAY_NAMES

from .gate import find_bosses

#: Rough characters-per-token for the prose part of a prompt.  Prose alone sits
#: near 4, but this prompt is not prose: it is dense with timestamps, boss names
#: and punctuation, and measured against ``gpt-oss:20b`` it comes out nearer 3.
CHARS_PER_TOKEN = 2.8

#: What one Discord snowflake costs.  An 18-digit id tokenises at roughly one
#: token per two digits, so the ``[msg_id]`` prefix and ``<@id>`` mention on
#: every rendered line cost about 4x what their length suggests.  Ignoring that
#: is how a 21-message burst was budgeted at 2.9k tokens and arrived as 4.1k.
TOKENS_PER_ID = 8

#: Tokens held back from ``num_ctx`` for the model's answer.  A burst of a dozen
#: messages can legitimately produce a page of JSON, and a prompt that leaves no
#: room for it comes back with ``done_reason="length"`` -- truncated JSON that
#: fails to validate, twice, and yields a garbage card.
CONTEXT_RESERVE = 2500

#: Runs of digits this long are ids rather than times or levels.
_ID_RE = re.compile(r"\d{5,}")

SYSTEM_PROMPT = """\
You read a MapleStory guild's boss-planning chat (Singapore/Malaysian English)
and extract *schedule changes*. You are an extractor, not the scheduler: a human
confirms everything you output, so report only what the messages say and never
fill a gap with a guess.

RULES
1. Evidence only. Every amendment must come from words in the messages. Never
   infer that someone is free, never use "usual" days, silence or past runs.
2. Never work out a date or a time. Copy the expression exactly as written into
   `day_ref` / `time_ref`: "weds", "tmr", "tonight", "9:30pm", "930",
   "1030~11+pm", "at 11". If a field was not stated, use null. Never invent one.
3. ALWAYS fill `bosses` when the amendment is about a run. Use the canonical
   names from the BOSSES table, with their difficulty letter: HMaleficStar, XKalos,
   NBaldrix. List EXACTLY the bosses the messages name -- if they say "the
   hcarl" and RUNS has HCarling + XKalos together, the amendment is about
   HCarling only. If a message names a boss with no difficulty ("limbo",
   "carling", "baldguy"), take the difficulty from the run in RUNS that has it.
   Only when the messages name no boss at all ("can change to wed?", "930
   postpone to 11") do you fall back to the bosses of the run they must mean.
   Leave `bosses` empty for an `rsvp`, or when you cannot tell which run.
4. "I", "me", "my", "we", "us", "our" mean the author of that message.
   `participants` holds discord user ids (the digits inside <@...>), never names.
   For an `rsvp`, `participants` is exactly the author of the answering message.
5. A question is not a decision. "can change to wed?", "This Sunday can anot?",
   "wanna try trio ncarling?", "930 can postpone to 11 anot ah?" -> `is_question:
   true`, and you still emit the amendment. A later "Can", "Ok", "ya", "I ok",
   "okay for wed" from someone else is a SEPARATE amendment of kind `rsvp`. If
   that reply also settles missing schedule details, emit an updated change with
   `is_question: false` too.
6. One amendment per affected run. "mon and tues cannot" with two runs in
   RUNS is two amendments, one per run, each with that run's own bosses.
7. A reply belongs to the thing it replies to. When someone proposes a run and
   the next messages give a time ("9pm i reach kk early", "amend to 9:45pm",
   "Wed i done with boss so 9:30pm onwards"), that time is the `time_ref` of
   THAT proposal -- do not attach it to a different run in RUNS. If the same
   person also agrees to come, that is an extra `rsvp`.
   If one proposal moves multiple runs to the same day, an unqualified follow-up
   time for that day applies to every proposed run.
8. Always emit an `rsvp` for a message that answers yes or no, even when
   nothing else about the run changes and even when the answer cancels the
   proposal ("Today can ah?" -> "today kenot sry" is one `rsvp` with rsvp="no").
9. `evidence_message_ids` lists the [msg_id] values you used, and only those.
10. If the messages contain no schedule change, return `{"amendments": [],
   "summary": "..."}`. Chat about gear, prices, ring fees, damage, bots,
   piloting, map/channel numbers ("cc9", "ch7") or plain banter is NOT a
   schedule change. Numbers like "290" (a level), "$18" (a price) and
   "91234567" (a phone number) are not times.

KINDS -- pick by asking "is this boss already in RUNS?"
  move   a run IN RUNS changes day/time ("amend to 9:45pm", "change to wed?",
         "postpone to 11", "shift our hstar to weds")
  add    a run NOT in RUNS happens ("we doing our nstar and ncarl tonight?",
         "wanna try trio ncarling also?"). Still `add` when no day or time was
         given -- day_ref and time_ref are then null and is_question is true.
  cancel a run is off this week ("mon and tuesday suddenly got things on",
         "we skip both runs this wk", "cannot make it this week")
  split  the party is divided: "we do X and u 3 the Y", "u all do X, i duo Y",
         "we take xkalos, you take nbaldrix". Any message that gives two groups
         of people different bosses is `split`, not `add` or `move`, even when
         one of the bosses is new. List every boss involved, both groups.
  otot   a run happens on people's own time, no reminders. The words are "otot",
         "own time", "we do ourselves". Literal "otot" must be `otot`, never
         `add`; "we otot do the hcarl" names HCarling only, not its whole run.
  sub    someone is out and a stand-in is wanted ("find temp for this week?",
         "can someone cover for me")
  rsvp   an answer about attending: "Can", "Ok", "I ok", "ya", "confirm",
         "kenot", "cannot", "cmi", "not free". Set `rsvp` to yes/no/maybe
         and `participants` to just that author's id.
  fix    a *recurring* timing, not a one-off. The giveaway words are "default
         time", "lock in", "lockin", "as usual", "every tuesday", "our standing
         time": "HLimbo+Nbaldrix we just lockin on Tue night 1030pm onwards as
         default time?" is `fix`. Those words beat `add` and `move` even when
         the boss has no run yet. Put the recurring day in day_ref.

EDGE CASES
- With a Monday run and a Tuesday run, "mon and tuesday got things on can change
  to wed?" affects both. If the reply is "Wed ... 9:30pm onwards can run le",
  both moves use wed/9:30pm and are no longer questions; also emit that author's
  RSVP.
- If HCarling and XKalos share a run, "we otot do the hcarl" produces exactly one
  `otot` amendment whose bosses are `["HCarling"]`.
- If an EARLIER MESSAGE asks about attending a run and the only NEW MESSAGE is
  "Can", emit an `rsvp` with `rsvp: "yes"` for the new message's author.
- "930 can postpone to 11 anot" is a `move` with `is_question: true`.
- "Today can ah?" followed by "today kenot sry" produces an `rsvp` with
  `rsvp: "no"` for the second author.

WORKED EXAMPLE
RUNS:  #a1  HMaleficStar + HFA  Mon 21:30  <@11>(A) <@22>(B)
[1] [.. Sun] [A <@11>] mon cannot leh, can change to wed?
[2] [.. Sun] [B <@22>] okay for wed
[3] [.. Sun] [A <@11>] and we add our nstar tmr 930?
->
{"amendments":[
 {"kind":"move","bosses":["HMaleficStar","HFA"],"day_ref":"wed","time_ref":null,
  "participants":["11"],"rsvp":null,"is_question":true,"confidence":0.8,
  "evidence_message_ids":["1"],"target_run_hint":"#a1"},
 {"kind":"rsvp","bosses":[],"day_ref":"wed","time_ref":null,"participants":["22"],
  "rsvp":"yes","is_question":false,"confidence":0.9,"evidence_message_ids":["2"],
  "target_run_hint":"#a1"},
 {"kind":"add","bosses":["NMaleficStar"],"day_ref":"tmr","time_ref":"930",
  "participants":["11"],"rsvp":null,"is_question":true,"confidence":0.8,
  "evidence_message_ids":["3"],"target_run_hint":null}],
 "summary":"HMaleficStar+HFA proposed for Wed, B agrees; NMaleficStar proposed for tomorrow 930"}

`confidence` is 0-1: 0.9 when someone stated the change plainly, 0.6-0.8 when it
was proposed and not yet agreed, below 0.5 when you are unsure it is scheduling
at all. `target_run_hint` is the #id of the run from RUNS this is about, or null.
"""


@dataclass(frozen=True)
class Msg:
    """One chat message as the prompt shows it."""

    id: str
    author_id: str
    author_name: str
    created_at: datetime
    content: str

    @classmethod
    def from_row(cls, row: dict, name: str) -> Msg:
        return cls(
            id=str(row["id"]),
            author_id=str(row["author_id"]),
            author_name=name,
            created_at=row["created_at"],
            content=row.get("content") or "",
        )


@dataclass
class PromptContext:
    """Everything the prompt is built from.  Plain data -- no Discord objects."""

    tz: ZoneInfo
    table: Any
    burst: Sequence[Msg]
    context: Sequence[Msg] = ()
    runs: Sequence[dict] = ()
    fixed_runs: Sequence[dict] = ()
    roster: Sequence[dict] = ()
    channel_name: str = ""
    #: Runs elsewhere in the guild, used only when this channel has none of its own.
    guild_runs: Sequence[dict] = field(default_factory=list)


# ---------------------------------------------------------------------------
# rendering helpers
# ---------------------------------------------------------------------------


def member_name(member: dict) -> str:
    return member.get("nickname") or member.get("display_name") or str(member["user_id"])


def name_map(roster: Iterable[dict]) -> dict[str, str]:
    return {str(m["user_id"]): member_name(m) for m in roster}


def render_time(when: datetime, tz: ZoneInfo) -> str:
    local = when.astimezone(tz)
    return f"{local:%Y-%m-%d %H:%M} {WEEKDAY_NAMES[local.weekday()]}"


def render_message(msg: Msg, tz: ZoneInfo) -> str:
    """``[123] [2026-08-30 13:07 Sun] [kanon <@114...>] then weds lah``."""
    content = " / ".join(line.strip() for line in msg.content.splitlines() if line.strip())
    return (
        f"[{msg.id}] [{render_time(msg.created_at, tz)}] "
        f"[{msg.author_name} <@{msg.author_id}>] {content}"
    )


def render_bosses(table: Any, only: Iterable[str] = ()) -> str:
    """One line per boss: canonical forms, full name, and the aliases seen in chat."""
    wanted = {s for s in only} or set(table.bosses)
    lines = []
    for short, boss in table.bosses.items():
        if short not in wanted:
            continue
        forms = ", ".join(boss.canonical(letter) for letter in boss.difficulties)
        aliases = ", ".join(boss.aliases[:6])
        lines.append(f"  {forms}  = {boss.full}" + (f"  (chat: {aliases})" if aliases else ""))
    return "\n".join(lines)


def render_run(run: dict, tz: ZoneInfo, names: dict[str, str]) -> str:
    who = " ".join(f"<@{uid}>({names.get(uid, '?')})" for uid in run["participants"])
    when = "own time" if run["status"] == "otot" else render_time(run["datetime"], tz)
    return (
        f"  #{short_id(run['id'])}  {' + '.join(run['bosses']) or '(none)'}  "
        f"{when}  [{run['status']}]  {who}"
    )


def render_fixed(fixed: dict, names: dict[str, str]) -> str:
    who = " ".join(f"<@{uid}>({names.get(uid, '?')})" for uid in fixed["participants"])
    return (
        f"  #{short_id(fixed['id'])}  {' + '.join(fixed['bosses']) or '(none)'}  "
        f"every {WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']}  {who}"
    )


# ---------------------------------------------------------------------------
# choosing what to include
# ---------------------------------------------------------------------------


def relevant_roster(context: PromptContext) -> list[dict]:
    """The members worth naming: authors, people mentioned, and run participants.

    DESIGN.md §2.1 -- the prompt only carries members relevant to the burst so it
    stays small as the guild grows.  Participants of runs in this channel are
    always included, because "we" resolves against them.
    """
    wanted: set[str] = set()
    for msg in list(context.burst) + list(context.context):
        wanted.add(msg.author_id)
        wanted.update(part.strip("<>@!") for part in _mention_ids(msg.content))
    for run in list(context.runs) + list(context.fixed_runs):
        wanted.update(run["participants"])
    ordered = [m for m in context.roster if str(m["user_id"]) in wanted]
    return ordered or list(context.roster)


_MENTION_RE = re.compile(r"<@!?(\d+)>")


def _mention_ids(text: str) -> list[str]:
    return _MENTION_RE.findall(text or "")


def named_bosses(context: PromptContext) -> set[str]:
    """Short names of every boss mentioned in the burst or already in a run here."""
    found: set[str] = set()
    for msg in list(context.burst) + list(context.context):
        found.update(hit.short for hit in find_bosses(msg.content, context.table))
    for run in list(context.runs) + list(context.fixed_runs):
        for canonical in run["bosses"]:
            parts = context.table.split(canonical)
            if parts is not None:
                found.add(parts[1].short)
    return found


# ---------------------------------------------------------------------------
# the prompt
# ---------------------------------------------------------------------------


def build_user_prompt(context: PromptContext) -> str:
    tz = context.tz
    roster = relevant_roster(context)
    names = name_map(roster)
    # A short table is easier for a small model than the full ten bosses; fall
    # back to everything when the burst named nothing recognisable.
    table_text = render_bosses(context.table, named_bosses(context)) or render_bosses(context.table)

    runs = list(context.runs) or list(context.guild_runs)
    scope = "this channel" if context.runs else "the guild (this channel has no runs of its own)"

    parts: list[str] = []
    if context.channel_name:
        parts.append(f"CHANNEL: #{context.channel_name}")
    if context.burst:
        parts.append(f"NOW: {render_time(context.burst[-1].created_at, tz)} ({tz.key})")

    parts.append("BOSSES (use these exact names):\n" + table_text)

    parts.append(
        "ROSTER:\n"
        + (
            "\n".join(f"  <@{m['user_id']}> = {member_name(m)}" for m in roster)
            or "  (nobody on the roster)"
        )
    )

    parts.append(
        f"RUNS scheduled in {scope}:\n"
        + ("\n".join(render_run(r, tz, names) for r in runs) or "  (none)")
    )

    parts.append(
        "FIXED weekly timings for this channel:\n"
        + ("\n".join(render_fixed(f, names) for f in context.fixed_runs) or "  (none)")
    )

    if context.context:
        parts.append(
            "EARLIER MESSAGES (background - a NEW message below may be answering one "
            "of these, but do not extract an amendment whose only evidence is here):\n"
            + "\n".join(render_message(m, tz) for m in context.context)
        )

    parts.append(
        "NEW MESSAGES (extract from these):\n"
        + "\n".join(render_message(m, tz) for m in context.burst)
    )

    parts.append(
        "Return the amendments these NEW messages support. If there are none, "
        'return {"amendments": [], "summary": "no schedule change"}.'
    )
    return "\n\n".join(parts)


def build_messages(context: PromptContext) -> list[dict[str, str]]:
    """The ``messages=`` payload for ``ollama.AsyncClient.chat``."""
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": build_user_prompt(context)},
    ]


def estimate_tokens(text: str) -> int:
    """Roughly what ``text`` costs the model, erring high.

    Not a tokeniser, and deliberately not a tight one: it decides whether a
    burst is split, so guessing low is the expensive mistake.  Calibrated
    against ``gpt-oss:20b``'s own ``prompt_eval_count`` over the fixtures and
    over synthetic bursts of 6 to 40 messages, it lands 4-17% above the truth.
    """
    ids = _ID_RE.findall(text)
    rest = _ID_RE.sub("", text)
    return len(ids) * TOKENS_PER_ID + int(len(rest) / CHARS_PER_TOKEN)


def estimate_messages(messages: Sequence[dict[str, str]]) -> int:
    """:func:`estimate_tokens` over a whole ``messages=`` payload."""
    return estimate_tokens("\n\n".join(m["content"] for m in messages))


def prompt_budget(num_ctx: int) -> int:
    """The most a prompt may cost and still leave the model room to answer."""
    return max(num_ctx - CONTEXT_RESERVE, CONTEXT_RESERVE)


__all__ = [
    "SYSTEM_PROMPT",
    "Msg",
    "PromptContext",
    "build_messages",
    "build_user_prompt",
    "estimate_messages",
    "estimate_tokens",
    "member_name",
    "name_map",
    "prompt_budget",
    "relevant_roster",
    "render_message",
]
