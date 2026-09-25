# Chat orchestration

Status: `src/chat/context/` and `src/chat/answer/` (slice C2). v4 reference:
`bot/chat/agent.py` (`ChatPilot.build_conversation`/`assemble`/`generate`/
`_loop`/`_chat`/`_budgeted_messages`). Gate, tools and reply hygiene are C1
(`src/chat/{gate,authority,tools,sanitize}`); the pre-screen, pollution
containment, persona-voiced failure lines, queueing and allowance refunds
are C3. Discord wiring is later.

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

## The question loop (`chat::answer`)

- `answer` opens one identity session (`identity::open_session`, so an
  external route fails closed while pseudonymization is off) and one governed
  question session (`ModelClient::open_question`: one permit for every
  round, `tool_rounds` + 1 requests, the timeout bounding the whole
  question, one requeue). Every conversation message, tool result and the
  voice reminder is encoded through the identity session; tool arguments are
  decoded by the dispatcher; the final reply is decoded (an unknown token is
  a malformed answer).
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
  bounds (`sanitize::shape_reply`).
- Failures are typed (`AnswerFailure`) with their allowance charge; timeouts
  read v4's `no answer within Ns`.

## Chat log

`answer::interaction` builds one `chat_interactions` row with a
`chat_rounds` row per model request (alias, reasoning, finish reason,
latency, bundles offered, tools called with outcomes, response text; never
the prompt). Outcomes: `answered`; `refused`/`clarified` when a write was
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
- Not here: strategy prefetch and source attribution (no boss-knowledge v2
  renderer outside `api`), and a model pre-screen.

## Vectors and named differences

`tests/chat/{context,looping}.rs` replay `context` (6/58, exact) and `loop`
(12/22) through `answer` with the fake provider and the tool-round setting at
v4's 4. Named: `D-SHAPING` (sampled requests carry the runner's
`max_tokens`), `D-CLEAN-RETRY` (empty/malformed answers use the clean retry;
`D-NO-THINKING`: responses carry no reasoning text), `D-STRICT-TOOL-CALLS`
(folded into `D-CLEAN-RETRY`, one step: a reply with a duplicate call id or
non-JSON arguments is unreadable to the runner as a whole, where v4 renamed
the id or ran the call with `{}`), `D-USAGE-PAIRS` (a round's usage counts
only when both counts are integers) and `D-TYPED-FAILURES` (governor/runner
error text). Tools-withheld rounds, the read-only turn and unoffered calls
replay as v4.
