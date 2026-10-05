# Limits

**Purpose:** the Kanata gateway's model groups: permits, queues, admission
refusals, breakers, windows, per-member allowances. Polled every 5 s.

## As built

Code: `apps/admin/src/limits/LimitsPage.svelte`, `limits/permits.ts`;
`_limits.scss`, `evidence.scss`.

- Unmounted route (404): one window centring the `B_Limits` unavailable state
  ("This isn't available on this server yet", key link to Config → Models);
  polling stops. Pair: limits. Use this pattern for any admin route the API
  has not mounted `[lesson]`.
- Live page today: pre-M3E layout inside `Tabs` **Backends n / Queue n /
  Admission n / Allowances n** (`LimitsPage.svelte:53`), one `WavyProgress`
  permit bar per group, wavy only with requests in flight, at most two waving
  (`wavingGroups`, `MAX_WAVES = 2`), `fullWave` at 100 % in flight.
  `[DR 2026-10-04]`

## Planned: live Limits M3E (boards approved 2026-10-05)

**Boards:** `B_LimitsLive`, `B_LimitsQueue`, `B_LimitsAdmission`,
`B_LimitsAllowances`, `B_PhoneLimits`, `B_PhoneLimitsAdmission` (round 2
approved); `B_LimitsLiveOne` (single-group variant, pending approval). PNGs in
`docs/research/2026-09-28-m3e-mockups/png/`. Not implemented yet.

Decisions `[DR 2026-10-05]`:

- One card per model group (permits, wave, breaker, waiting calls); Backends
  becomes full-width row cards.
- Keep a **Capacity in Config** title-bar pill.
- Refusals appear **only** in the Admission tab (not on cards, not in the footer).
- Footer "Updated <server time> · every 5 s" from a new `generated_at` on
  `GET /api/admin/limits` (Rust DTO + mock + api-types). Full server times,
  never shortened.
- Phones: open / half-open breaker groups sort first; tabs keep all four
  counts and scroll sideways with edge fades. The Limits lane implements the
  scrolling tabs per this decision; the phone boards were fixed to match
  (2026-10-05).
- A dev-only pwa-mock seed for model groups so fidelity shows three groups and
  two waves.
- Keep `WavyProgress`, the two-wave cap and `fullWave`.
- Real art wherever the screen shows bosses.
