---
title: The leaf-dioxus editor in a terminal
part_of: '[Proposals](/docs/proposals/proposals.md)'
status: draft
created: 2026-10-21
---

# The leaf-dioxus editor in a terminal

## Status

`draft`, 2026-10-21. Written with the prototype in `apps/leaf-dioxus-tui`, on
the `dioxus-tui` branch. Nothing is decided. leaf-tui is untouched; replacing it
is Adam's call (see "Decisions to make").

## Does Dioxus have a TUI renderer?

Not any more. Dioxus had one, `dioxus-tui`, on a core called rink (later
`plasmo`) with taffy for layout. Its last crates.io release was
`0.5.0-alpha.0`, on 2024-02-23. Commit `7cc326488` ("Move TUI renderer into
blitz repo", 2024-03-15) took it out of the Dioxus repo before 0.5 shipped. The
Blitz repo has no TUI package today: its `packages/` are the DOM, paint, shell,
net, HTML, the test harness and dioxus-native. So Dioxus 0.8 has no terminal
renderer to use or revive. The `dioxus-tui` code on crates.io is for the 0.4 /
0.5-alpha core and would need porting to 0.8's.

## The two routes

**Revive dioxus-tui.** Port rink/plasmo to Dioxus 0.8. It has its own DOM, its
own style subset and its own layout. leaf-dioxus would then need to work on two
DOMs with different CSS support, and the editor depends on Blitz details
(`keys.rs`, `view.rs` record several). Large, and it doubles the surface.

**Paint Blitz into cells.** Keep Blitz's DOM, Stylo and taffy exactly as the Mac
app uses them, and replace only the last step: walk the laid-out tree and write
cells instead of pixels. This is the prototype. The cost was small because
everything above the painter already existed:

- `DioxusDocument::new` runs with no window (`apps/leaf-dioxus/src/headless.rs`
  already does it for tests).
- The editor needs a winit `Window` only for size, scale, theme and redraw
  requests. A stub is enough (`offscreen.rs` already does it for PDF).
- The editor takes keys as `(Key, Code, Modifiers)` through `Editor::key`, so a
  crossterm key maps straight in.
- The editor lays out its own text with parley. If the face is monospace and a
  cell is one advance wide and one line high, its wrapping, caret motion and
  hit-testing count in cells without any change to the editor.

The prototype is about 1,500 lines in four files: the painter (~650), the
host loop with its tests, the terminal I/O and the
window stub. It needed **no change to
leaf-dioxus or unit**.

## What the prototype does

Measured on 2026-10-21 in tmux at 80×24, driving the debug build:

- Paints headings, paragraphs, bold/italic/underline, links, inline and block
  quotes, nested bullet and numbered lists, code blocks with syntax colours,
  tables, the selection and the status line, in the light and dark palettes.
- Typing, arrows, Home/End, PageUp/PageDown, ⇧-selection, ⌘ shortcuts as ⌃
  (bold, save), the WYSIWYG reveal of markup at the caret, bracketed paste.
- Mouse click to place the caret, wheel to scroll, caret-follow.
- Saves through `Doc::save`.
- One frame (settle + layout + paint) takes 1–3 ms on a 400-paragraph document.
  The process holds about 55 MB resident.
- Five tests paint a document headlessly and check the grid.

## What it does not do yet

Against the [leaf-tui inventory](#what-leaf-tui-does) below:

- **Pictures, formulas and diagrams** draw as a labelled frame. leaf-tui paints
  them through kitty, iTerm2 or sixel. Doable: the painter knows each `img`
  box's cell rectangle, so ratatui-image (or its encoders) can paint into it.
- **Chrome.** The command palette, context menu, key reference, prompts and
  unsaved-changes / changed-on-disk dialogs are leaf-tui's own. Here they
  would be Dioxus components (unit's menus and dialogs already exist for the
  Mac app) and so shared with it. Not tried: unit is not in the prototype yet.
- **File-watch, New, Save As, read-only, `-w` column width.** Host work, not
  rendering.
- **Terminal theme detection.** `LEAF_THEME` only; leaf-tui asks the terminal
  (OSC 11).
- **Pixel chrome.** The editor sizes some chrome in pixels (find bar 34px rows,
  table cell padding). In cells this rounds: table rules get no row of their
  own, so a table shows its column rules only. A terminal stylesheet, or
  the editor sizing chrome in lines, would fix this.
- **Emacs ⌃ keys.** On macOS ⌃ stands in for ⌘, so ⌃A/⌃E/⌃K reach the editor
  as ⌘A/⌘E/⌘K. leaf-tui keeps both.
- **Wide characters** (CJK, emoji) take two cells, but the editor measures them
  with the font's advance, which may not be exactly two cells. Not checked.
- **Not checked at all:** Linux (Menlo is assumed; the face list falls back to
  DejaVu Sans Mono), release size and build time, and the IME.

## What leaf-tui does

`apps/leaf-tui` (about 8,000 lines) and `crates/leaf-ratatui` (about 6,200)
paint leaf-core's `VisualMap`, a grid of glyphs core lays out for a given
width. It has a centred column with a scrollbar, a status toast, a find and
replace bar, two modal choice dialogs, a nested context menu, a fuzzy command
palette, a one-line prompt and a key reference, all from one table of about 60
commands. It draws images, display math and large headings through the
terminal's graphics protocol. It watches the file on disk. It has 274 tests,
and CI builds it for four targets and ships it to Homebrew.

## The argument

For the replacement: one editor, the one the Mac app uses, in every place.
leaf-ratatui is a second renderer of the same document, with its own input
mapping, its own layout pass and its own tests. A behaviour fixed in
leaf-dioxus is fixed in the terminal too. Its chrome could be unit's.

Against: the terminal then depends on Dioxus 0.8 alpha, Stylo and the Blitz
fork, which is a much larger build than ratatui: the prototype's lockfile lists 620
packages (every platform's), where the root workspace's comment counts 141
that leaf-tui uses. It uses more memory. The graphics-protocol work and
the chrome above would have to be redone on the new base before it matches.

## Decisions to make

1. Whether to keep going: images through the graphics protocol next, then
   unit's menus and dialogs, then the host features. That is the order that
   closes the largest gaps first.
2. Whether the editor should size its chrome in lines when a host asks (a
   `Look` option), so a terminal needs no stylesheet patch.
3. Whether the prototype stays its own workspace, like `apps/leaf-dioxus`, or
   joins it.
4. Only after it matches leaf-tui: whether it replaces leaf-tui.
