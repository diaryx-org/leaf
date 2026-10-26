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

Steps 4–7 of [How to get there](#how-to-get-there) are built, on the
`mermaid-native-spike` branch:

- **Core** (`5c21272`). A frontend that names `directive:<name>` in
  `set_owned_regions` finds that name's leaf directives in
  `VisualMap::regions`, each with its whole source as its content. Owning one
  changes no rows. `RegionInfo` and both bindings' `RegionView` also carry
  `display`, and a directive's `name`, `label` and `attrs`.
- **Renderers, Swift** (`5ec8958`). `regionRenderers[kind]` answers a
  `RegionPicture`: a size, a baseline and a draw closure, made from an SVG, a
  bitmap or by hand. A picture with no width takes the column's. The layout
  places it inline (an attachment, as a formula was) or as a block box
  (`PictureLayout`), and draws it with the text. So pictures page, print and
  go into a PDF. `MathGlyph` and `MathLayout` are gone; leaf's `math` renderer
  is leaf-math through `MathRenderer`. Each view caches pictures by kind,
  content, size, ink and appearance, and `redrawRegions(kind:)` forgets them.
  The app's Mermaid fence is a renderer.
- **Mermaid out of leaf** (`1ec775f`). `render_mermaid` moved to
  leaf-mermaid-ffi, which leaf-ffi links only under its `mermaid` feature.
  This follows the resvg-uniffi pattern, since a second Rust archive would
  duplicate `std`. Its Swift module, `LeafMermaidFFI`, is generated beside the
  app in `apps/leaf-editor/LeafMermaid`, and the app's pre-build turns the
  feature on. Default leaf-ffi no longer depends on the mmdr fork.
- **The web** (`1bb937c`). The same `regionRenderers` and `redrawRegions`.
  A picture is `{svg, width, height, baseline}`, or `{element}` for a block.
  A renderer may answer with a promise, and the placeholder stands until it
  settles. The `directive` hook and `redrawDirectives()` are gone. The demo
  registers Mermaid as a renderer that renders in its worker.
- **Directives, Swift** (`f3d3217`). `directiveView` and `DirectiveHost` are
  gone. The one host view left is the open region's (`HostedLayout`). The
  app's `::embed` card is a picture drawn in Core Graphics.
- **One Rust session** (`b2a9833`). leaf-session puts every editor leaf runs
  behind one trait, and `open(kind, …)` chooses one by kind. Both bindings
  export it as `EditSession` in place of `MathSession`. Math is its one kind.

Decided while building them:

- **Inline directives** (open question). Not published. Core makes a region
  only of a leaf (block) directive. A `:name[…]` in a line stays as it was,
  until something needs to draw one.
- **A host's math editor.** Refused, on both platforms. A host may replace
  the `math` renderer, but leaf's session edits a formula inside a line,
  where no host view can stand. `directive:page-break` is refused as a
  renderer kind and as an editor kind.
- **Synchronous on Swift, a promise on the web.** A Swift renderer is called
  on the main thread as the layout needs it. One that waits on data draws what
  it has, then calls `redrawRegions(kind:)`. A web renderer may return a
  promise instead.
- **`dark`.** On Swift it comes from the view's appearance; a sheet for print or a
  PDF is never dark. On the web it comes from the brightness of the text
  colour.
- **`{element}` pictures are web-only.** A web page prints live elements, so
  the web keeps a host's element as a block picture. On Apple everything
  closed is a picture.
- **Paper sheets own what the view owns.** A sheet for print, a PDF or a page
  image shares the view's `LeafDoc`, and used to reset its owned kinds to
  formulas alone. It now takes the view's renderers and editors, and a test
  holds it to that.
- **A picture's caret band.** ↑ and ↓ from either side of a block picture
  now probe past its whole box, as they did past a directive's view. This
  includes a display formula, which before probed past the picture alone.
- **Android and the terminal.** Unchanged. Neither owns regions, and core
  still publishes `DocView.directives`, which both draw directives from. The
  Android app builds and its five directive tests pass; leaf-ratatui's pass
  in CI.

**For Diaryx (migrating from `directiveView`).** `AlbumDirective` and
`PageDirective` answer `model.directiveView` with SwiftUI cards. That hook is
gone. For each name:

1. Register `model.regionRenderers["directive:album"]` (and `page`). It gets
   a `RegionView` with the `name`, `label` and `attrs` the `DirectiveView`
   had, and answers a `RegionPicture`. For a card, use
   `RegionPicture(width: nil, height:)` with a draw closure that draws into
   any width. `ImageRenderer` can snapshot a SwiftUI card. The leaf-editor
   app's `EmbedDirective.card` draws one in Core Graphics.
2. A card's photographs arrive after it is first drawn. Draw what is there,
   and call `model.redrawRegions(kind: "directive:album")` when they land.
   The card no longer needs a fixed height for that reason, since the layout
   asks again.
3. A tap that followed the link belongs to
   `model.regionEditors["directive:album"]`. Its `open` is called on a click,
   with `.point`. It can follow the link and answer nil, and the caret is
   then placed as on any picture.

Not checked by hand: opening a region by a click or an arrow in the Mac app.
Synthetic clicks do not reach the app's window, and keystrokes through System
Events are off limits. The unit tests open fences, formulas and directives
through the same calls. Drawing was checked in the app in light mode, and in
paper mode, where diagrams, formulas and the `::embed` card are pictures.

Left:

- On the web, the placeholder shows for a moment when the Mermaid editor
  closes, while the new picture renders. The renderer could keep the previous
  picture until the new one arrives.
- Font fallback for emoji and other scripts in resvg, as before.
- A second Rust editor would need `RegionSlot.edits` to learn its kind. Today
  it knows only `math`, through `editsFormulasAsStructure`.

**Mermaid's cost on the Mac** (`e22ae47`, `c850508`). The 230–340 ms
reported for a large flowchart was mmdr's layout, much of it the debug build.
Label measuring was small, and the SVG round trip smaller. mmdr layout, as the
`timing` example in `crates/leaf-mermaid` measures it, Apple M-series:

| Diagram | Debug before | Release before | After (debug or release) |
|---|---|---|---|
| 20 nodes, 19 edges, a cycle | 1077 ms | 147–174 ms | 40–50 ms |
| 60 nodes, 87 edges | 40.4 s | 8.4 s | 1.55–1.6 s |
| every edge kind | — | 101 ms | 38 ms |

What took it there:

- **mmdr.** Its routing tested every candidate path against every node and
  every other segment exactly, allocating as it went. It now rejects by
  bounds first. Output is byte-identical over the spike's corpus. The change
  is in the fork's `diaryx` branch (`e680063`); it belongs upstream too.
- **Debug builds** of this workspace optimise mmdr.
- **Labels.** A render asks the host about each label once (412 asks became
  76 for the 60-node chart). The app keeps a `CTFont` per thread and every
  width. Core Text was 6–10 ms of a sequence diagram's 8 ms render, and is
  now under 0.1 ms on a repeat render.
- **Off the main thread.** A Swift renderer may answer later
  (`RegionRenderer(later:)`), as a web one may with a promise. The region
  keeps the size of its last picture with the same content, else the
  placeholder, until the answer comes. Then only that region is laid out
  again. A stale answer is dropped. Paper waits for the answer. The app renders
  Mermaid, parses the SVG and makes the screen bitmap on its queue. Sampled
  while it opened the 60-node chart, the main thread was idle for 1510 of 1809
  samples.
- **Repaints.** A block picture is drawn on screen from a kept bitmap: 59 ms of
  vectors became a 12 ms blit for the 60-node chart, 3.1 became 1.7 ms for the
  20-node one. Paper sheets share the view's pictures, and print vectors.

Not done: a vector list straight from mmdr, in place of SVG text that resvg
parses again. Parsing costs 4–17 ms, now off the main thread, so it was not
worth the change. The first parse in a process also loads the system's fonts
(25–180 ms); that too now happens on the queue.

Steps 8–10 are built, on `mermaid-native-spike`:

- **Inline directives** (`f8c9d3f`). An owned `:name[label]{attrs}` on a
  surface that paints in a line is one atom glyph and an inline
  `directive:<name>` region with its name, attributes, label and ranges; its
  content is its whole source. Android and the terminal own no regions and
  draw them as before.
- **The picture session** (`2a3ff6f`), on Swift and the web, with every item
  of [Editors a host draws as pictures](#editors-a-host-draws-as-pictures).
- **The date chip** (`6eb4ebb`), in the leaf-editor app and the web demo.

Decided or changed while building them:

- **Math's typing coalesces.** A run of typing in a formula is one undo step,
  as the protocol now says for every picture session, where it used to be one
  step a keystroke. That is a change to math beyond its caret, made on
  purpose; a key in between still splits the steps.
- **`RegionHost.view` (Swift).** The caret's and region's rects are in the
  text view's coordinates, so the host is also given the view, weakly, to
  anchor its popover to.
- **The caret on Apple is the system's**, placed at the session's caret:
  `NSTextInsertionIndicator` on the Mac (leaf's drawn bar before macOS 14),
  and UIKit's caret through `caretRect(for:)` on iOS, which no longer clears
  its tint while a formula is open. Leaf draws the selection rects and the
  composition.
- **IME on the web** composes in a hidden `textarea` at the caret, since
  composing into the document's own element would push the picture aside.
- **Key names.** Plain Home and End are line ends on the web, everywhere;
  ⌘↑ ⌘↓ and Ctrl+Home/End are the region's ends. On the Mac, Home and End
  (`scrollToBeginningOfDocument:`) are the region's ends, as AppKit binds
  them. Math answers the new keys by passing them on, so they leave a formula
  and act on the document, as they did before they had names.
- **Accessibility.** The open session's value is what `accessibilitySelectedText`
  answers on the Mac, and is announced on opening and after each edit on both
  Apple platforms; on the web it is the region's label and a polite live
  region.

Not checked by hand: opening a chip, typing, undo, and the Japanese input
source in the Mac app. Nothing available to the session could type into a
Mac app — the computer-use tools were absent, System Events keystrokes are
off limits, and events posted to the process do not reach its window. The
protocol, composition and undo are covered by unit tests through the view's
own text-input methods (`PictureSessionTests`), and the chips were checked
drawn in the app beside Japanese text. The web chip was driven in headless
Chrome through real input events. The date chip's own logic has no unit
tests: it lives in the app, which has none.

Left: the date chip's logic tested in a package of its own; IME on iOS
inside a picture session (built as on the Mac, not exercised); a session
that opts in to composition itself; Android and the terminal still have no
region protocol.

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

Then open the picture session to hosts, as
[Editors a host draws as pictures](#editors-a-host-draws-as-pictures) sets
out:

8. Inline directives published by core as `directive:<name>` regions.
9. The public picture session on Swift and the web. Leaf draws the caret and
   selection, leaf composes IME text, keys can be added without breaking
   hosts, `RegionHost` gives the caret's rect, typing coalesces into one undo
   step, and sessions give an accessibility value. Math moves onto it, with no
   change in behaviour beyond its caret.
10. The date chip in the leaf-editor app and the web demo.

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

## Editors a host draws as pictures

Decided 2026-10-12. Editing is open to a host in one form only: a view over a
block. A session that leaf drives with keys and draws as a picture, the only
kind that can sit in a line of text, is leaf's alone. The type is internal
(`RegionPictureSession`), only `math` has one, and `RegionSlot.edits` names it.

So the picture session becomes public on every platform with region editing:
a Swift protocol, and a JS interface in `editor.d.ts`. `RegionEditor.open` may
return one, for any kind, inline or block. Leaf's math editor is registered
through the same door as a host's. A host may replace it, and
`RegionSlot.edits` asks whether a kind has an editor. Rust editors in
`leaf-session` stay one implementation behind it (`LeafRegionSession`), so a
registry of Rust editors can come later without changing the protocol.

What a session leaf does not write needs from the protocol:

- **Leaf draws the caret and selection.** A session's picture comes with a
  caret rect and selection rects, in the picture's coordinates. Leaf draws
  them, with its own blink, caret colour and selection colour. Math moves to
  this as well and stops drawing a caret in its SVG.
- **Leaf composes IME text.** Marked text is shown by leaf, at the caret, over
  the picture. The session gets only committed text, through `type`. A session
  may opt in to composition later.
- **Keys can be added without breaking hosts.** Add word moves, line moves,
  Home/End and ⇧↑/↓ to `RegionInput`. In Swift, the input is not frozen: a
  session's `switch` has a default, and the documentation says so. On the
  web, keys are names.
- **Popovers are the host's.** `RegionHost` gives the caret's rect and the
  region's rect in view coordinates, so a host can anchor its own completion
  list or calendar. Leaf draws no popups.
- **Pictures are cheap while a region is open.** An open session's picture is
  asked for on every keystroke, synchronously. The asynchronous renderers are
  for closed regions only. A kind whose drawing is slow edits through a view.
- **Typing is one undo step.** Consecutive `type` edits coalesce into one undo
  step, as typing in the document does, and a key or a click ends the step.
- **Accessibility.** A session gives an accessibility value, the region's
  content by default.

Core publishes inline directives (`:name[label]{attrs}`) as regions of kind
`directive:<name>`, inline. That is what most inline editors will edit:
mentions, dates, tags. Android and the terminal keep drawing them as they do
now.

The second client, as Mermaid was for host views, is a **date chip**, in the
leaf-editor app and the web demo, not in leaf. `:date[2026-10-12]` is drawn as
a pill. ← and → move between year, month and day. ↑ and ↓ change the field
under the caret. Digits overwrite it. A calendar popover opens at the caret's
rect. It uses every item in the list above, and it is inline.

## Open questions

- **Inline host views.** Should an inline region ever be a host view, or are
  inline regions always pictures from a leaf session? Pictures only is simpler,
  and nothing foreseen needs more.
- **Android.** Should Android get the protocol now (`docs/tasks/closed/android-block-views.md`
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
- **Inline directives.** First decided 2026-10-12: only once something needs
  it. Now needed: core publishes them, as
  [Editors a host draws as pictures](#editors-a-host-draws-as-pictures) sets
  out.
- **Inline host views on the web.** An inline element is cheap on the web, so
  an existing editor such as MathLive could sit in a line there. Apple keeps
  to pictures. Left until someone wants it.
- **A built-in source-and-preview editor** for any kind with a renderer, so
  that registering a renderer brings editing with it. Proposed 2026-10-12, not
  yet decided.
- **Persistence of the open state.** Whether an open region should survive a
  re-render that rebuilds the view (it does not today).
