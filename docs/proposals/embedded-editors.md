---
title: Editors inside a document
status: draft
created: 2026-10-11
updated: 2026-10-11
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# Editors inside a document

## Status

`draft`. Written after reviewing `spike/math-editor` (`e865963`..`a65e65c`),
which edits formulas in place as structure on the web, macOS and iOS. CI is
green on that branch merged with `main`. It stays off `main` until the change
argued here has been made with math as its first client.

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
- **Persistence of the open state.** Whether an open region should survive a
  re-render that rebuilds the view (it does not today).
