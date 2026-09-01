# The chatbot

A mention-gated, persona-driven chatbot that answers scheduling questions
and drafts changes as the same ✅/❌ cards everything else uses.

A chatbot with a persona, answering in one channel, through the same Ollama
daemon as the extractor. `CHAT_PILOT_MODEL` defaults to the extractor's local
model, and then nothing it is told leaves the machine; point it at one of
Ollama's hosted `*-cloud` models for a bigger brain, knowing that what members
say to the bot is then processed by that service instead of staying local. The
extractor's model is a separate setting and is not affected either way.

It is **three gates deep and silent about all of them**: it answers only when
mentioned (a reply to one of its own messages counts), only in a channel it was
told about, and only from a member holding `CHAT_PILOT_ROLE_ID`. Anything else
gets no reply and no reaction at all — a bot that announces "you may not use me"
is a bot anyone can make post.

You summon it by @-mentioning it, by mentioning its own bot role (Discord's
autocomplete offers that one, and it counts), or by replying to something it
said. Mentions of *other* roles it happens to hold, and `@everyone`/`@here`, are
ignored — a channel-wide ping must not summon it.

It answers with reactions while it works, because a reply takes 10-30 seconds:

| | meaning |
|---|---|
| 👀 | heard you, writing an answer — it comes off when the reply lands |
| ⏳ | you have had your answers for now (see the rate limit below) |
| 💬 | still answering somebody else in this channel; ask again in a moment |

Anything else it declines, it declines in silence.

Where it listens is `CHAT_PILOT_CHANNEL_IDS` (explicit channels) and/or
`CHAT_PILOT_CATEGORY_IDS` (every text channel under a category, including ones
added later). A thread counts as its parent channel. These resolve exactly as
the extractor's `CHAT_CHANNEL_IDS`/`CHAT_CATEGORY_IDS` do — same code — but they
are **separate lists**: a watched party channel does not become a chat channel,
and a chat channel is not read by the extractor.

Holders of `ADMIN_ROLE_ID` — the existing "who runs this bot" role — pass the
chat-role check without also holding the pilot role, and stay exempt from the
rate limit. Everyone else needs the pilot role.

**It cannot change the schedule.** Its read tools answer questions; its six
write tools post the *same* ✅/❌ proposal card the extractor posts, through the
same code, and a participant still has to react ✅ before anything happens. It cannot
approve, reject or edit a card, and it can only ever RSVP for the person talking
to it. Prompt injection is bounded by that structure rather than by prompt
wording: the worst a "cancel everything" message achieves is a stack of cancel
*cards*.

Its write tools are `propose_add` (a new run), `propose_move` (one dated run to
another night), `propose_cancel` (one dated night off), `propose_remove_fixed`
(the recurring weekly baseline — future weeks stop being scheduled),
`propose_change_fixed` (the same baseline changed in place: a new night, a new
party, or both) and `propose_rsvp`. The two removals are deliberately distinct:
cancelling frees an evening, removing a fixed timing stops the boss being
scheduled at all, and the cards say which is which. So are the two changes:
`propose_move` moves one week's run, `propose_change_fixed` moves the weekly
itself — and neither is `propose_add`, which would leave a second weekly beside
the first. Ask it "what's on in this channel?" and it filters to that channel;
ask without that and it names which channel each run lives in.

**It asks rather than guesses.** When a write is missing something — no time, a
boss with no difficulty ("bellona" is three different fights), a name that could
be two people — the tool refuses with the valid options and the bot asks one
short question. Answer it as a normal reply; a reply to the bot counts as a
mention, so you do not need to @ it again.

