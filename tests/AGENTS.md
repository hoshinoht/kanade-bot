# Integration tests guide

- One target per directory (`tests/<target>/main.rs`): `scheduler`, `notify`, `store`, `discord`, `delivery`, `persona`, `provider` (target name `provider_contract`), `governor`, `extract`, `api`, `chat`, `runtime_bootstrap`. `tests/fixtures/` holds synthetic inputs: `provider/` (model listings and loopback TLS test certs), `chat/world.json` (the chat world), `persona/cases.json` (prompt-assembly cases with their pinned prompts), and the pinned encodings `drafts/draft_ops.json` and `history/golden.json` (never regenerate those to make a test pass: an encoder change is a new format version).
- Shared helpers live in `tests/common/mod.rs` (pinned clock and ID sequences, a scheduler over the memory store, JSON field readers, `assert_sound`) and are included with `#[path = "../common/mod.rs"] mod common;` — never import another target's private files.
- Intentional v5 differences from v4 are named in the test that pins them (`D-…`/`K-…`). Do not loosen comparisons instead.
- Store tests use fresh temp directories and clean up; Discord and model tests use `FakeDiscord`, the fake provider or in-process loopback stubs. No test may touch the network, `.env`, `data/` or private `config/`.
- Run one target: `cargo test --all-features --test <target>`; everything: `cargo test --all-targets --all-features --locked`.
