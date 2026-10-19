# leaf-dioxus

**Leaf**, the document app, in Rust from `main` up: the leaf-dioxus editor
(`crates/leaf-dioxus`) in windows drawn by Dioxus's native renderer
(`dioxus-native` 0.8.0-alpha.3, Blitz on wgpu, no webview), on every platform
that renderer reaches. The Mac's is `Leaf.app` (`org.diaryx.leaf`), and
replaced the SwiftUI app that hosted the same editor in an `NSView`; iOS is
next. It edits the same documents as every other frontend, through
`leaf-core`.

```sh
cargo xtask mac                       # Leaf.app, built and launched on a copy of the sample
cargo xtask mac notes.md              # on a document of your own
cargo xtask package                   # signed, notarised Leaf.app and .dmg in target/package/
cargo run --manifest-path apps/leaf-dioxus/Cargo.toml -- corpus/document.md
cargo test --manifest-path apps/leaf-dioxus/Cargo.toml
```

`cargo xtask swift` still builds and launches the Mac app, for the hands
that type it.

## How it is put together

Every window is a Dioxus page, the same on every platform:

| | |
|---|---|
| `page.rs` | a document window: the toolbar over the editor, the page's overlay over both; and the `Page` the app keeps per window, through which its events and the menus' commands reach the editor |
| `toolbar.rs` | the formatting toolbar — LeafToolbar's tools, in its order — then Source and the display menu |
| `context.rs` | the context menu, and the Link prompt |
| `prefs.rs` | the Settings window |
| `menus.rs`, `commands.rs` | every menu as data, and what each item does and whether it can — drawn as the system's menu bar where there is one, and as the page's own menus |
| `settings.rs` | the display choices, under the keys the SwiftUI app stored them by |
| `shell.rs` | the editor's `Host` |
| `pdf.rs`, `offscreen.rs` | Export as PDF and Print, laid out by the editor offscreen |
| `app.rs` | the event loop's handler |

The menus, the dialog and the choice controls are
[`crates/leaf-dioxus-ui`](../../crates/leaf-dioxus-ui)'s, written to work
under Blitz: Dioxus's own primitives close through JavaScript `eval`, which
dioxus-native answers with nothing, and move DOM focus from tasks, which
panics there. Its README says what it does instead.

What each platform does its own way is `platform/`, one module per platform
under the same names (`platform/mod.rs` lists them):

| | the Mac (`platform/macos`) |
|---|---|
| a document | `LeafDocument`, an `NSDocument` subclass defined from Rust: Open, Save, autosave in place, Versions, Duplicate, Rename, Move To, Revert, Open Recent, Finder's Open With, the question an untitled document asks as it closes |
| the menu bar | `menus::bar()` as `NSMenu`s; AppKit's own items by selector, the rest `leafCommand:` |
| spelling | `NSSpellChecker`: underlines, guesses, Check Document Now, autocorrect, smart quotes and dashes, text replacement — `Spelling.swift`'s rules |
| Look Up | the Dictionary's popover, by force click or the context menu |
| paper | the print panel over `pdf.rs`'s PDF, and Export's save sheet |
| the rest | the clipboard, the user defaults, opening a URL, the Character Viewer |

**One window per document.** dioxus-native's `launch` makes one window
before the loop starts and none after. A document app makes one whenever
the system opens a document, so `app.rs` owns blitz-shell's
`BlitzApplication` itself and assembles each window's page as `launch`
would, on the handler's turn. What the platform tells the app — a document
wanting its window, a menu item, the spelling checker's answer — arrives
outside that turn and is queued (`app::post`, `app::on_turn`), so the editor
is only ever touched from the handler. Each window has its own `document`
context (`head.rs`), which `launch` would provide and keeps private, so a
`<style>` meant for the page's head is put there.

**Closing.** The close button and File ▸ Close are `performClose:`, and for a
window with a document AppKit asks the document first — autosaving a titled
one, asking about an untitled one — and calls the window delegate only once
it agrees. winit reports that as `CloseRequested`, so the page closes on it
without asking again; asking again waits on the first question's file
access, which does not end.

**Under test.** Run a build with `LEAF_BUNDLE_ID` set to another identifier
(`build.rs` and `cargo xtask mac` both read it), so it keeps its own
preferences, recents and autosaves apart from the installed Leaf's. A debug
build reads `LEAF_DRIVE`, a FIFO of pointer and key gestures delivered to a
window as winit's own events (`drive.rs`) — how a session with no hand on
the system's pointer clicks in the page. Menu bar items are pressed through
the accessibility API, which reaches only this app's menus.

