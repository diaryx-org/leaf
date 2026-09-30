---
status: open
created: 2026-09-30
updated: 2026-09-30
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Ctrl+Z under Gboard undoes the keyboard's text, not leaf's step

**Where.** `packages/leaf-android/leaf-compose`: `LeafInputConnection`, and
how a hardware Ctrl+Z reaches `LeafEditorState.key` when Gboard is the IME.

**What.** With Gboard attached (the emulator's hardware keyboard goes through
it), Ctrl+Z does not run leaf's undo when the last edit changed structure. From

```
# Directives

Before the embed.

::embed{src="https://example.com/talk"}

After the embed.
```

step onto the directive's before stop (Right from the end of `Before the
embed.`) and type `x`: core opens a paragraph above the directive and puts the
`x` in it. Ctrl+Z then takes out the `x` and leaves the empty paragraph — the
source keeps an extra blank block — where the formatting bar's ↶ restores the
document exactly. Selecting the directive and deleting it, then Ctrl+Z, leaves
blank lines and no directive; ↶ brings the directive back. The keyboard
appears to answer Ctrl+Z itself, by reverting its own last commit through the
input connection, so leaf's `undo` never runs.

Before `36a466b` the same path also wrote a lone `⧉` or dropped only the
directive's closing `}`; that core fix put typing and deleting at a
directive's stops right, and what is left is the undo.

**Done when** Ctrl+Z with Gboard attached runs leaf's undo — the directive
cases above come back exactly as ↶ brings them — or the editor is shown to
receive Ctrl+Z and a note here says why it cannot.
