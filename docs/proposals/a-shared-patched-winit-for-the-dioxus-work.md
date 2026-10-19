---
title: A shared, patched winit for the Dioxus work
part_of: '[Proposals](/docs/proposals/proposals.md)'
status: draft
created: 2026-10-15
---

# A shared, patched winit for the Dioxus work

## Status

`draft`, 2026-10-15; updated 2026-10-19. Written with the session porting
Diaryx to Dioxus on iOS, after both found winit needed patching. What Adam has
decided is under "Decisions"; nothing has been sent upstream. The macOS half
of step 1 is done: `winit-appkit` is in `patches/` at leaf's root, and flower's
Dioxus app takes it from there by git.

## Where it stands

Two Dioxus efforts each vendor a winit backend at **0.31.0-beta.3**, the version
dioxus-native pins (0.8.0-alpha.2 when this was written, and still at
0.8.0-alpha.3), and swap it in with `[patch.crates-io]`:

| | leaf's Dioxus app | diaryx's iOS spike |
|---|---|---|
| Backend | `winit-appkit` (macOS) | `winit-uikit` (iOS) |
| What | the text input client answers from surrounding text: press-and-hold accents, the emoji picker and dictation | rust-windowing/winit#4723 verbatim, "ios: support the UIScene lifecycle" — without it iOS 27 refuses an app built with the iOS 27 SDK |
| Where | `patches/winit-appkit` at leaf's root (public); `apps/leaf-dioxus/patches/` until 2026-10-19 | `spikes/dioxus-ios/patches/winit-uikit` on diaryx's `spike/dioxus-ios` (private) |
| `[patch]` | `apps/leaf-dioxus` by path: an exception Adam granted for the spike, 2026-10-15, kept when it became the app. flower's `apps/flower-dioxus` by git, which the org rule allows since 2026-10-19 | an exception Adam granted for that spike, 2026-10-15 |
| Upstream | prepared as a PR, not sent: `patches/winit-appkit/upstream/PR.md` | #4723 is open and mergeable, not on winit's master |

The two touch different crates, so they do not conflict. `[patch]` is the only
way to swap either: each backend reaches the app through dioxus-native →
blitz-shell → winit, and no fork short of that whole chain avoids it.

## The plan

1. **One home for the patched backends: leaf.** leaf is public and diaryx
   private, and code may only move from public to private, so a copy both use
   lives in leaf. Both vendored crates go to a `patches/` directory at leaf's
   root, outside its root workspace — `winit-appkit` from
   `apps/leaf-dioxus/patches/`, and `winit-uikit` with #4723 applied, which is
   winit's MIT/Apache code and already public. Each keeps a `PATCH.md` saying
   what changed, from where, and when it goes.
   Done for `winit-appkit`, 2026-10-19; `winit-uikit` is still on diaryx's
   spike branch.
2. **Everyone takes them from there.** `apps/leaf-dioxus` by path. flower's
   `apps/flower-dioxus` by git since 2026-10-19, for its edit line's input
   method. diaryx's spike by git, as the org spells a leaf dependency:
   `[patch.crates-io] winit-uikit = { git = "https://github.com/diaryx-org/leaf", branch = "main" }`
   (and `winit-appkit` the same way, so the diaryx spike gets the macOS fixes
   too). A `[patch]` with a git source no longer needs an exception (see
   "Decisions"), so this changes only where the crate comes from.
3. **Upstream, one PR per backend.**
   - appkit: the prepared PR (`patches/winit-appkit/upstream/`), redone against
     winit's master, built, linted and tested there. Before it goes:
     `cargo +nightly fmt`, a reproduction with one of winit's own examples, and
     a decision on splitting it (`PR.md` has the reasons). Adam approves
     sending it.
   - uikit: support #4723 — review it, say it is needed for iOS 27, report the
     test on a real app — rather than open a duplicate.
   - uikit, later: `UITextInput`. winit-uikit implements `UIKeyInput` only, so
     iOS has no marked text: no CJK input, no autocorrect or dictation into the
     caret. It would answer `textInRange:`/`selectedTextRange` from the same
     `ImeRequestData::surrounding_text` the appkit change uses, so the editor's
     IME path (`ime.rs`) would work unchanged. Its own branch and PR, after
     the vendored copy can carry it.
4. **Stay on 0.31.0-beta.3 until dioxus-native moves.** Rebasing the vendored
   copies forward would not settle the uikit side (#4723 is not on master), and
   a winit newer than Blitz's is a second copy in the graph. Rebase both when
   dioxus-native takes a newer winit; drop each one when its fix is in the
   winit it takes.

## Not winit, noted so they are not lost

- iOS: winit does not report the keyboard's frame (the diaryx spike observes
  `UIKeyboardWillChangeFrameNotification` itself), and Return arrives as
  `insertText("\n")`, a character, not Enter. Each could be a winit issue.
- blitz-shell subtracts `safe_area()` from its viewport and paints white
  outside it.

## Decisions for Adam

Made, 2026-10-15:

- The diaryx spike keeps its winit-uikit `[patch]` as a spike-only exception,
  like leaf's.
- The native spike is promoted to a supported app, `apps/leaf-dioxus`, and
  keeps its `[patch]`.
- leaf's `main` is pushed, so diaryx can take a shared copy by git.
- The appkit PR waits: it is not sent yet.

Made, 2026-10-19:

- The org rule changes: a `[patch]` in a tracked manifest takes a git source,
  never a local `path` (the org's `AGENTS.md`, "Cross-repo dependencies").
  A consumer outside leaf patches by git without an exception.
- The vendored backends move to `patches/` at leaf's root; `winit-appkit`
  has.
- flower's Dioxus app takes `winit-appkit` from leaf by git.

Open:

- Whether `apps/leaf-dioxus` keeps its path `[patch]` as an exception — it
  stays inside leaf, and builds against the patch beside it — or takes the
  crate by git like everyone else.
- Moving `winit-uikit` from diaryx's spike to leaf's `patches/`.
- When to send the appkit PR, and supporting #4723 upstream.
