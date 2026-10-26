---
title: Ship the Dioxus terminal editor as leaf
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-26
---
# Ship the Dioxus terminal editor as leaf

**Where.** `apps/leaf-dioxus-tui`, `.github/workflows`, `flake.nix`,
`.config/ship.figl`.

**What.** leaf-tui is archived (`archive/apps/leaf-tui`), and its successor,
`apps/leaf-dioxus-tui` (binary `leaf-dtui`), is built by nothing in CI and
shipped by nothing. The Homebrew `leaf` formula in `diaryx-org/tap` stays at
0.5.1, which is leaf-tui, and the flake no longer has a package. The
release workflow and its rehearsal are at
`archive/.github/workflows/homebrew.yml` and `ci.yml`.

- **Releasing.** The org's shared Homebrew workflow builds
  `cargo build -p <package>` in the root workspace. The terminal editor is a
  workspace of its own, on Dioxus's alpha and the Blitz fork from git, which
  the root workspace keeps out on purpose. Either the shared workflow learns a
  `--manifest-path`, or leaf has its own release job. The binary becomes
  `leaf`, and the four targets leaf-tui was cross-built for are checked again;
  Linux has not been run at all.
- **CI.** Add it to `ship ci`'s `dioxus` job (decision 4 in the proposal
  "The leaf-dioxus editor in a terminal").
- **nix.** `buildRustPackage` vendors every git entry of the lockfile, so
  packaging it means the Blitz fork and unit.
- **What leaf-tui did that it does not yet** (the proposal's "What it does not
  do yet"): New, the scrollbar, hover previews, the large-heading rasters, the
  `::embed` card, the table commands in a menu, the fuzzy palette, the Emacs ⌃
  keys beyond ⌃K, and start-up time. Not checked: Linux, sixel, iTerm2, wide
  characters and the IME. Off Apple the editor takes Ctrl as ⌘, so on Linux
  Ctrl+←/→ and Ctrl+Backspace act on the line, not as leaf-tui's keys did;
  leaf-tui's ⌥ chords (⌥v plain paste, ⌥w the view) are unchecked.

**Done when.** A `v*.*.*` tag ships the Dioxus terminal editor as the `leaf`
formula, CI builds it, and the list above is done or deliberately left out.
