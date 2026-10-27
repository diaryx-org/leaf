---
title: The editor and Blitz fall back to different faces
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
---
# The editor and Blitz fall back to different faces

**Where.** `crates/leaf-dioxus/src/text.rs` (where parley breaks the
editor's lines) and the CSS family list each line is drawn in
(`apps/leaf-dioxus/src/settings.rs`, `family`).

**What.** When the chosen body face is not installed, as Iowan Old Style,
the default, is not on Linux, the editor's line breaking and Blitz's
drawing pick different fallbacks. parley breaks a line at the width it
measured in one face; Blitz sets the line's text in the face its CSS
fallback (`"Iowan Old Style", Georgia, serif`) resolves to, which is
wider, and wraps it again inside the editor's line. The line is drawn two
lines tall, over the line below.

`sheet.rs`'s `a_quotes_bars_meet_down_its_lines_and_rows` catches it: on
Linux a wrapped line in a quote is 48px tall beside a 24px bar. It runs on
the Mac only until this is fixed.

**Done when.** Both resolve one face for a family list (the editor asks
Blitz's font context, or both are given the face file), the test runs on
Linux again, and a document set in a face that is not installed draws each
line once.
