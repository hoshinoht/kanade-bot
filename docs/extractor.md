# The chat extractor

How party chat becomes ✅/❌ proposal cards, how rescans work, and how to
tune the model against exported conversations.

## How the extractor decides

The bot watches the party channels and turns the conversation into schedule
changes. **The model is an extractor, not the scheduler** — five stages, only one
of which is the LLM, and a human ends every one of them.

**1. Gate (Python, no model).** Every message is scored for a boss alias, a clock
time, a weekday or relative day, a scheduling verb, an `@here`, a mention of a
roster member, or an agreement ("Can", "Ok", "kenot"). Banter is dropped here, so
a 13 GB model is never woken for "botter again sigh". Boss tokens tolerate the group's
spelling — `hlimb`, `nbald`, `bladrix`, `hkarling`, `exkalos`, `hstarr` — but
`start` never becomes `Star` and `cc9`/`ch7` is a map channel, not a time. Bare
answers ("Can") only trigger a call if the channel was scheduling in the last 6 h.

**2. Burst.** Gated messages buffer per channel and go to the model together
after `EXTRACT_DEBOUNCE_SECONDS` of silence — or immediately when a message has a
mention/@here **and** a boss or a time. One model call per burst, never per
message. Only messages from people holding the bossing role count.

**3. Model.** One call to Ollama, serialised server-wide (only ever one at a
time), constrained by a JSON schema, at `temperature 0` with `keep_alive=-1`.
The prompt carries the boss table, **that channel's** runs and fixed timings, the
roster members who actually appear, and the messages with `[msg_id]` prefixes so
it can cite evidence. It stays around 2k tokens whatever size the group grows to.
The model emits `kind`, `bosses`, the **literal words** it saw for the day and
time (`"weds"`, `"1030~11+pm"`), who it is about, `is_question`, a confidence and
its evidence message ids. It never computes a date.

**4. Deterministic assembly (Python again).** The per-message pieces are merged
into one candidate per affected run (latest stated value wins, so "9pm" then
"amend to 9:45pm" is one run at 21:45). `day_ref`/`time_ref` are resolved against
the **last evidence message's** local time: `tonight`, `tmr`, `weds`, `next mon`,
`9:30pm`, `930`, `1030~11+pm`, `at 11`, `11pm onward`. A bare hour of 1–11 means
pm, because every run here is an evening run; anything unparseable stays `TBD`
rather than being guessed. Each candidate is then matched to a run by
**bosses ∩ participants**, scoped to the channel it was said in.

**5. Card, then ✅.** Anything below `EXTRACT_MIN_CONFIDENCE` is logged and never
posted. Everything else becomes one card per burst in that channel:

- 📋 **Proposed change** — the thread reached a decision.
- 💡 **Suggested amendment** — it ended on a question, or a field is still
  unknown. Unknown fields read **TBD** and the card names who has not answered.
  Nobody's availability is ever inferred from silence or past attendance.
