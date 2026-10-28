use super::*;
use crate::style::{FontFamily, LineSpacing, SizeStep};

mod active_marks;
mod atoms;
mod click;
mod clipboard;
mod code_blocks;
mod containers;
mod display_columns;
mod enter;
mod file;
mod golden_cases;
mod line_ends;
mod links;
mod map_cache;
mod mark_edges;
mod math;
mod motion;
mod move_block;
mod ops;
mod presentation;
mod ragged_formats;
mod rich_view;
mod rules;
mod statistics;
mod table_keys;
mod tasks;
mod typing;
mod verse;
mod view_parity;

/// A document open in `view`. WYSIWYG motion reads the visual map, which the
/// renderer stamps each frame, so the map is built here too — a WYSIWYG doc
/// without one is a view no user is ever in.
fn doc_in(view: View, name: &str, body: &str) -> Doc {
    let p = fixture_path(&format!("{name}.md"));
    std::fs::write(&p, body).unwrap();
    let mut d = Doc::open(p).unwrap();
    d.view = view;
    if view == View::Wysiwyg {
        d.build_visual(80);
    }
    d
}

thread_local! {
    /// The directories this thread's test has asked for. libtest runs each
    /// test on a thread of its own, so they drop, and are removed with
    /// whatever the test wrote in them, when the test ends, pass or fail.
    static FIXTURE_DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `file_name` in a temp directory no other fixture shares, in this run or a
/// concurrent one, removed when the calling test ends. Nothing is written.
fn fixture_path(file_name: &str) -> PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("leaf_test_")
        .tempdir()
        .unwrap();
    let p = dir.path().join(file_name);
    FIXTURE_DIRS.with(|dirs| dirs.borrow_mut().push(dir));
    p
}

// Source-view document for the source-behaviour tests. `Doc::open` now
// defaults to WYSIWYG (leaf's default view), so pin the source view here;
// `wysiwyg_doc` builds the rich-text variant on top of this.
fn doc_with(name: &str, body: &str) -> Doc {
    doc_in(View::Source, name, body)
}

fn wysiwyg_doc(name: &str, body: &str) -> Doc {
    doc_in(View::Wysiwyg, name, body)
}

/// Every visual row's drawn text — what the reader actually sees, which is
/// the only thing the reveal preference is supposed to change.
fn drawn_rows(d: &Doc) -> Vec<String> {
    d.vmap
        .rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect()
}

/// Put the caret at the first byte of `needle` and rebuild, so the row under
/// it becomes the revealed line.
fn caret_at(d: &mut Doc, needle: &str) {
    d.caret = d.source.find(needle).expect("needle in source");
    d.build_visual(80);
}

/// Press Right until it stops, collecting the offsets walked through. Every
/// caret bug in the WYSIWYG view shows up here as a walk that ends early:
/// two stops sharing one source offset can't be moved between, so the caret
/// stalls on the first of them and the walk never reaches the rest.
fn walk_right(d: &mut Doc) -> Vec<usize> {
    let mut seen = vec![d.caret];
    for _ in 0..2000 {
        let before = d.caret;
        d.move_right(false);
        if d.caret == before {
            break;
        }
        seen.push(d.caret);
    }
    seen
}

/// The node the source parses as at `caret` — what confirms an inserted
/// `---` actually reads back as a rule, not stray text or a setext heading.
///
/// The *narrowest* node covering the offset. Every ancestor covers it too,
/// and since twig 2.8 that includes the `doc` root, which now carries a real
/// span (it reported none before, so taking the first match used to land on
/// the block by luck and now always answers `"doc"`).
fn kind_at(d: &mut Doc, caret: usize) -> Option<Kind> {
    d.nodes()
        .into_iter()
        .filter(|n| n.span.start <= caret && caret < n.span.end)
        .min_by_key(|n| n.span.end - n.span.start)
        .map(|n| n.kind)
}

