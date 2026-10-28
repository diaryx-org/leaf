---
title: A quote's bar breaks a line Blitz was told not to wrap
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
updated: 2026-10-27
---
# A quote's bar breaks a line Blitz was told not to wrap

**Where.** parley 0.12.0's line breaker (`layout/line_break.rs`), which
Blitz lays each of the editor's lines out with; the quote gutter's
`.qbar` box (`crates/leaf-dioxus/src/row.rs`) is what meets it.

**What.** A wrapped line in a quote can be drawn two lines tall, over the
line below. Each line is a `white-space: pre` div whose first child is the
gutter's bar, an inline-block a quote level wide, then the line's text,
which keeps the space it wrapped at. The editor's own parley layout lets
that space hang past the width, as wrapping text does; under `pre` it
does not hang, so a line whose text fits only without it overflows in
Blitz by less than a space. Overflowing is right for `pre`. Breaking is
not, and parley breaks: it lags `text_wrap_mode` one atom behind the
text, starting from `Wrap`, so a box before the first atom of a layout
is read as wrappable and marks a break after it. The text then goes to a
second line, under a bar one line tall.

`sheet.rs`'s `a_quotes_bars_meet_down_its_lines_and_rows` catches it on
Linux, where the default face is not installed and Liberation Serif sets
the paragraph: "The quick brown fox … again, until " is 605.64px with its
space, beside a 22px bar in a 624px line, and Blitz lays the bar alone on
line 1 and the text on line 2 (48px beside a 24px bar). It runs on the
Mac only until this is fixed.

It is not the faces, as this task first said. The editor and Blitz both
set that line in LiberationSerif (each glyph run's PostScript name,
checked 2026-10-27): both use parley over the system collection with one
family list. On the Mac the same line in Iowan Old Style happens not to
land within a space of the width.

parley alone shows it: a 22px box and a text under
`TextWrapMode::NoWrap`, broken at 50px.

| box at | lines |
|---|---|
| none | 1 |
| the end | 1 |
| byte 4 | 1 |
| byte 0 | 2 |

parley's `main` (dd6fe3ae, 2026-10-26) has made in-flow boxes inline
objects inside text runs, which may take the run's wrap mode, and still
lags the mode one atom. Not tried: it is unreleased, and did not resolve
against this toolchain's index.

**How**, one of:

- **Fix it upstream** in parley (seed the line's wrap mode from the first
  item's style, or read a box's own) and take it through the Blitz fork.
- **Patch parley** in the Blitz fork until then, as winit is patched.
- **Keep the bar out of the line's text flow** in leaf: drawn by the line
  (an absolutely placed bar beside `padding-left`) rather than as an inline
  box, so no layout begins with a box.

**Done when.** A quoted line that fits the editor's width with its
trailing space hanging is drawn one line tall by Blitz, and the test runs
on Linux again.
