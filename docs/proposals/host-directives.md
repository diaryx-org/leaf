---
title: Directives a host names, inserts and draws
status: accepted
created: 2026-09-30
updated: 2026-09-30
part_of: '[Proposals](/docs/proposals/proposals.md)'
---

# Directives a host names, inserts and draws

## Status

`accepted` on 2026-09-30. Steps 1–5 were built the same day, so every
frontend now handles leaf directives (`::name{…}`). Core and both bindings are
in `ffdd8b2` and `c27d753`, leaf-web in `0a02aa2`, leaf-swift in `cabc9d1`,
`ecd69f2` and `5bf7847`, leaf-android in `2fbb408`, `f7ef085` and `65cbd0c`,
and leaf-ratatui in `2151531` and `54721f9`. The
[web directive hook](/docs/tasks/closed/web-directive-hook.md) task closed with
the web step. Step 6, containers, is still open and waits on a twig gesture, so
the proposal stays `accepted` until it lands.

What the text below did not foresee:

- Typing at either caret stop of a drawn directive went into the directive's
  own source line, and the result was no longer a directive. Block media
  already guarded its stops; leaf directives now share that guard
  (`36a466b`). Backspace and Delete at a stop also treat the directive the way
  they treat a picture.
- Djot cannot round-trip two of the shapes twig writes: a bare attribute, and
  a label. leaf writes a bare attribute as `key=""` and refuses a label in
  djot. The twig-side half is twig's task
  `djot-insert-directive-round-trip`.
- `Capabilities::directives` is false for HTML and AsciiDoc. twig writes both,
  but the walker reads neither back as a directive by name.
- leaf-swift and leaf-android lay out in points, so, like media, they never
  report heights through `set_directive_rows`. Only the terminal reports them.
- On Android, a hardware Ctrl+Z under Gboard bypasses leaf's undo
  ([task](/docs/tasks/ctrl-z-under-gboard.md)). The on-screen undo is correct.

## The picture

leaf reads every directive a document carries and draws none of them for
real. A leaf directive (`::embed{src=…}`) is a `⧉ embed` placeholder row that
the caret steps over; a container (`:::warning` … `:::`) is a dashed panel
with a `.warning` label and editable prose inside. Both bindings already
publish each leaf directive in `DocView.directives` — its rows, name, label
and attributes — "so a renderer that knows the host app's vocabulary can
paint the real thing", and no frontend lets a host do so. The one directive
that draws as something is `::page-break`, and it does because the name is
leaf's own.

Nor can an author write one without typing its syntax. `Doc::insert_page_break`
calls twig's `insert_directive` with one fixed name. twig's gesture takes any
name, label and attribute list, checks the name against the grammar every
format reads back, and refuses a format that cannot spell it.

So the pieces are all there. What is missing is the way for a host to say
*which* directives it has, and *how each one looks*.

## Why not a general insert in leaf's own UI

A free-form "Insert directive…" that asks for a name and attributes would
write markup leaf cannot interpret. The presentation vocabulary kept to the
rule that leaf writes only names it knows how to draw, and left `style="…"`
carried and ignored for exactly this reason. A directive with no one to draw
it is the `⧉ name` row, which tells the author only that something is there.
Whoever knows the syntax can already type `::name` or `:::name` in the source.
The vocabulary is the host's, so the host should be the one to offer it.

## The proposal

Three parts, and each one works without the others.

### 1. The gesture, general

`Doc::insert_directive(name, label, attrs)`, twig's gesture at the caret with
`insert_page_break`'s caret placement: a paragraph parts around it, and the
caret lands on a line under it. `insert_page_break` becomes one call to it.
A `Capabilities::directives` flag answers twig's
`supports_with(Gesture::InsertDirective)` under leaf's parse extensions. It is
true for Markdown and djot, and false for HTML and AsciiDoc until the walker
reads their spellings of an arbitrary name back, as it now reads their page
break. A name or label twig refuses sets a status and writes nothing. Both
bindings export it, with the attributes as the `DirectiveAttr` list the view
already uses, so a directive goes in the way it comes out.

### 2. The catalogue, the host's

Each frontend takes a list of directive items from its host and offers them
where Insert's other rows are:

