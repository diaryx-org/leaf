---
title: "Proposal: a short row, a keyboard panel, and categories on the Mac"
status: implemented
created: 2026-09-23
updated: 2026-09-23
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# A short row, a keyboard panel, and categories on the Mac

## Status

`implemented` on 2026-09-23, unreleased: the catalogue and the Mac's
categories in `8221499`, the iOS row and panel in `90f9129`, the demo app in
`cd69781`. Where the build
differs from the text below:

- **The panel's presentation key is Text▾** (the large-and-small A), not a
  second `Aa▾`. The reason is under point 3.
- **List▾ does not light, and Style never reads "Quote".** The published
  `EditorState` says whether the caret's item has a box (Checklist lights by
  it) and whether it is in a code block, but not whether it is in a list or a
  quote. Lighting either would mean asking core for one more fact on every
  frame.
- **Style offers H1 to H3.** `setHeading` takes 1 to 6 and the Format menu
  offers all six. The key still names H4 to H6 when the caret is in one.
- **No Math in Insert.** The model has no insert-math command yet.
- **The Mac's menus show no shortcuts.** SwiftUI draws a key equivalent
  beside a menu row only by registering it, and a chord registered by a view
  fires wherever the window's focus is: ⌘B in a host's sidebar field would
  bold the document. The menu bar's Format menu keeps the chords, gated on
  the focused editor.
- **The panel's Link asks through a sheet** when no host has claimed
  `onEditLink`. A popover on the key would go away with the panel, because
  the field takes focus and the text view resigns.
- **No panel on a Mac.** `Aa` is left off where an iPad or iPhone app runs
  on macOS, because there is no soft keyboard there to stand in for.
- **The demo app hangs the row over the keyboard on iOS** (`LeafEditor(model:
  accessory:)`), where it used to stack the bar over the editor as on the
  Mac, and its display chrome moves to the navigation bar. The row there is
  the nine tools and doesn't scroll.

`accepted` on 2026-09-23. Adam's decision, after studying Pages, Apple
Notes, Bear and Obsidian: leaf-swift's `LeafFormattingToolbar` stops being
one row of every tool and becomes three renderings of one catalogue — a short
row above the iOS keyboard, a panel that takes the keyboard's place, and a
row of category menus on the Mac. Reordering is deferred, and Writing Tools
stays out of all three for now.

## The picture

The bar is one row of 31 tools in five groups — seven inline marks, eleven
block tools, nine presentation tools, indent, history — plus whatever the
host adds. It was built a group at a time, and each group earned its place;
together they are too many for either shape the row ships in.

On iOS the row is a keyboard accessory, and at accessory metrics it runs to
about 1300 points on a phone roughly 400 wide. It scrolls, which means about
nine tools are on screen and twenty-two are somewhere to the right. The
scroll has no affordance beyond a button cut in half at the edge, and which
tools are visible depends on where the reader last left the row rather than
on what they are likely to want next. Bold is always one flick away from
being out of reach.

On the Mac the row is a strip over the document and pages by group. The
paging was the right fix for a pointer that cannot flick, but at a typical
window width the strip is two or three pages long, and a page turn to find
Align is a click that tells the reader nothing about where Align went.

## What the others do

**Apple Notes** puts five things on the row above the keyboard (a table, `Aa`,
a checklist, attachments, markup) and the rest behind `Aa`. `Aa` replaces the
keyboard with a formatting sheet the keyboard's height: styles (Title,
Heading, Subheading, Body, Monostyled), B I U S, the lists, block quote,
indent, colour. A close button brings the keyboard back. The row is what a
note-taker reaches for mid-sentence, and the sheet is everything else.

**Pages** on iPhone does the same thing with more depth: a short bar over the
keyboard, and a brush that opens a format panel where the keyboard was, with
paragraph styles first and the character and layout controls under them. On
the Mac, Pages puts the same controls in a Format inspector rather than a
toolbar strip. The toolbar holds *categories* (Insert, Table, Chart, Text,
Shape, Media), each a menu.

