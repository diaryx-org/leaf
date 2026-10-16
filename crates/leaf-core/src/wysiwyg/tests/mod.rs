use super::*;
use crate::Format;
use crate::style::{FontFamily, LineSpacing, SizeStep};
use twig::{Editor, NodeId};

mod block_boundaries;
mod code_blocks;
mod decorations;
mod directives;
mod display_columns;
mod footnotes;
mod hidden_blocks;
mod mapping;
mod math;
mod presentation;
mod rows;
mod tables;
mod verse;

fn map(src: &str) -> VisualMap {
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    build_t(&ed.nodes().unwrap(), src, Some(80))
}

/// [`map`] over a Djot source. Djot is the format that spells superscript
/// and subscript at all — Markdown has no syntax for either.
fn map_djot(src: &str) -> VisualMap {
    let mut ed = Editor::new_str(src, Format::Djot.twig()).unwrap();
    build_t(&ed.nodes().unwrap(), src, Some(80))
}

/// The baseline every glyph spelling `ch` was built with, in row order —
/// how a test reads a raised or lowered run off the map without caring
/// which row it landed on.
fn baselines_of(m: &VisualMap, ch: char) -> Vec<Baseline> {
    m.rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .filter(|g| g.ch == ch)
        .map(|g| g.style.baseline)
        .collect()
}

/// [`map`] at a chosen wrap width.
fn map_at(src: &str, wrap: Option<usize>) -> VisualMap {
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    build_t(&ed.nodes().unwrap(), src, wrap)
}

/// [`map`], but with twig's `directives` extension on (off by twig's own
/// default) — the `:::name{.class}` fenced-div containers leaf-core's
/// `"directive"` wysiwyg arm renders.
fn map_directives(src: &str) -> VisualMap {
    let mut ed =
        Editor::new_with_features(src.as_bytes(), Format::Markdown.twig(), &["directives"])
            .unwrap();
    build_t(&ed.nodes().unwrap(), src, Some(80))
}

/// [`map`] in `format`, parsed the way every leaf document is — the
/// extensions [`crate::doc::parse_extensions`] turns on, which is what
/// pairs a Markdown `<div …>` with its `</div>` into a container and makes
/// `::page-break` a directive rather than a paragraph of colons.
fn map_leaf(src: &str, format: Format) -> VisualMap {
    let mut ed = Editor::new_with_flags(
        src.as_bytes(),
        format.twig(),
        crate::doc::parse_extensions(format),
    )
    .unwrap();
    build_t(&ed.nodes().unwrap(), src, Some(80))
}

/// [`map`] with soft breaks preserved (`LineFlow::Preserve`).
fn map_preserve(src: &str, wrap: Option<usize>) -> VisualMap {
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    build(
        &ed.nodes().unwrap(),
        src,
        wrap,
        true,
        &Surface::default(),
        None,
    )
}

/// The cache-free reference [`build`], with no per-image height overrides —
/// every block image stays its default one-row placeholder. The tests that
/// need a taller image drive it through [`crate::Doc::set_media_rows`] instead.
fn build_t(nodes: &[FlatNode], src: &str, wrap: Option<usize>) -> VisualMap {
    build(nodes, src, wrap, false, &Surface::default(), None)
}

/// The [`Role`] of the first glyph spelling `ch` — how a test reads one
/// run's styling off a map without caring which row it landed on.
fn role_of(m: &VisualMap, ch: char) -> Role {
    m.rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .find(|g| g.ch == ch)
        .unwrap_or_else(|| panic!("no glyph spelling {ch:?}"))
        .style
        .role
}

/// A production-path build with both extensions on — the only way to put a
/// promoted HTML element and a directive in one document, which is what the
/// `container` kind made necessary to tell apart. Returns the whole `Doc`
/// because [`VisualMap`] is not `Clone`; read `doc.vmap`.
fn doc_built(src: &str) -> crate::Doc {
    let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
    doc.build_visual(80);
    doc
}

/// The row texts of `m`, one string per row.
fn row_texts(m: &VisualMap) -> Vec<String> {
    m.rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect()
}

fn rendered(m: &VisualMap) -> String {
    m.rows
        .iter()
        .map(|r| r.glyphs.iter().map(Glyph::drawn).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a source both ways: `build` over the whole marshalled arena (the
/// reference), and `build_cached` driven the way [`crate::Doc`] drives it —
/// top-level blocks from `child_spans`, per-block subtrees on a miss.
fn render_both(
    ed: &mut Editor,
    src: &str,
    wrap: Option<usize>,
    cache: &mut BlockCache,
) -> (VisualMap, VisualMap) {
    let all = ed.nodes().unwrap();
    let surface = Surface::default();
    let plain = build(&all, src, wrap, false, &surface, None);
    let top = top_blocks(ed);
    let cached = build_cached(&top, src, wrap, false, &surface, None, cache, |id| {
        ed.subtree(NodeId(id)).unwrap_or_default()
    });
    (plain, cached)
}

/// `| Name | Qty |` with Name left-aligned and Qty right-aligned.
const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

/// Every drawn boundary in `src`, in order, as `(above, below)`.
fn boundaries(m: &VisualMap) -> Vec<(BlockClass, BlockClass)> {
    m.rows
        .iter()
        .filter_map(|r| r.boundary)
        .map(|b| (b.above, b.below))
        .collect()
}
