# Chat vectors

Frozen outputs of the v4 `bot.chat` oracle (gate, tools, pilot loop and
context assembly), for the Rust chat port. The generator
(`scripts/v5_vectors/chat` in the v4 tree, git history up to `487c4ed`) wrote
every `<family>.json`, its `<family>.schema.json` and `index.json`; it is gone
with v4, so these files are frozen. Never hand-edit them. This README is the
only hand-written file here.

## Discipline

- Same envelope as `extract/`: each case is `{case_id, input: {..., steps}}`,
  each step result is exactly `{value}` or a declared `{error: {type,
  message}}`; replay applies `$defs/replayCase` before calling v4 and the
  document gate pairs values with `$defs/result_<op>`.
- Nothing reads the wall clock or the process environment. Stateful families
  replay against one clean in-memory `Repo` seeded from the case's `world`
  (rows get the listed IDs; every later ID is drawn from `uuid_sequence`),
  with every `utcnow` seam pinned to `clock` (`set_clock` moves it) and a
  scripted monotonic clock for history TTL and rate limits. `Settings` is the
  real pydantic class built with the environment masked.
- Only Discord transport and the model are stood in for: card posts run the
  real `Pipeline.apply_plan` and delivery journal over stub channels with a
  deterministic message-ID counter; the model is a scripted client returning
  raw OpenAI-shaped completions. `ChatPilot` loads only the tracked public
  Kanade bundle. Fixtures are synthetic (short digit IDs, the extraction
  vectors' six-boss public-name catalog, synthetic UUIDs).
- Tests (`tests/test_v5_chat_vectors.py`) replay every case twice from JSON,
  regenerate twice into temp dirs and compare bytes, and check drift,
  tampering, the input gate, environment isolation and provenance.

## Families

| family | cases / steps | oracle |
| --- | --- | --- |
| `gate` | 7 / 72 | `decide`/`access_decide` refusal order (bot, self, DM, guild, kill switch, configuration, channel, mention, role, admin bypass), channel/category/thread allow-list, resolved user/managed-role/reply mentions only, `would_check_mention`, per-person and guild-pool budgets (read before spend, admin exempt, `retry_after_s`), `is_bot_admin`, `retry_note` |
| `authority` | 3 / 24 | `require_authority`: party membership, weekly-owner fallback through `fixed_run_id`, home-channel refusal only when that home is a pilot channel (threads included), admin exemption; exact refusal texts |
| `participants` | 6 / 66 | `validate_participants` (asker default, first person, bot mention/ID/name stripped, joining words forgiven, strangers, bare snowflakes, non-bossers), `new_party`, `validate_bosses` refusals, tolerant `is_true` |
| `tool_schemas` | 1 / 3 | full and read-only tool surfaces (`text` is the exact compact serialization in v4 key order; `tokens` its `estimate_tokens`), write-tool set, loop/reply constants and reactions |
| `read_tools` | 7 / 74 | `get_schedule` (calendar vs boss weeks, `auto` days, scope, participant, trusted force flags, upcoming-only, all-past footers, refusals), `get_run`/`resolve_run`, `list_bosses`, `list_fixed`, `get_pending`, `get_boss_strategy` validation, dispatcher boundary (unknown tool, read-only writes, string/malformed/non-object arguments) |
| `propose` | 6 / 40 | every `propose_*` tool: amendment rows created (kind, week, run, instant, party, rsvp, payload, summary, card posted), supersession of an older card, and each refusal before a row is written |
| `sanitize` | 6 / 64 | `defuse_notes`, `_member_facing`, false-card-claim stripping, clarification detection, trusted `_schedule_defaults`, `_tidy` bounds with a protected listing, schedule regrounding and the full post-loop reply shaping |
| `loop` | 12 / 22 | `ChatPilot.generate` over a scripted provider: every request body (tools as names), 4-round cap with tools withheld on the last round, round after a posted card offered no tools, refused-write reply overwrite vs clarification, duplicate/missing/non-string call IDs, malformed calls, read-only surface, usage sums, transport errors, missing function tools/model alias, `ContextBudgetError` |
| `context` | 6 / 58 | `history` window (6 exchanges) and TTL, `forget`, reply chains (depth 4, deleted/uncached parents, bot turns, dedupe), anchor re-injection, `_speaker` defusing, conversation token budget (2500 cap, 256 floor), focus line and clock header, `_budgeted_messages` trims and `ContextBudgetError` |

## Portability notes

- Text is v4's, including oddities: `get_schedule` for `this_boss` with a
  weekday resolves inside that boss week, `Card's up` does not match the
  read-turn false-claim filter, a bare roster snowflake like `33` is refused
  while `<@33>` resolves, `list_bosses` prints `lv \`None\`` for a synthetic
  boss without a level, and a malformed-argument call runs with `{}`.
- Errors swallowed by `generate` are recorded as v4 formats them
  (`TypeName: message`, `no answer within 60s`); a port should compare the
  envelope and, for `ContextBudgetError`, the estimate numbers.
- Tool outputs omit `duration_ms`; generations omit latencies. Row
  timestamps and native message IDs are reduced to `card_posted`.
- Instants in `propose` rows are UTC as stored; schedule text is guild-local.

Not covered here: `ChatPilot.offer`/`_answer` orchestration (placeholders,
typing, lease-gated reactions, model lock, posting and the interaction log),
strategy prefetch and source attribution (they need real boss-knowledge
documents; `get_boss_strategy` answers from a stand-in `<guide ...>` store),
behaviour-plugin overlays, rejection follow-ups (`followup.py`) and staging
placeholders. v4's per-member chat memory was removed before this freeze
(schema v16) and is deliberately not vectored; only the bounded per-channel
conversation history above is.