**winit is patched.** Its macOS backend is vendored in
`patches/winit-appkit` (`PATCH.md` says what and why; `upstream/` is the
change prepared for winit): the text input client answers from the text
around the caret, so press-and-hold accents, the Character Viewer and
dictation work. That `[patch]` is an exception to the org's rule, granted
for this workspace, and only this one has it — a `[patch]` reaches no build
but its own workspace's. The proposal "A shared, patched winit for the
Dioxus work" in `docs/proposals/` is the plan for it, and for iOS's.

What the SwiftUI app did and this one does not yet is the task "What the Mac
app does not do yet" in `docs/tasks/`.

## The editor on Blitz (2026-10-15)

The rest of this file is how the editor came to work on Blitz, as the app it
grew in found out.


An editor working toward `apps/leaf-editor`'s feature
set, run on a Mac with a real GPU — the machine the spike's "What this still does not
answer" asked for. It has the Swift editor's key bindings (marks, headings,
lists, quotes, code blocks, tasks, indent, word and line deletes, block
moves, table-aware Return and Tab, Emacs keys), clipboard with the HTML
flavor, save, click / double- / triple-click / shift-click / drag-select,
task boxes, a click past the end, page motion, a status line, and only the
rows in view rendered. `LEAF_TRACE=1` logs every key, pointer event and
repaint to stderr.

Almost none of that was leaf work: every binding is a line calling the `Doc`
method Swift calls through leaf-ffi. What took the time was Blitz, which
differs from a browser in six ways an editor walks straight into:

| Blitz does | So the editor |
|---|---|
| focus only form controls on a click, and *clear* focus on anything else | reads keys from winit (`use_window_event`), not `onkeydown` |
| report ⌘ as `SUPER`, not `META` (through 0.3.0-beta.3; beta.4 reports `META`, `keyboard-types` 0.8 having retired `SUPER`) | matches the one Blitz reports, `META` now; matching the other typed ⌘I as an `i` |
| poll tasks while it holds its document borrowed, so `MountedData::scroll` / `set_focus` from a handler panic ("RefCell already borrowed") | scrolls itself: owns the offset, shifts the rows by a margin |
| paint the first frame of an `overflow: hidden` element and not its changes | clips at `body`, never `.leaf` |
| skip painting a child that hangs past its parent's box | gives `.leaf` no fixed height |
| lay out an absolutely positioned element on the first paint and not when a later frame inserts it | draws the caret inline, spliced into the run |
| leave a freed node in its layout tree when a box is shown in the frame rows arrive in an absolutely positioned child of it — every layout after panics (a `should_panic` test in `headless.rs` keeps watch) | keeps the page's overlay displayed, letting the pointer through (`pointer-events: none`), and shows and hides what is in it |

and implements neither `onresize` (the height comes from the winit window) nor
standard macOS key bindings for app code (it keeps them for its own inputs).
None of these is hard to live with once found, and each is a Blitz bug or a
0.8-alpha gap rather than a design limit — but together they are what
"0.8-alpha" means in practice.

One leaf finding came out of it: `Doc::counts()` is 130 ms at 5 200 rows, so a
status line that asks per keystroke lags every keystroke by that much. The
editor counts after 400 ms without an edit, as core's own doc says to.

One trap for anyone testing it: Blitz paints nothing while macOS reports the
window **occluded** — covered by other windows — so a capture of a covered
window is its last frame, and looks exactly like a hung editor. It cost the
spike an afternoon of chasing a "freeze" that was never there; `LEAF_TRACE`
now logs `occluded true/false` so the two can be told apart.

## Proportional text

The spike's "What it would take" item 4 is done the way the Swift editor does it. Core
builds one *unwrapped* row per block (`build_visual_unwrapped`), and
`text.rs` lays each out with **parley** — the engine Blitz
shapes with, over the same system font collection, in the same CSS family
lists (Iowan Old Style for body, Menlo for code and tables) — breaking it at
the column's pixel width. Each visual line renders as its own
`white-space: pre` div, so Blitz never wraps anything: parley's breaks are the
truth, and only a sub-glyph x could drift between the two. A click is
`Cursor::from_point` on its line, a byte, a display column, `Doc::click`; the
caret was already an inline span, so it sits after the right glyph in any
font. ↑/↓, Home/End and ⌘←/→ move by *visual* line from the caret's x with a
sticky goal x, Swift's rule, because core's own motion knows only its rows.
List items hang under their text. A quote's gutter is not the `│ ` core
spells it with but, as in the Swift editor, a cell a fixed 1.375em wide per
level (22 px at 16) with a 3 px bar down its left edge, as tall as the line it
stands in — so the bars of a quote's lines, rows, tables and images meet in one
unbroken rule. parley measures the same cell, so a click past the gutter still
lands on its text; a quoted list item hangs under its text like any other.