- 📌 **New fixed timing** — a recurring time was stated ("lock in Tue 1030pm as
  default"); ✅ creates the fixed run exactly as `/fixed add` would.

✅ from a participant of the target run (or an admin, or the server owner)
applies it and edits the card to "✅ applied by …"; ❌ rejects it. Anyone else's
reaction is ignored. Cards nobody answers expire after 24 hours. RSVPs are the
one exception: "Can" / "kenot" is recorded straight away through the same
path as a ✅ reaction, because it records an opinion rather than changing a
schedule.

Every call — prompt, raw JSON, latency, the amendments it produced — is written
to the `extractions` table. That is the prompt-tuning tool, and the portal
renders it.

```
/rescan                                  # queue a re-read of this channel's boss week
/rescan window:2weeks                    # week (default) · 2weeks · 48h · 24h
/rescan scope:all channels               # every watched channel, one at a time
/rescan cancel:True                      # stop the one that is running
/debug extract                           # read it now, ephemeral, raw JSON, no card
```

A rescan is **queued, never awaited**: re-reading a boss week is minutes of model
time, so the bot answers commands, ticks reminders and handles reactions
throughout. One worker drains the queue a channel at a time (the model is
single-threaded anyway), and asking twice for the same channels attaches to the
job already running rather than reading everything twice. Cancelling lands
between conversations — a call already in flight is left to finish.

A rescan does three things in order. It **backfills from Discord first** —
paging `channel.history()` for the window, threads included — so it still works
on a database that was just reset, which is exactly when you reach for it. Then
it splits the window back into the conversations it came from (a 15-minute gap
ends one) and sends **each conversation as its own prompt**, oldest first: a
whole boss week in one prompt would be enormous and impossible for the model to
attribute. Finally, anything it finds that is already over — a time more than
three hours in the past, or a run that is finished or cancelled — is dropped
rather than posted, so re-reading old chat never puts up a card about last
Tuesday. The reply says what it read, not just what it found.

If the current boss week holds no scheduling chat at all, it checks the week
before it once (and says so). Never further back than that.

**What it reads, and how it is cut up.** History is grouped into the
conversations it actually was: the unit is the local calendar day, so a planning
thread that ran from lunchtime to the evening with long pauses stays one prompt.
Only a day too big for one prompt is split, at its longest pause. Each
conversation is one model call, and the channel gets **one card at the end** —
not one per burst, which used to leave a stack of cards superseding each other.

**Automated re-reads are capped at 48 hours** and never widen to the previous
week; only a person asking can choose `week` or `2weeks`.

The same backfill runs on startup for every watched channel — no model call,
just history into the database. Turn it off with `BACKFILL_ON_START=false`.

Turn it off with `EXTRACT_ENABLED=false` (messages are still stored) or
`/bot pause` at runtime. If Ollama is unreachable or slow the extraction is
logged as failed and nothing else happens — the reminders keep running.

## Tuning it

```sh
# replay a real exported channel through the extractor; no Discord, no writes
uv run python -m bot.extract --file data/exports/<name>.jsonl \
    --since 2026-06-01 --host http://127.0.0.1:11434

# the regression suite: 14 fixtures of real (anonymised) chat vs the real model
uv run pytest -m ollama -v
```

`tests/fixtures/extract/*.json` are the group's own messages with names reduced
to single letters and ids replaced by fake snowflakes; each carries the channel's
runs at the time and the amendments a correct extraction produces. Scoring is
strict — everything expected must be found and nothing extra invented.

## Exporting chat

The extractor is tuned against real conversations, which have to be on disk
first. `python -m bot.export` logs in with the **bot token** (never a user token —
self-botting breaks Discord's ToS), pages channel history, and writes one JSONL
file per channel into git-ignored `data/exports/`:

```sh
docker compose run --rm bot python -m bot.export --category <category-id> --since 2026-06-01
```

- `--channel <id>` / `--category <id>` — repeatable. With neither, every watched
  channel is exported. **Only watched channels can be exported**; anything else
  is refused.
- `--since` / `--until` — `YYYY-MM-DD` (midnight group time) or an ISO timestamp.
  `--since` defaults to the start of the current boss week.
- `--out <path>` — a single file; only valid when one channel is selected.
  Otherwise files are named `data/exports/<channel-name>-<since>.jsonl`.

Threads are included and filed under their parent channel. Each line carries
`id, channel_id, channel_name, thread_id, author_id, author_name, author_bot,
created_at, content, mentions[], reply_to, reactions{emoji: [user_ids]},
attachments[]` (plus `edited_at` when edited). **Attachments are recorded as
`[image] name.png` / `[file] name.txt` markers only — the bot never downloads
them.** Exported messages are also upserted into the `messages` table.

The export needs the same privileged intents as the bot, and it does not post,
sync commands, or run the reminder loop.
