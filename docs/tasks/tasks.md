---
title: Tasks
description: Deferred work on leaf, one file each — a commitment with a done state
created: 2026-09-04
updated: 2026-10-04
contents:
- '[Writing Tools in the Apple views](writing-tools.md)'
- '[Right-to-left text](right-to-left-text.md)'
- '[The Mac app runs unsandboxed](sandboxed-mac-app.md)'
- '[Enter in an HTML document writes whitespace](enter-in-html-writes-whitespace.md)'
- '[Text glued under a closing `</div>` draws until the next full rebuild](text-glued-under-a-closing-div.md)'
- '[An IME composition undoes a keystroke at a time](ime-composition-undo.md)'
- '[Handoff of an open document between the Mac and iOS apps](handoff.md)'
- '[A force click on a date shows no Calendar card](date-event-popover.md)'
- '[Closed tasks](/docs/tasks/closed/closed.md)'
- '[What the Mac app does not do yet](/docs/tasks/what-the-mac-app-does-not-do-yet.md)'
- '[Try Libero again](/docs/tasks/try-libero-again.md)'
- '[Leaf''s Dioxus app on unit](/docs/tasks/leafs-dioxus-app-on-unit.md)'
- '[Ship the Dioxus terminal editor as leaf](/docs/tasks/ship-the-dioxus-terminal-editor.md)'
- '[A web demo drawn by the Dioxus editor](/docs/tasks/a-dioxus-web-demo.md)'
- '[What iOS does not do yet](/docs/tasks/what-ios-does-not-do-yet.md)'
- '[What Android does not do yet](/docs/tasks/what-android-does-not-do-yet.md)'
- '[What Linux and Windows do not do yet](/docs/tasks/what-linux-and-windows-do-not-do-yet.md)'
- '[A quote''s bar breaks a line Blitz was told not to wrap](/docs/tasks/a-quotes-bar-breaks-a-line-blitz-was-told-not-to-wrap.md)'
- '[Tests leave their fixtures in the temp directory](/docs/tasks/tests-leave-their-fixtures-in-temp.md)'
part_of: '[leaf](/README.md)'
---
# Tasks

Work that has been deferred, not work that is planned. A task lands here when
it is worth doing and will not be done in the commit that noticed it; a fix
that takes ten minutes gets a commit, not a file. Each carries `status`
(`open`, `in-progress`, `done`, `dropped`) in its frontmatter — `org tasks`
reads the key — and closing one is an edit, naming the commit or release
that resolved it, never a deletion. What is done then moves to
[Closed tasks](closed/closed.md) (`org shelve`), which is part of this index —
the index is the spine, and what is open is a view of it (`org tasks`) — and
stays findable by grep.

What this is not: a commitment to consumers, which goes in the changelog's
unreleased region; or a description of what shipped, which goes in the
guides.
