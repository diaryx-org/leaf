---
status: open
created: 2026-09-26
updated: 2026-09-26
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# UniFFI 0.32 needs a resvg-swift release

**What.** `leaf-ffi` is on UniFFI 0.32, and `resvg-uniffi`, whose scaffolding
rides in the same library, is still on 0.28 in every published version. A
binary links one UniFFI runtime, and 0.31 changed method checksums, so until
the two agree the lock carries both runtimes and the Swift tests die with a
contract-version mismatch in `ResvgFFI`. resvg-swift's `5bfcf75`
(`build(deps)!: uniffi 0.32`) moves it; it is committed in that checkout and
not yet released.

Until it is, build against that checkout: `[patch.crates-io] resvg-uniffi`
pointed at `../resvg-swift/crates/resvg-uniffi`, and
`swift package edit resvg-swift --path ../resvg-swift` for the Swift tests
(`swift package unedit resvg-swift` after — the edit is a symlink under
`packages/`, which is `Packages/` on a case-insensitive disk).

**Done when** resvg-swift has released it, `resvg-uniffi` in
`crates/leaf-ffi/Cargo.toml` and `resvg-swift` in `Package.swift` are moved to
that version, and `cargo xtask ci`, `scripts/test-swift.sh` and
`cargo xtask android` pass with no patch.
