---
status: open
created: 2026-10-04
updated: 2026-10-26
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# Handoff of an open document between the Mac and iOS apps

**Since the archive (2026-10-26).** The iOS app named here is archived
(`archive/apps/leaf-editor`); the iOS end is now the Dioxus app on iOS, which
has no iOS platform module yet.

**Where.** `apps/leaf-dioxus/Info.plist` and `apps/leaf-editor/project.yml`
(the document types), and the scene in `apps/leaf-editor/App/LeafApp.swift`
that would receive an activity on iOS.

**What.** Leaf advertises no `NSUserActivity` for the document a window has
open, so Handoff never offers it on a nearby device. The Mac's `LeafDocument` is an
`NSDocument` (`apps/leaf-dioxus/src/platform/macos/document.rs`), and `NSDocument`
makes a document's activity only for a file in iCloud, and only when the
document type in `CFBundleDocumentTypes` names an
`NSUbiquitousDocumentUserActivityType`. Neither half is cheap here, for three
reasons this checkout cannot settle alone:

- The activity carries the document's *ubiquitous* URL, which the receiving
  app can open only as an iCloud document — a file in iCloud Drive opened
  through the system, which on iOS means the document browser handing it
  over, not a path in the activity. A document in a local folder on the Mac
  has nothing a phone could open, and advertising it would offer a Handoff
  icon that fails when tapped.
- Handoff pairs apps signed by the same team with the same activity type.
  The Mac app is Developer ID–signed only by `cargo xtask package`; ordinary
  and simulator builds are unsigned, so there is no build that could be
  checked end to end without two devices and a release-signed pair.
- The iOS half needs a receiver: `onContinueUserActivity` on the document
  scene, resolving the URL through the document browser's security scope.

**Done when.** With both apps release-signed and a Markdown file in iCloud
Drive open on the Mac, the Leaf icon appears in the iPhone's app switcher,
and tapping it opens the same file at the same place — and a document
outside iCloud offers nothing.
