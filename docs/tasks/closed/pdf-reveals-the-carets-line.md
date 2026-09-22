---
status: done
created: 2026-09-20
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# A PDF shows the caret's line revealed

**Status.** Done, in `fix: a PDF and a printout show the caret's line as
every other`. Core's `Doc::set_unrevealed` lays the map out with no line
revealed, setting the screen's map and caret aside and putting them back,
so the screen's next build is free. Both bindings carry it as
`paper_view()`, a whole frame that moves neither the frame count nor the
rows kept for the next change. The Swift paper sheet lays out from it (the
AppKit sheet through `layOutAsPaper()`, the UIKit one when `isPaper` is
set), and leaf-web renders it on `beforeprint` and the screen's frame again
on `afterprint`. `testTheCaretsLineIsNotRevealedOnPaper` prints the same
page with the caret on the formula's line and off it.

**Where.** `packages/leaf-swift`, `DocumentPDF.swift`; `crates/leaf-ffi`,
`LeafDoc::view`; `crates/leaf-core`, `Doc::reveal_line`.

**What.** The paper sheet is a second view over the *same* `LeafDoc`, and the
frame it lays out is the one the screen gets — caret line and all. Under
`MarkupMode::Full` that line's delimiters are raw on the page; in every mode a
formula on it is its TeX in the code style rather than its picture, since math
reveals on the caret's line whatever the mode. Export with the caret standing
on `$E = mc^2$` and the PDF says `E = mc^2` in monospace. Print goes through
the same sheet.

**Why.** `reveal_line` is a function of the document's caret, and the sheet
has no caret of its own to give it. Everything else that only exists on screen
— the caret itself, the selection, the landing flash, the placeholder cue — the
sheet already leaves off, because those are the *view's*; the reveal is
core's, folded into the rows before the view sees them.

**How.** A frame with no reveal, on request: a `LeafDoc` call the sheet uses
in place of `view()` — `set_unwrapped` is the shape — that builds with
`reveal: None`, keyed apart in the caches so it does not evict the screen's
build. Both bindings, since the web's print path has the same need. Noticed
while closing [An exported PDF leaves every formula blank](/docs/tasks/closed/pdf-export-lacks-math.md),
whose test keeps its caret on a line of prose for exactly this reason.

**Repro.** Open the sample, click into a formula, File ▸ Export as PDF….
