---
status: open
created: 2026-09-20
updated: 2026-09-20
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# The Mac app runs unsandboxed and unsigned

**Where.** `apps/leaf-editor/project.yml`, and a packaging step in `xtask`.

**What.** Leaf is a document app now — `DocumentGroup`, declared document
types, an icon, a marketing version — but `ENABLE_APP_SANDBOX` is off and
`CODE_SIGNING_ALLOWED` is `NO`, so `cargo xtask swift --release` produces an
app that runs on the machine that built it and nowhere else. Turning either on
has a consequence the app does not answer yet:

- **Signing** needs a Developer ID, and a distributable build needs
  notarisation on top; both are Adam's to run. The build should take the team
  and identity from the environment (`DEVELOPMENT_TEAM`, `CODE_SIGN_IDENTITY`)
  rather than the project, so a checkout without them still builds.
- **The sandbox** grants a document app the file it was handed and not its
  siblings, and a Markdown file's images are its siblings. The two known
  answers are a *related items* declaration (`NSIsRelatedItemType`, which
  covers a sidecar of the same name and not a `media/` folder) and asking for
  the folder once with an `NSOpenPanel` and keeping a security-scoped
  bookmark — the second is what Marked and iA Writer do, and what
  `documentDirectory` in `ContentView.wire()` would then be set from.

**Done when.** `cargo xtask package` (or `swift --release --sign`) produces a
signed, sandboxed, notarised `Leaf.app` in a `.dmg`, and a document opened
from the Finder shows the images beside it.
