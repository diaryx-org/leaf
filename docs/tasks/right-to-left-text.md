---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# Right-to-left text

**Where.** `crates/leaf-core` first, then every frontend; `packages/leaf-swift`
hard-codes left-to-right in `baseWritingDirection(for:in:)` and the row shaping.

**What.** A Hebrew or Arabic paragraph lays out left-to-right with its caret
motion reversed against the glyphs. Core's visual map has no notion of base
direction, so no frontend can answer the system's writing-direction questions
truthfully.

**How.** Core learns a paragraph's base direction (first strong character, or
an explicit mark) and carries it on the row; Core Text is handed the direction
as a paragraph style, and caret stops map through the bidi-reordered line. The
Swift views then report the real direction from `baseWritingDirection` and lay
the caret and selection rects by the reordered positions.

**Done when.** A right-to-left paragraph renders right-aligned with correct
caret motion in both Apple views, and the terminal and gpui frontends at least
render it in order.
