# Extraction vectors

Frozen outputs of the v4 `bot.extract` oracle (plus the card formatting it
uses), for the Rust extraction port. The generator
(`scripts/v5_vectors/extract` in the v4 tree, git history up to `487c4ed`)
wrote every `<family>.json`, its `<family>.schema.json` and `index.json`; it
is gone with v4, so these files are frozen. Never hand-edit any of them. This
README is the only hand-written file here, apart from the v5 set below.

## Discipline

- Inputs are serialized, JSON-round-tripped, then replayed; expected values
  come only from the oracle. Each case is `{case_id, input: {..., steps}}`
  and each step result is exactly `{value}` or a declared `{error: {type,
  message}}` (`error_type` on the step names the only class captured).
- Replay applies `$defs/replayCase` before calling v4; the document gate
  pairs every value with `$defs/result_<op>` and every error with its
  declaration. Instants must carry an offset (pattern-checked).
- Nothing reads the wall clock: `now`, anchors and `clock` are always
  explicit. `commit` uses one clean in-memory `Repo` per replay with the
  scheduler vectors' seams (pinned clock, `set_clock`, deterministic UUID
  sequence whose exhaustion fails generation). No network, Discord or model:
  the model is a scripted client. Fixtures are synthetic (short digit user
  IDs, synthetic UUIDs, a six-boss public-name catalog rebuilt through
  `BossTable.from_dict`).
- Tests (`tests/test_v5_extract_vectors.py`) replay every case twice from
  JSON, regenerate twice into temp dirs and compare bytes, and check drift,
  tampering, the input gate and provenance.

## Families

| family | cases / steps | oracle |
| --- | --- | --- |
| `gate` | 6 / 72 | `find_bosses` (exact, spelled-out and letter prefixes, fuzzy ≤1 edit only after a prefix, stopwords, invalid difficulty → `canonical: null`), `canonical_bosses`, `find_times` (ranges, meridiem, compact, `at 11`, masked channels/prices/IDs/URLs/emoji, years), `find_days`, `find_mentions`, `explicit_rsvp`, `evaluate`, `should_extract`, `pipeline.urgent` |
| `window` | 3 / 32 | `group_bursts` (3 h gap is inclusive), `group_for_rescan` (local day, cap, longest-pause split, central tie-break), `split_until`, `clamp_window`, `window_since` (boss weeks vs hours), `previous_week_start`, `should_widen` |
| `resolve` | 4 / 63 | `parse_clock` (bare 1–11 → pm, bare 12 → midnight, range start, noise words, refusals) and `resolve` (relative days, weekdays, `next`, ISO dates, soon-words rolling, explicit days never rolling, weekday rolling a week; one `America/New_York` DST case) |
| `match` | 3 / 33 | `match_run` (boss ∩ participant scoring, `no-boss-overlap`, single run, guild-wide fallback, ties → `ambiguous`, hints ≥4 chars that must share a named boss, dead statuses), `runs_spanned`, `reachable`, `needs_run` |
| `merge` | 4 / 10 | `merge`: latest explicit value per field, unions, max confidence, rsvp keyed by person, time changes folded into an `add`, one time carried across moves to the same day |
| `prompt` | 4 / 23 | exact `SYSTEM_PROMPT` and user-prompt bytes (channel, NOW, narrowed/full boss table, roster, runs incl. own time, fixed, earlier/new messages, guild fallback), `relevant_roster`, `named_bosses`, `json_schema` (`text` is the exact compact serialization in v4 key order; `schema` is the same object), `json_instruction`, `extraction_body` per capability set, token estimates and budget |
| `parse` | 5 / 30 | `parse_response` accept/coerce/reject, and `Extractor.extract` over a scripted gateway: first-answer acceptance, one retry carrying `RETRY_INSTRUCTION`, quarantine (`ok: false`, `error`, no extraction) after a second bad answer, timeouts, transport errors, missing model alias (`misconfigured`), and every request body sent |
| `plan` | 8 / 14 | `plan_burst` from a raw scripted model response: boss normalisation, injected explicit RSVPs, merge/resolve/match, split across runs with volunteers, `move`/`split` → `add` conversion, later-week filtering, drop reasons (confidence floor, no run, ambiguous, stated add without day/time, already passed, already scheduled), `inherit_from_run`, payloads, `one_per_run`; `consolidate` across rescan bursts |
| `commit` | 10 / 120 | `may_commit`; `commit` for every kind (applied effects on runs, reminders, RSVPs, fixed timings; exact refusal texts), sibling `supersede` on commit, channel-scoped `supersede`, `reject`, `expire_stale` at the 24 h boundary; fixed create/adoption/edit/remove with the live `materialise_weeks` callback |
| `cards` | 4 / 35 | `when_text`, `proposal_line` per kind (incl. weekly create/edit/remove, audience rendering, `also_mentioned`), `card_kind`, `Pipeline._unanswered`, `proposal_card` (one card for a burst), notices |

## v5 set (no v4 oracle)

`cards_redesigned.json` and its schema sit beside the frozen families but are
not generated from v4 and not listed in `index.json`: they pin the redesigned
proposal card (`kanade::bot::cards::styled_card`, message style
`redesigned`), 8 cases / 19 steps covering the open card (written labels,
difficulty marks, no audience), applied, rejected, superseded and out of
date, the suggestion and weekly-timing colours, every change kind and
multi-change cards. The classic card stays on `cards.json`, byte-exact.
`tests/extract/cards_redesigned.rs` replays it; never regenerate it to absorb
a change. `KANADE_PRINT_GOLDEN=1 cargo test --all-features --test extract
cards_redesigned -- --nocapture` prints each replayed value for review.

## Portability notes

- Rejection messages are Python's. `parse_response` records `ValueError`
  text verbatim (`not JSON: <json.decoder message>`, `expected a JSON object,
  got <type>`, `the model returned an empty response`); only the envelope is
  portable. Schema violations are recorded as pydantic `loc`/`type` pairs,
  never pydantic's text. The one exception is `extract_call`'s `error` after
  a schema violation, which is pydantic's rendered message (it is what v4
  logs and sends back in the retry); a port should compare `ok`, `attempts`
  and the retry structure there, not that text.
- Coercions are v4's, oddities included: `"   "` is not "empty", an object
  where `bosses` expects a list becomes `["{}"]`, `is_question: "maybe"` is
  `false`, a percentage confidence is divided by 100.
- `commit` does not check the amendment's status: the `late` case commits a
  proposal `expire_stale` has just expired. The live client guards that before
  calling `commit`; the vector records the oracle as it is.
- Datetimes in `commit` state are UTC as stored; plan/card instants keep the
  guild offset v4 produced.

Not covered here: the stateful `Pipeline` itself (debounce/urgent flush,
`_prepare`/`fit_to_budget` over a store, `apply_plan` recording rows, posting
one card through the delivery journal, applying chat RSVPs, stranded-card
reposts) and `rescan_window` backfill/widening. Those need a Discord-backed
host and are left for a later vector pass.
