# Extractions

**Purpose:** each extraction model call: what it read, what it proposed, the
prompt and raw response; re-read channels.

**Boards:** `B_Extract`, `B_ExtractPrompt` (pairs: extract, extract-prompt).

## As built

Code: `apps/admin/src/extractions/` (`ExtractionsPage`, `ExtractionPage`,
`CallList`, `CallOutcome`, `ExtractionDetail`, `CodeViewer`, `RescanPanel`,
`code.ts`, `progress.ts`), `logs/`; `_extract.scss`.

- One window "Calls" with search, Filters (n) and **Re-read channels** on the
  title bar; Re-read is a popover that stays mounted so a running job keeps
  its card and progress (`ExtractionsPage.svelte:141`).
- List-detail (G5): list 360 px (`_extract.scss:91`), detail at
  `/extractions?call=`; below 900 px one at a time with Back.
- Detail pill tabs **Changes n / Chat read n / Prompt / Raw**
  (`ExtractionDetail.svelte:36`); thread with one-line messages + outcome
  card; Prompt/Raw in the code viewer (line numbers, Wrap on by default,
  find, Copy; raw JSON pretty-printed); "masked" chip only on historical
  pseudonymised rows. Reasoning block and token counts as in Chat.

## Phone

Title bar and prompt bar stay on screen (`e2e/phone-fit.spec.ts`).
