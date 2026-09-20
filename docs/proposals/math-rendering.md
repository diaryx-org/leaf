---
title: Math in leaf
status: implemented
created: 2026-09-19
updated: 2026-09-19
part_of: '[Proposals](/docs/proposals/proposals.md)'
---
# Math in leaf

## Status

`implemented` on 2026-09-19, unreleased, in the order the text below gives:
`leaf-math` in `9a94ec1`; the Markdown flag in `d9c145c` and the core
rendering in `e6df368`; both bindings in `8b52460`; leaf-ratatui in
`7957102`; leaf-web in `7f3a520`; leaf-swift in `1522f21`. leaf-gpui is not
done: it is archived, and stays on the code-styled text it drew before.
The wasm measure below was then answered: the typesetter moved into a module
of its own, `leaf-math-wasm`, that the web editor fetches on the first
formula, so a page without one pays nothing for it — the numbers are in
`packages/leaf-web/README.md`.

Five things the argument did not settle. An inline atom is not on every
surface — a terminal cannot composite a picture over one cell — so a
frontend says it paints pictures in a line (`Doc::set_inline_pictures`) and
the terminal, which does not, keeps the code-styled TeX for inline math and
gets display math as pixels over its rows, only on a graphics protocol. The
reveal line the builder takes became a `Reveal` that says whether it reveals
all markup or only math, and in the hidden modes core threads the caret's
line through only when the last build's layout says a block with a formula
meets it, so a document with no math still pays nothing for caret motion.
The media heights, the math heights and that capability moved into one
`Surface`. On the web, an inline formula keeps its one-character atom in the
DOM — the unit the caret counts the row by — and draws as the picture by a
transparent glyph stretched to the picture's width, which is also what put a
`map_key` on the frame so a caret move onto a revealed line repaints, a gap
`MarkupMode::Full` had too. And the wasm measure the text asked for: 3.69 MB
to 5.81 MB, 0.52 MB of it KaTeX's fonts, the rest RaTeX's code — the
`<text>`-mode option would save only the fonts.

One thing found rather than built: twig's `insert_literal` does not escape a
`$` under the math extension, so typing `$x$` in `MarkupMode::None` mints a
formula. Filed in twig, and as `docs/tasks/a-typed-dollar-mints-math-in-the-hidden-mode.md` here.

## The picture

A djot document with `$`E = mc^2`` in a sentence and `$$`\int_0^1 x\,dx``
on a line of its own. Open it in any leaf view.

The inline formula draws as `E = mc^2` in the code style, delimiters hidden —
the `verbatim | inline_math` arm of the inline walk, which treats math as a
code span wearing a sigil. Readable, and the caret behaves, but it is a
formula shown as its own source. The display formula is worse: `display_math`
has no arm of its own, so it falls to the default one, which pushes the
interior text in body style at the *node's* span start — three bytes before
where the text actually sits, past `$$` and the opening backtick. The caret
that lands on the `x` is standing on a delimiter it cannot see.

A Markdown document has no math at all. [`parse_extensions`] turns on four of
twig's Markdown flags and not `math`, so `$E = mc^2$` is prose with two
dollar signs in it, and a `$$` block is a paragraph whose `\,` twig has
already read as an escaped comma. The formula's source survives — leaf edits
source and twig writes back what it read — but on the rich surface it is a
sentence, and an author who types a backslash in it is authoring Markdown,
not TeX.

Nothing leaf builds on is missing. twig has `inline_math` and `display_math`
as node kinds in every language: djot's `$`…`` natively, Markdown's `$…$`
behind a flag, AsciiDoc's `stem:[…]`. What is missing is the thing that turns
the text between the delimiters into a picture.

[`parse_extensions`]: ../../crates/leaf-core/src/doc.rs

## What was considered

**A WebView.** KaTeX and MathJax are the renderers every Markdown editor with
math uses, and they need a DOM. leaf's web frontend has one; its terminal,
gpui, and Apple frontends do not, and putting a WebView inside an `NSView`
for each formula is a process per equation and a caret that stops at its
edge. Out on the same grounds every other pictured thing in leaf is drawn
natively.

**MathML.** `latex2mathml` is small, pure Rust, and turns TeX into markup.
But markup is not a picture: Apple renders MathML only inside WebKit, gpui
and the terminal never, and the web frontend would be the one platform it
helped. It also answers a different question — how to *export* a formula —
which is twig's and plates', not leaf's.

