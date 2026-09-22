---
status: done
created: 2026-09-19
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# Dynamic Type on paper should be a zoom, not a repagination

**Status.** Done, in `fix(swift): on paper, iOS Dynamic Type zooms the
sheet instead of repaginating`. On paper `renderTheme` is the host's theme
and the factor is `typeZoom`, a multiplier on whatever `zoom` resolves to
(`Zoom.resolve(in:page:factor:)`): `.fitWidth` at AX3 is the fit times AX3's
factor, a `.scale` is the zoom before the factor so a pinch and View ▸ Zoom
move on from the product, and `zoomScale` reports it. A text-size change
re-resolves the zoom on paper and relayouts only in the flow; setting or
clearing `pageSetup` moves the factor between the type and the zoom. The
cap: none of its own — the product is held to `Zoom.range` like any zoom,
so a fit-width sheet at the largest sizes scrolls sideways, as it does in
Preview. The PDF sheet takes no factor at all. `ZoomTests` pins the pages
and line breaks at the default and an accessibility size, the zoom as the
fit times the factor, the flow still scaling its type, and the move on and
off paper.

**Where.** `packages/leaf-swift`, `LeafTextView` (iOS) — `applyDynamicType`
and `renderTheme`; `LeafEditorModel.zoom`.

**What.** The iOS view scales the host's theme to the reader's Dynamic Type
content size (`EditorTheme.scaled(by:)`) and lays out with the result. In the
continuous flow that is right: the column reflows, and bigger type is bigger
type. On paper it is wrong in a way the reader can see: the type on the
*sheet* grows, the lines break differently, the page count changes, and the
document a Larger Text user is looking at is not the one they will print or
export — `pdfData(page:)` deliberately ignores Dynamic Type (a point is a point
on paper), so screen and PDF disagree.

A sheet's type is a fact about the document; how large the sheet is on a
screen is the zoom's business, and since `b2f30a0` there is a zoom on both
platforms with a fit-width default. The accessibility setting should reach
the page the way it reaches a PDF in Preview: by scaling the *view*, not by
resetting the type.

**How.** On paper (`pageSetup != nil`) leave `renderTheme` at the host's
theme, and fold the Dynamic Type factor into the zoom instead — most simply
as a multiplier on whatever `zoom` resolves to, so `.fitWidth` at AX3 is the
fit at AX3's factor and a pinch still moves it. `zoomScale` then reports the
product, which is what a "125%" label should say on that phone. Keep the
continuous flow as it is. `traitCollectionDidChange` re-resolves the zoom on
paper rather than relaying out. The Mac has no Dynamic Type and needs
nothing.

Decide whether the factor should cap: a fit-width page at the largest
accessibility size on an iPhone would be scrolled sideways a great deal,
which is what a Larger Text reader gets in Preview too, and may be right.

**Done when.** Changing the text size in Settings on a paginated document
leaves its page count and line breaks alone and changes how large the sheet
is on screen; the on-screen page and `pdfData(page:)` break the same lines;
and a pinch still zooms from wherever the setting put it.
