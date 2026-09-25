# Extraction guide

- Pure v4 `bot/extract` port: no I/O, no clock reads, no Discord/SQL/HTTP types. Reuse `domain` types (`schedule::Run`, `RsvpState`, `time::ZonedDateTime`, `weeks`, `catalog::BossTable`, `ids`) and `domain::pytext` for Python `str` semantics instead of redefining them.
- `gate/` (keyword gate, `BossLexicon`, `urgent`), `window/` (burst grouping and rescan windows), `resolve/` (literal day/time → date/clock/instant; wall-clock comparisons as v4), `matching/` (run matching over `&Run`), `merge/` (burst folding over `Amendment`). `amendment.rs` is the coerced model output; parsing and coercion belong to the schema slice.
- Patterns use the `regex` crate (leftmost-first, like Python for lookaround-free patterns); Python's `\s` also matches U+001C..U+001F, so use `text::SPACE`. The one lookbehind (`resolve/clock.rs`) is emulated by capturing the digit.
- Behaviour must match `docs/v5/vectors/extract/{gate,window,resolve,match,merge}.json`, v4 quirks included; any change needs regenerated vectors. Check with `cargo test --all-features --test extract`.
