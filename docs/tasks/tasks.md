---
title: Tasks
description: Deferred work on leaf, one file each — a commitment with a done state
created: 2026-09-04
updated: 2026-09-20
contents:
- '[Bold inside code inserts visible delimiters](bold-inside-code-inserts-delimiters.md)'
- '[A typed dollar mints math in the hidden mode](a-typed-dollar-mints-math-in-the-hidden-mode.md)'
- '[Bold over italic does not toggle off](bold-over-italic-does-not-toggle-off.md)'
- '[Table cells omit active formatting from the toolbar](table-active-formatting-missing.md)'
- '[Selecting a formatted word drops its final character](formatted-selection-drops-last-character.md)'
- '[Clicks left of a table jump to its last cell](table-left-margin-hit-testing.md)'
- '[Table clicks lose inline markup source offsets](table-inline-markup-hit-testing.md)'
- '[Table caret and selection drift after Unicode](table-unicode-geometry.md)'
- '[The find bar covers the first line of a document at its top](find-bar-covers-first-line.md)'
- '[Spelling and autocorrect in the macOS view](macos-text-checking.md)'
- '[Writing Tools in the Apple views](writing-tools.md)'
- '[Find on iOS](ios-find-interaction.md)'
- '[Right-to-left text](right-to-left-text.md)'
- '[A host hook for directives in the web editor](web-directive-hook.md)'
- '[The frame crosses the binding whole on every keystroke](wasm-frame-crosses-whole.md)'
- '[An empty last line in a fence has no row](empty-last-line-in-a-fence-has-no-row.md)'
- '[SVG through resvg-swift](svg-through-resvg-swift.md)'
- '[A link reference definition''s lines are drawn as blank rows](link-definitions-are-blank-rows.md)'
- '[The heading the caret is under is a question `Doc` cannot answer](heading-at-offset.md)'
- '[`can_undo` counts edits, and twig counts steps](can-undo-counts-edits-not-steps.md)'
- '[Backspace at the start of a table cell eats the column separator](backspace-at-cell-start-eats-the-separator.md)'
- '[The rule and table buttons at a paragraph''s end split nothing and leave blank lines behind](rule-and-table-at-a-paragraph-end-split-nothing-and-leave-blank-lines.md)'
- '[The macOS formatting bar pages by group instead of scrolling](macos-bar-pages-by-group.md)'
- '[A page break in HTML and AsciiDoc is a row the walker does not draw](page-break-in-html-and-asciidoc.md)'
- '[Dynamic Type on paper should be a zoom, not a repagination](ios-dynamic-type-on-paper.md)'
- '[The Mac app runs unsandboxed and unsigned](sandboxed-mac-app.md)'
- '[An exported PDF leaves every formula blank](pdf-export-lacks-math.md)'
- '[The Apple editor does work proportional to the document on every interaction](editor-is-proportional-to-the-document-per-interaction.md)'
- '[A PDF shows the caret''s line revealed](pdf-reveals-the-carets-line.md)'
part_of: '[leaf](/README.md)'
---
# Tasks

Work that has been deferred, not work that is planned. A task lands here when
it is worth doing and will not be done in the commit that noticed it; a fix
that takes ten minutes gets a commit, not a file. Each carries `status`
(`open`, `in-progress`, `done`, `dropped`) in its frontmatter — `dx tasks`
reads the key — and closing one is an edit, naming the commit or release
that resolved it, never a deletion. What is done keeps its place in the list
above — the index is the spine, and what is open is a view of it (`dx tasks`)
— and stays findable by grep.

What this is not: a commitment to consumers, which goes in the changelog's
unreleased region; or a description of what shipped, which goes in the
guides.