| field | meaning |
|---|---|
| `id` | stable, as a tool id is (`"directive.embed"`), so a saved arrangement survives |
| `name` | the directive's name, what `insert_directive` writes |
| `title` | the menu row's text, localised by the host |
| `icon` | an SF Symbol on Apple, a Material icon on Android, a glyph on the web and terminal |
| `label`, `attrs` | what to write, or a callback that asks the author and answers them (an embed's URL) |

In leaf-swift the items join the Insert tool's variants in `ToolCatalogue`, so
the iOS short row, the key panel and the Mac's menus all offer them. On the web
the list is an `EditorOptions` field, and the demo's toolbar lists it. Android
takes it as a parameter of the editor composable. In the terminal, a leaf-ratatui
host adds commands to the palette. Each item is dimmed by
`capabilities.directives`, as page break is by `pageBreak`. With an empty
catalogue there are no new rows, and a frontend looks the way it does today.

### 3. The drawing, the host's

Each frontend asks its host for a drawing per leaf directive, by name. A nil
answer leaves the placeholder as it is, so an unknown directive, or a host
with no hook, is drawn exactly as it is today.

- **leaf-web**: `EditorOptions.directive(view) => HTMLElement | null`, as the
  [task](/docs/tasks/closed/web-directive-hook.md) specifies. It is asked per directive
  on each frame, and the answer is keyed on name and attributes and reused the
  way a media row is. It is wrapped as a `contenteditable="false"` atom over the
  rows in `[start_row, end_row)`, with a caret stop either side. `_domPoint` and
  `_rangeForRow` map through it by `atomCoreLen`.
- **leaf-swift**: a `directiveView: (DirectiveView) -> PlatformView?` on the
  editor view (an `NSView`/`UIView`, since `EditorLayout` places platform
  views, as it places media). `EditorLayout` collapses the directive's rows
  onto the first, and the host's view sets its height through
  `intrinsicContentSize`, or through `fittingSize` where there is none. Pages
  flow around it as around a picture. The caret and hit test use the media
  path's two stops.
- **leaf-android**: a `directiveContent: @Composable (DirectiveView) -> Unit`
  slot, or null, placed by `EditorLayout` in the space `placeMedia` reserves for
  an image, with the rows collapsed the same way. This is the first
  host-supplied block on Android. The [block views](../tasks/android-block-views.md)
  task's tables and formulas are built-in blocks placed the same way.
- **leaf-ratatui**: a `DirectiveRenderer` trait the host passes to the widget,
  which returns the `Vec<Line>` to draw for a directive at a given width, or
  `None`. The renderer reports each answer's line count back through a new
  `Doc::set_directive_rows`, the peer of `set_media_rows`, and core reserves that
  many rows, as `DirectiveMark::rows` has always said a terminal frontend would.
  The lines are drawn over the reserved rows. They hold no caret.

In every frontend the host's drawing is display-only as far as leaf is
concerned. What it does with a click, a tap or a key of its own is the host's.
The directive's source is edited by deleting it or through the host, not by a
caret inside the drawing. A host that wants a "reconfigure" gesture gets the
directive's name and attributes with the view, and writes the change back with
the ordinary replace-source path.

### What stays leaf's

`::page-break` keeps its own drawing and its own button, and the hook
is never asked about it. The name is
leaf's, and a page opening there is a layout fact, not a picture.

## What a host's directive looks like elsewhere

A plates build, GitHub, and any frontend with no hook draw the placeholder or
the raw markup. That is the price of a vocabulary that is the host's: a document
that uses diaryx's `::embed` looks right in diaryx and reads as a marked place
everywhere else. It degrades to legible text rather than to nothing, which is
the most a closed vocabulary can promise.

## Containers, later

A container directive holds prose, and prose is leaf's to lay out, so a host
cannot replace one. What it can do is *style* one: a tint, a border, an icon
and a title in place of the `.warning` label. That wants two things this pass
does not build.

- **A view of containers.** Rows carry `directive` and `directive_label`, but
  no list names each container's name, attributes and row span, as
  `DocView.directives` does for leaf directives. A `DocView.containers` list
  would, and a `containerStyle(ContainerView) -> Style?` hook per frontend
  would read it.
- **A gesture to write one.** twig's `toggle_block_container` wraps a range in a
  quote or a list, and has no fenced-div kind. Inserting `:::name` around the
  caret's blocks is a twig gesture first, filed as a twig task when this step
  starts, and then a catalogue item of a second kind.

## Sequence

1. **Core and bindings.** `Doc::insert_directive`, `Capabilities::directives`,
   `insert_page_break` through it; `Doc::set_directive_rows` and the walker
   honouring it; both bindings export the gesture, the flag and (wasm aside)
   the rows setter, and `the_two_bindings_export_the_same_methods` holds them
   level.
2. **leaf-web.** The hook and the atom mapping, the catalogue option, the demo
   painting one directive kind, and the task's done-when: the caret steps over
   the atom in one keypress each way, and a test in `test/editor.test.html`
   round-trips a caret through `_syncFromDom` over a replaced row.
3. **leaf-swift.** The view hook on macOS and iOS, the rows collapsed, pages
   flowing around it, and the catalogue in `ToolCatalogue`'s Insert. The
   leaf-editor app paints one kind.
4. **leaf-android.** The composable slot, the rows collapsed, and the catalogue
   in the toolbar. The leaf-android app paints one kind.
5. **leaf-ratatui.** The renderer trait, rows reported back, and the catalogue
   in the palette. leaf-tui paints one kind.
6. **Containers**, once twig has the gesture.

The kind each app paints is a demonstration, not a vocabulary: an
`::embed{src=…}` drawn as a titled card holding the URL, the same in all four,
so the apps show the hook without leaf adopting a name.