**A ❌ on its card gets a follow-up.** Reject a card the chatbot posted for you
and it asks — in character — what you would like instead; reply to that message
and it puts up the corrected card. The question is deliberately hard to farm: it
comes only for cards the *chatbot* posted (never the extractor's), only when the
person the card was drafted for is the one rejecting, and once per card —
un-reacting and re-reacting asks nothing, and rejecting several cards inside
half a minute gets one question, not one each. Portal rejections, quiet mode and
`chat_mode off` produce no follow-up at all. The question itself cannot post a
card: your reply is what does that, and the reply spends your normal chat
allowance while the bot's question spends none of it.

### Set it up

```sh
# 1. In .env -- note the CHAT_PILOT_ prefix. CHAT_CHANNEL_IDS / CHAT_CATEGORY_IDS
#    are different lists (what the *extractor* reads) and must not be touched.
CHAT_PILOT_ROLE_ID=...          # the role that may talk to the bot
CHAT_PILOT_CHANNEL_IDS=...      # a channel made for this
CHAT_PILOT_CATEGORY_IDS=...     # or a whole category; both empty = feature off

# 2. Write the persona. It is NOT in git: it is per-deployment flavour text,
#    edited by hand, and may name a character you would rather not publish.
#    `personas/` is bind-mounted into the container, so this is a file on the
#    Mac and a restart -- see personas/README.md.
cp personas/persona.example.md personas/persona.md
$EDITOR personas/persona.md

# 3. Rebuild, then mention it in the channel.
docker compose up -d --build
```

A missing persona file falls back to the tracked template and logs a WARNING, so
a wrong `PERSONA_PATH` is obvious rather than being an outage — and the Config
page's Chatbot panel names the file it actually loaded, marked as a fallback
when it is the template.

**Switching voices does not need a restart.** Keep several files in `personas/`
and pick one from the dropdown on that panel: the choice is stored with the rest
of the runtime config, the pilot drops its cached document, and the next
question is answered in the new voice. `PERSONA_PATH` only seeds which file the
setting starts on. Adding a voice is still a file drop — it appears in the list
on the next page load, because the directory is read on each render rather than
cached.

### Controlling it

`chat_mode` is the runtime kill switch, in the same place `quiet_mode` and
`extract_enabled` live — the Config page, `bossctl config set chat_mode off`, or
`PUT /api/config`. It survives restarts, and turning it off is instant. Quiet
mode covers the chatbot like everything else: it keeps answering, and notifies
nobody.

Each member gets `CHAT_PILOT_RATE_COUNT` answers per `CHAT_PILOT_RATE_WINDOW_S`
seconds (4 per 5 minutes by default); past that the bot reacts ⏳ and stays
quiet. `ADMIN_ROLE_ID` holders are exempt, so testing it does not use up your
own. One answer at a time per channel — a question asked while it is still
thinking also gets ⏳ rather than being queued behind a minute of GPU.

`/debug status` reports whether it is on, how many channels and categories it
answers in, and which model it is using — never the ids themselves.

A message the chatbot handles is **not** also read by the extractor. The two
gate on different lists, but those lists can overlap — if your pilot channel
sits under a category in `CHAT_CATEGORY_IDS`, one "@bot move hstar to wednesday"
would otherwise get both a reply and a stray proposal card. A message addressed
to the bot is a conversation; the pilot acts on it through its tools. Ambient
chat in the same channel (no mention) still goes to the extractor exactly as
before.

### Tuning the voice

Two levers sit in code; the persona document itself is yours.

`CHAT_PILOT_TEMPERATURE` (default `0.7`) is the chatbot's own sampling
temperature. It is deliberately *not* the extractor's `0`: that one reads a
schedule out of chat, where the only good answer is the literal one, while this
holds a conversation and a greedy decode reads like a form letter. Warmth is
safe here because every change it drafts is a card somebody still has to ✅.
`top_p` is left at the model's default.

The persona document is thousands of tokens long and sits at the very top of the
prompt — the furthest point from where the model actually composes. So one line
of it is repeated as the **last** thing in the system prompt. Put it in your
persona file as:

```markdown
**Voice:** Dry, fond of the party, allergic to exclamation marks.
```

It is also repeated as the **last message of every model call** — after the
conversation and after any tool results — because that is where recency actually
lands. Card confirmations and error relays have the most tool output in front of
them and were the flattest replies before this. That trailing copy travels as a
bracketed scheduler note in a `user`-role message, not a `system` one: the
gpt-oss chat template hoists every system message into the instructions header
at the top of the prompt, which is exactly the burial the repetition exists to
escape.

The bot also shows the model your `**Good**` worked examples as few-shot lines
(at most 8, ~600 characters, first ones win). Write them as `> ` followed by the
line in backticks under a `**Good**` heading; the `**Bad**` block below it is not
promoted, code fences are skipped, and unfilled `<placeholders>` are ignored.

`Voice:`, `**Voice**:` and `<!-- voice: ... -->` all work; the first one in the
file wins. Leave the angle-bracket placeholder in and the bot falls back to a
generic "answer in the voice defined above" reminder. Keep it to one sentence —
the sentence you would give a stand-in who had thirty seconds to learn the
character.

### Working out why it said that

Every answer leaves one INFO line naming the person, the channel, how long it
took, how many rounds it used, and each tool it called with how that call went:

```
chat: answered 1234 in channel 5678 in 8431 ms (2 round(s), 1 tool call(s):
  get_schedule:ok) -> proposal 9f2c1a4e
```

That is usually enough to tell a slow model from a looping one from a refused
tool. When it is not, `LOG_LEVEL=DEBUG` adds a line per model round (its own
latency) and a line per tool call with **the arguments the model actually
passed**, the duration, the outcome (`ok` / `refused` / `unknown tool` /
`failed`), and the id of any card it produced:

```
chat: round 1/4 model answered in 7902 ms (1 tool call(s))
chat: round 1/4 tool propose_move(run_query='9f2c', to_when='sunday 22:00')
  -> ok in 12 ms card 9f2c1a4e
```

Those arguments are the answer to "why did it propose *that*" — usually the
model resolved a run differently than the asker meant. Everything stays in the
container logs; nothing is stored and there is no UI for it.
