---
title: What iOS does not do yet
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-26
updated: 2026-10-26
---
# What iOS does not do yet

**Where.** `apps/leaf-dioxus` (it has no iOS module under `src/platform/`
yet), `crates/leaf-dioxus`, and the patched winit-uikit under `patches/`.

**What.** Leaf for iOS was `archive/apps/leaf-editor` over LeafUI's UIKit
view, `archive/packages/leaf-swift/Sources/LeafUI/LeafTextViewiOS.swift`.
What it did that the Dioxus path does not, with where the archived code shows
how:

- **Text input.** A full `UITextInput`: marked text, a tokenizer, geometry,
  and `inputDelegate` notifications (`LeafTextViewiOS.swift`, its
  `UITextInput` sections and `LeafTokenizer`). winit-uikit implements only
  `UIKeyInput`, so there is no CJK marked text, autocorrect or dictation into
  the caret. The editor already takes winit's `Ime` events (`ime.rs`).
- **Input traits.** Autocorrect, spell-check, smart quotes and dashes,
  autocapitalisation and the return key (`LeafTextViewiOS.swift`, near
  lines 762–823). Return arrives as `insertText("\n")`; check that the editor
  takes a committed `"\n"` as a new line.
- **Selection.** `UITextInteraction`'s loupe, long-press select, and the edit
  menu with leaf's own items (`textInteraction`, `UIEditMenuInteraction`).
  The editor draws its own handles (`touch.rs`, `row.rs`) and gives hooks for
  a menu (`Editor::dragging_grip`, `drawn_rect_of`), which no host uses yet.
- **The keyboard.** The formatting row over the keyboard, the panel that
  replaces it, and the caret kept above it (`FormattingToolbar.swift`,
  `FormattingPanel.swift`, `ToolCatalogue.swift`, `inputAccessoryView` and
  `inputView`; the proposal "Keyboard panel"). winit does not report the
  keyboard's frame (proposal "A shared patched winit for the Dioxus work"),
  and `Editor::set_bounds` has no iOS caller.
- **VoiceOver.** Line numbers, line content and frames, and announcements
  (`LeafTextViewiOS.swift` near lines 2788–2877, `regionAnnounce`). The
  editor's AccessKit text area (`access.rs`) is not wired to an iOS host;
  `spikes/blitz-ios-a11y` is the start.
- **Writing Tools**, inline and in the panel (`WritingTools.swift`; the task
  "Writing Tools in the Apple views").
- **Zoom and type size.** Pinch zoom and Dynamic Type (`handlePinch`,
  `ZoomView.swift`, `Zoom.swift`, `traitCollectionDidChange`), and page
  sheets (`Sheets.swift`, `PageSetup.swift`).
- **Peeks and media.** Link and footnote cards on a tap, inline playback,
  and a picture dragged out (`LinkPeek.swift`, `FootnotePeek.swift`,
  `MediaPlayback.swift`, `AudioStrip.swift`, `PictureDrag.swift`).
- **The app.** The document browser (`DocumentGroup`), the document types,
  the date chip, the embed card and the Mermaid fence
  (`archive/apps/leaf-editor/App/*.swift`, `project.yml`).

The system find panel (`UIFindInteraction`) is not on the list: the editor's
own find bar does the same job.

**Done when.** Each is done in the Dioxus app on iOS or deliberately left
out, with this list updated as they go.