Measured on the corpus: a click lands the caret between the two letters
under the pointer, ↑ keeps its x across lines, End stops at the wrapped
line's end. Laying out all 4 601 rows of the 200× corpus takes 70 ms (once, and
on a width change); a keystroke lays out the rows it changed — a Return 3 of
them, because layouts are cached by row handle and the frame keeps a shifted
row's handle.

## IME and dead keys

`ime.rs`. Native needs none of what the spike's item 3 asks of
`dioxus-html`: winit reports the input method itself — `Ime::Preedit` with
the marked text and a cursor inside it, then `Ime::Commit` — and on macOS that
covers every input method *and* dead keys (⌥E opens a composition of `´`; the
E after it commits `é`). Blitz turns IME on only for its own `<input>`s and
sends its IME events to the focused node, so the editor allows IME on the
window itself and reads `WindowEvent::Ime` beside the keys.

The marked text is written into the document as it is composed, through
core's `edit_composing`, each step replacing the last one's provisional
bytes, and `end_composition` on commit — so a whole composition is one undo
step, as in the Swift editor. It is underlined by a `composing` mark the
bridge's `Marks` now carries beside the selection, and the input
method is told where the caret is so its candidate window opens beside it.

One core finding: moving the caret — even to where it already is — ends an
edit run, so placing it at the input method's cursor after each step made
every step its own undo. The editor *draws* the caret at that cursor
instead and leaves core's where `edit_composing` put it.

Unit-tested (a dead key, a kana composition folding into one undo, a
withdrawn composition, a composition over a selection). In the app on macOS,
dead keys work (⌥E E → é, ⌥U O → ö), and so does Japanese (romaji → kana →
漢字, undone in one step).

Two macOS text features do not, and neither is the editor's to fix:

- **Press-and-hold accents.** Holding E repeats it instead of opening the
  accent popup: AppKit shows that popup only for a view that handles key
  repeats as its own text views do, and winit's does not.
- **The emoji picker.** ⌃⌘Space is the key equivalent of *Edit ▸ Emoji &
  Symbols*, which AppKit adds to an app's Edit menu, and this app has no
  menu bar. And were the picker opened another way, winit's `insertText`
  forwards only text that ends a composition, so its insertion would be
  dropped.

Both are now fixed by a patch to winit's macOS backend, vendored under
`patches/winit-appkit` (its `PATCH.md` says what and why): the
text input client answers from the surrounding text the editor reports,
replacement ranges become `Ime::DeleteSurrounding`, picker and dictation text
arrives as `Ime::Commit`, and keys meant for the accent menu are not typed.
⌃⌘Space opens the Character Viewer from the editor, for want of an Edit menu.

Emoji then needed a different renderer: Blitz's default `vello-hybrid` cannot
draw a bitmap glyph (Apple Color Emoji is all sbix PNGs), drawing nothing or,
with glifo's `png` on, panicking. The editor builds with classic `vello`,
which draws them. Checked on macOS: press-and-hold accents and emoji.

## Source view and find

**⌘E** shows the markup, one row per source line, from core's `SourceMap` —
styled byte ranges off the same parse the caret rides, so delimiters are
dimmed and a fence keeps its syntax colours (`source.rs`).
Core already works in source rows and columns for `click`, motion and the
caret, so the rest of the editor does not know which view it draws: rows are
`RowView`s either way, set in Menlo, wrapped, every run at body size. With no
`VisualMap` to diff, a source row is reused by *content*, so a Return relays
out the line it split, not every line below. The column widens to 82 cells,
since markup is usually wrapped near 80.

**⌘F / ⌥⌘F / ⌘G / ⇧⌘G**: a find bar with a replace row
(`find.rs`) — incremental, case-insensitive (ASCII folding,
so offsets never move), wrapping, "2 of 5", the match shown as the selection
in either view. Replace replaces the selected match and steps on; Replace All
(⌥↩ in the replace field) is one undo step. Its fields are the editor's own,
not Blitz `<input>`s, which depend on the DOM focus the editor gave up on.

Checked on macOS: ⌘E both ways; find stepping across a view switch; Replace
All of 5 and its single undo.

## Tables as a grid