/// The counts of `body`, from a document open in the WYSIWYG view — the
/// shape the statistics cases start from.
fn counts_of(name: &str, body: &str) -> TextCounts {
    doc_in(View::Wysiwyg, name, body).counts()
}

/// Both views, each with the name an assertion reports it by.
const VIEWS: [(View, &str); 2] = [(View::Source, "source"), (View::Wysiwyg, "wysiwyg")];

// ── golden-case harness ──────────────────────────────────────────────────
// The pattern the whole parity suite can reuse: write a fixture with the
// caret marked by `|`, run one action, and compare the rendered result —
// also caret-marked — against the expected string. One readable line per
// behavior, and it exercises the exact `Doc` ops both frontends call.

/// Split a `|`-marked fixture into `(source, caret_offset)`.
fn parse_caret(marked: &str) -> (String, usize) {
    let caret = marked.find('|').expect("fixture needs a `|` caret marker");
    (marked.replacen('|', "", 1), caret)
}

/// Render a doc's source with `|` at the caret (and `[`…`]` around any
/// selection) so a result reads like the fixtures.
fn render_caret(d: &Doc) -> String {
    // (offset, rank, char); rank keeps coincident markers ordered `[ | ]`
    // so the caret always renders inside its own selection.
    let mut marks: Vec<(usize, u8, char)> = vec![(d.caret, 1, '|')];
    if let Some((s, e)) = d.selection() {
        marks.push((s, 0, '['));
        marks.push((e, 2, ']'));
    }
    // Insert right-to-left: descending offset, then descending rank.
    marks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = d.source.clone();
    for (at, _, ch) in marks {
        out.insert(at, ch);
    }
    out
}

/// Load a `|`-marked fixture, run `action`, return the caret-marked result.
fn golden(name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
    golden_in(View::Source, name, marked, action)
}

/// [`golden`] in a chosen view — the editing ops are the view's to share, so
/// the same fixture has to read the same way in both.
fn golden_in(view: View, name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
    let (src, caret) = parse_caret(marked);
    let mut d = doc_in(view, name, &src);
    d.caret = caret;
    action(&mut d);
    render_caret(&d)
}

// ── reference maps ───────────────────────────────────────────────────────

/// A from-scratch, cache-free WYSIWYG map for `source` — the ground truth the
/// incremental (`build_spliced` / `build_cached`) path must always match.
fn reference_map(source: &str) -> crate::wysiwyg::VisualMap {
    reference_map_revealing(source, None)
}

/// [`reference_map`] with a reveal line — the ground truth for the
/// `MarkupMode::Full` builds, where the map is a function of the caret's
/// line as well as the text.
fn reference_map_revealing(source: &str, reveal: Option<Reveal>) -> crate::wysiwyg::VisualMap {
    // The same parse `Doc` uses. With twig's plain defaults instead, the two
    // sides disagree on what the *document* is before the renderer is even
    // reached — a bare `:word` is a text directive to one and prose to the
    // other — and the mismatch reads as a splice bug that isn't one.
    let mut ed = twig::Editor::new_with_flags(
        source.as_bytes(),
        Format::Markdown.twig(),
        parse_extensions(Format::Markdown),
    )
    .unwrap();
    let nodes = ed.nodes().unwrap();
    crate::wysiwyg::build(
        &nodes,
        source,
        None,
        false,
        &wysiwyg::Surface::default(),
        reveal,
    )
}

fn maps_differ(a: &crate::wysiwyg::VisualMap, b: &crate::wysiwyg::VisualMap) -> bool {
    if a.rows.len() != b.rows.len() {
        return true;
    }
    for (ra, rb) in a.rows.iter().zip(&b.rows) {
        if ra.end_src != rb.end_src || ra.glyphs.len() != rb.glyphs.len() {
            return true;
        }
        for (ga, gb) in ra.glyphs.iter().zip(&rb.glyphs) {
            if ga.ch != gb.ch || ga.src != gb.src {
                return true;
            }
        }
    }
    false
}
