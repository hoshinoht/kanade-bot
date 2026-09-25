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
  `NO_ROUND_LEFT`, adds nothing and is not charged. The last round, and the
  round after a posted card, offer no tools and are still sent. A round with
  no tool calls ends the loop; running out of rounds is `KeptCallingTools`.
- Deadline: the question session's deadline also bounds each tool call's
  store load, `ChatPorts::pending`, the dispatch (proposals included) and
  `ChatPorts::post_card`, as v4's `wait_for` bounded the whole loop. Expiry
  ends the question as `Timeout` (`timeout` in the log); proposals created
  before it are still reported.
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
`rate_limited` and `withheld` are recorded by the caller (gate, pre-screen).

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
