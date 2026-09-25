# Limits contract (proposal)

Status: proposed; the backend confirms or revises. DTOs:
`web/packages/api-types` (`BackendGroup`, `Refusal`, `Allowance`, `Limits`,
`AliasLimit`, `KeyLimits`). UI: `web/apps/admin/src/limits/LimitsPage.svelte`
(live usage, polled every 5 s) and the Config Models section (declared
capacity, validated on save). v4 equivalent: `GET /limits`,
`GET /limits/live`, `GET /limits/events` (SSE), `POST /limits/windows/{id}/reset`,
`POST /limits/overrides`, `POST /limits/overrides/{id}/clear` — SSE is
replaced by polling; overrides are not yet in the PWA.

## Admission model

Kanata admits per route (alias + operation); Kanade declares capacity per
backend group and validates it against what Kanata publishes:

- `AliasLimit`: `{alias, max_in_flight, adapter_max_in_flight?,
  source: 'published' | 'declared'}`. `max_in_flight` is the route cap from
  `/v1/models` `kanata.admission` metadata, or the operator's declaration
  when Kanata publishes nothing for the alias. `adapter_max_in_flight` caps
  every route on the adapter.
- `KeyLimits`: `{max_in_flight, shared}`. The deployment's key bounds the
  governor's summed concurrency across all groups; the key is shared with
  the owner's other clients, so its limits are sized for both. Per-key
  limits are always operator-declared: Kanata never publishes them.
- Published admission is `kanata.admission` `{max_in_flight, max_queue,
  queue_ms, adapter_max_in_flight}` on Kanata's private listener only (the
  public listener and plain Ollama publish none). The provider parses it into
  `ModelCapabilities::admission`; `GovernorConfig::capacity_warnings` flags a
  group whose permits exceed min(`max_in_flight`, `adapter_max_in_flight`)
  for any of its aliases.
- Rule: each group's N ≤ the minimum over its aliases of
  min(route max_in_flight, adapter_max_in_flight). An alias belongs to
  exactly one group, listed once; an alias Kanata does not list is refused,
  and so is a grouped alias with neither a published nor a declared limit
  (declare one first). A row's permits are a whole number from 1 to 64;
  anything else (including a cleared input, sent as `null`) is refused with
  422 naming the row, never clamped. An absent `adapter_max_in_flight` caps
  nothing. A role's model with no group only warns on save; the governor
  then refuses that role's model calls (it never runs ungoverned).

## Live model

- `BackendGroup`: `{name, backend, models[], permits: {in_use, total},
  queue: [{position, kind, who, waiting_s}], rate: {available, capacity,
  refill_per_min}, retry: {remaining, capacity}, breaker: {state, failures,
  since, retry_at?}}`.
  - `breaker.state`: `closed` (calls flow), `half_open` (probing), `open`
    (calls refused). `retry_at` is set only while probing is scheduled.
  - `queue[].position` is 1-based and dense within the group; `kind` is the
    call kind (`extraction`, `chat`, `rewrite`, …); `who` is a human
    description (a boss-week run, a member), never a prompt.
- `Refusal`: `{kind, scope, target, count, last_at}`. `kind` is `rate`,
  `concurrency`, `quota`, `key_rate`, `key_quota`, …; `scope` is `group`
  (one backend group) or `key` (the deployment's Kanata key); `target`
  names the group or key. Counts accumulate over `admission.window`
  (a human string such as `"the last hour"`), which the server chooses.
- `Allowance`: `{member, staff, allowance: {count, per_s} | null, used,
  override}`. `allowance` null means the member is on the guild default;
  `override` marks a per-member allowance set by an admin.

`GET /api/admin/limits` returns all three sections at once (`Limits`):
`{groups, admission: {window, refusals}, allowances}`. Empty arrays — not
nulls — when there is nothing to show.

## Operations

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/limits` | — | `Limits` | Polled; cheap enough for a 5 s cadence. |
| `DELETE /api/admin/limits/windows/{id}` | — | `{message}` | Clears one member's answer window (`{id}` is the member id, URL-encoded). v4 `POST …/reset`. |

## Open questions for the backend

1. Override editing (`POST /limits/overrides`, `POST
   /limits/overrides/{id}/clear`) and the member-facing allowance view are
   not in the PWA yet; confirm the shapes above still allow them.
2. Confirm `DELETE` (not v4's `POST …/reset`) for clearing an answer window.
3. Confirm `admission.window` stays a display string rather than a
   machine-readable range.
