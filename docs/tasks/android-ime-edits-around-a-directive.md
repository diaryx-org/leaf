---
status: open
created: 2026-09-30
updated: 2026-09-30
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Keyboard edits over a leaf directive corrupt the source on Android

**Where.** `packages/leaf-android/leaf-compose`: `LeafInputConnection`, and
the UTF-16 view of the document it hands the IME, over a leaf directive's
`⧉ name` placeholder row — whose glyphs all carry the directive's start
offset, with the row's end past it.

**What.** With Gboard attached (the emulator's hardware keyboard goes through
it), a key over a leaf directive edits the wrong source. From a document

```
# Directives

Before the embed.

::embed{src="https://example.com/talk"}

After the embed.
```

put the caret on the directive's after stop (Right, Right from the end of
`Before the embed.`), select back over it (Shift+Left) and press Delete: the
source loses only the closing `}`. Ctrl+Z then leaves

```
# Directives
Before the embed.


After the embed.
```

— the directive gone and the blank line under the heading with it. Typing a
letter at either stop and undoing it through the same path has also written
a literal `⧉` into the source. Deleting `}` alone is what an IME's
`deleteSurroundingText(1, 0)` at the directive's end would do, so the first
suspect is how an IME's edits over the placeholder's characters map back to
source offsets; that core's own `backspace` and `undo` do the right thing
from the same selection is not yet checked.

It reproduces with and without a host drawing the directive (the
[host directives](../proposals/host-directives.md) hook changes the drawing,
not the row core sends), so it predates the hook; the hook makes it easier to
reach, since a drawn directive is something one selects and deletes.

**Done when** selecting a directive and pressing Delete removes the whole
directive and Undo restores exactly the text before it, with Gboard attached,
and a letter typed at either stop and undone leaves the source as it was.
