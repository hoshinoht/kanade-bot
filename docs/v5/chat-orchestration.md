# Chat orchestration

Status: `src/chat/context/` and `src/chat/answer/` (slice C2). v4 reference:
`bot/chat/agent.py` (`ChatPilot.build_conversation`/`assemble`/`generate`/
`_loop`/`_chat`/`_budgeted_messages`). Gate, tools and reply hygiene are C1
(`src/chat/{gate,authority,tools,sanitize}`); the pre-screen, pollution
containment, persona-voiced failure lines, queueing and allowance refunds
are C3. Serve wiring is `chat::driver` (below).

## Context (`chat::context`)

- In memory, per channel, forgotten on restart: history of at most
  `HISTORY_EXCHANGES` (6) exchanges, dropped once `CHAT_PILOT_HISTORY_TTL_S`
  old; the last posted card (`focus`, same TTL, `>=`); up to `ANCHOR_CACHE`
  (64) answered exchanges keyed by the bot's reply id, re-injected when a
  member replies to an aged-out answer; `REFERENCE_CACHE` (256) replied
  authors. Monotonic seconds are passed in.
- A question's turns: the anchored exchange, live history, the resolved reply
  chain (`REPLY_CHAIN_DEPTH` 4, cached parents only; the bot's own parents
  are assistant turns), deduplicated by message id, then the question. Member
  turns are `Name: text` with forged scheduler notes defused.
- `assemble`: the persona system prompt (clock header, runtime model, focus
  line) and the latest turns within `min(2500, prompt budget − system)`
  tokens, never below 256; the question always stays.
