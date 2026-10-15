# Upstream PR, prepared and not sent

The leaf patch to winit-appkit (see `../PATCH.md`), redone against winit's
`master` as one commit, ready to open as a pull request on
`rust-windowing/winit`. **Not sent: Adam approves it first.**

- Base: winit `250cd01a` ("wayland: ignore queued updates for destroyed
  windows (#4760)"), 2026-10-15.
- Patch: `0001-macOS-surrounding-text-replacement-ranges-and-keyles.patch`
  (`git am` it onto that base). It touches `winit-appkit/src/view.rs`,
  `winit-appkit/src/window_delegate.rs` and `winit/src/changelog/unreleased.md`;
  none of the leaf-only `LEAF_TRACE` logging or `leaf patch` markers.
- Checked on that base: `cargo build`, `cargo clippy --all-targets` (no
  warnings) and `cargo test` for `winit-appkit`, on macOS.

## Before opening it

1. `cargo +nightly fmt -p winit-appkit` — winit formats with nightly-only
   rustfmt options, and this machine's nightly has no rustfmt component, so the
   patch has not been through it.
2. Try it with winit's own `examples/` (an IME-using example) as well as leaf:
   reviewers will ask how to reproduce it without Dioxus.
3. Decide whether to split it: the surrounding text and replacement ranges
   (press-and-hold) and the keyless `insertText:` (picker, dictation) are
   independent, and the repeat rule is the part a reviewer is likeliest to push
   back on. One PR keeps the story whole; two would land the uncontroversial
   half sooner.

## Title

macOS: surrounding text, replacement ranges and keyless insertText in the text input client

## Description

winit's `NSTextInputClient` on macOS answers nothing about the document:
`selectedRange` is always `{NSNotFound, 0}`, `attributedSubstringForProposedRange:actualRange:`
always `nil`, and the `replacementRange` of `insertText:` and `setMarkedText:` is
ignored — two `TODO`s say it "requires changing the event to report surrounding
text", which winit-core has since done with `ImeRequestData::surrounding_text`
and `Ime::DeleteSurrounding`. And `insertText:` is reported only when it ends a
composition, on the assumption that any other text arrives as `KeyboardInput`.

Two macOS text features fall through those gaps:

- **Press-and-hold accents.** The input system reads the character before the
  caret, marks it, and replaces it with the chosen accent. With no text to read
  it never opens its menu, and the held key just repeats.
- **The Emoji & Symbols picker**, dictation and services insert with
  `insertText:` and no keystroke behind it, so their text is dropped.

This PR:

- keeps the surrounding text the application reports
  (`ImeRequestData::with_surrounding_text`, with the capability) and answers
  `selectedRange` and `attributedSubstringForProposedRange:actualRange:` from it;
- resolves a `replacementRange` against it and reports `Ime::DeleteSurrounding`,
  relative to the caret, before the `Ime::Commit` in `insertText:` or the
  preedit in `setMarkedText:` (press-and-hold marks the letter it offers
  accents for);
- reports `insertText:` from outside `keyDown:` as `Ime::Commit`, with
  `Ime::Enabled` first if the application has not had it;
- does not forward a key repeat the input system swallowed — nothing inserted,
  marked or commanded, which is press-and-hold holding the key under its menu —
  nor the key that then picks from the menu, as `KeyboardInput`.

An application that does not ask for the surrounding-text capability sees the
old behaviour, except that keyless insertions now arrive as `Ime::Commit` and a
swallowed repeat is no longer forwarded.

Found and tested in an editor built on Dioxus native (Blitz), which reports the
caret's line as surrounding text and handles `DeleteSurrounding`: press-and-hold
accents (with the digit, the arrow keys and a click choosing), their undo, and
emoji from the picker, on macOS.

- [x] Tested on all platforms changed (macOS)
- [x] Added an entry to the `changelog` module
- [ ] Updated documentation — `ImeRequestData::with_surrounding_text`'s docs
      could say macOS now uses it
- [ ] Example program — none; reproducible with any IME-enabled example that
      reports surrounding text
