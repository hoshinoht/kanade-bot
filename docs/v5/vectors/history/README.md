# Change-history vectors

`golden.json` pins the canonical encoding and hashes of change-history
format `kanade.change.v1` (see `docs/v5/history.md`): a genesis record and one
record exercising non-ASCII text, control characters (NUL, U+0001, tab,
newline, DEL), quotes, `null` optionals, sub-second instants and `refs`.

Each entry holds the exact stored `body` text and its lowercase hex SHA-256
`hash`. The unit test `golden_records_match_the_pinned_vector`
(`src/domain/history/record.rs`) rebuilds both records and checks the body,
hash, round-trip parse and stored-bytes verification.

The vector was produced by the v1 Rust encoder (regenerate the text with
`cargo test --all-features --lib print_golden_vector -- --ignored --nocapture`)
and cross-checked independently: Python's
`json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)`
reproduces each body byte for byte and `hashlib.sha256` each hash.

Never regenerate this file to make a failing test pass: any change to the
encoder's output is a new format version with its own vector.
