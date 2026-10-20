---
title: Leaf's Dioxus app on unit
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
created: 2026-10-20
---
# Leaf's Dioxus app on unit

**Where.** `apps/leaf-dioxus` (the toolbar's menus, the context menu, the
Link prompt, Settings), and `crates/leaf-dioxus-ui`, which unit replaces.

**What.** [unit](https://github.com/diaryx-org/unit), diaryx-org's
component library for Dioxus on the web and on Blitz, grew out of
`leaf-dioxus-ui`: its `Unit` is `Layer` with a focus of the page's own, and
its menus, dialog and field are the same code with pointer events and ARIA.
The app would take it by git (`branch = "main"`), wrap each window's page in
`UnitRoot { keys: Keys::Host }`, call `Unit::key` and `Unit::text` where it
calls `Layer::key` and `Layer::text` now, and give keys to Blitz while
`Unit::typing()`; Settings could be unit's `Segmented`, `Switch` and
`Select`. Its toolbar's controls would then be buttons assistive tech can
press: today they are spans, which VoiceOver reads but cannot press.

It needs Blitz's changes unit was built against (diaryx-org/blitz#1) on the
fork's `leaf` branch, which the app builds on, and those change two things
the app may notice: a press focuses a focusable element, and a click goes to
the nearest element the press and the release share.

**Done when.** The app's menus, prompt and Settings are unit's, work as they
do now with the mouse, the keys and VoiceOver, and `crates/leaf-dioxus-ui`
is gone.
