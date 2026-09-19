---
status: open
created: 2026-09-04
updated: 2026-09-18
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A host hook for directives in the web editor

**Still open, with one directive drawn by hand.** The presentation
vocabulary's page break needed exactly the narrow half of this: `render` reads
`DocView.directives`, and a row whose directive is named `page-break` is built
as a dashed rule instead of the placeholder (`_pageBreakRowEl`). That is not
the hook — the name is leaf's own vocabulary, so the editor needs no host to
tell it what it means, and the row is an atom holding *no* caret rather than
one with a stop on each side. What it does settle is the row-length mapping
this task worried about: the media row's `data-leaf-core-len` marker is now
`atomCoreLen`, general to any row the renderer draws its own element on, and a
hook's atom would use it as it stands.

**Where.** `packages/leaf-web/src/editor.js`, in `render`, beside the way a
`TableView` replaces its picture rows.

**What.** The binding publishes every leaf directive (`::name{…}`) in
`DocView.directives`, with its rows, name, label and attributes, expressly so a
renderer that knows the host app's vocabulary can paint the real thing — an
`<iframe>` for an `::embed{src=…}`, say — over the `⧉ name` placeholder row.
The web editor ignores the list and paints the placeholder.

**How.** An `EditorOptions.directive(view) => HTMLElement | null` hook, asked
per directive on each frame; a non-null answer is wrapped as a
`contenteditable="false"` atom standing in for the rows in
`[start_row, end_row)`, keyed and reused like a media row. The caret has to be
able to stand before and after it — the media row's zero-width-space stops are
the model — and `_domPoint` / `_rangeForRow` must map those rows through the
atom the way `atomCoreLen` does, or the caret placed on a replaced row would
address nothing and be dropped. That mapping is the whole of the work; the
hook itself is a dozen lines.

**Done when.** The demo paints one directive kind through the hook, the caret
steps over it in one keypress in each direction, and a test in
`test/editor.test.html` shows a row replaced by the hook round-trips a caret
through `_syncFromDom`.
