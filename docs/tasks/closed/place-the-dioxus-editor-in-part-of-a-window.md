---
title: Place the Dioxus editor in part of a window
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
status: done
created: 2026-10-16
updated: 2026-10-16
---

# Place the Dioxus editor in part of a window

**Status: done.** `Editor::set_bounds` gives the editor a box in the window:
its viewport is the box's height less the status line, its column is centred
in the box's width, the find bar and the status line are pinned to its top
and bottom, and the input method is told the caret's place offset by the
box's origin. The clip stayed where Blitz forced it: the editor does not clip
to its box, and a host covers what lies outside it with its own chrome, in a
stacking layer above the editor's (`Bounds`' doc says so). Diaryx's Dioxus
port is the first host with a box: a navigation bar over the top, a keyboard
over the bottom, and on a Mac a sidebar beside it.

**Where.** `crates/leaf-dioxus/src`: `view.rs`, `editor.rs`, `dispatch.rs`.

**What.** The widget assumes the window is its own, in three places. A host
that draws a sidebar or a toolbar beside it — Diaryx, being ported to Dioxus —
needs all three relaxed.

- **The stylesheet clips at `body`** (`view::css`: `html, body { overflow:
  hidden; height: 100vh }`), and the status line and find bar are pinned to
  the page's edges. Blitz forced the clip there: it paints the first frame
  of an `overflow: hidden` element and not its changes, and skips a child
  that hangs past its parent's box (`view.rs`'s comment). A widget in a box
  needs a container whose clip Blitz repaints, which may mean a Blitz fix.
- **The viewport is the window's surface** less the status line
  (`Signals::new`, `Editor::resize` from `WindowEvent::SurfaceResized`).
  A host would give the editor its box's size instead.
- **The input method is told where the caret is relative to the window's
  top-left corner** (`Editor::ime_area`: `row::PAD + x`, the row's top less
  the scroll). In a box, that is off by the box's origin, so the candidate
  window and the accent menu open in the wrong place.

**Why deferred.** The split into a widget and its app came first, and the
host that needs a box is Diaryx's port, which is the one to draw it against.