**typst.** A serious typesetter in Rust with a math mode of its own. Two
things rule it out here: its syntax is not the TeX that people already have
in their `$…$`, so every existing document would render wrong until
rewritten; and the crate is the whole compiler, an order of magnitude more
than a display list.

**ReX / other TeX-math crates.** ReX is the elegant one — real OpenType
`MATH`-table layout — and unmaintained, with a font-loading path that needs a
`MATH` font shipped and found at runtime. The others are early.

**RaTeX.** A KaTeX port, in pure Rust. Its pipeline is
`ratex-parser` → `ratex-layout` → a flat [`DisplayList`] of absolute-position
items — `GlyphPath { font, char_code, x, y, scale }`, `Line`, `Rect`, `Path`
— with `width`, `height` and `depth` in em. KaTeX's own fonts are embedded
(22 files, about 500 KiB) behind an `embed-fonts` feature; the same fonts are
what its `<text>`-mode SVG names by `font-family`. Coverage is stated against
KaTeX's test corpus and tracked, and it carries KaTeX's extensions — mhchem's
`\ce`, `bussproofs`. MIT, 0.1.14 as of July 2026, one workspace with a
C ABI, a wasm binding, PNG through tiny-skia, PDF through `pdf-writer`, and
`ratex-svg`. What matters most for leaf is the last one: with the
`standalone` feature, [`render_to_svg`] writes every glyph as a `<path>`,
producing an SVG document that depends on nothing.

[`DisplayList`]: https://github.com/erweixin/RaTeX/blob/main/crates/ratex-types/src/display_item.rs
[`render_to_svg`]: https://github.com/erweixin/RaTeX/blob/main/crates/ratex-svg/src/lib.rs

## The argument

**A formula is an SVG picture, and leaf already draws those.** Since
[SVG as vectors in leaf-swift](svg-as-vectors-on-apple.md), an SVG reaches
the Mac and iOS views as vectors through resvg-swift and the PDF as real
paths; the terminal rasterizes one through `leaf-raster::load_svg` and ships
the pixels over its graphics protocol; the web gives it to an `<img>`. Every
one of those paths was built for `![](logo.svg)`, and none of them cares
where the bytes came from. So the whole of "render math on four frontends"
collapses to one Rust function that turns a TeX string into an SVG string
and three numbers, and the frontends' existing picture paths do the rest.

The alternative — replaying RaTeX's `DisplayList` into `CGContext`, into
gpui's path API, into a canvas — is exactly the display-list replayer
resvg-swift already is, written a second time for a second list, and with a
cost that one did not have: `GlyphPath` names a font and a code point, so
every replayer needs the nineteen KaTeX faces registered with its text
system before it can draw a single glyph. Going through `ratex-svg` with
`embed-fonts` puts the outlines in the picture instead, once, in Rust. The
formula is byte-identical on every frontend because it is the same SVG, the
way a logo is.

**What crosses to the frontends is the picture plus its metrics.** The three
numbers are `width`, `height` and `depth`, RaTeX's own, in em. A display
formula needs only the first two to size its box; an inline one needs all
three, because it sits *in* a line and the text baseline has to pass through
it at `height` from the top. A frontend multiplies by its font size and
places the picture so the baselines agree — a `CTRunDelegate` with that
ascent, descent and width on Apple, an inline-block on the web, an element
in gpui. Colour is an input to the render, so a theme change is a re-render,
and the render is pure layout over embedded fonts: no I/O, no fonts to find,
fast enough to run on the layout thread and cache by `(tex, display, size,
colour)`.

**Core keeps doing what it does, and does not typeset.** `leaf-core` renders
a document to rows of glyphs with a source offset each, and for anything a
plain surface cannot draw it emits a placeholder and a side-table — block
media, leaf directives — that a capable frontend paints over, with
`rows_span` reserving the room. Display math is that pattern exactly: a
paragraph whose only visible child is `display_math` promotes, the way
`media_only` promotes a lone image, to a placeholder row and a
`MathInfo { rows_span, tex, display: true, span }` entry, with the two caret
homes [`block_media_stop`] already defines — one in front, one past, and
nothing inside the markup, because typing against a display formula
dissolves it into a paragraph the same way it dissolves a picture.

