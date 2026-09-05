---
status: open
created: 2026-09-04
updated: 2026-09-04
---
# The find bar covers the first line of a document at its top

**Where.** `packages/leaf-swift`, the macOS `LeafTextView` hosted by
`LeafEditor` in an `NSScrollView`.

**What.** ⌘F slides the system find bar in over the top of the scroll view.
When the document is scrolled to its top, its first line stays where it was,
under the bar, until the reader scrolls. `NSTextView` shifts its content down
instead.

**What is known.** Logged from inside the demo app with the bar up: the clip
view keeps the scroll view's full height, `contentInsets` stays zero,
`automaticallyAdjustsContentInsets` is on, and the bar's frame is the top 32
points of the scroll view. So AppKit neither shrinks the clip view nor widens
the automatic inset in this hosting, and the clip refuses to scroll above zero.
Three fixes were tried in a `NSScrollView` subclass and reverted:

- `findBarPosition = .aboveHorizontalRuler` — no change in geometry.
- Carving the bar's height off the top of the clip view's frame in `tile()` —
  the first line shows, but the finder's dimming overlay is placed a bar's
  height too low, leaving an undimmed band under the bar.
- A manual `contentInsets.top` of the bar's height with the bar pinned back to
  the top edge — the overlay is again offset by the inset, and the scroll to
  `-inset` did not hold.

The overlay is positioned by the finder from geometry this view does not
control, which is why moving the clip or the inset moves it wrongly.

**Done when.** With the document at its top, ⌘F leaves the first line fully
visible below the bar, the dimming covers the whole visible document, and the
match highlights stay in place. A plain `NSScrollView` in a non-SwiftUI window
is worth checking first: if the automatic inset works there, the cause is the
SwiftUI hosting and the fix may be a `safeAreaInsets`/`additionalSafeAreaInsets`
answer rather than a tiling one.
