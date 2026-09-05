//! Syntax highlighting for the source view — the AST read as *markup* rather
//! than as rendered text.
//!
//! [`crate::View::Wysiwyg`] resolves the document's markup away and styles what
//! is left; [`crate::View::Source`] shows the markup itself, and until now
//! showed it unstyled. This module is the missing half: a [`SourceMap`] of
//! styled byte ranges over `Doc::source`, so a frontend painting raw source can
//! tell a heading from its `# `, a link from its destination, and a fence from
//! the code inside it.
//!
//! # Why this and not a syntax-highlighting library
//!
//! leaf already has a parse of these exact bytes — twig's, the one the caret
//! rides. A second parser (syntect, tree-sitter) is a second opinion about what
//! the document is, and the two disagreeing is visible: text painted as emphasis
//! that the editor then refuses to treat as emphasis. Reading the styling off
//! the same AST the editing model uses makes that class of bug unrepresentable.
//!
//! It also costs nothing per format. twig normalizes Markdown, Djot, HTML and
//! XML into one [`Kind`] vocabulary, so `<b>bold</b>`, `**bold**` and `*bold*`
//! all arrive as [`Kind::Strong`] and are styled by the same line of code.
//!
//! # The rule
//!
//! Every node knows its whole extent ([`FlatNode::span`]) and, where it has
//! delimiters, the extent of what is *inside* them
//! ([`FlatNode::content_span`]). The difference between the two is exactly the
//! markup:
//!
//! ```text
//!   [link](https://example.dev)
//!   ^^^^^^^^^^^^^^^^^^^^^^^^^^^  span
//!    ^^^^                        content_span
//!   ^    ^^^^^^^^^^^^^^^^^^^^^^  the gaps — the markup
//! ```
//!
//! So the whole highlighter is: style a node's span by its kind, then restyle
//! the bytes its content doesn't cover as [`Role::Delimiter`]. Children paint
//! over their parents, inheriting the parent's style the same way
//! [`crate::wysiwyg`] threads a `base` down the tree — which is what keeps
//! `*em*` inside a heading both heading-colored and italic.
//!
//! # What it does not do
//!
//! **The inner language of a fenced code block.** ` ```rust ` gets
//! [`Role::Code`] over the whole body; twig knows the fence and the info string,
//! not Rust. Highlighting *that* is the one job an external highlighter is
//! actually right for, and it belongs in the frontends that can afford the
//! dependency — not in a core that also ships to wasm and iOS.
//!
//! **Bytes no node covers.** A link-reference definition and a footnote
//! definition hang off no parent (see `Editor::definitions`), and twig leaves
//! some inter-element whitespace unparented; the walk starts at the root, so
//! those stay [`Role::Body`]. Unstyled is the correct failure here — the text is
//! still the text.

use std::ops::Range;

use twig::{FlatNode, Kind};

use crate::style::{Baseline, MarkColor, Role, Style};

/// A run of source bytes that share one style. Ranges are source byte offsets,
/// like the caret and [`crate::Highlight`], so nothing has to be converted to
/// paint one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledRun {
    /// The bytes this run covers, `[start, end)`.
    pub span: Range<usize>,
    /// What to paint them as.
    pub style: Style,
}

/// The source view's styling, as non-overlapping runs in ascending order.
///
/// Gaps between runs are [`Role::Body`] — the map stores only what differs from
/// plain text, so an ordinary prose document is a handful of runs rather than
/// one per byte.
///
/// Built by [`build`] and cached on the [`Doc`](crate::Doc) against its
/// revision; a frontend reads it through [`SourceMap::style_at`] for a one-shot
/// question, or [`SourceMap::edges_in`] when it is already walking lines in
/// order and wants to know where the styling changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceMap {
    /// Ascending, non-overlapping, and never [`Role::Body`] — see the type docs.
    runs: Vec<StyledRun>,
}

impl SourceMap {
    /// The styled runs, ascending and non-overlapping. Bytes between them are
    /// [`Style::default`].
    pub fn runs(&self) -> &[StyledRun] {
        &self.runs
    }

