---
title: LeafUI's own toolbar still defines every control a second time
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
---
# LeafUI's own toolbar still defines every control a second time

LeafToolbar (`packages/leaf-swift/Sources/LeafToolbar`) is the formatting
toolbar as a window's toolbar, over any `LeafToolbarEditor`. The Mac app
draws it over leaf-dioxus, and the iOS app draws it in an iPad's navigation
bar. LeafUI's `LeafFormattingToolbar` still has its own definition of the same
controls, in `ToolCatalogue.swift`, which feeds three surfaces: the iOS
keyboard row, the formatting panel, and the macOS strip (`Style.bar`). So
Bold is defined twice, and the two definitions can drift: they already
differ on which headings Style offers (1–3 against 1–6) and on what Format
holds.

**Done when** the macOS strip is gone and LeafUI's catalogue is built on
`LeafToolbarCommand` and `LeafToolbarState`:

1. Diaryx's Mac editor draws `LeafToolbarItems` in its window toolbar in place
   of `LeafFormattingToolbar(editor:tools:)`, with its attach, drawing and
   voice-memo tools as `ToolbarItem`s of its own
   (`apps/swift/App/Workspace/ContentView+Detail.swift` and
   `apps/swift/App/Library/VolumeLeaf.swift` in diaryx). Nothing else uses the
   strip, so `Style.bar` and its paging (`ToolbarPaging`) can then go.
2. `ToolCatalogue`'s tools call `perform(_:)` and read `toolbarState` rather
   than `LeafEditorModel`'s methods, and its menu rows are LeafToolbar's
   (`LeafHighlightColourRows`, `LeafTableRows`, `LeafAlignmentRows`), so the
   keyboard row, the panel and the window toolbar share one table.
   `HighlightColourRows` and `TableRows` in `FormattingToolbar.swift` are then
   wrappers or gone; `TableRows` is public, so check Diaryx first.
