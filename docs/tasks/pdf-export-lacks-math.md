---
status: open
created: 2026-09-20
updated: 2026-09-20
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# An exported PDF leaves every formula blank

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
