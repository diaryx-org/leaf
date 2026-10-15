# winit-appkit 0.31.0-beta.3, patched

The crate dioxus-native pins, vendored unmodified in the commit before this
file's and patched after it, so `git log -p -- .` is the patch. Every change
is marked `leaf patch` in the source.

## What was wrong

winit's macOS view implements `NSTextInputClient`, but answers none of its
questions about the document:

- `selectedRange` is always `{NSNotFound, 0}`, and
  `attributedSubstringForProposedRange:actualRange:` always `nil`;
- the `replacementRange` of `insertText:` and `setMarkedText:` is ignored
  (two `TODO`s: "requires changing the event to report surrounding text" —
  which winit-core 0.31 since did, with `ImeRequestData::surrounding_text`
  and `Ime::DeleteSurrounding`, but the AppKit backend never used them);
- `insertText:` reports text only when it ends a composition, on the
  assumption that anything else arrives as a `KeyboardInput`.

Two macOS text features fall through those gaps:

- **Press-and-hold accents.** The input system needs to read the character
  before the caret, marks it, and replaces it with the accent chosen. With
  no text to read it never opens the menu, and the held key just repeats.
- **The emoji and symbols picker** (and dictation, and services) inserts with
  `insertText:` and no keystroke at all, so its text was dropped.

## What the patch does

- Keeps the text around the caret that the application reports with
  `ImeRequestData::with_surrounding_text` (`window_delegate.rs`), and answers
  `selectedRange` and `attributedSubstringForProposedRange:` from it.
- Resolves a `replacementRange` against that text and reports it as
  `Ime::DeleteSurrounding` relative to the caret — before the replacement's
  `Ime::Commit` in `insertText:`, and before the preedit in `setMarkedText:`
  (press-and-hold marks the letter it is offering accents for).
- Reports `insertText:` from outside `keyDown:` as `Ime::Commit`.
- Drops a key *repeat* the input system swallowed — `interpretKeyEvents:`
  reached none of insert, mark or command — instead of forwarding it as
  `KeyboardInput`: that is press-and-hold holding the key under its menu.

An application that never asks for the surrounding-text capability gets
exactly the old behaviour, except that picker and dictation text now arrives
as `Ime::Commit`, and that a swallowed repeat is no longer forwarded.

## Upstream

Worth offering to winit as it stands: it fills in the two `TODO`s with
the API winit-core already defines. The repeat rule is the part to argue
about; it is scoped to repeats, which only press-and-hold swallows.
