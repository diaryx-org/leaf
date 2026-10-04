---
status: open
created: 2026-10-04
updated: 2026-10-04
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A force click on a date shows no Calendar card

**Where.** `packages/leaf-swift/Sources/LeafUI/LeafTextView.swift` —
`lookUp(under:event:)` and the data detectors beside it — and
`apps/leaf-editor/project.yml` for the usage string.

**What.** A force click on a date the data detectors find ("Oct 3rd, 2026")
looks up the one word under the pointer, as in any other prose, because
handed the whole phrase the Look Up panel searches it and never comes back.
`NSTextView` answers the same force click with Calendar's card for a new
event, in a popover anchored to the date, all day when the text gave no time. That card is the private DataDetectors
framework's, which a custom view cannot reach.

Handing Calendar the context menu's one-event `.ics` instead was tried and
turned down: it raises Calendar's "This calendar contains a new event" import
dialog in another app, not a card beside the text, and makes a one-hour
event where `NSTextView` makes an all-day one.

**How.** A popover of leaf's own over EventKit, anchored to the date's glyphs
as a footnote peek is: the title, the date (all day unless the text named a
time), a calendar picker, and Add, which saves through `EKEventStore`. It
needs calendar access, so every host — Leaf and Diaryx — declares
`NSCalendarsWriteOnlyAccessUsageDescription` (or the full-access key, to list
calendars) and shows the system prompt the first time. The detectors' public
API does not say whether the text gave a time; telling a date from a date
and time is a heuristic over `NSTextCheckingResult`'s range and `date`.

**Done when.** A force click on "Oct 3rd, 2026" in the Mac editor shows a
popover by the date that adds an all-day event to the chosen calendar, and
the context menu's Add to Calendar does the same.
