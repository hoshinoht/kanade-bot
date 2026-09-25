# Integration tests guide

- One target per directory (`tests/<target>/main.rs`): `domain`, `scheduler`, `notify`, `store`, `discord`, `delivery`, `persona`, `provider` (target name `provider_contract`), `governor`, `extract`, `runtime_bootstrap`. `tests/fixtures/provider/` holds synthetic model listings and loopback TLS test certs.
- Shared helpers live in `tests/common/mod.rs` (vector loading, pinned clock and ID sequences, `Deviation`/`apply_deviations`, `assert_sound`) and are included with `#[path = "../common/mod.rs"] mod common;` — never import another target's private files.
- Vector replays must assert the replayed case count equals the file's, panic on unknown ops, and validate files against their JSON schema.
- Intentional v5 differences from v4 go through named `Deviation` lists: each entry asserts the frozen v4 value is present before substituting the v5 value, and the test fails if an entry is unused. Do not loosen comparisons instead.
- Store tests use fresh temp directories and clean up; Discord and model tests use `FakeDiscord`, the fake provider or in-process loopback stubs. No test may touch the network, `.env`, `data/` or private `config/`.
- Run one target: `cargo test --all-features --test <target>`; everything: `cargo test --all-targets --all-features --locked`.
