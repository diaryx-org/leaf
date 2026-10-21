---
title: The leaf-dioxus editor in a terminal
part_of: '[Proposals](/docs/proposals/proposals.md)'
status: draft
created: 2026-10-21
---

# The leaf-dioxus editor in a terminal

## Status

`draft`, 2026-10-21. Written with the prototype in `apps/leaf-dioxus-tui`, on
the `dioxus-tui` branch, with unit's `dioxus-tui` branch beside it. Decided by
Adam on 2026-10-21: keep going (decision 1 below), in the order proposed —
pictures, then unit's menus and dialogs, then the file features. All three
rounds are in; "What it does not do yet" is what is left. leaf-tui is
untouched; replacing it is still Adam's call.

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

The first prototype was about 1,500 lines in four files and needed no change
to leaf-dioxus or unit. With pictures, chrome and files it is about 2,700
lines in eight, with tests, and two small changes outside it:

- **leaf-dioxus:** `Type::uniform`, which sets every run at the body size, as
  the source view already does. Without it the editor lays a heading out at
  twice the size the painter can draw it, and a click on a heading lands on
  the wrong character.
- **unit (branch `dioxus-tui`):** `Density::Cell`, a density whose menu rows,
  separators and controls are each one cell high, with the menu's margin and
  overlap per density so it stays on the grid; `Unit::hot_first` made public;
  and keyboard choice for an action sheet (↑/↓/Tab, Return), which is useful
  on a desktop too.

## What the prototype does

Measured on 2026-10-21 in tmux at 80×24 and in Ghostty, driving the debug
build:

- **Text:** headings (bold at the body size), paragraphs, bold, italic,
  underline, links, quotes, nested bullet and numbered lists, code blocks with
  syntax colours, tables, rules, the selection and the status line, in the
  light and dark palettes. The terminal is asked which it is (OSC 11), as
  leaf-tui asks.
- **Pictures, formulas and diagrams** through ratatui-image: kitty, iTerm2 or
  sixel where the terminal has it (Ghostty drew a picture at full resolution
  through kitty), half-blocks where it does not (tmux). A picture part-way off
  the screen is cropped to its rows in view, and the status line stays on top.
- **Editing:** typing, arrows, Home/End, PageUp/PageDown, ⇧-selection, the ⌘
  shortcuts as ⌃ (⌃K stays the editor's delete to the line's end), the
  WYSIWYG reveal of markup at the caret, bracketed paste, find (⌃F).
- **Mouse:** a click places the caret on the character under it, a drag
  selects, the wheel scrolls, a right-click opens the context menu, and menu
  items highlight on hover.
- **Chrome, from unit:** the command menu (⌃P or F10), built from one command
  table, each item with its keys, so it is the key reference too; the context
  menu (Open, Edit and Copy Link, Cut, Copy, Paste, Select All); the Link and
  Save As prompts; the Save / Don't Save / Cancel question before quitting.
  All of them are unit's, as the Mac app's are, in its cell density.
- **Files:** Save and Save As; a save over a file changed on disk asks
  Overwrite / Reload / Cancel; a clean document reloads an outside write once
  the file stands still; a dirty one says so once. `-r` reads only, and `-w N`
  sets the column (80 by default, 0 fills), as in leaf-tui.
- **Cost:** a frame (settle, layout, paint) takes 1–3 ms on a 400-paragraph
  document. The process holds about 55 MB resident.
- **Tests:** 14 in the prototype, painting documents headlessly and checking
  the grid, the pictures, the menus, the quit question and the file watch.

## What it does not do yet

Against the [leaf-tui inventory](#what-leaf-tui-does) below:

- **Pixel chrome in the editor.** The find bar's rows are a Rust constant of
  34px, so the bar takes two terminal rows; table cells are padded in pixels,
  so a table gets its column rules but not its row rules. Decision 2 below.
- **What leaf-tui adds that leaf-dioxus has no part of:** New, the scrollbar,
  hover previews, the large-heading rasters, the `::embed` directive card,
  the table commands in a menu, and a fuzzy palette (the command menu is a
  menu, not a search). Each is host work or a small widget change.
- **Emacs ⌃ keys** beyond ⌃K. On macOS ⌃ stands in for ⌘, so ⌃A and ⌃E reach
  the editor as ⌘A and ⌘E.
- **Start-up** takes about two seconds in the debug build, and keys typed
  while the terminal's graphics query is out can be lost.
- **Not checked at all:** Linux (Menlo is assumed; the face list falls back to
  DejaVu Sans Mono), sixel and iTerm2 (only kitty and half-blocks were seen),
  wide characters' cells, release size and build time, the IME, and CI (the
  prototype is its own workspace and no CI job builds it).
- **A Blitz behaviour found on the way:** Blitz broke a line between a quote's
  bar (an inline box) and its text, though the line is `white-space: pre`,
  when a trailing space did not fit. The prototype gives each line room to
  avoid it; the Mac app could meet it in a rare case. Not reported upstream.

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
fork, which is a much larger build than ratatui: the prototype's lockfile lists
more than 600 packages (every platform's), where the root workspace's comment
counts 141 that leaf-tui uses. It uses more memory, and starts slower. And
unit's cell density is a third way unit draws, to be kept working.

## Decisions to make

1. ~~Whether to keep going~~: yes (Adam, 2026-10-21). The three rounds are
   done; what is left is listed above.
2. Whether the editor should size its chrome in lines when a host asks (a
   `Look` option), so the find bar and table padding fit the grid. The
   `uniform` type is the first piece of it.
3. Whether unit's `dioxus-tui` branch (the cell density, keyboard choice on
   an action sheet) merges into unit's main.
4. Whether the prototype stays its own workspace, like `apps/leaf-dioxus`, or
   joins it, and whether CI builds and tests it.
5. Only after it matches leaf-tui: whether it replaces leaf-tui.
