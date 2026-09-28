---
status: done
created: 2026-09-25
updated: 2026-09-28
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Android needs a twig release

**What.** `twig-sys` 3.11.0 has no Cargo-to-Zig mapping for
`aarch64-linux-android` or `x86_64-linux-android` and panics in its build
script, so `leaf-ffi` cannot build for Android against the published crate.
twig's `0547db4` (`fix(twig-sys): build for aarch64 and x86_64 Android`) adds
both mappings and builds the C library as PIC there; it is committed in the
twig checkout and not yet released.

Until it is, `cargo xtask android` builds against that checkout through
`LEAF_ANDROID_CARGO_ARGS` (see `packages/leaf-android/README.md`).

**Done when** twig has released it, `twig-doc` in the workspace manifest is
moved to that version, and `cargo xtask android` builds with no extra
arguments — at which point the patch paragraph comes out of the README.

**Done.** twig 3.11.1 released it; `twig-doc` is pinned to 3.11.1, and
`cargo xtask android` builds and runs on the emulator with no extra arguments.
