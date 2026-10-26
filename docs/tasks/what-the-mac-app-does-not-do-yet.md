---
title: What the Mac app does not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-15
updated: 2026-10-26
---
# What the Mac app does not do yet

**Where.** `apps/leaf-dioxus` (the app, and `src/platform/macos`, its AppKit
half) and `crates/leaf-dioxus` (the editor).

**What.** Leaf for the Mac moved from LeafUI's AppKit editor to the
leaf-dioxus editor, first in a SwiftUI document app and then in a Rust one,
its windows Dioxus pages and its AppKit reached through objc2. The document
system (Open, autosave in place, Versions, Rename, Move To, Revert, Open
Recent, Finder's Open With, an untitled document's question as it closes),
the menus, the toolbar, Settings, the context menu, formulas, Mermaid
diagrams, tables, pictures, find and replace, the source view, spelling with
its substitutions, Look Up, Export as PDF and Print came across. What the
LeafUI app did that this one does not, each a piece of work of its own:

- **The rest of the system text services.** Done: spelling (underlines,
  guesses, Ignore and Learn, Check Document Now), autocorrect, smart quotes
  and dashes, text replacement, and Look Up by force click and from the
  context menu. Still to do: grammar; the spelling panel (Show Spelling and
  Grammar); the blue underline under an autocorrected word and the offer to
  revert it; underlines inside a table's cells; Writing Tools; data
  detectors; Speech and Services; and Look Up by a three-finger tap, which
  winit's view does not pass on (`quickLookWithEvent:`).
- **Windows reopening.** A document window is not restored when the app
  relaunches, as `DocumentGroup`'s were: winit's windows are not restorable.
  Each reopens at its saved frame when the document is opened again.
- **Peeks.** A footnote's note, and a link's target, in a popover on hover.
- **Media playback.** Video plays inline and audio in its strip; here a
  video is its poster and audio a chip.
- **Drag and drop.** The selection dragged out or within, a block dragged by
  its margin, a picture dragged to the Finder, and a file or image dropped in
  or pasted.
- **The page view.** The document on sheets on screen, two columns, and the
  zoom's Fit Width and Fit Page. Print and PDF are paginated; the window is
  not.
- **Region editors.** LeafUI's structural formula editor and Mermaid's
  side-by-side preview. Here a formula's TeX shows while the caret is on its
  line (a display formula's in the source view), and a fence shows its code
  while the caret is in it.
- **The host's own regions.** The date chip (`:date[…]`) and the embed card
  (`::embed{…}`) the iOS app draws; Insert ▸ Embed….
- **The presentation menus.** Alignment, text size, font, text colour and
  line spacing for a paragraph or a run (`PresentationRows.swift`), and the
  Paragraph spacing setting.
- **Keys.** Cocoa's standard key bindings beyond the editor's ⌃A/E/K/F/B/N/P/D/H:
  the kill ring with ⌃Y, ⌃T to transpose, ⌃O, the mark (`setMark`,
  `selectToMark`, `swapWithMark`), the scrolling commands, Transformations
  (upper case, lower case, capitalise), selecting a paragraph or a line, and
  the user's own `DefaultKeyBinding.dict`. LeafUI did them in
  `archive/packages/leaf-swift/Sources/LeafUI/LeafTextView.swift` (near lines
  2798–3200).
- **Trackpad zoom.** Pinch (`magnify(with:)`) and smart zoom
  (`smartMagnify`), in the same file near lines 189–205.
- **A window that is not key.** LeafUI blinked the caret (`resetBlink`) and
  dimmed the selection when its window was not key (`keyStateChanged`).
  Neither was found in the editor; to check.
- **The context menu's other items.** Share…, Paste and Match Style, and
  Writing Tools (`LeafTextView.swift` near lines 3400–3445).
- **Remote media.** A picture at a web URL draws as a chip: there is no
  counterpart to `onResolveMedia` fetching it.

Not yet seen on a screen, for want of the app in front during the session
that wrote it: Look Up's popover, the print panel, and Export's save sheet.
The PDF they print is unit-tested (`pdf.rs`).

LeafUI is archived (`archive/packages/leaf-swift`); its files are where to
look for how it did each of these. The document is a VoiceOver text area
since `46711670`, so accessibility is off this list.

**Done when.** Each is either done in the Mac app or deliberately left out,
with this list updated as they go.
