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
  line) and the latest turns within `min(2500, window − reserve − system)`
  tokens, never below 256; the question always stays. `window` and `reserve`
  are the chat route's resolved context window and completion reserve
  (default 1024), not v4's fixed 2500-token prompt reserve
  (`prompt_budget`), so history assembly keeps more turns than v4 on the same
  window.
- `budgeted`: the whole request (turns, tool calls, the offered tools' JSON)
  plus the route-resolved completion reserve (`max_output_tokens`, not the
  v4 1024 constant) must fit the route-resolved window. Per request, in order:
  1. prior history (turns before the question) is dropped oldest first;
  2. only once no prior history is left, older tool-result contents are
     replaced oldest first with `[tool result elided for context budget]`.
     Contents are elided, never removed, so every assistant `tool_call` keeps
     its tool message; the latest tool round's results, the system prompt and
     the question are protected.
  The trim sticks for later rounds. When the protected material still
  overflows, the question fails with the typed
  `AnswerFailure::ContextBudget` before that round's request is sent (allowance
  refunded, outcome `error`, `error_code: context_budget`); the log row's
  error is `ContextBudgetError: chat request estimate E exceeds context budget
  W with completion reserve R`, and the member is told “Sorry — that question
  is too long for this model's context. Please shorten it and try again.”
  (`CONTEXT_BUDGET_REPLY`). If an earlier round of the question already
  posted a card, the reply is instead “The requested card was posted, but the
  request did not finish cleanly.” (the write-finishing line), so the member
  is not invited to resend a recorded change; the row still records
  `context_budget`. The driver also records the resolved window,
  reserve and source under the row's `guardrail.context`.
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
- `D-CROSS-CHANNEL-PROPOSAL` (deliberate v5 difference from v4): after the
  unchanged authority checks, a run-bound chat proposal reuses the oldest
  submitted, unexpired proposal for the same change in another channel,
  whether its source is extraction or chat. Equality is the same run and
  kind, plus the destination instant for move, or the member/answer set for
  RSVP; cancel needs only the run. Add and weekly tools have no target run
  and are unchanged. The scheduler's `propose_chat` uses atomic store
  lookup-or-create under SQLite's existing write transaction or memory's
  existing lock, including before card details are saved. A chat proposal
  without a `proposal_cards` row qualifies only while its age is less than
  `CARDLESS_CHAT_GRACE` (two minutes); at that boundary a new ask creates
  normally. Saved but unbound cards and extraction proposals retain the
  normal TTL/boss-week eligibility. Cardless proposals are not automatically
  discarded on timeout or question deletion: they remain actionable in the
  admin Inbox, and lifecycle cleanup is deferred. Reuse creates no draft/card
  and leaves the kept proposal unchanged, but still retires other proposals
  under the normal scope: this channel, or all channels when asking from the
  run's home channel. Retired ids travel separately from new cards through
  the dispatcher; the answer loop awaits `CardDesk::refresh_proposals` via
  its port before checking the post-dispatch deadline. Its
  successful tool result names the existing proposal and channel, supplies
  the Discord jump link when bound, otherwise says the card is still being
  posted, and tells the model it is already proposed and awaiting approval,
  not done. The jump link may name a channel the asker cannot see; authority
  remains on the run, without an additional channel-visibility check.
  Same-channel superseding and different-target cross-channel
  proposals keep their old behaviour. Closed proposals, elapsed TTLs and
  past boss weeks do not block creation. Covered by
  `tests/chat/proposal_dedupe.rs` and the shared proposal-store conformance
  suite; frozen v4 vectors remain unchanged.
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
- `get_schedule` forward read (`D-AUTO-FORWARD`, user decisions 2026-10-03):
  `week:"auto"` with no `day` lists the non-cancelled runs that are not over,
  from now to the end of the current boss week (so early next calendar week
  counts, the next boss week does not), earliest first, under the usual
  `MAX_RUNS`/reply bounds, whether or not the question asked for upcoming
  runs. Headings and empty replies say "this boss week" ("Your 2 upcoming
  runs this boss week", "No upcoming runs for you this boss week."); the
  "already done" note reads the member's runs since the earlier of this
  calendar week and this boss week. A plain singular "next run" question
  (`ScheduleDefaults::next_only`: the trusted question holds
  `\bnext\s+(?:boss\s+)?run\b` and, besides it and "for me", only words from
  a small whitelist such as when/what's/is/my/our/the/in here/this channel;
  any boss, day, "after", "and", count, mention, other name or punctuation
  opts out, because `get_schedule` cannot filter by them, and "next runs" or
  "next week" never match) keeps only the soonest upcoming run
  after scope and participant filtering, headed "Your next run" ("Next run"
  for the group; an explicit period is named, e.g. "next boss week"). The
  `week` description tells the model to use it for "next run" asks (+47
  estimated tokens on the full-set surface, +48 read-only). Explicit
  `this`/`next`, `this_boss`/`next_boss` and `auto` with a day keep their
  periods.
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
  inserts the tool's full listing as v4 did. A conversational sentence that
  names a run by id with a schedule fact (not a `[id]` record line or
  `**day — boss**` pair, and ending a sentence with `.`, `!`, `?` or `…`;
  user decision 2026-10-03) is kept as written and the records go after its
  paragraph, set off by blank lines; when several sentences or retold
  records name runs, the records still appear once, at the first of them,
  and retold record lines are replaced as before. List-style retellings
  without sentence punctuation (`Boss - 21:30 - run ID 'id'`) are replaced
  as v4 did. A sentence without a listed id is kept the same way when it
  names a run by date and time (user decisions 2026-10-03, "fact-check prose
  in place" and "Fail-closed rewrite"): the sentence is kept only when every
  fact-like token in it was read and matches the one run it picks; anything
  left over falls back, so more persona prose is lost rather than a wrong
  fact kept. It must state exactly one `HH:MM`, which must pick exactly one
  listed run, narrowed by a stated `DD Mon`/`Mon DD` date, else by a stated
  weekday, else unique among the listed times, else by the boss it states.
  Read and compared with that run: the time, every date and weekday, every
  tally (`n/m`, `n of m`, `n out of m`, with a following `have said yes`),
  every status (including bare `done`, `at risk`/`at-risk`, and
  `own time`/`own-time` as `otot`) and every `<#id>`/`[#id]` channel. Any
  digit in any script that no check read falls back (`the 7th`,
  `2026-10-07`, `all 3 said yes`, a full-width `０７`), as does an answer
  word (`yes`, `rsvp`, `answered`, `declined`, `signed up`…) outside a read
  tally, and so does any relative day (`today`, `tonight`,
  `tomorrow`/`tmr`/`tmrw`, `yesterday`, `this`/`next`/`last`/`coming` week,
  weekend, weekday or part of day, `in N days`; grounding has no clock), any
  negation (`not`, `n't`, `never`, `cannot`, `no longer`), any am/pm marker
  (`9:00 pm`, `a.m.`; 12-hour times are not converted), any `#name` channel
  (the listing has only `<#id>`), any bare 8-hex id, and any count, date
  or status in words: number words (`one`…`twenty`, tens, `hundred`,
  `half`, `both`, `few`…), ordinals (`first`…`thirty-first`), quantifiers
  (`all`, `everyone`, `nobody`, `no one`, `none`…), `un`-statuses
  (`unconfirmed`…) and status synonyms (`called off`, `postponed`,
  `rescheduled`, `moved`, `finished`, `completed`, `cleared`, `over`,
  `happened`, `ended`, `delayed`, `pending`…). Bosses are checked
  against the guild's boss catalog (`GuildView::catalog`, handed to
  `shape_reply`; user decision 2026-10-03 "Use the boss list"): every boss
  the sentence names, in any case, by short or full name, alias or prefixed
  form, must be one of the run's bosses (a `+`-joined or multi-word label is
  read through the catalog); every difficulty word must start a
  `difficulty + boss` phrase, and every difficulty letter or `hm`-style
  shorthand before a boss (`N Carling`, `N-Carling`, `HM Carling`) and every
  prefixed token (`ncarling`) must be the run's difficulty for that boss (`Normal Carling` on a Hard Carling run
  falls back, and so does `hard to say`); every `**bold**` span must be in
  the run's label or name only its bosses. Catalog aliases that are everyday
  words or names (`will`, `lot`, `star`, `bell`, `bella`, `carl`, `karl`,
  and `climb`/`clot` as prefixed forms) are bosses when capitalised, after
  a difficulty word, in bold or inside a longer name, so `Karl` or
  `Hard Will` is a boss while lowercase "will" or "a lot" is not. Member and
  persona names, possessives and timezones are not facts and stay. Known
  gaps, where a wrong fact can still be kept: a plain, non-bold name of a
  boss that is not in the catalog; a lowercase everyday-word alias, which is
  not read as a boss (`star is on tue 06 oct at 20:00.` is kept on a Lotus
  run); who is on the run (member names are not compared with the roster);
  and any status, count or channel phrased in words outside the lists above
  (`scrubbed`, `in the carling channel`). A failing sentence, an ambiguous or repeated time, or any other
  id-less fact line in the reply leaves the reply on v4's hint rule (the
  span from the first to the last hint line is replaced by the full
  listing). A record-shaped line whose
  `[id]` no tool output carries is dropped (never with a real run's line)
  whenever real runs are named, all of them included; the listing then goes
  at the first real run. With no run named, v4's full listing stands (a
  reply with code but no schedule text keeps its text, listing appended).
  Grounding matches v4 exactly only for a reply without fenced code,
  invented record lines or run-naming sentences that names every run or
  none. `D-GROUND-WRITE`
  (user decision 2026-10-02): a turn whose last write call posted its card
  is not regrounded at all, so the model's "card is up, needs a ✅" reply
  posts as written; v4 regrounded it, and a time such as `22:00` in that
  reply pulled in the turn's earlier `get_schedule` listing instead. Over the
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
  The driver's delivery (see "Delivery" below) posts the parts in order,
  all pinging nobody; a part that does not land ends the answer with the
  incomplete marker, without failing or retrying the answer. A fence is reopened with ```` ``` ````
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

Token usage (schema v19, user decision 2026-10-01): each round row records
the provider-reported `prompt_tokens`/`completion_tokens` of its response
(both or neither; unset when the response carried none) and
`prompt_estimate`, the request estimate `context::budgeted` fitted (messages,
tool calls and offered schemas, without the completion reserve), the clean
retry included. The interaction totals stay the sum of the rounds' reported
pairs (`D-USAGE-PAIRS`). Pinned in `tests/chat/answer.rs`.

Response reasoning (schema v21, 2026-10-02): each answered round retains
`reasoning_content` and `reasoning_tokens`, distinct from the requested effort
in `reasoning`. Text is capped at 64 KiB **including** the visible
`… [reasoning truncated]` marker, cut at a UTF-8 boundary; empty text is NULL.
The independent provider count is NULL when unreported, never inferred as zero.
It is recorded separately without adding it to completion totals. Both fields share
the row's 90-day retention and are never added to conversation messages.

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
- **Staging line.** `pilot::staging_line(text, catalog, staging)` is v4's
  `placeholder_for` over `route_strategy_intent`, pure: a strategy cue
  naming one to three bosses resolved only through the catalog →
  `guide_named_for` with their short names joined by `, ` (an unsafe or
  over-budget render falls back to `guide`); any other strategy cue →
  `guide`; then v4's write and schedule hints; else `generic`. `staging` is
  the compiled persona's `staging_lines()`. Named deviation
  `D-STAGING-WORD-CLASS` (`tests/chat/staging.rs`). The driver posts it as
  the silent placeholder (see "Delivery").
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
  `Answerer` for the model side and `Surface` for reactions and message
  effects),
  composed in `runtime::serve::chat`. `ChatDriver::start` validates the
  timeout and reloads withheld ids before the driver exists. `offer` gates
  (the live `Setup`), then `Traffic::admit`s: answer now, queue (keycap
  position reaction) or shed (💬, refund); a spent budget gets ⏳, the
  limited reply and its row. A channel's worker reserves the clean retry
  when it dequeues a question, answers and delivers it (see "Delivery")
  and then `conclude`s with that reservation under the state lock
  (`conclude` sees the already-delivered result), then records the row.
  Before `prepare` a dequeued question is re-gated (chat still on, channel
  still in a chat category, re-read from the live channel directory so a
  channel moved out of a chat category meanwhile is caught; an unknown
  channel keeps its admission snapshot) and after it re-checked for
  deletion; either drops it with a refund and no model call. Whenever a
  waiter leaves the queue (dequeued, deleted or expired) the others' keycaps
  move to their current FIFO positions, each move waiting for the keycap it
  replaces to land. Deleting a waiting question
  refunds it; deleting a running one lets it finish (a dispatch is never
  cut) but delivers nothing more and withholds the question, its unposted
  answer and every delivered part from later context. Once a question is prepared, an armed
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
- Strategy guides: with `KANADE_KNOWLEDGE_DIR` set, `get_boss_strategy`
  answers from the startup-validated schema v2 directory (shared with the
  admin API, re-read per call) through `tools::read::render_guide`: v4's
  `render(include_sources=False)` shape, with per-difficulty facts and
  letter-keyed `difficulty_notes` under `### <Difficulty>`. A checked-in
  document that can no longer be read or rendered (edited after startup)
  answers "could not be read right now" (`GuideError::Unreadable`, distinct
  from the absent-guide text) and logs `chat_guide_unreadable` with the
  path and problem. Without the directory, live chat removes the
  `strategy` bundle (`ToolOffer::disallow`): `get_boss_strategy` is never
  offered and `request_tools` neither advertises nor accepts `strategy`.
  `D-SEASONAL-LIST` (tests in `tests/chat/strategy_guides.rs`,
  `tests/chat/tool_schemas.rs`, `tests/chat/context.rs` and
  `tests/chat/looping.rs`): `list_bosses` preserves its catalog block and
  appends key-ordered seasonal guide bosses with event name, availability and
  aliases when present; it adds no section without event guides. The tool
  descriptions also direct the model between the listing and guide lookup;
  their added length raises each full-set surface estimate by 49 tokens, which
  is named in the context and loop vector replays.
  Full-set (v4) schema shapes and key order remain; these named descriptions
  differ. Dynamic surfaces with every bundle available use the same schemas.
