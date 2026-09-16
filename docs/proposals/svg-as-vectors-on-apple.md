---
title: SVG as vectors in leaf-swift, through usvg
status: implemented
created: 2026-09-12
updated: 2026-09-13
part_of: '[Proposals](/docs/proposals/proposals.md)'
---
# SVG as vectors in leaf-swift, through usvg

## Status

Implemented 2026-09-13, in `feat(leaf-swift): SVG as vectors, through
resvg-swift`. The renderer is its own repository,
[resvg-swift](https://github.com/diaryx-org/resvg-swift) 0.1.0 — a usvg →
display list crate held to resvg's own test suite (1,697 cases, 1,676
byte-identical), a UniFFI binding, and a `CGContext` replayer — because nothing
in it is leaf's. leaf's part was the task
[SVG through resvg-swift](../tasks/svg-through-resvg-swift.md).

## The picture

`![logo](logo.svg)` in a document. The TUI shows the logo: `leaf-raster`
rasterizes it with resvg and the terminal paints the pixels. The macOS and iOS
views show a dashed box.

`MediaStore` decodes every still through ImageIO
(`packages/leaf-swift/Sources/LeafUI/MediaLayout.swift`, `load`/`decode`), and
ImageIO has no SVG codec: `CGImageSourceGetCount` is zero, `still` is `nil`,
and a `nil` still on an image box is exactly the `isBroken` chip. Nothing is
wrong with the document. The renderer can't read the format.

## What was considered

**CoreSVG, Apple's own.** `NSImage(data:)` renders SVG on macOS, undocumented,
through the private CoreSVG framework. On iOS there is no public entry at all
— `UIImage(data:)` returns `nil` for SVG bytes; only asset-catalog SVGs render.
Reaching it means `CGSVGDocument*`, private API. And what it renders is a
subset tuned for icons, with no specification and at least one known crash
(CSS `opacity` alongside `fill`). leaf-swift has an iOS view, so this is
macOS-only or private-API. Out.

**A Swift SVG library.** SwiftDraw is the credible one: pure Swift, renders
through `CGContext`, active, supports text (with embedded WOFF fonts),
gradients, patterns, clips, masks — and of filters, `feGaussianBlur` only.
It would work, and quickly. But it is a second SVG parser with its own edge
cases beside the resvg the TUI uses: a document would render one way in the
terminal and another on a Mac, and each disagreement would be a bug to chase
in a codebase that isn't ours.

**Route leaf-raster through the FFI.** resvg's bitmap over UniFFI — what
`leaf-raster::load_svg` already does for the terminal. Consistent with the
TUI, trivial to wire. But a bitmap: rasterized at some scale, soft when the
column is wider or the display sharper than it guessed, and a bitmap in the
PDF `DocumentPDF` writes. The TUI has no choice; the Apple views do.

## The argument

usvg is a *simplifier*, separable from resvg the *rasterizer*. Its output tree
has CSS applied, `<use>` expanded, units resolved, and `<text>` laid out into
glyph paths with system fonts. What is left is paths, groups, and paints —
which is what CoreGraphics draws. Walking that tree and emitting a flat list
of draw operations is a few hundred lines; replaying the list into a
`CGContext` is a few hundred more. `vello_svg` is the same pattern for vello.

That gives the Apple views what neither alternative does: **the same parse as
the terminal** (one set of edge cases, one answer to "what does this SVG look
like in leaf") and **vector output** — crisp at any size and real paths in the
PDF. Filters, masks and pattern paint, which CoreGraphics has no primitive
for, are rendered by resvg at the exact scale they will be shown at and
arrive as pixels inside the otherwise-vector list; the consumer picks the
scale, so they are as sharp as the screen.

And it is testable without a Mac in the loop: a tiny-skia replayer of the
same list, compared pixel for pixel against resvg's own render over the same
SVG, proves the flattening lossless. The CoreGraphics side is then a
translation of that replayer, one match arm at a time.

## Why a separate repository

Nothing in it is about leaf. A display-list crate over usvg is useful to any
Rust → native-canvas bridge, and the Swift replayer is generic over
`CGContext`. The org's test for a sibling rather than a part — *usable without
the parent at all* — is met, and the org's dependency rule (crates.io by
version, never a path across repos) means leaf takes it as a pin either way.
gpui's leaf view doesn't render SVG today either (`leaf-gpui` treats it as a
failed decode); a second replayer for gpui's path API belongs next to the
first, not in leaf.

## What leaf does

Small. `MediaStore` gains a second decode path — `.svg` by extension, or bytes
ImageIO refuses — producing an `SVGPicture` rather than a `CGImage`;
`MediaLayout` takes its intrinsic size from the document; the box draws it
vector into the view and into the PDF. `leaf-raster::load_svg` stays as the
terminal's path. The linking constraint is the one thing to get right: two
Rust staticlibs can't share an executable, so `leaf-ffi` takes `resvg-uniffi`
as a Cargo dependency and the app keeps force-loading the one archive it
already builds.
