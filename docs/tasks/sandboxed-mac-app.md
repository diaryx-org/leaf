---
status: open
created: 2026-09-20
updated: 2026-09-30
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The Mac app runs unsandboxed

**Where.** `apps/leaf-editor/project.yml`, and `ContentView.wire()`.

**What.** Leaf is a document app — `DocumentGroup`, declared document types,
an icon, a marketing version — and `cargo xtask package` now makes a
Developer ID–signed, notarised and stapled `Leaf.app` in a `.dmg`. But
`ENABLE_APP_SANDBOX` is off, and turning it on has a consequence the app does
not answer yet.

The sandbox grants a document app the file it was handed and not its
siblings, and a Markdown file's images are its siblings. The two known answers
are a *related items* declaration (`NSIsRelatedItemType`, which covers a
sidecar of the same name and not a `media/` folder) and asking for the folder
once with an `NSOpenPanel` and keeping a security-scoped bookmark — the second
is what Marked and iA Writer do, and what `documentDirectory` in
`ContentView.wire()` would then be set from.

A Developer ID app does not have to be sandboxed; the Mac App Store is where
it becomes a requirement.

**Done when.** A sandboxed build from `cargo xtask package` opens a document
from the Finder and shows the images beside it.