    /// Whether the map styles nothing — a document with no markup in it, or one
    /// that has not been built yet.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The style covering source byte `offset`, or [`Style::default`] where no
    /// run does.
    ///
    /// A binary search, for a caller asking about one offset. A painter walking
    /// the document in order should use [`edges_in`](Self::edges_in) instead and
    /// ask once per *run* rather than once per byte.
    pub fn style_at(&self, offset: usize) -> Style {
        match self.runs.binary_search_by(|r| {
            if r.span.end <= offset {
                std::cmp::Ordering::Less
            } else if offset < r.span.start {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(i) => self.runs[i].style,
            Err(_) => Style::default(),
        }
    }

    /// Append every styling boundary strictly inside `range` to `out`, in
    /// ascending order — the offsets where a painter has to break a span
    /// because the style changes there.
    ///
    /// Both edges of every overlapping run, since a run that starts inside the
    /// range and one that ends inside it are equally a place the color changes.
    /// The range's own ends are left to the caller, which already has them.
    pub fn edges_in(&self, range: Range<usize>, out: &mut Vec<usize>) {
        // The first run that reaches into the range. Runs are ascending and
        // disjoint, so from here it is a walk until one starts past the end.
        let from = self.runs.partition_point(|r| r.span.end <= range.start);
        // Two runs that abut share one boundary; it is one place the style
        // changes, so it is reported once. `last` rather than a dedup pass
        // because the edges arrive in ascending order already.
        let mut last = None;
        for run in &self.runs[from..] {
            if run.span.start >= range.end {
                break;
            }
            for edge in [run.span.start, run.span.end] {
                if edge > range.start && edge < range.end && last != Some(edge) {
                    out.push(edge);
                    last = Some(edge);
                }
            }
        }
    }
}

/// Style the source of a parsed document.
///
/// `nodes` is the whole arena as [`twig::Editor::nodes`] returns it, over the
/// `source` it was parsed from — the spans do the work, and the text is read
/// only to tell a delimiter from the whitespace around it (see
/// [`fill_markup`]).
///
/// The walk starts at the [`Kind::Doc`] root and goes depth-first, so a node is
/// always painted before the children that overwrite parts of it. It uses an
/// explicit stack rather than recursion: nesting depth is the *document's*, and
/// a thousand nested block quotes should slow a repaint down, not end it.
pub fn build(nodes: &[FlatNode], source: &str) -> SourceMap {
    let Some(root) = nodes.iter().position(|n| n.kind == Kind::Doc) else {
        return SourceMap::default();
    };
    let len = source.len();
    if len == 0 {
        return SourceMap::default();
    }

    // One style per byte, collapsed to runs at the end. The document is walked
    // once and each byte written once per level of nesting over it, which for
    // real markup is a small constant — and it makes "the child wins" fall out
    // of the write order instead of needing an interval tree to arbitrate.
    let mut paint = vec![Style::default(); len];
    let mut stack = vec![(root, Style::default())];
    while let Some((id, base)) = stack.pop() {
        let node = &nodes[id];
        let style = style_of(node, base);

        // The node's own extent first, then the bytes its content leaves out —
        // those are its delimiters, and they are scaffolding whatever the node
        // itself is. `Role::Delimiter` sits on top of the run's own emphasis,
        // exactly as `wysiwyg::Builder::push_delim` lays it on a revealed line,
        // so the `**` around a bold phrase comes out dim *and* bold.
        //
        // Markup goes down through `fill_markup`, which declines a stretch with
        // no markup actually in it — a `soft_break` that is one bare newline, a
        // block whose span runs a line further than its content. Both are gaps
        // in the arithmetic sense and neither has anything to dim.
        if style.role == Role::Delimiter {
            fill_markup(&mut paint, source, &node.span, style);
        } else {
            fill(&mut paint, &node.span, style);
        }
        if let Some(content) = &node.content_span {
            let delim = style.role(Role::Delimiter);
            fill_markup(&mut paint, source, &(node.span.start..content.start), delim);
            fill_markup(&mut paint, source, &(content.end..node.span.end), delim);
        }

        let mut child = node.first_child;
        while let Some(cid) = child {
            let i = cid.0 as usize;
            let Some(n) = nodes.get(i) else { break };
            stack.push((i, style));
            child = n.next_sibling;
        }
    }

    SourceMap {
        runs: to_runs(paint),
    }
}

/// Paint `span` with `style`, clipped to the buffer. A span reaching past the
/// source can only come from an arena and a string that have drifted apart; the
/// clip means that renders wrong rather than panicking in a paint loop.
fn fill(paint: &mut [Style], span: &Range<usize>, style: Style) {
    let start = span.start.min(paint.len());
    let end = span.end.min(paint.len());
    if start < end {
        paint[start..end].fill(style);
    }
}

/// [`fill`] for a stretch of *markup*, which declines one that holds none.
///
/// Almost every block's span runs to the end of the line its content ends on, so
/// the arithmetic leaves a trailing `"\n"` outside `content_span` — and a plain
/// `soft_break` is a bare newline that this module dims for the sake of the
/// `"> "` a block quote sometimes hangs on it. Painting either changes nothing a
/// reader can see: whitespace has no glyph to dim.
///
/// It is not free, though. It splits the run that covers it, so a document of
/// ordinary prose comes back as one styled run per line instead of none — which
/// is a map every painter then walks, and a `SourceMap::is_empty` that is never
/// true. Declining is what keeps "no markup" costing nothing.
fn fill_markup(paint: &mut [Style], source: &str, span: &Range<usize>, style: Style) {
    let blank = source
        .get(span.start.min(source.len())..span.end.min(source.len()))
        .is_none_or(|s| s.trim().is_empty());
    if !blank {
        fill(paint, span, style);
    }
}

/// Collapse the per-byte buffer into ascending runs, dropping the [`Role::Body`]
/// stretches — those are the default the map's gaps already mean.
fn to_runs(paint: Vec<Style>) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut start = 0usize;
    for i in 1..=paint.len() {
        if i < paint.len() && paint[i] == paint[start] {
            continue;
        }
        if paint[start] != Style::default() {
            runs.push(StyledRun {
                span: start..i,
                style: paint[start],
            });
        }
        start = i;
    }
    runs
}

/// A node's style, layered on the style it inherits from its parent.
///
/// Deliberately the same decisions [`crate::wysiwyg`] makes for the rendered
/// view — `emph` is italic in both, `verbatim` is [`Role::Code`] in both — so
/// toggling ⌘E between the two views recolors the markup without recoloring the
/// prose.
///
/// [`Kind`] is `#[non_exhaustive]`; an unmapped kind inherits its parent's
/// style, which is why a node twig grows later shows up as ordinary text rather
/// than as a compile error.
fn style_of(node: &FlatNode, base: Style) -> Style {
    match node.kind {
        // A heading's level picks the style, as it does in the rendered view.
        // `level` is `None` on a malformed heading; treat it as the top one.
        Kind::Heading => base.role(Role::Heading(node.level.unwrap_or(1).clamp(1, 255) as u8)),

        // The inline marks, matched to `wysiwyg`'s arms one for one.
        Kind::Emph => base.italic(),
        Kind::Strong => base.bold(),
        Kind::Mark => base.role(Role::Mark(MarkColor::from_attrs(&node.attrs))),
        Kind::Insert => base.underline(),
        Kind::Delete => base.strikethrough(),
        Kind::Superscript => base.baseline(Baseline::Super),
        Kind::Subscript => base.baseline(Baseline::Sub),

        // Code, and the things that read like it. `raw_block`/`raw_inline` are
        // markup twig passed through untouched (an HTML tag in a Markdown
        // document) — verbatim source inside a document, which is what
        // `Role::Code` means.
        Kind::CodeBlock
        | Kind::Verbatim
        | Kind::InlineMath
        | Kind::DisplayMath
        | Kind::RawBlock
        | Kind::RawInline => base.role(Role::Code),

        // Anything that points somewhere. A reference and a citation resolve to
        // a definition elsewhere in the document, which is a link by another
        // name — `wysiwyg` styles them `Role::Link` for the same reason.
        Kind::Link
        | Kind::Url
        | Kind::Email
        | Kind::Reference
        | Kind::Citation
        | Kind::FootnoteReference
        | Kind::CitationReference
        | Kind::SubstitutionReference => base.role(Role::Link),

        Kind::ThematicBreak => base.role(Role::Rule),

        // Scaffolding with no rendered form of its own: an XML declaration, a
        // doctype, a comment, a CDATA wrapper. Dimmed whole rather than by its
        // delimiters, because all of it is machinery.
        Kind::Comment | Kind::Doctype | Kind::ProcessingInstruction | Kind::Cdata => {
            base.role(Role::Delimiter)
        }

        // A soft break carries the *continuation* markers with it — the `> ` a
        // block quote repeats on its second line, the indent under a list item
        // — so dimming it dims those, which no node's delimiter gap reaches. A
        // plain soft break is one invisible newline and is dimmed for nothing.
        Kind::SoftBreak => base.role(Role::Delimiter),

        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twig::{Editor, Format};

    /// Build a map the way `Doc` does, and hand back the source alongside it so
    /// assertions can name bytes by the text they cover rather than by offset.
    fn map(src: &str, format: Format) -> SourceMap {
        let mut ed = Editor::new_str(src, format).unwrap();
        let nodes = ed.nodes().unwrap();
        build(&nodes, src)
    }

    fn md(src: &str) -> SourceMap {
        map(src, Format::Markdown)
    }

    /// Every byte of `src` whose style satisfies `pred`, as a string — the
    /// readable form of "what came out dim?".
    fn where_style(m: &SourceMap, src: &str, pred: impl Fn(Style) -> bool) -> String {
        (0..src.len())
            .filter(|&i| src.is_char_boundary(i) && pred(m.style_at(i)))
            .filter_map(|i| src[i..].chars().next())
            .collect()
    }

    #[test]
    fn a_headings_hash_is_markup_and_its_text_is_a_heading() {
        let src = "# Title\n";
        let m = md(src);
        assert_eq!(
            where_style(&m, src, |s| s.role == Role::Delimiter),
            "# ",
            "the `# ` opens the heading and is not part of it"
        );
        assert_eq!(
            where_style(&m, src, |s| s.role == Role::Heading(1)),
            "Title",
            "the text is the heading"
        );
    }

    #[test]
    fn a_links_destination_is_markup_and_its_label_is_a_link() {
        let src = "see [here](https://example.dev) now\n";
        let m = md(src);
        assert_eq!(where_style(&m, src, |s| s.role == Role::Link), "here");
        assert_eq!(
            where_style(&m, src, |s| s.role == Role::Delimiter),
            "[](https://example.dev)",
            "the brackets and the destination are the link's markup"
        );
    }

    #[test]
    fn emphasis_inside_a_heading_is_both() {
        let src = "## a *b* c\n";
        let m = md(src);
        let b = src.find('b').unwrap();
        let style = m.style_at(b);
        assert_eq!(style.role, Role::Heading(2), "still heading text");
        assert!(style.italic, "and italic");
    }

    #[test]
    fn a_coloured_highlights_emoji_is_markup_and_its_words_are_the_mark() {
        // The source view's answer to the same question the rendered one gets:
        // `==🔴 ` is the delimiter, `red` is the mark, and the mark knows
        // which colour it was written in. Parsed with `parse_extensions` — the
        // flags leaf actually opens documents with — because `==…==` is a
        // Markdown *extension*, and a map built without them would show the
        // literal text this test would then be asserting nothing about.
        let src = "a ==🔴 red== b\n";
        let mut ed = twig::Editor::new_ext(
            src.as_bytes(),
            Format::Markdown,
            crate::doc::parse_extensions(),
        )
        .unwrap();
        let m = build(&ed.nodes().unwrap(), src);
        assert_eq!(
            where_style(&m, src, |s| s.role == Role::Mark(Some(MarkColor::Red))),
            "red",
            "the words carry the mark and its colour"
        );
        assert_eq!(
            where_style(&m, src, |s| s.role == Role::Delimiter),
            "==🔴 ==",
            "the fences and the emoji between them are its markup"
        );
    }

    /// The delimiter role sits *on top of* the run's own emphasis rather than
    /// replacing it, so a frontend can dim the `**` and still draw it bold —
    /// the same composition `wysiwyg::Builder::push_delim` does.
    #[test]
    fn a_marks_delimiters_keep_the_emphasis_they_delimit() {
        let src = "a **b** c\n";
        let m = md(src);
        let star = src.find('*').unwrap();
        assert_eq!(m.style_at(star).role, Role::Delimiter);
        assert!(m.style_at(star).bold, "the `**` belongs to the bold run");
        assert!(m.style_at(src.find('b').unwrap()).bold);
        assert_eq!(m.style_at(src.find('b').unwrap()).role, Role::Body);
    }

    #[test]
    fn a_fence_is_markup_and_the_body_is_code() {
        let src = "```rust\nfn main() {}\n```\n";
        let m = md(src);
        assert_eq!(
            m.style_at(src.find("fn").unwrap()).role,
            Role::Code,
            "the body of the block is code"
        );
        assert_eq!(
            m.style_at(0).role,
            Role::Delimiter,
            "the opening fence is markup"
        );
        assert_eq!(
            m.style_at(src.rfind("```").unwrap()).role,
            Role::Delimiter,
            "and so is the closing one"
        );
    }

    #[test]
    fn frontmatter_fences_are_markup() {
        let src = "---\ntitle: x\n---\n\ntext\n";
        let m = md(src);
        assert_eq!(m.style_at(0).role, Role::Delimiter, "the opening `---`");
        assert_eq!(
            m.style_at(src.find("title").unwrap()).role,
            Role::Body,
            "the metadata itself is text"
        );
    }

    /// A list marker and a block quote's gutter are authored bytes with no node
    /// of their own; they fall in the leading gap of the paragraph inside, which
    /// is exactly what the delimiter rule is for.
    #[test]
    fn list_markers_and_quote_gutters_are_markup() {
        let src = "- one\n- [ ] two\n";
        let m = md(src);
        assert_eq!(m.style_at(0).role, Role::Delimiter, "the `- `");
        assert_eq!(m.style_at(src.find("one").unwrap()).role, Role::Body);
        let box_at = src.find("[ ]").unwrap();
        assert_eq!(m.style_at(box_at).role, Role::Delimiter, "the task box");
    }

    /// The `> ` a quote repeats on its continuation lines is inside the
    /// paragraph's content span, so no delimiter gap reaches it — the soft break
    /// it rides does.
    #[test]
    fn a_quotes_continuation_marker_is_markup_too() {
        let src = "> one\n> two\n";
        let m = md(src);
        assert_eq!(m.style_at(0).role, Role::Delimiter, "the opening `> `");
        let second = src.rfind('>').unwrap();
        assert_eq!(
            m.style_at(second).role,
            Role::Delimiter,
            "and the one on the second line"
        );
        assert_eq!(m.style_at(src.find("two").unwrap()).role, Role::Body);
    }

    /// One vocabulary, three grammars: the same assertion holds however the
    /// document spells its markup, which is the whole argument for reading this
    /// off twig's AST instead of off a per-language grammar.
    #[test]
    fn every_format_styles_bold_the_same_way() {
        for (format, src, word) in [
            (Format::Markdown, "a **b** c\n", "b"),
            (Format::Djot, "a *b* c\n", "b"),
            (Format::Html, "<p>a <b>bee</b> c</p>\n", "bee"),
        ] {
            let m = map(src, format);
            let at = src.find(word).unwrap();
            assert!(
                m.style_at(at).bold,
                "{format:?} should style {word:?} bold in {src:?}"
            );
            assert_eq!(
                m.style_at(at).role,
                Role::Body,
                "{format:?}: the bold text is prose, not markup"
            );
        }
    }

    #[test]
    fn html_tags_are_markup_and_a_comment_is_dim_throughout() {
        let src = "<h1>Title</h1>\n<!-- note -->\n";
        let m = map(src, Format::Html);
        assert_eq!(m.style_at(0).role, Role::Delimiter, "the `<h1>` tag");
        assert_eq!(
            m.style_at(src.find("Title").unwrap()).role,
            Role::Heading(1),
            "what the tag contains is a heading"
        );
        assert!(
            where_style(&m, src, |s| s.role == Role::Delimiter).contains("note"),
            "a comment is machinery all the way through"
        );
    }

    #[test]
    fn plain_prose_styles_nothing() {
        let m = md("Just a sentence with no markup in it at all.\n");
        assert!(m.is_empty(), "no runs, so a painter does no extra work");
    }

    #[test]
    fn an_empty_document_is_an_empty_map() {
        assert!(md("").is_empty());
    }

    /// The invariant every consumer relies on: ascending, disjoint, and never
    /// the default style (which the gaps already mean).
    #[test]
    fn runs_are_ascending_disjoint_and_never_default() {
        let src =
            "---\na: b\n---\n\n# H *i*\n\n- [ ] t `c`\n\n> q\n> r\n\n```rs\nx\n```\n\n[l](d)\n";
        let m = md(src);
        assert!(!m.is_empty());
        let mut prev = 0;
        for run in m.runs() {
            assert!(run.span.start < run.span.end, "no empty runs: {run:?}");
            assert!(run.span.start >= prev, "ascending and disjoint: {run:?}");
            assert_ne!(run.style, Style::default(), "no default runs: {run:?}");
            assert!(run.span.end <= src.len(), "inside the source: {run:?}");
            prev = run.span.end;
        }
    }

    /// `style_at` and `runs()` are two views of one answer, so a scan through
    /// either has to agree with the other at every byte.
    #[test]
    fn style_at_agrees_with_the_runs_it_reads() {
        let src = "# H\n\ntext **b** and `c` and [l](d)\n";
        let m = md(src);
        for run in m.runs() {
            for i in run.span.clone() {
                assert_eq!(m.style_at(i), run.style, "byte {i}");
            }
        }
        // And a byte in no run is plain.
        let gap = src.find("text").unwrap();
        assert_eq!(m.style_at(gap), Style::default());
    }

    #[test]
    fn edges_in_reports_every_boundary_inside_the_line_and_none_outside() {
        let src = "a **b** c\n";
        let m = md(src);
        let mut cuts = Vec::new();
        m.edges_in(0..src.len(), &mut cuts);
        // `**b**` spans 2..7: dim `**` at 2..4, bold `b` at 4..5, dim `**` 5..7.
        assert_eq!(cuts, vec![2, 4, 5, 7]);

        // A range that ends mid-run reports only what falls strictly inside it.
        let mut cuts = Vec::new();
        m.edges_in(0..5, &mut cuts);
        assert_eq!(cuts, vec![2, 4]);
    }

    /// The source view paints line by line, so the map has to answer for a
    /// window that starts and ends in the middle of runs.
    #[test]
    fn edges_in_answers_for_a_line_in_the_middle_of_a_document() {
        let src = "# One\n\ntwo **three** four\n\n# Five\n";
        let m = md(src);
        let line_start = src.find("two").unwrap();
        let line_end = src[line_start..].find('\n').unwrap() + line_start;
        let mut cuts = Vec::new();
        m.edges_in(line_start..line_end, &mut cuts);
        assert!(
            cuts.iter().all(|&c| c > line_start && c < line_end),
            "every cut lands inside the line: {cuts:?}"
        );
        assert_eq!(cuts.len(), 4, "the two `**` pairs and the word between");
    }

    /// A document twig cannot make sense of still has to paint. The arena and
    /// the string can only disagree through a bug, but a paint loop is the wrong
    /// place to find out.
    #[test]
    fn a_span_past_the_end_of_the_source_is_clipped_not_panicked() {
        let mut ed = Editor::new_str("# H\n", Format::Markdown).unwrap();
        let nodes = ed.nodes().unwrap();
        let m = build(&nodes, "# ");
        for run in m.runs() {
            assert!(run.span.end <= 2, "clipped to the length given: {run:?}");
        }
    }
}
