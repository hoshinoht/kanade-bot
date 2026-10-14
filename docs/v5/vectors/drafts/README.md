# Draft operation vectors

`draft_ops.json` pins the stored encoding of draft operations, format
`kanade.draft_op.v1` (see "Drafts" in `docs/notes/history.md`): one sample of
every operation, covering `null` optionals, created and existing targets,
per-run and update-all choices, and quotes in text.

The integration test `drafts::codec_matches_the_pinned_v1_encoding_and_round_trips`
(`tests/scheduler/drafts.rs`) encodes the samples, compares each string and
decodes it back. Print the encoder's output with
`KANADE_PRINT_GOLDEN=1 cargo test --all-features --test scheduler codec_matches -- --nocapture`.
Each string was cross-checked with Python's
`json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)`.

Never regenerate this file to make a failing test pass: any change to the
encoder's output is a new format version with its own vector.
The one in-place extension, before any release (no stored drafts existed),
appended the `fixed_participants` sample (a weekly-timing party delta);
the earlier strings are unchanged. A second additive extension (E3
proposals) appended `set_run_bosses`, `ensure_reminders`, `recount_run`
and `revive_run` samples, again leaving every earlier string unchanged.
