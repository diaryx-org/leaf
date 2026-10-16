---
title: What the Mac app does not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-15
---
# What the Mac app does not do yet

**Where.** `apps/leaf-mac` (the SwiftUI host), `crates/leaf-dioxus-embed`
(its C ABI), and `crates/leaf-dioxus` (the editor).

**What.** Leaf for the Mac moved from LeafUI's AppKit editor to the
leaf-dioxus editor in a native document app. The document system, the menus,
the toolbar, Settings, the context menu, formulas, Mermaid diagrams, tables,
pictures, find and replace, the source view, Export as PDF and Print came
across. What the LeafUI app did that this one does not, each a piece of work
of its own:

- **System text services.** Spelling and grammar (red waves, guesses in the
  context menu), autocorrect, smart quotes and dashes, text replacement,
  Writing Tools, Look Up by force click, data detectors, Speech and Services.
  All of them read the text through the text input client, whose
  `attributedSubstring(forProposedRange:)` and `selectedRange()` answer
  nothing yet; answering them from core's offsets is the first step for each.
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
- **Accessibility.** The editor is not yet a VoiceOver text area.
- **Remote media.** A picture at a web URL draws as a chip: there is no
  counterpart to `onResolveMedia` fetching it.

**Done when.** Each is either done in the Mac app or deliberately left out,
with this list updated as they go.
