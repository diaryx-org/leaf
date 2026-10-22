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

Also decided by Adam, 2026-10-21: the renderer goes in the Blitz fork. It is
now `blitz-tui`, a package on the fork's `leaf` branch beside blitz-paint and
blitz-shell, which runs any Blitz document, a Dioxus app included; the
prototype here is what leaf adds to it ([Where the renderer lives](#where-the-renderer-lives)).

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
to leaf-dioxus or unit. With pictures, chrome and files it came to about
2,700 lines in eight, with tests. The painter, the terminal and the input are
now blitz-tui's (about 2,000 lines with its tests); what stays here is about
1,500. Two small changes outside it:

- **leaf-dioxus:** `Type::grid`, the host's cell, for a surface of character
  cells. On a grid every run is set at the body size, as the source view
  already does: without it the editor lays a heading out at twice the size
  the painter can draw it, and a click on a heading lands on the wrong
  character. And the editor's own chrome is sized in whole cells: the find
  bar and the status line are a row each, and a table is padded a cell
  either side, its rules a cell wide or a row high, ruled under the head and
  the last row only, as core's picture of a table is (decision 2).
- **unit (merged into `main`, unit#10):** `Density::Cell`, a density whose menu rows,
  separators and controls are each one cell high, with the menu's margin and
  overlap per density so it stays on the grid; `Unit::hot_first` made public;
  and keyboard choice for an action sheet (↑/↓/Tab, Return), which is useful
  on a desktop too.

## Where the renderer lives

In the Blitz fork, `diaryx-org/blitz`, as `packages/blitz-tui` on the `leaf`
branch (merged from `tui`, diaryx-org/blitz#12). It is a renderer in Blitz's terms, not an engine: it
replaces blitz-paint and blitz-shell, and keeps the DOM, Stylo and taffy, so
it changes with Blitz's paint tree in the same repository. It is not an
anyrender backend, which would get only shapes and glyph runs: drawing text
a cell to a character and borders only on free rows needs the boxes and
lines.

- **Generic:** the painter, a `Grid` of cells; the terminal, with pictures
  from the image data Blitz loaded, an SVG `data:` URL, or the host's decoder;
  the terminal's keys and mouse as Blitz's events, with a key's text, so a
  focused element's handlers get it; `run`, which drives any
  `blitz_dom::Document`; and `TermWindow` (the `winit` feature).
  `examples/tui_counter` is a Dioxus counter whose clicks and keys work
  through it, tested headlessly.
- **Leaf's, here:** the editor and unit's chrome, the command table, the file
  rules, ⌃ as ⌘ for the editor's shortcuts, the caret classes, the shift of a
  click by the editor's padding, and leaf-raster as the picture decoder.
- **Not done for other apps:** the text and caret of `<input>` and
  `<textarea>`, `::marker` list markers, outlines, shadows, gradients,
  transforms and scrollbars.

The move changed one rule: a horizontal border takes the row boundary nearest
it, so a table now gets its bottom rule, which it lacked. Since then
(diaryx-org/blitz#14) a vertical side takes the nearest column boundary too,
and borders that meet in one layer are joined, `┬`, `┼`, `├`, so a table's
cells, each ruled on its own, read as one grid.

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
- **Tests:** 18 in the prototype, painting documents headlessly and checking
  the grid, a table's rules and a click in its cell, the find bar, the
  pictures, the menus, the quit question and the file watch; 16 in blitz-tui
  and its example.

## What it does not do yet

Against the [leaf-tui inventory](#what-leaf-tui-does) below:

- **The column's side padding** is still 16px, a cell and two thirds: the
  painter rounds it to a whole cell, and a click is shifted to match
  (`term.rs`). `Type::grid` could make it a cell, as it does a table's.
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
2. ~~Whether the editor should size its chrome in lines when a host asks~~:
   yes (Adam, 2026-10-21), as `Type::grid`, part of the `Look`, which
   replaces `Type::uniform`.
3. ~~Whether unit's `dioxus-tui` branch merges into unit's main~~: merged
   (diaryx-org/unit#10); the prototype takes unit from `main`.
4. Whether the prototype stays its own workspace, like `apps/leaf-dioxus`, or
   joins it, and whether CI builds and tests it.
5. Only after it matches leaf-tui: whether it replaces leaf-tui.
6. ~~Whether the fork's `tui` branch merges into its `leaf` branch~~: merged
   (diaryx-org/blitz#12); the prototype takes Blitz from `leaf`, like the Mac
   app. Still open: whether blitz-tui is offered to DioxusLabs/blitz.
