---
title: Tasks
description: Deferred work on leaf, one file each — a commitment with a done state
created: 2026-09-04
updated: 2026-09-21
contents:
- '[Spelling and autocorrect in the macOS view](macos-text-checking.md)'
- '[Writing Tools in the Apple views](writing-tools.md)'
- '[Find on iOS](ios-find-interaction.md)'
- '[Right-to-left text](right-to-left-text.md)'
- '[A host hook for directives in the web editor](web-directive-hook.md)'
- '[The heading the caret is under is a question `Doc` cannot answer](heading-at-offset.md)'
- '[`can_undo` counts edits, and twig counts steps](can-undo-counts-edits-not-steps.md)'
- '[Backspace at the start of a table cell eats the column separator](backspace-at-cell-start-eats-the-separator.md)'
- '[A page break in HTML and AsciiDoc is a row the walker does not draw](page-break-in-html-and-asciidoc.md)'
- '[Dynamic Type on paper should be a zoom, not a repagination](ios-dynamic-type-on-paper.md)'
- '[The Mac app runs unsandboxed and unsigned](sandboxed-mac-app.md)'
- '[A PDF shows the caret''s line revealed](pdf-reveals-the-carets-line.md)'
- '[A fence inside a quote or list has no language](fence-inside-a-container-has-no-language.md)'
- '[Closed tasks](/docs/tasks/closed/closed.md)'
part_of: '[leaf](/README.md)'
---
# Tasks

Work that has been deferred, not work that is planned. A task lands here when
it is worth doing and will not be done in the commit that noticed it; a fix
that takes ten minutes gets a commit, not a file. Each carries `status`
(`open`, `in-progress`, `done`, `dropped`) in its frontmatter — `dx tasks`
reads the key — and closing one is an edit, naming the commit or release
that resolved it, never a deletion. What is done then moves to
[Closed tasks](closed/closed.md) (`dx shelve`), which is part of this index —
the index is the spine, and what is open is a view of it (`dx tasks`) — and
stays findable by grep.

What this is not: a commitment to consumers, which goes in the changelog's
unreleased region; or a description of what shipped, which goes in the
guides.
