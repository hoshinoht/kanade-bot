# Chat

**Purpose:** the chatbot's interactions: what was asked, what it said, how
(tools, model rounds), what it produced. Budget: result rows > filter bar >
summary line. `[guide]`

**Boards:** `B_Chat`, `B_ChatTrace` (pairs: chat, chat-trace).

## As built

Code: `apps/admin/src/chat/` (`ChatPage`, `ChatList`, `ChatTurn`,
`Conversation`, `ToolTrace`, `ModelTrace`, `ModelView`, `ModelStats`,
`transcript.ts`), `logs/` (`LogFilters`, `filters.ts`, `Reasoning`,
`TokenUsage`, `LogTime`); `_chat.scss`.

- Page line: "Chat · n interactions" + `ModelStats` chips for the two busiest
  models (hidden rather than cut when the line is too narrow) and a
  "+n models · e errors" disclosure with the full table. `[web/AGENTS]`
- List-detail in one window (G5): list `clamp(280px, 36%, 400px)`, detail at
  `/chat/:id`; below 900 px one at a time with a tagged history entry so Back
  returns to the row. Filters server-side through the query string in a
  popover; Dates (range picker) in the filter row. `[DR 2026-10-04]`
- Rows: question + took; who · time · model · outcome chip; `in → out`
  tokens (+ reasoning count) `[DR 2026-10-01]`, `[DR 2026-10-02]`.
- Detail pill tabs **Conversation / Tool trace n / Model trace n / Produced n
  / Raw** (`ChatTurn.svelte:32`); Markdown/JSON + Copy transcript; collapsed
  "Reasoning · n tokens" block. Only historical masked turns show a Model
  view.

## Known gaps / Planned

- **Planned:** a "profanity" outcome and filter; the turn names the matched
  word, side and line sent instead. `[DR 2026-10-05]`