Inline math is the case the media pattern has no answer for, because an
inline image today is just its alt text. It needs one new thing: an *inline
atom* — a single glyph in the row, `stop: true`, whose `src` is the node's
start, standing for the whole formula, plus an entry in the same side-table
saying which row and column it is and what it stands for. The caret has a
stop before it and a stop after it, and the atom has a width the frontend
supplies from the metrics above. This is the `mark_ends` shape — a source
position with no glyph of its own that the caret can nonetheless stand at —
turned around: a glyph with no source of its own. It is a general mechanism
(an inline image could take it next), but math is what forces it.

**Editing a formula is editing its source, revealed.** `**bold**` hides its
delimiters because the content between them is the text; a formula's content
is *not* its picture, so hiding is not enough — there has to be a way to see
the TeX. leaf already has it: the reveal line. Under `MarkupMode::Full` the
caret's line renders its markup raw, and a `verbatim` or `inline_math` node
whose span meets that line already draws its delimiters. The proposal is
that math reveals on the caret's line **in every mode**, not only `Full`,
because for math the choice is not between a clean surface and a raw one but
between editable and not. Step the caret into a formula — from the stop
before it, with →, or by clicking the atom — and the row rebuilds with the
node revealed: `$`E = mc^2`` in the code style, exactly today's rendering.
Step out and it folds back to the picture. A display formula reveals to its
source lines, in the code style, the way a fence does. This is what Typora
and Obsidian do, and it costs core nothing new: `reveal` is already threaded
through every build and every cache key.

**Errors are shown, not hidden.** RaTeX's parser returns a `ParseError` for
TeX it cannot read. A formula that fails to typeset renders as its revealed
source with an error tint — the surface says "this is not yet a formula" in
place, rather than a blank box or a silently dropped node. That is also the
fallback for a frontend that cannot paint pictures inline at all: the
terminal, which today has no inline-picture path in leaf-raster, keeps
inline math as the code-styled text it already shows, and gets display math
as pixels over `rows_span` the way it gets a block image.

**Markdown gets the flag.** `math` joins `parse_extensions` as the fifth
extension, with the same justification the others have: leaf has somewhere
to put the node. This is a behavioural change for every Markdown document
leaf opens — `$` starts to mean something — and it is bounded by twig's own
rule that a dollar followed by whitespace never opens math, so `$5 and $6`
stays prose. It gets a `Behavioural-change:` trailer, and it gets its own
commit. A fenced code block whose info string is `math` (GitHub's spelling)
or `stem`/`latexmath` (what twig's AsciiDoc reader hands a stem block back
as) is the same display atom by another name, and takes the same path.

## What leaf does

**`leaf-math`, a small crate**, is the one function: `typeset(tex, display,
size, colour) -> Result<MathPicture { svg, width, height, depth }, MathError>`
over `ratex-parser`, `ratex-layout` and `ratex-svg` with `embed-fonts`. A
crate of its own rather than a corner of `leaf-raster` because `leaf-raster`
carries cosmic-text and resvg, which `leaf-wasm` does not take and should
not have to for this. `leaf-ffi` and `leaf-wasm` expose it; `leaf-ratatui`
and `leaf-gpui` call it directly. Nothing in it is leaf's, so if it grows a
consumer outside leaf it moves out, the way resvg-swift did.

**`leaf-core`** gains the `display_math` arm it never had, the block
promotion for a lone display formula, the inline atom, the `math` side-table,
and reveal-on-caret for math in every `MarkupMode`. The revealed rendering is
the existing `verbatim | inline_math` arm and needs no change. The Markdown
`math` flag is a one-line change with a trailer.

**Each frontend** wires its picture path to the new source: Apple through
`SVGPicture(data:)` and a run delegate for the inline case; the web through
an inline `<svg>` or `<img>`; gpui through its `svg` element, which is usvg
underneath and so the first SVG leaf-gpui draws at all; the terminal through
`load_svg` for display math over `rows_span`, and text for inline. The Swift
side already force-loads one Rust archive, and `leaf-math` lives inside it.

**Out of scope, and said so.** A palette or structured formula editor;
equation numbering that counts across a document (RaTeX numbers within one
call, so each display block starts at 1 — a document-level counter is a
later proposal if anyone wants it); AsciiMath, which AsciiDoc's stem defaults
to and RaTeX does not read; MathML export, which is twig's. And the size:
RaTeX with its fonts is roughly what resvg was, and the wasm build is the one
place that number is felt — the web task measures it, and if it is too much
the web frontend has the option the others do not, `ratex-svg`'s `<text>`
mode over KaTeX's webfont CSS.

[`block_media_stop`]: ../../crates/leaf-core/src/wysiwyg.rs