- Not here: strategy prefetch and source attribution, and a model
  pre-screen.

### Delivery (`chat::driver::delivery`, R05)

One `Delivery` per question, run by the channel's worker; its
`DeliveryRecord` is in memory only and shared with the `Held` guard.
`deleted` and the shutdown cut only set flags and wake the worker; they
never issue Discord effects.

- **Staging.** After `prepare` (queued questions show only their keycap;
  shed and limited ones never get a placeholder) the worker posts the
  persona's staging line (`pilot::staging_line` over
  `staging_lines()`) as a silent reply to the question, mentions none.
  Not sent / rate limited: one retry, unless the question was deleted or
  cut meanwhile (then unstaged, `create_rejected`). Refused: unstaged. Ambiguous:
  unstaged and never re-created. While the answer runs, typing triggers
  every `TYPING_EVERY` (8 s) from the worker's own select loop, from the
  placeholder's outcome until the answer returns; failures are ignored and
  never retried, and one in flight when the answer returns is dropped.
- **Answer.** Text is the reply or the failure line (a failure is a
  one-part answer, I5), split by `reply_parts`. With a live placeholder,
  part 1 is edited into it (posted id = the placeholder's): Unknown
  Message → part 1 posted as a new reply; another refusal → a new reply,
  then the placeholder deleted; ambiguous → one identical retry, then
  treated as landed (`unknown: [1]`, D2) and never deleted or rewritten.
  Without one, part 1 is today's reply (audible, `reply_to`). Parts 2..n
  are silent, reply to nothing and post only after the previous part
  landed (I4); not sent → one retry; refused or ambiguous → ending (an
  ambiguous create is never replayed, I2).
- **Ending.** ` *(reply incomplete)*` (`INCOMPLETE_MARKER`) is appended by
  editing the last confirmed part; when that would exceed 2000 UTF-16
  units, or no part was confirmed, `*(reply incomplete)*` is one silent new
  message (a reply to the question only when nothing landed), never
  replayed. Named deviation `D-DELIVERY-MARKER`: the only bytes added to an
  answer's text; delivered parts are otherwise byte-identical to today's
  reply and follow-ups.
- **Deletion.** Waiting or preparing: refund, no placeholder. Staging in
  flight: awaited, then deleted if it landed. Staged: deleted at once
  beside the running answer. Answer returned, first effect not started:
  re-checked right before it, then withdrawn; likewise when part 1's edit
  was refused or found the placeholder gone (withdrawn, not incomplete, no
  marker). Editing or later: what landed
  stays, later parts stop, the marker is added (D4). Withdrawal: Unknown
  Message counts as done; one retry when not sent or ambiguous, else
  `orphaned`.
- **Shutdown (D5; user decision 2026-10-01).** A cut is observed only
  while the answer is still running: it turns a live placeholder into the
  failure line (nothing is posted without one). Mid-delivery a cut does not
  end the answer: once the answer has returned, its remaining parts keep
  posting within the cut budget, with no incomplete marker for the cut, and
  only the final hard abort stops them. The hard abort drops the effect in
  flight (recorded ambiguous), posts no marker, records the row
  `incomplete: delivered k of n parts` with `cause: shutdown` and anchors
  the first delivered id.
- **Held drop.** A panic or abort with a live placeholder, no effect in
  flight and no answer effect started: a spawned task withdraws the
  placeholder if the question was deleted (same retry rules), else edits it
  into the failure line (nothing that can panic runs while unwinding).
  Otherwise no Discord effect; the row is built from the record. Its error
  starts like a normal conclusion's: `cancelled: the question was deleted`
  when deleted, `cancelled: serve shut down` when the cut came before the
  answer returned, `failed: the question stopped unexpectedly` only for a
  genuine abort (a panic) that delivered nothing; the incomplete note is
  appended whenever parts are missing. A deleted question's delivery cause
  is `deleted` (deletion wins over `shutdown` and `aborted`), matching its
  row error.
- **Row.** `guardrail.delivery` always carries `placeholder`
  (`none|edited|deleted|create_rejected|create_ambiguous|orphaned`),
  `parts` and `delivered`; `unknown` (1-based parts whose landing is
  unknown), `incomplete: true` and `cause`
  (`rejected|ambiguous|deleted|shutdown|aborted`) appear only when set. An
  incomplete answer appends `incomplete: delivered k of n parts` to the
  row error after any cancel string (`; `-separated). An aborted question
   with delivered parts never reads `failed: the question stopped
   unexpectedly`.

### Rejection follow-up (R05)

A successful ❌ on a proposal card may start one read-only clarification. It is
not a normal admission: it spends no allowance, takes no clean-retry
reservation and is dropped rather than queued. The reaction worker supplies
only stored card details and proposal source ids; it never reads card text.

- It is silent unless chat is enabled and ready, the card channel passes a
  fresh `is_chat_channel` check, every successfully rejected proposal has
  `ProposalSource::Chat`, and every source chat row names the reactor as its
  author. Removed, repeated, unauthorised, already-closed and
  extraction-sourced ❌s therefore do nothing.
- A channel has one in-memory monotonic 30-second cooldown. A follow-up is
  also dropped when that channel has an answering or waiting question (or an
  earlier rejection follow-up still runs). It runs as a driver-tracked task,
  so chat shutdown drains or aborts it with the driver's other tasks. An armed
  guard spans preparation through row persistence: a model/Discord panic or
  hard abort releases the channel slot, records `kind: rejection_followup`
  from the shared `DeliveryRecord` (including incomplete/unknown delivery),
  and changes an idle staged placeholder into the failure line. During panic
  unwinding it defers that work to a tracked task and retains a release-only
  fallback.
- The prompt is the v4 `followup.prompt` wording, built from stored summaries,
  card party ids resolved to roster names (never mentions), and the asking
  member's roster name. The synthetic prompt is sent only to the model;
  `ToolContext::read_only` both withholds write tools and refuses a remembered
  write call with `READ_ONLY_TURN`.
- Delivery reuses `Delivery` with the persona's `generic` staging line: a
  silent placeholder replies to the card message, is edited into part one,
  and continuations are silent and unreferenced. Every effect mentions nobody.
  Its row carries normal `context` and `delivery` guardrails plus
  `guardrail.kind = "rejection_followup"`.
- Once a visible reply lands, only that assistant turn is remembered and
  assistant-anchored. No synthetic scheduler note, personal memory, focus
  card or ping is added to channel context; replying to that visible answer
  still re-anchors it.

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
`D-NO-THINKING`: the frozen replay adapter omits legacy thinking; production
now records Kanata's response-only reasoning without changing the vectors), `D-STRICT-TOOL-CALLS`
(folded into `D-CLEAN-RETRY`, one step: a reply with a duplicate call id or
non-JSON arguments is unreadable to the runner as a whole, where v4 renamed
the id or ran the call with `{}`), `D-USAGE-PAIRS` (a round's usage counts
only when both counts are integers), `D-TYPED-FAILURES` (governor/runner
error text), `D-CONTEXT-BUDGET-REPLY` (`context-budget` step 1 replies
`CONTEXT_BUDGET_REPLY` where v4 stayed silent, no card having been posted; R01, user decision
2026-10-01), `D-CONTEXT-BUDGET-RESERVE` (the budget error names its resolved
reserve: `… with completion reserve 1024`, in `loop` `context-budget` step 1
and `context` `request-budget-trims-prior-history` step 3) and
`D-GROUND-FILTERED` (`read-then-grounded-answer` step 0's
reply shows only the run the model named; also `sanitize` case
`schedule-grounding` step 8). Tools-withheld rounds, the read-only turn and unoffered calls
replay as v4.
