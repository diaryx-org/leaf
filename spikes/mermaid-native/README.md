# Spike: Mermaid without a browser

Can [mmdr](https://github.com/1jehuang/mermaid-rs-renderer)
(`mermaid-rs-renderer` 0.3.1, MIT) replace mermaid.js in leaf's Mermaid
editors? The idea is to draw a fence as SVG the way leaf draws a formula: through
resvg on Apple, and as an image on the web. See the open question "Bring your
own renderer" in
[the proposal](../../docs/proposals/embedded-editors.md).

This directory is its own Cargo workspace, so leaf itself does not depend on
it.

## Run it

```sh
cargo run --release -- corpus <out>   # SVG + resvg PNG per diagram, report.tsv, index.html
cargo run --release --example stages corpus/flow-big.mmd
```

`<out>/index.html` loads mermaid.js from a CDN, so serve `<out>` over HTTP.
Each diagram then appears three ways, side by side: mermaid.js, mmdr's SVG,
and that SVG drawn through resvg 0.46, the version leaf uses.

`wasm-size/` builds mmdr as a one-function web module, to measure its size
and check that it runs.

## What it found (2026-10-11)

The corpus has 30 diagrams of 14 kinds: flowchart, sequence, class, state,
ER, gantt, pie, mindmap, timeline, gitGraph, journey, quadrant, XY chart,
and one invalid input.

**Coverage is broad.** All 29 valid diagrams render, and the invalid one is
refused with a usable message (`invalid flowchart edge syntax: A -->`).
mmdr writes plain SVG `<text>` with no `<foreignObject>`, so resvg draws every
one. Flowcharts, class, ER, gantt, pie, quadrant, XY chart, timeline,
mindmap, gitGraph and journey all read well next to mermaid.js. Gantt is
arguably clearer.

**Fidelity gaps:**

- Edge layout gets tangled when a flowchart uses many edge kinds
  (`flow-edges`), and an edge label is clipped. mermaid.js lays out the same
  diagram in a straight line.
- Edge labels inside subgraphs sit awkwardly.
- Markdown labels (`` "`**Bold**`" ``) are drawn as raw text.
- In a sequence diagram, `rect rgb(...)` is drawn as a labelled frame
  instead of a coloured background.
- State diagrams with composites overlap their labels.

**Speed.** Most diagrams take 0.05–2.5 ms warm. Flowcharts with many edges
are much slower: `flow-basic` takes 22 ms, `flow-edges` 105 ms and
`flow-big` 162 ms. Nearly all of that is the port-assignment stage of
layout: in one run of `examples/stages.rs`, 178 ms of `flow-big`'s 222 ms of
layout. That is too slow to redraw a preview on every keystroke without debouncing,
and it is worth raising upstream.

**The web.** As published, mmdr panics on `wasm32-unknown-unknown`: its
layout calls `std::time::Instant` even for a plain `render`. With a copy
patched to use the `web-time` crate (four files), it runs in Chrome. A
flowchart with Unicode and an emoji renders correctly, with label widths
estimated, because there are no system fonts there. The module is 1.6 MB raw
and about 600 KB gzipped after `wasm-opt -Oz`. That is about half of leaf's
math module.

**Fonts.**

- mmdr measures text through a process-wide `fontdb` that loads system
  fonts. Where there are none, as on the web, it falls back to an estimate.
- It also caches the faces it finds under `~/.cache/mmdr/`, which a
  sandboxed app may not be able to write, and which a library should not do
  unasked.
- Through resvg with system fonts, labels in other scripts and emoji came
  out as empty boxes (`flow-long-labels`, `state-composite`'s `⇧←`). The
  browser draws the same SVG correctly. leaf's raster path needs font
  fallback configured before these diagrams can go through it.
- resvg-swift can already load system fonts (`ParseOptions.load_system_fonts`),
  so mmdr's `<text>` can be drawn on Apple.

## The fork (2026-10-11)

Upstream already has open PRs for the two library problems. Until they
merge, the spike uses
[adammharris/mermaid-rs-renderer](https://github.com/adammharris/mermaid-rs-renderer),
branch `diaryx`, which is upstream `master` plus:

- [#155](https://github.com/1jehuang/mermaid-rs-renderer/pull/155):
  `web-time` in place of `std::time::Instant`. With it, the wasm module
  renders in Chrome with no local patch.
- [#151](https://github.com/1jehuang/mermaid-rs-renderer/pull/151): a
  `TextMetrics` trait on `LayoutConfig`, so a host can measure text itself
  (Core Text, or a canvas) and mmdr's system-font loading and `~/.cache`
  writes never run.

Upstream `master` is 24 commits past 0.3.1. Against this corpus:

- **Speed is unchanged.** Run back to back under the same load, 0.3.1 and the
  fork are within noise of each other.
- **Edge labels on `flow-edges` are no longer clipped,** though the layout is
  still tangled.
- **A dagre-style flowchart engine** (`layout.flowchart.engine`, set by
  `MMDR_ENGINE=dagre|auto` here) lays `flow-edges` out in rank order but does
  worse on `flow-big`. `Auto` chooses between the two per diagram.
- **A `scene` feature** turns the SVG into draw commands through `usvg`. leaf's
  resvg-swift already does that with any SVG, so leaf does not need it.

## What came of it

Built on the `mermaid-native-spike` branch as `crates/leaf-mermaid` and
`crates/leaf-mermaid-wasm`, with `render_mermaid` in leaf-ffi. See the
proposal's Status section. Items 1–3 below are done. Font fallback (4) is
not. Debouncing (5) is done in both demos, and the web one renders in a
worker.

## What it would take

1. **Upstream fixes to mmdr.** Both are open PRs, carried in the fork
   until they merge (above).
2. **A leaf-side binding:** `render_mermaid(source) -> svg` in leaf-ffi and in
   a lazily loaded web module, as the math typesetter is.
3. **The demos:** the editors draw that SVG. On Apple it goes through
   `SVGPicture` with system fonts. On the web it is an `<img>`, or the SVG
   inline.
4. **Font fallback** in leaf's resvg path, for emoji and other scripts.
5. **Debounce the preview** for large flowcharts, until port assignment is
   faster.

Fidelity on the gaps above is mmdr's to improve. They are cosmetic, except for
the tangled flowchart edges, which is the one to watch, since flowcharts are
the most common kind.
