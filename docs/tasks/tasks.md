---
title: Tasks
description: Deferred work on leaf, one file each — a commitment with a done state
created: 2026-09-04
updated: 2026-09-04
contents:
- '[The find bar covers the first line of a document at its top](find-bar-covers-first-line.md)'
- '[Spelling and autocorrect in the macOS view](macos-text-checking.md)'
- '[Writing Tools in the Apple views](writing-tools.md)'
- '[Find on iOS](ios-find-interaction.md)'
- '[Right-to-left text](right-to-left-text.md)'
- '[A host hook for directives in the web editor](web-directive-hook.md)'
- '[The frame crosses the wasm boundary whole on every keystroke](wasm-frame-crosses-whole.md)'
- '[An empty last line inside a code fence gets no row](empty-last-line-in-a-fence-has-no-row.md)'
---
# Tasks

Work that has been deferred, not work that is planned. A task lands here when
it is worth doing and will not be done in the commit that noticed it; a fix
that takes ten minutes gets a commit, not a file. Each carries `status`
(`open`, `in-progress`, `done`, `dropped`) in its frontmatter — `dx tasks`
reads the key — and closing one is an edit, naming the commit or release
that resolved it, never a deletion. What is done stays findable by grep and
leaves the list above.

What this is not: a commitment to consumers, which goes in the changelog's
unreleased region; or a description of what shipped, which goes in the
guides.
