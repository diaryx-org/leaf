---
title: Editors inside a document
status: draft
created: 2026-10-11
updated: 2026-10-11
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# Editors inside a document

## Status

`draft`, being built on `embedded-editors`. Written after reviewing
`spike/math-editor` (`e865963`..`a65e65c`), which edits formulas in place as
structure on the web, macOS and iOS.

Steps 1 and 2 of [How to get there](#how-to-get-there) are built, with both
clients on the web and in the leaf-editor app:

- **Core** (`4a1d64a`). `Doc::set_owned_regions` replaces `set_edits_math`,
  and `VisualMap::regions` lists every owned region. An owned fence is one
  `▤ lang` placeholder row. `MathView` keeps what drawing needs, and the
  spike's `end`/`tex_start`/`tex_end` moved to the formula's `RegionView`.
- **leaf-web** (`412e486`), **leaf-swift** (`e2715d9`). One open-session
  slot per view. The Swift one, `RegionSlot`, is shared by the AppKit and
  UIKit views. Math runs on it unchanged: the spike's web, Swift and iOS
  tests pass, two of them reading the new slot instead of the spike's
  internals. The Swift layout asks the open region for the formula's
  picture, so the stand-in TeX and `MathStore.open` are gone.
- **Mermaid** on the web demo (`845750f`) and in the leaf-editor app
  (`e534037`). leaf depends on none of it.

What the text below did not foresee, or decided:

- **Inline host views** (open question). Not built. A host may name only
  `fence:` kinds, and every fence is a block. Inline regions are pictures from
  a leaf session.
- **Android and the terminal** (open questions). Not touched. Neither calls
  `set_owned_regions`, so both draw formulas and fences as before.
- **Where the Mermaid editor lives** (open question). In each demo app
  (`apps/leaf-web-demo/mermaid-editor.js`, `apps/leaf-editor/App/MermaidFence.swift`),
  as the reference for Diaryx. It is not a leaf package.
- **Rendering Mermaid on Apple** (open question). Not measured. The demo
  takes the conservative route: one `WKWebView` per diagram, mermaid.js from
  a CDN, at a fixed height, so the layout never waits on a web view to
  measure itself. A shared offscreen renderer is the obvious next step if
  documents hold many diagrams.
- **Persistence of the open state** (open question). An open region does not
  survive a view being rebuilt, as before.
- **A fence's content** is its lines between the fences, each with its
  newline, and empty for an empty fence. A fence inside a quote or a list
  item is not owned, because its lines carry the container's prefix and a
  splice on its content would write over them.
- **Bytes on the wire.** Splices are in UTF-8 bytes of the content, as
  `edit_source` is. Because a text widget speaks in its own units, the host
  half also has `replace(content)`, which leaf diffs into one splice.
- **Host views and history.** A host's view stays open across undo and redo
  (`host.undo()`), and gets the change as `reload`. A formula closes and
  then undoes, as in the spike.
- **"Edited" on open.** Changing the owned kinds changes the rows. The Swift
  views reported that as an edit, and the Mac app marked a document holding
  a fence as edited the moment it opened. Re-showing is now told apart from
  editing.

A review of the protocol found five problems, fixed in `ff97e56`:

- An open region was found again only by its start, so an undo above it
  closed it. It is now found by its place among regions of its kind, on both
  platforms.
- A fence edit that kept the rows equal was not reported as an edit.
- On iOS, the cleared tint hid a host view's caret.
- A key that reached the text view past a host's view acted on the document.
- Contents were compared as characters rather than bytes.

A second review, of core and the web, found more, fixed in the commit after
`4fc0a9c`:

- A host's view could write a line that closes the fence (```` ``` ````),
  which ended the region early and turned the rest of the document into
  code. Such an edit is now refused, and the session shown the content as it
  stands, on both platforms. Lengthening the fences around the content is the
  friendlier answer, and it is left for later.
- `Session::reload` kept a stale selection anchor, which panicked.
- Paste wrote newlines and an unescaped `%` into the TeX as given. Such text
  is now typed instead.
- On the web, a formula closed by a click elsewhere kept drawing its caret.
- On the web, a click on a formula after an edit above it could open the
  wrong formula.
- On the web, `destroy()` left an open region's session unclosed.
- Not verified: Safari may not fire `copy` or `cut` when the DOM selection is
  collapsed, so ⌘C and ⌘X in an open formula may do nothing there. Chrome
  fires them.

Step 3 has begun, with the math editor's "Not done" list in
`crates/leaf-math-edit/README.md`:

- Selection, copy, cut, paste and delete-forward (`fbcfeaf`). The region
  protocol gained `copy` and `paste` for them.
- The open formula's layout is kept while its source stands (`8b4d678`).
- An empty script goes when the caret leaves it (`b9efe35`).

Left on that list: matrices and environments as grids, `\left(` pairing,
IME, user colours, and the terminal frontend.

## The problem

Some parts of a document are a second document in another language: a
formula's TeX, a Mermaid diagram's fence, and later perhaps a chart's spec or
a table's CSV. Each is best edited by an editor that understands the language
and draws the result, not by typing the markup into the prose editor.

The math spike shows that this works. It also shows what a second mode would
cost if each one were added the way math was. Every layer has a piece that
knows only about math:

| Layer | Math-specific piece |
|---|---|
| leaf-core | `Doc::set_edits_math`. `MathInfo` / `MathView` carry the formula's end and its TeX range |
| bindings | `MathSession`, exported by `leaf-math-wasm` and again by `leaf-ffi` |
| leaf-swift (macOS) | `openFormula` in `LeafTextView`, with checks added to `doCommand`, `insertText`, `mouseDown`, `undo`/`redo` and caret drawing |
| leaf-swift (iOS) | The same routing again in `LeafTextViewiOS` |
| leaf-web | The same routing again in `editor.js` |
| drawing (Swift) | The open formula reaches the layout under a fake TeX string per revision, which `MathStore` recognises (`MathStore.open`, a static table) |

A Mermaid editor written the same way would repeat all six rows, plus a core
change for fences: core publishes nothing structured about a fenced code
block today, only rows with a `codeLang`.

## What is already general

Most of the spike is not about math:

- **A region.** Core finds part of the source with an outer range (`src..end`),
  an inner content range (`tex_span`), and either an inline or a block place in
  the layout. It keeps the region drawn as a picture even on the caret's line.
- **A session.** Opening a region gives a session the input focus. The session
  takes keys, text and clicks, and answers with splices relative to the content
  range. The view applies them through `LeafDoc::edit_source` as one undo
  group. `OpenFormula.apply` already does this without knowing about math.
- **Following.** A session follows its region from frame to frame, reloads
  when an undo changes the content under it, and closes when the region is
  gone (`OpenFormula.follow`).
- **Leaving.** Edge keys, Escape, Return and Tab leave the region, before it or
  after it. Any other command leaves it and then runs.

## Two kinds of session

The modes we can foresee divide by who owns the focus while the region is open.

1. **Leaf runs the session.** The view passes keys and text to a session object
   and draws the picture the session returns. Math is this kind. The session is
   Rust, so one implementation serves the web, Swift, Android and, in time, the
   terminal.
2. **The host's view runs it.** Leaf places a view the host supplied over the
   region, gives it the focus, applies the splices it sends back, and takes the
   focus back when it closes. Mermaid is this kind: its likely editors (source
   text with a live preview, or a node editor) are platform UI, and rendering
   Mermaid is mermaid.js on the web and probably a web view on Apple. Diaryx
   should be able to plug this in without leaf depending on Mermaid.
   `directiveView` / `DirectiveHost` already size and place host views over
   block placeholder rows. This kind extends that to views that edit.

## The change

### Core

- Replace `set_edits_math(bool)` with one declaration of the region kinds the
  frontend edits itself. For example `set_owned_regions(["math",
  "fence:mermaid"])`. A region of an owned kind stays drawn as its picture or
  placeholder on the caret's line, as math does now.
- Publish every owned region in one list in the frame: kind (`math`,
  `fence:<lang>`), inline or block, its rows, outer range and content range.
  Fences are new. An owned fence becomes placeholder rows, as a display
  formula does. `MathView` either becomes this record or is derived from it,
  whichever leaves the bindings simpler.
- `edit_source` stays the single way edits come back.

### The session contract

A small protocol each frontend defines once in its own language. Rust
sessions implement it through the bindings, and host views implement it
natively.

- Open, given the content and where the caret enters: home, end, or a point.
- Handle a key, typed text, or a click. Each answers *edited* (with
  splices), *moved* (redraw only) or *leave* (before or after).
- Reload when the content changed underneath the session (undo, collaboration).
- Draw: either a picture for the layout to place (an inline or block glyph), or
  a view the host owns (block only, at first).
- Close.

### Frontends

- One open-session slot per view in place of `openFormula`. Routing for keys,
  typing, clicks, undo/redo and caret hiding goes through the protocol, so it
  is written once per view, not once per mode.
- The layout asks the open session for its picture directly. This replaces the
  fake TeX string and `MathStore.open`.
- The host registers editors by region kind, as it registers `directiveView`.
  Leaf ships math. Diaryx (or the leaf-editor app, as a demo) supplies Mermaid.

## How to get there

Do the restructuring before iterating further on the math editor, and build it
with two clients from the first commit:

1. Math, moved onto the protocol with no change in behaviour. The spike's tests
   (`MathEditingTests`, the web tests, the core tests) must still pass.
2. A minimal Mermaid fence editor, run by a host view: the fence's source in a
   text view with a rendered preview, writing back through splices. On the web
   and in the leaf-editor demo app, not in leaf's own frontends.

With one client, the protocol would end up shaped like math. With two clients
that differ in who owns the focus, it has to be general.

Only then work through the spike's own "Not done" list in
`crates/leaf-math-edit/README.md`: selection, copy/paste and delete-forward
inside a formula, IME, matrices and environments, `\left(` pairing, removing
empty scripts, user colours, and caching the layout of the open formula.

## Open questions

- **Inline host views.** Should an inline region ever be a host view, or are
  inline regions always pictures from a leaf session? Pictures only is simpler,
  and nothing foreseen needs more.
- **Android.** Should Android get the protocol now (`docs/tasks/android-block-views.md`
  is related) or follow later?
- **Terminal.** Whether leaf-ratatui gets math sessions at all, and what a
  host-view region looks like there. Probably a placeholder plus "open in
  $EDITOR".
- **Where the Mermaid editor lives.** In Diaryx only, in the leaf-editor app
  as the reference, or as an optional leaf package.
- **Rendering Mermaid on Apple.** A web view per diagram, a shared offscreen
  renderer, or a native renderer if a good one exists. Measure before choosing.
- **Bring your own renderer.** Should any region kind take a renderer a host
  supplies, for any format, not only a fence's editor? The protocol already
  keys editors by kind and splits drawing (`view`) from editing (`open`). It
  would also need: kinds beyond fences (directives, HTML blocks, perhaps
  inline spans), renderers that only draw, a host-supplied *picture* the
  layout places (today only leaf's own session draws one), and one registry
  across platforms. To be decided after Mermaid is drawn natively (mmdr
  through resvg), which is the first host renderer that would want a picture
  rather than a view.
- **Persistence of the open state.** Whether an open region should survive a
  re-render that rebuilds the view (it does not today).
