---
title: Editors inside a document
status: draft
created: 2026-10-11
updated: 2026-10-12
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
- **Rendering Mermaid on Apple** (open question). First answered with one
  `WKWebView` per diagram and mermaid.js from a CDN. Now native, everywhere:
  a spike (`spikes/mermaid-native/`) measured mmdr, a Mermaid renderer in
  Rust, against mermaid.js. `crates/leaf-mermaid` wraps it, from a fork
  carrying upstream's open PRs for wasm and for host text metrics until they
  merge. leaf-ffi exports `render_mermaid`, which the app calls with Core
  Text measuring the labels, and LeafUI's `SVGView` draws the result as
  vectors. On the web, `leaf-mermaid-wasm` is a module of its own that the
  demo loads in a worker, measuring labels on a canvas. The npm package does
  not carry it. leaf's frontends still draw no diagrams. So for now, on the
  user's choice, Mermaid is a leaf crate behind the bindings. That question is
  now decided: under [Renderers and editors](#renderers-and-editors), Mermaid
  moves out of leaf's default build.
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

Then split rendering from editing, as
[Renderers and editors](#renderers-and-editors) sets out:

4. The public picture, and renderers by kind on every platform that draws
   regions. Math's renderer is leaf-math, unchanged in what it draws.
5. Mermaid as a host renderer. `leaf-mermaid` leaves the default `leaf-ffi`
   and npm package, and the leaf-editor app and web demo register it.
6. Directives as regions, with `directiveView` and `DirectiveHost` folded in.
   Every frontend that draws directives today, Android and the terminal
   included, still draws them afterwards.
7. One generic Rust session across the bindings, with math as its first user.

## Renderers and editors

Decided 2026-10-12, after Mermaid was drawn natively. The protocol built in
steps 1 and 2 can be extended in one direction only: a host can add a block
fence edited by a view of its own. A host cannot claim any other kind of
region. It cannot draw anything inline. It also cannot hand the layout a
picture: a closed region is drawn either by leaf's placeholder or by a live
view laid over it, and a live view does not paginate, print or export.
Directives meanwhile have a second, parallel host-view mechanism
(`directiveView` / `DirectiveHost`). And `leaf-ffi` depends on `leaf-mermaid`,
so leaf ships a Mermaid renderer it does not use.

So each region kind gets two parts. Both are registered by kind, and both are
open to a host.

- **A renderer**, required. It turns a region's content into a
  `RegionPicture`. The layout places the picture inline, on the baseline, or as
  a block on the region's rows, and caches it by kind and content. Because the
  layout lays out and draws pictures itself, a host's regions page, print and
  export the way formulas do. The picture is vectors with a size and a
  baseline, in whatever form each platform draws cheaply: a display list or
  SVG on Apple (LeafUI's `SVGView` already draws SVG), an element or SVG on
  the web. `MathGlyph` becomes the math renderer's picture, not a type the
  layout knows by name. A kind with no renderer keeps leaf's placeholder row.
- **An editor**, optional. It is what `open` is today: a session leaf draws,
  which answers keys and returns a picture (math), or a session a host runs in
  a view of its own. Only an open region is ever a live view. Inline regions
  are edited only by sessions leaf draws.

Kinds:

- `math`, inline or block. Leaf registers its renderer and its editor by
  default, and a host may replace either.
- `fence:<lang>`, block.
- `directive:<name>`, block, as `::name[label]{attrs}`. The region carries the
  name, the attributes and the label that `DirectiveInfo` carries now, as well
  as its ranges. Its content is its whole source line, and its editor writes
  through splices like any other. A directive renderer replaces
  `directiveView`, which goes.
- Inline directives and HTML blocks: not yet. See Open questions.

Registration is one call per platform, with the same shape everywhere:
`regionRenderers[kind]` and `regionEditors[kind]`, or one `regions[kind] =
{ render, open }`, whichever reads better in each language. Core still needs
`set_owned_regions` with every kind the frontend registers, so that it
publishes those regions.

The bindings export one Rust session type, chosen by kind, in place of
`MathSession`. A second editor written in Rust then needs no new binding.

Mermaid leaves leaf's default build. `leaf-mermaid` stays a crate in this
workspace, but `render_mermaid` moves behind a cargo feature or into its own
small FFI module, and `leaf-mermaid-wasm` stays out of the npm package. The
leaf-editor app and the web demo register it as the renderer for
`fence:mermaid`, and their editors draw their previews with it.

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
- **Bring your own renderer.** Decided 2026-10-12: yes. See
  [Renderers and editors](#renderers-and-editors).
- **Inline directives.** Whether core should publish `:name[…]{…}` as an
  inline `directive:<name>` region now, or only once something needs it.
- **Persistence of the open state.** Whether an open region should survive a
  re-render that rebuilds the view (it does not today).
