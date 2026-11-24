# dugong-graphlib 0.11.0, patched for VOMD

The library's sources (`src/`) and licenses (MIT or Apache-2.0; graphlib's MIT license and its
notice in `THIRD_PARTY_LICENSES/` and `THIRD_PARTY_NOTICES.md`) of dugong-graphlib 0.11.0 from
crates.io (<https://github.com/Latias94/merman>, a port of dagrejs/graphlib), used through
`[patch.crates-io]` in VOMD's `Cargo.toml` and `fuzz/Cargo.toml` by `vendor/dugong`. Its
`Cargo.toml` drops the tests and the dev-dependencies, and `rust-version` is VOMD's (1.89). The
changes (size: no serde or serde_json in VOMD's binary):

- `json.rs` (graphlib's JSON reader and writer) is gone.
- `GraphOptions` no longer derives serde's `Serialize` and `Deserialize`.

To upgrade: copy the new version's `src/` and licenses here, apply the same changes, and run
VOMD's Mermaid tests.