- `budgeted`: per request, prior history is dropped oldest first until the
  whole request (turns, tool calls, the offered tools' JSON) plus
  `COMPLETION_RESERVE_TOKENS` (1024) fits `MODEL_CONTEXT_TOKENS`; the trim
  sticks for later rounds. Nothing left to drop is `ContextBudgetError`.
- Every round is a separate model request, not a stateful provider session:
  resend the system/persona prompt along with retained turns and tool results.

## The question loop (`chat::answer`)

- `answer` formats prompt content with `PassthroughSession` and opens one
  governed question session (`ModelClient::open_question_on`): one permit
  across every round, `tool_rounds` + 1 requests, the timeout bounding the
  whole question, and one requeue. Prompt messages, persona text, history,
  member names and IDs, tool results and complete URLs are passed through
  unchanged when present. Tool arguments are dispatched without identity
  decoding; the final reply follows normal shaping, also without identity
  decoding. There is no boundary scanner or external-call opt-in. External
  and not-yet-classified routes send raw requests under the
  same governor admission, with route/alias pinned for the question. A roster
  read failure logs `chat_members_unreadable` and uses an empty roster; it is
  not a masking refusal. History carries no tool calls (`assemble` builds
  none), so no earlier model-written arguments need rewriting.
- Historical Model view only: new turns do not create `chat_masked` rows,
  mapping snapshots or Model views. Existing masked turns and mappings remain
  readable in the admin chat detail as `model_view` on the admin listener
  only, under the existing chat-log retention and purge behavior; no historical
  row is rewritten (`admin-api.md`).
- Rounds: `tool_rounds` (D-TOOL-ROUNDS, default 8, admin 1..=12). A bundle
  `request_tools` adds is held until the next round starts, so every call is
  judged against the tools its round was actually sent (a tool requested in
  the same reply gets a "next step" note). An added bundle spends one round
  of the cap; a request that would leave no round offering the bundle before
  the final no-tools round (round + 3 > remaining cap) is refused with
  `NO_ROUND_LEFT`, adds nothing and is not charged; nor is one beside a
  posted card (every later round withholds tools, so the bundle is never
  offered). The last round, and the
  round after a posted card, offer no tools and are still sent. A round with
  no tool calls ends the loop; running out of rounds is `KeptCallingTools`.
- Deadline: the question session's deadline also bounds each tool call's
  store load, `ChatPorts::pending` and `ChatPorts::post_card`, as v4's
  `wait_for` bounded the whole loop. The tool dispatch itself (staging a
  proposal and superseding older cards) is never cancelled: a cut between a
  committed proposal and its supersede would leave an unreported proposal
  and stale cards live. The deadline is checked right after each call
  instead; once past it the question ends as `Timeout` (`timeout` in the
  log) with that call's outcome and created proposals reported and its cards
  never posted. A card post cut by the deadline ends the question the same
  way; that card's delivery is unknown (Discord may already have it).
- Cards: proposal tools hand `ProposalCard`s to `ChatPorts::post_card`. A card
  that could not be posted turns that call into a refusal the model reads
  (`CARD_NOT_POSTED`, v4's wording) before the next round; a posted card sets
  the channel focus (`Generation::focus`, for `Conversations::note_card`).
  `ChatPorts::pending` supplies the inbox for `get_pending` each call;
  intent labels and card context reach bundle routing through `ToolOffer`.
- Tool calls: chat sessions validate tool calls leniently (runner
  `ToolCallValidation::Lenient`, user decision 2026-09-25), so unknown,
  unoffered and schema-invalid object-argument calls reach the dispatcher and
  are answered as v4 did: the unknown-tool note, the `request_tools` steering
  note, or the handler's own refusal for the coerced arguments. A withheld
  round keeps earlier tool calls in its transcript. The runner still rejects
  a reply it cannot read (non-JSON or non-object arguments, empty or
  duplicate call ids, bad names) as `InvalidOutput`.
- `get_schedule` first-person recovery: trusted
  `schedule_defaults` reads the original question, excluding the bot mention.
  Only a conservative self-only form (with no additional person/group intent)
  may recover an omitted `participant`, one unrecognized Discord mention, or
  one exact `@bot-name`/bot name from the trusted guild self-name cache as the
  trusted author id. Name recovery applies only when roster resolution found
  no member and no other tokens; mixed requests keep their old group/refusal
  path, and a recognized other member remains that member. This fallback is
  confined to the read tool's trusted self-only path and does not relax member
  access or write-tool validation.
- Clean retry (reserved request): a malformed, empty or undecodable answer
  (including a reply the runner rejects as unreadable) or a content-filtered
  one (`ContentFiltered`) is
  resent once with the system prompt, the asker's message and the reminder,
  no tools, through `Session::clean_retry` (group retry budget, closed
  breaker). If it is refused or also fails, the question fails with
  `Malformed` or `ContentBlocked`; C3 supplies the member-facing line. The
  `clean_retry` flag is set only when the session's request count shows the
  retry was actually sent (a refusal, or a requeue that lost its permit
  before sending, leaves it unset and keeps the original reason).
- Finishing (v4 order): an unposted write overwrites a claiming reply unless
  it already asks a question; new-card claims are stripped on turns that
  posted nothing; then schedule regrounding, member-facing scrubbing and
  bounds (`sanitize::shape_reply`). Named v5 differences: `D-CODE-FENCES`
  (text inside paired ```` ``` ```` fences skips scrubbing and blank-line
  tidying, so code keeps its indentation), `D-ELLIPSIS` (runs of three or
  more dots are kept; v4 turned `Mou...` into `Mou..`) and
  `D-GROUND-FILTERED` (user decision 2026-09-27). Regrounding never reads
  or replaces fenced code, even when every run is named. A reply naming
  only some of the listing's runs (by id) gets just those runs' canonical
  records at the first named run, under its own heading; naming all of them
  inserts the tool's full listing as v4 did. A record-shaped line whose
  `[id]` no tool output carries is dropped (never with a real run's line)
  whenever real runs are named, all of them included; the listing then goes
  at the first real run. With no run named, v4's full listing stands (a
  reply with code but no schedule text keeps its text, listing appended).
  Grounding matches v4 exactly only for a reply without fenced code or
  invented record lines that names every run or none. Over the
  member bound (`MAX_MEMBER_REPLY`, 1200 characters) runs that already
  happened leave the listing (counted in `*(and N more)*`) while an upcoming
  one remains; nothing else is cut (v4 kept the listing and dropped the text
  around it, and cut any other reply at the bound). The chat log keeps the
  whole reply. Posting (`sanitize::reply_parts`, user decision 2026-09-27):
  a reply within the bound is one message, unchanged; a longer one is split
  at paragraph breaks outside fenced code into parts within the bound (and
  Discord's 2000 UTF-16 units), a fence too long for one part is split at
  line boundaries into fences that reopen its opening line, a single line
  too long for one part is cut by characters, and at most
  `MAX_REPLY_PARTS` (4) parts post, the last ending `*(reply trimmed)*`.
  `DiscordSurface` posts the first part as the reply and the rest as plain
  follow-ups in order, all pinging nobody; a failed follow-up is logged
  (`chat_followup_failed`, `chat_followup_stopped`) and ends the reply
  without failing or retrying the answer. A fence is reopened with ```` ``` ````
  plus its language only (a short bare word alone on the opening line), a
  cut never lands inside a fence marker or a joined emoji, and a cut at a
  fence's closing line leaves no empty fence. Known limitation: only the
  first part's message id is kept, so replying to a follow-up part does not
  anchor to the answer the way replying to the first part does.
- Failures are typed (`AnswerFailure`) with their allowance charge; timeouts
  read v4's `no answer within Ns`.

## Chat log

`answer::interaction` builds one `chat_interactions` row with a
`chat_rounds` row per model request (alias, reasoning, finish reason,
latency, bundles offered, tools called, response text; never the prompt).
The round's alias and reasoning are what the governed session actually sent
(`Session::last_sent`: the request's alias and the effort after capability
shaping, `None` when no `reasoning_effort` went out), so a later alias or
effort change never rewrites them; each round also records its `route`
(`homelab`/`external_unmasked`; `external_masked` is historical) and whether it was the
clean retry. The row carries the persona bundle, the reply profile and how it
was chosen (`with_persona`: `saved`/`role`/`default`) and a stable
`error_code` (`AnswerFailure::code`; `rate_limited` for a limited question).
Each round's `tool_calls` entry is `{name, outcome, arguments, created,
posted, result, took_ms}` (user decision 2026-09-26, for agent debugging):
`result` is the tool output the model read, passed through unchanged, capped at 8 KiB on a char boundary with a trailing
`… [truncated, N bytes]` (N = the full length); `took_ms` is the call's
monotonic wall time (store load, dispatch, card posting), the same span
summed into `tools_ms`. The clean retry's round logs no calls. Kept for the
log's 90-day retention. Outcomes: `answered`; `refused`/`clarified` when a write was
refused (clarified if the reply asks); `content_blocked` (guardrail
`{"content_filter": true}`), `timeout`, `turned_away` (governor refusals,
gateway admission, backend down), else `error`. `clean_retry` is a flag;
`rate_limited` rows come from `ChatPilot::limited` and the `withheld` flag
from `ChatPilot::conclude` (below).

## Pilot: traffic and safety (C3)

`chat::pilot` wraps one question; it holds no Discord types (replies go
through `ReplyPort::post_reply`, the adapter wires it later).

- **Allowance.** v4's per-member (4/300 s) and guild-pool (12/900 s)
  windows; `Allowance::apply` loads the defaults and replaces the per-member
  overrides (`chat_allowance_overrides`, `window_ms` → seconds). v5 refunds:
  a question whose failure charges `Refunded` (shed, turned away, before any
  model work), or one dropped from the queue, cancelled or expired, gives its
  slot back to both windows (`Allowance::refund` with the stamp the gate
  spent). A refused member gets v4's static limited reply (their own
  override's count) once per refusal episode, and a `rate_limited` row.
- **Queue** (user decision 2026-09-24; v4 dropped a second question with a
  busy reaction). `Traffic`: one answer per channel; later questions queue
  FIFO per channel (default 3 per channel, 10 guild-wide, 120 s wait), get a
  1-based position, are given up (`Busy`) past a bound, and are cancellable
  by message id. `Traffic::finish` gives up stale waiters (returned in
  `Handoff::expired`) before handing over the next. `spent_at` is `None` for
  admins. Reactions and position display belong to the adapter.
- **Clean-retry guard.** `CleanRetryGuard`: one clean retry per member per
  600 s; more than 3 in 60 s suspends clean retries guild-wide for 600 s and
  raises one `StormAlert`. Every clean retry counts, whatever triggered it
  (content filter, empty or malformed; user decision 2026-09-25).
  `ChatPilot::reserve_clean_retry` reserves one when a question starts (the
  value for `AnswerSettings::clean_retry`), so concurrent questions cannot
  all pass: one reservation per member, and at most 4 recent or reserved
  guild-wide. `conclude` settles it only for the question that holds it
  (`Finished::reserved`): counted if sent, else released; a refused
  question never frees another's reservation.
  Guarded off, the question fails with its original reason and no retry is
  sent. The governor's retry budget and breaker still apply.
- **Routing.** v4 had no model pre-screen, so `ChatPilot::route` is
  code-only (`bundles::select` over the text and card, no intent label).
- **Glue.** `ChatPilot::conclude` posts the answer or a fixed failure line
  (blocked: the persona's `failures.content_blocked`, else
  `CONTENT_BLOCKED_REPLY`; any other failure: v4's `FAILURE_REPLY`),
  remembers the question and reply, anchors the reply id, notes
  `Generation::focus`, refunds, settles the clean-retry reservation and
  builds the log row. "Blocked" is `Generation::is_blocked`: no reply and
  some attempt was content-filtered, even when the clean retry then timed
  out or was malformed (the row keeps that final outcome, with
  `withheld = true` and guardrail `{"content_filter": true}`).
- **Pollution containment.** A blocked question and the reply to it are
  withheld (`Conversations::withhold`): every later prompt shows them as
  `[message withheld]`, they are never anchored or re-anchored, and a reply
  chain through them is withheld too. Up to 4096 ids are kept (by count,
  not the history TTL, since a reply can reach any old message);
  `ChatPilot::reload_withheld` restores them at startup from the chat log's
  withheld rows (`list_chats`, newest first). The bot's replies are not
  logged by id, but they carry only the fixed line.
- **Limits.** `ChatPilot::limits` returns `LimitsView` (member and pool
  used/limit/window/resets-in with override flags, the queue, the guard) for
  the API's Limits page.
- **Serve composition** (required when serve wires the pilot):
  - take the clean-retry reservation when a question is dequeued and starts,
    not when it is admitted to the queue;
  - every path that reserved ends in `conclude` with `reserved: true`;
  - the configured question timeout stays below `per_member_s` (600 s), or
    `prune` can drop a live reservation;
  - call `reload_withheld` before admitting any question.

  Serve wires it in `chat::driver` (`ChatDriver`, no Discord types; ports
  `Answerer` for the model side and `Surface` for reactions and replies),
  composed in `runtime::serve::chat`. `ChatDriver::start` validates the
  timeout and reloads withheld ids before the driver exists. `offer` gates
  (the live `Setup`), then `Traffic::admit`s: answer now, queue (keycap
  position reaction) or shed (💬, refund); a spent budget gets ⏳, the
  limited reply and its row. A channel's worker reserves the clean retry
  when it dequeues a question, answers, posts the reply (`reply_to`, no
  mentions) and then `conclude`s with that reservation under the state
  lock (`conclude` sees the already-posted result), then records the row.
  Before `prepare` a dequeued question is re-gated (chat still on, channel
  still in a chat category, re-read from the live channel directory so a
  channel moved out of a chat category meanwhile is caught; an unknown
  channel keeps its admission snapshot) and after it re-checked for
  deletion; either drops it with a refund and no model call. Whenever a
  waiter leaves the queue (dequeued, deleted or expired) the others' keycaps
  move to their current FIFO positions, each move waiting for the keycap it
  replaces to land. Deleting a waiting question
  refunds it; deleting a running one lets it finish (a dispatch is never
  cut) but posts nothing more and withholds both the question and its
  unposted answer from later context. Once a question is prepared, an armed
  `Held` guard exists before the state lock is taken; the reservation is
  taken last under that lock and recorded on the guard at once, so the
  guard concludes the question (refund, reservation settled) even if its
  future is dropped (a panic, including one while context is built,
  caught per question so the channel is handed over, or a shutdown abort).
  Shutdown refunds waiters, lets running answers finish within a grace,
  then cuts the rest (refunded, concluded, row `cancelled: serve shut
  down`), bounding the tidy-up and aborting what is left. The bot's managed
  role (`@Kanade` as a role mention) summons it like a user mention and is
  stripped the same way (`ToolContext::self_role_id`); `bot_names` are its
  user, global and guild names. A persona identity change forgets
  history. Proposals are recorded against the interaction id.
- Not here: strategy prefetch and source attribution (no boss-knowledge v2
  renderer outside `api`), and a model pre-screen.

### Lifecycle events (`Answerer::observe`)

The driver reports lifecycle facts through `Answerer::observe(&ChatEvent)`
(`src/chat/driver/events.rs`; default no-op). Events carry ids, counts and
classes only — never question or reply text, nor member ids;
`interaction_id` links to the chat-log row. Serve maps them to JSON log
lines in `src/runtime/serve/chat_log.rs`:

| Event | When | Log line |
| --- | --- | --- |
| `Admitted { interaction_id, thread, position }` | taken; `position` is the 1-based queue slot, `None` when it runs at once | `chat_admitted` (INFO) |
| `Ignored { reason }` | a summons not taken: `bot_author`, `disabled`, `not_ready`, `not_chat_category`, `no_pilot_role`, `staff_only`, `rate_limited`, `shed`; refusals about another guild or a DM are not reported | `chat_ignored` (INFO) |
| `Cancelled { interaction_id, reason }` | dropped after admission: `deleted`, `expired`, `not_admitted` (re-gate failed at dequeue), `not_ready` (`prepare` found no persona or route), `shutdown` (cut after the grace), `aborted` (the `Held` guard concluded a dropped future: a panic or abort) | `chat_cancelled` (INFO) |
| `Finished { interaction, generation, persona, model, reasoning }` | concluded after a model attempt | `chat_answered` (INFO) or `chat_failed` (WARN) with outcome, persona/profile, model, effort, route, rounds, tools, timings, clean retry and withheld |
| `SetupChanged { enabled, ready }` | the live `Setup` differs from the last reading (the first included) | `chat_setup_changed` (WARN when enabled but not ready, with `not_ready` causes) |

`observe` runs inline on the driver's path, so implementations must be
cheap and must not block or fail.

## Vectors and named differences

`tests/chat/{context,looping}.rs` replay `context` (6/58, exact) and `loop`
(12/22) through `answer` with the fake provider and the tool-round setting at
v4's 4. Named: `D-SHAPING` (sampled requests carry the runner's
`max_tokens`), `D-CLEAN-RETRY` (empty/malformed answers use the clean retry;
`D-NO-THINKING`: responses carry no reasoning text), `D-STRICT-TOOL-CALLS`
(folded into `D-CLEAN-RETRY`, one step: a reply with a duplicate call id or
non-JSON arguments is unreadable to the runner as a whole, where v4 renamed
the id or ran the call with `{}`), `D-USAGE-PAIRS` (a round's usage counts
only when both counts are integers), `D-TYPED-FAILURES` (governor/runner
error text) and `D-GROUND-FILTERED` (`read-then-grounded-answer` step 0's
reply shows only the run the model named; also `sanitize` case
`schedule-grounding` step 8). Tools-withheld rounds, the read-only turn and unoffered calls
replay as v4.
