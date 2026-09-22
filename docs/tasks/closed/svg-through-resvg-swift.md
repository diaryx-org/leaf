---
status: done
created: 2026-09-12
updated: 2026-09-13
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# SVG through resvg-swift

**Status.** Done, in `feat(leaf-swift): SVG as vectors, through resvg-swift`
(2026-09-13), against resvg-swift 0.1.0. The steps below went as written, with
one change: `MediaStill` is an enum over `CGImage` and `SVGPicture` rather than
a second optional beside `still`, so the box has one thing to measure and one
to draw. The PDF assertion is `testAnSVGReachesThePageAsPathsAndAPNGAsAnImage`.

**Where.** `packages/leaf-swift`, `MediaStore`/`MediaLayout` in
`Sources/LeafUI/MediaLayout.swift`; the media box's draw in `LeafTextView` and
`LeafTextViewiOS`; `DocumentPDF`; `crates/leaf-ffi/Cargo.toml`.

**What.** An SVG `![…](x.svg)` shows the dashed broken-image chip in both
Apple views, because `MediaStore` decodes stills through ImageIO and ImageIO
has no SVG codec. The proposal
[SVG as vectors in leaf-swift, through usvg](/docs/proposals/svg-as-vectors-on-apple.md)
settles how: the [resvg-swift](https://github.com/diaryx-org/resvg-swift)
package, whose `SVGPicture` parses with usvg and draws vector into a
`CGContext`.

**Waits on.** resvg-swift's first release (its
`docs/tasks/first-release.md`). Until there is a version to pin, this cannot
start — the org's dependency rule is crates.io by version, and the Swift
package resolves from a tag.

**Steps, once it can.**

1. `leaf-ffi` takes `resvg-uniffi` as a dependency, `pub use resvg_uniffi as _;`
   so its UniFFI scaffolding is in the one staticlib the app force-loads. Two
   Rust archives in one executable is a duplicate-`std` link error, so this is
   the only shape that works.
2. `Package.swift` adds the `resvg-swift` package by version; `LeafUI` depends
   on `ResvgCoreGraphics`.
3. `MediaStore.Entry.ready` grows a vector alternative to `still: CGImage?` —
   an `SVGPicture` — chosen when the extension is `.svg`/`.svgz` or when
   `CGImageSourceCreateWithData` returns nothing decodable. `data:` URIs too.
   `MediaLayout` takes the aspect ratio from `SVGPicture.size`.
4. The box's draw calls `picture.draw(in:rect:scale:)` with the view's
   backing scale, in the flipped coordinate space the views already draw in;
   `DocumentPDF` passes `1`. Both stay vector.
5. `isBroken` stays what it is: an image box with neither a still nor a
   picture.
6. `Tests/LeafUITests` gets an SVG fixture through `MediaLayoutTests` and a
   PDF test asserting the SVG's path reaches the PDF as a path, not an image.

`leaf-raster::load_svg` is untouched; the terminal keeps its bitmap.
