---
status: done
created: 2026-09-20
updated: 2026-09-20
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# An exported PDF leaves every formula blank

**Status.** Done, in `fix(leaf-swift): a formula's ink on paper is resolved
under the sheet's appearance, not the screen's` (2026-09-20). The "Why" below
was a guess and wrong — the typesetter is synchronous, and every picture was
on the page. It was *white*. A formula's ink is bytes handed to the
typesetter, resolved from the theme's dynamic `labelColor` when the row is
laid out, and that resolution read whichever appearance was current — the
dark-mode window the export was asked from — where the sheet itself is
`aqua` and its paper white. The text beside it was fine because its colours
are dynamic and resolve when drawn, under the sheet's own appearance. Both
views now build their layout under their own appearance (`effectiveAppearance`
on the Mac, `traitCollection` on iOS, with the iOS sheet overridden light),
and a light/dark switch on screen is a relayout, since the shaped rows hold
pictures in the old ink. The assertion is
`testAFormulaIsInkOnThePaperWhateverTheScreensAppearance`, on both toolkits.

**Where.** `packages/leaf-swift`, `DocumentPDF.swift` and `MathLayout.swift`.

**What.** File ▸ Export as PDF… in the Mac app writes the page with a blank
where each formula is — the sample's `$E = mc^2$` and its display integral are
both gaps of the right size. On screen the same document shows them.

**Why.** A formula's picture is typeset lazily and arrives after the first
layout that asks for it; the screen repaints when it lands. `paperSheet(of:)`
lays out a fresh view and draws it once, before any picture has arrived, so
the PDF is the first paint. Print goes through the same sheet and will show
the same gaps.

**How.** Have the PDF (and print) sheet typeset every formula synchronously
before drawing — the typesetter is in-process, so there is nothing to wait
on but the work — or reuse the on-screen view's math cache when the export
is made from a window that already has the pictures.

**Repro.** `cargo xtask swift`, File ▸ Export as PDF…, open the PDF.