`table.rs` draws a table from core's `TableInfo` — each
cell's glyphs with their source offsets and styles, its range and alignment,
which rows are the head — instead of the map's box-drawn monospace picture.
Columns are as wide as their widest cell and squeezed only when the grid will
not fit, cells wrapping into what is left (leaf-gpui's rule). Every cell is a
`RowView` laid out by `text.rs` at its column's width and aligned in it, so a
cell wraps, draws its caret and selection, and hit-tests like any other text:
a click lands on the glyph under it and that glyph's source offset, never a
column of the picture. A table's picture rows stay in the row list at zero
height, so row indices still match core's map; the first stands for the grid.
↑/↓ in a table are core's `cell_move_vertical`, which lands with the cell
selected, as in the Swift editor. A quote's gutter is drawn on every grid
row, as the picture does.

Checked on macOS: alignment (`:---`, `---:`), a wrapping cell, a bold shaded
head, a table in a quote; a click into a word in a cell, typing there, ↓ to
the cell below, and the source saved as expected.

## Images

`media.rs` draws a block-level image, video or audio from
core's `MediaInfo` in place of the one-row `🖼 alt` placeholder, as
`Block::Media` in `blocks.rs`. Blitz draws the picture itself: an `img` whose
`src` is the file's `file://` URL, which its `net` provider reads and decodes
(PNG, JPEG, GIF, WebP, TIFF, SVG). The block's height has to be known before
Blitz decodes anything, so the size comes off the file's header (`image`) or
an SVG's canvas (`usvg`), once per path, a failure cached too. A relative path
resolves against the document's directory. The box is fitted to the column
past any list marker or quote gutter, keeps its aspect ratio, is never scaled
up, and is capped at 420 px tall (Swift's `MediaLayout.fit`). Like Swift and
leaf-gpui it never calls `Doc::set_media_rows`, which is for a terminal: the
placeholder stays one row and the block stands in for it.

A click on the left half of a picture puts the caret at core's stop in front
of it, and on the right half at the stop past it; the caret draws as a bar
at that side of the box. There is no playback, as in Swift: a video draws its
poster (or a dark 16:9 box) under a play badge, audio a fixed-height chip,
and a picture that would not load a dashed chip with its alt text. A web URL
or `data:` URI also gets the chip, because nothing reads its size before
layout.
## Table editing, links and the context menu

**Format ▸ Table** is Swift's `TableRows`: Insert Table (2 × 2), Insert Table
With 3–5 columns, insert and delete rows and columns, Align Column, and move
a row or column — each one `Doc` call (`insert_table`, `table_insert_row`, …),
made by `input::apply_table`. Swift gives none of them a shortcut, so they are
`menu::Command::Table` items rather than chords; everything but Insert is
disabled while `caret_in_table()` is false, re-asked when the caret or the
text moves, and a no-op there regardless.

**⌘-click** follows what it lands on, Swift's order: a footnote reference to
its note and a note back to its reference (`footnote_at`,
`footnote_definition_at`), else a link — a `#fragment` to the place `locate`
finds, a URL or an existing file beside the document to `open`
(`link.rs`). **Right-click** (or ⌃-click) places the caret
unless it lands inside the selection, then pops up Go to Note / Back to
Reference, Open Link and Copy Link when they apply, Cut and Copy with a
selection, then Paste, Paste and Match Style and Select All — the clipboard
items replaying the Edit menu's chords. The app adds the spelling checker's
guesses for a misspelled word and Look Up, and draws it in the page
(`context.rs`).

A ⌘-click or right-click also takes no drag after it: the hand settling
between press and release arrives as a drag, which extended a selection from
wherever following the link had put the caret.

Checked on macOS: the Table menu present, its grid items disabled outside a
table, and Insert Row Below, Align Column ▸ Right and Move Column Right on a
cell, saved as expected; the context menu on a link (Open Link to a
`#fragment`), on plain text (no Cut or Copy), and inside a selection (kept,
with Cut and Copy); Select All from it; ⌘-click from a reference to its note
and from the note back. Not tried on screen: Copy Link, and a URL or file
handed to the system — the resolution is unit-tested. A click on a note's own
`[1]` marker lands on the line above (core's mapping of that decoration), so
following back is a ⌘-click on the note's text.

## Dark mode, settings and text zoom

**Dark mode needs no Rust.** Blitz turns winit's theme into the stylesheet's
`prefers-color-scheme` and recascades on `ThemeChanged` (blitz-dom's
`DeviceChanges::COLOR_SCHEME`), and Stylo has CSS custom properties. So every
colour — the bridge's `STYLESHEET` and `view.rs`'s chrome —
is a `var(--…)`, set once in a light `:root` and once under
`@media (prefers-color-scheme: dark)`, with the Swift editor's values
(`Palette` in `LeafUI/Platform.swift`: AppKit's semantic colours as each
appearance resolves them, its syntax pairs, its 28% highlighter washes). A
test fails if a property is used but not set, set for one appearance only, or
a literal colour appears outside the two palettes. The one Rust is a
`request_redraw` on `ThemeChanged`, which Blitz handles without asking for a
frame — and the Appearance override below, which cannot go through Blitz:
winit sends no `ThemeChanged` for the app's own `Window::set_theme` (its
appearance observer ignores a window whose appearance is set), so a forced
appearance rewrites the dark block's media query to `all` or `not all`
(`view::themed`).

**Settings (⌘,, Leaf ▸ Settings…)** are Swift's `SettingsView` choices with
its values: font and code font, size (14/16/19), line spacing
(1.3/1.5/1.75/2), column width (52/68/88 characters, or full) — plus
Appearance (System/Light/Dark, through `Window::set_theme`), which Swift does
not have and which is how to see the dark stylesheet without switching the
whole Mac. They are a window of their own (`prefs.rs`): the book and screen
faces the system ships as chips, and every other family installed under
Other ▾, by letter. Choices persist in the app's user defaults, under the
keys the SwiftUI app used (`settings.rs`), so a reader's carry over.

The faces, size and leading that were `text.rs` constants are one runtime
`text::Type`, held by `Fonts` and written into the stylesheet by
`view::css(&Type)`, so parley measures what Blitz draws; a change relays out
every row (the layout cache is keyed by type as well as width) and drops the
tables' cache. The column's measure is counted in characters of the body face
— `Fonts::char_width`, Swift's `averageCharWidth`, measured the same way — so
it follows the face, the size and the zoom.

**Text zoom (⌘+ / ⌘− / ⌘0)** scales the type through browser-like steps
(50–300%) and reflows; it is not saved. Blitz has its own *page* zoom on
⌘= / ⌘- / ⌘0 (blitz-shell's `window.rs`, matched on the physical key with
⌘ or ⌃): it scales CSS pixels, which leaves the editor's window width and the
viewport in the wrong units. The editor wins by naming the three chords as
View menu items: macOS hands a key equivalent to the menu before the window,
so Blitz never sees them. ⇧⌘= (⌘+ typed with Shift) is not the menu item's
equivalent: it reaches the editor through the window, which zooms the text,
and Blitz, which almost certainly zooms the page as well — dioxus-native gives
the app no handle on Blitz's viewport to undo it. Not verified, since the
tools here synthesise that chord as `NumpadEqual`.

Checked on macOS, when Settings was a panel in the window: Dark, Light and
back, the window's title bar with them; Dark kept across a relaunch; Charter
at Large, and
⌘= / ⌘− / ⌘0 from the keyboard (125%, 150%, back to 100%), each reflowing
with the column widening by its measure while the status line stays at 12px —
Blitz's page zoom never acted; a click in the right half of a glyph putting
the caret after it, in Charter at 19px and again at 125%; Restore Defaults
back to the System appearance; the source view at its 82-cell measure. Not
checked: a change of the *system's* appearance while the app runs (the
`prefers-color-scheme` path is the one the launch used, and Blitz's handler
is above), which needs System Settings changed.

## Working on it

The editor is `crates/leaf-dioxus`; this app is its wiring. Every event
source — the window's keys and IME, the menus, the pointer — reaches an
`Editor` (the widget's `editor.rs`) through the window's `Page`; what the
editor cannot do itself it asks of the window's `Shell` (`shell.rs`), the
widget's `Host`. Where a change goes:

- **A command**: a key binding in the widget's `input.rs`, and an item in
  `menus.rs` given as that chord. Nothing else; the item replays the chord,
  and `commands::status` ticks and enables it. One with no shortcut is a
  `Command` variant, carried out in `Page::command`. One the editor
  recognises but the app carries out (Save, the zoom, ⌘,) is a `Request`,
  answered in `Shell::request`.
- **Something drawn in place of rows** (media, a formula, a diagram): a
  variant in the widget's `blocks.rs` and a module of its own for what it
  draws, as `table.rs` is for tables.
- **A menu, a dialog, a choice control**: `crates/leaf-dioxus-ui`.
- **Something the system does**: a function under the same name in every
  `platform/` module.
- **A display choice**: `settings.rs` and its row in `prefs.rs`, handed to
  the editor as a `Look` (`Shell::look`).

Pure logic is unit-tested beside it, and pages are laid out by Blitz with no
window in `headless.rs`; `LEAF_TRACE=1`, `LEAF_OUTLINE=1` and `LEAF_DRIVE` are
the instruments for the rest. A window covered by others does not paint —
see `occluded` in the trace.