**Bear** has a short row as well, and its formatting panel is a *keyboard*: a
grid of keys the size and shape of the system's, white keys for formatting,
grey ones for Space, Return, Delete and caret movement. With the panel up the
writer can still type a space, start a new line and delete, so they can
restructure a list without calling the keyboard back up. Keys with more than
one form (heading level, list kind, code) carry a small ▾ and open their
variants on a long press.

**Obsidian** keeps everything on one scrolling toolbar, much as leaf does
today, and makes that bearable by letting the user choose and reorder what is
on it. It shows that reordering works, and also what it costs: the default
row is long, and the fix is left to the user.

## The decision

1. **One catalogue.** Every tool is defined once: a stable string ID, a
   glyph (an SF Symbol or a text label), a localized name, its action, its
   active state, its enablement and capability gate, and optionally the
   variants behind a ▾. The iOS row, the iOS panel and the macOS bar are
   three arrangements of that catalogue, so they cannot disagree about what
   Bold does or when Underline is dark. The IDs are stable because user
   reordering is a likely next step. It is not built now, but nothing about
   the arrangement should prevent it.

2. **iOS: a short row that fits.** `Aa · Style▾ · B · I · U · Checklist ·
   List▾ · Insert▾ · Link`, then the host's tools. Nine targets fit an
   iPhone 15 to 17 (393–402 points) at accessory size without scrolling.
   These are the tools a writer uses in the middle of a sentence, and they
   stay where the thumb expects them. When host tools, or a large Dynamic
   Type size, push the row past the width, it falls back to the scroll it
   has today.

3. **iOS: a panel that replaces the keyboard.** `Aa` swaps the text view's
   input view for a panel the keyboard's height. The row stays above it and
   `Aa` stays lit, and pressing `Aa` again brings the keyboard back. The
   caret, the selection and first responder all survive the swap, because
   the text view never resigns: it re-reads its input views. The panel is a
   6×4 key grid in Bear's manner:

   | Style▾ | B | I | U | S | ⌫ |
   | --- | --- | --- | --- | --- | --- |
   | List▾ | Checklist | Quote | Code▾ | Highlight▾ | Link |
   | Align▾ | Text▾ | Outdent | Indent | Move ↑ | Move ↓ |
   | Insert▾ | Undo | Redo | Space (two cells) | ↩ |

   Text▾ is the presentation vocabulary (size, font, colour, line spacing).
   It was first sketched as a second `Aa▾`, but `Aa` on the row already
   means "show the panel", and two keys with one glyph and two jobs would be
   one too many. So it wears the large-and-small A and is called Text.

   It keeps Delete, Space and Return, as Bear's does, and they go through
   the text view's own `insertText`/`deleteBackward`. That makes them the
   same keystrokes the keyboard sends: one undo history, Return continuing a
   list, Return in a table dropping a cell. A tap does a key's primary action,
   and a press-and-hold opens its variants. A key with no primary action
   (Style, Align, Text, Insert) opens its variants on a tap.

   Together, the row covers what is used most often and the panel covers the
   rest. Nothing is hidden behind a scroll.

4. **The Mac: categories.** The strip becomes a row of category menus:
   Style▾ (which shows the current style's name), Format▾, Align▾ (whose
   glyph is the current alignment), Lists▾, Text▾, Insert▾, then the host's
   tools. Each menu ticks what is in force and shows the Format menu's
   shortcut beside each item. Six categories fit a narrow window, and
   `ToolbarPaging` remains the fallback below that. Its arithmetic already
   takes a width per group, so a Style button wider than a glyph is only a
   wider group.

   Undo and Redo are left off. They are the Edit menu's, with ⌘Z and ⇧⌘Z,
   and a Mac user doesn't look for them on a formatting strip. On iOS they
   are in the panel, where there is no menu bar.

## What this leaves out

- **Reordering and a customize sheet.** Deferred. The stable IDs are the
  part that has to be right now, because reordering would persist them.
- **Writing Tools.** Deliberately left off every surface for now. The system
  already offers them in the edit menu on both platforms, and a button of
  our own would be a second entrance whose state (available, running, what
  it applies to) the system owns.
- **A second panel page.** One grid holds everything the old row had except
  Undo and Redo on the row, and those are in the panel. A tool that doesn't
  fit a 6×4 grid should go into a ▾ before the panel grows a second page.
