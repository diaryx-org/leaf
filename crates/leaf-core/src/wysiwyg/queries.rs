//! AST queries the builds share: top-level blocks, frontmatter, footnote
//! definitions and labels, and where a heading's text starts.

use super::*;

/// Where the rendered document begins when a leading `metadata` block is all
/// there is — the end of that hidden frontmatter, past the newline that closes
/// its last line so the floor sits at the start of the (empty) body rather than
/// on the closing `---`.
///
/// With a real block after it the frontmatter's end is never needed: the floor
/// is that block's start, and the rows begin there. With nothing after it, both
/// the caret floor and the trailing-blank-line count would otherwise fall back
/// to offset 0 — inside the hidden frontmatter — which put the caret *before*
/// the metadata and made typing land ahead of the opening `---`.
pub(super) fn hidden_prefix_end(source: &str, meta_end: Option<usize>) -> usize {
    let Some(end) = meta_end else { return 0 };
    let end = end.min(source.len());
    let rest = &source[end..];
    if rest.starts_with("\r\n") {
        end + 2
    } else if rest.starts_with('\n') {
        end + 1
    } else {
        end
    }
}

/// The caret floor: the first block's start, or the first blank line above it
/// when those lines draw rows ([`Builder::emit_leading_blank_lines`]).
pub(super) fn floor_of(rows: &[VRow], first_start: usize) -> usize {
    rows.first()
        .map_or(first_start, |r| r.end_src.min(first_start))
}

/// The end of the document's hidden frontmatter: the last `metadata` child of
/// `doc`, which is what [`top_level`] and [`Builder::blocks`] drop. `None` when
/// there is none.
pub(super) fn metadata_end_of(nodes: &[FlatNode], doc: usize) -> Option<usize> {
    let mut end = None;
    let mut child = nodes[doc].first_child;
    while let Some(cid) = child {
        let n = &nodes[cid.0 as usize];
        if n.kind == Kind::Metadata {
            end = Some(n.span.end);
        }
        child = n.next_sibling;
    }
    end
}

/// The document's rendered top-level blocks, as node indices in source order.
///
/// Not simply `doc`'s children, for two reasons. Frontmatter (a leading
/// `metadata` block) is document metadata rather than prose and is dropped, the
/// way [`Builder::blocks`] drops it. And a **footnote definition** (`[^1]: …`)
/// is not a child of `doc` at all: twig parses it as a root of its own, a
/// *sibling* of the document node with `parent == None`. A walk that starts at
/// `doc` therefore never reaches one, which is why a definition — and every
/// byte of its body — used to render as nothing at all. Merging the roots back
/// in by `span.start` puts each definition on screen exactly where it was
/// written, which is what keeps rows, stops, and offsets monotonic.
///
/// A **link reference definition** (`[foo]: /url`) is a root of the same kind,
/// and is merged for the opposite reason: it draws *nothing*, and the walk has
/// to know where it stands to step over it. A definition closing a README —
/// the `[links]: …` block under the prose — left no block over its lines, so
/// the separator logic read them as blank lines and drew an empty paragraph
/// per definition. Merged in, it is a hidden block like a comment, and
/// [`Builder::block_or_hidden`] moves the walk past it. One with no span
/// (`0..0`, what twig before 3.3.3 reported for every one) has nowhere to be
/// merged, and is left out as before.
///
/// Only those roots are merged. twig also leaves stray orphan `str` nodes
/// parented to nothing (the `*` of an emphasis run, for one); those are already
/// rendered as part of the subtree that owns their bytes, and re-emitting them
/// here would double them.
pub(super) fn top_level(nodes: &[FlatNode], doc: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut child = nodes[doc].first_child;
    while let Some(cid) = child {
        let n = &nodes[cid.0 as usize];
        if n.kind != Kind::Metadata {
            out.push(cid.0 as usize);
        }
        child = n.next_sibling;
    }
    out.extend(
        nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.is_none() && is_placed_definition(&n.kind, &n.span))
            .map(|(i, _)| i),
    );
    out.sort_by_key(|&i| nodes[i].span.start);
    out
}

/// Is a parentless node of `kind` at `span` a definition the top-level walk
/// merges in — a footnote definition, or a link reference definition that
/// knows where it stands? Shared by [`top_level`] and [`top_blocks`] so the
/// two walks cannot disagree about what the top-level blocks are.
pub(super) fn is_placed_definition(kind: &Kind, span: &Range<usize>) -> bool {
    match *kind {
        Kind::Footnote => true,
        Kind::Reference => span.end > span.start,
        _ => false,
    }
}

/// The top-level blocks to hand [`build_cached`] / [`build_spliced`] — the
/// incremental path's twin of [`top_level`], which the two must agree with block
/// for block or the render paths diverge.
///
/// `child_spans(None)` gives `doc`'s children, which is all of them for an
/// ordinary document. A **footnote definition** is not one: twig parses `[^1]: …`
/// as a root beside `doc` with no parent, and indexes it at no offset either —
/// `node_at` inside its bytes answers `doc`, and a `query("footnote")` selector
/// finds nothing. Leaf used to discover them by marshalling the whole arena with
/// `nodes()` — the very cost the incremental path exists to avoid — behind a
/// byte-scan gate that gave documents with no `[^…]:` line a substring search
/// instead. twig 3.0's `definitions()` asks the library the question directly,
/// so both the marshal and the gate are gone.
///
/// The link reference definitions `definitions()` also reports are merged on
/// the same terms as [`top_level`] merges them — see [`is_placed_definition`].
///
/// This is the one part of the render that needs an [`Editor`] rather than a
/// marshalled node array. The builders themselves stay editor-free; this only
/// prepares their input.
pub(crate) fn top_blocks(editor: &mut Editor) -> Vec<QueryMatch> {
    let mut top = editor.child_spans(None).unwrap_or_default();
    let defs: Vec<QueryMatch> = definitions(editor)
        .into_iter()
        .filter(|m| is_placed_definition(&m.kind, &m.span))
        .collect();
    if defs.is_empty() {
        return top;
    }
    top.extend(defs);
    // Source order — what every offset-keyed thing downstream (rows, stops, the
    // splice path's block-for-block match) is built to assume.
    top.sort_by_key(|m| m.span.start);
    top
}

/// Every `[^label]: …` definition in the document, in whatever order twig
/// reports them.
///
/// Filtered to [`Kind::Footnote`]: `definitions()` also reports the *link*
/// reference definitions (`[foo]: /url`), which are [`top_blocks`]'s business
/// and not [`crate::Doc::footnote_at_caret`]'s.
///
/// Empty when the document can't be walked, which leaves [`top_blocks`] with
/// the ordinary top-level children and [`crate::Doc::footnote_at_caret`] with an
/// undefined reference — in both cases the same answer as a document that has
/// no definitions, which is the right way to degrade.
pub(crate) fn footnote_definitions(editor: &mut Editor) -> Vec<QueryMatch> {
    definitions(editor)
        .into_iter()
        .filter(|m| m.kind == Kind::Footnote)
        .collect()
}

/// Every definition twig resolves by label rather than by position — footnote
/// and link reference definitions both — or nothing when the document can't be
/// walked.
pub(super) fn definitions(editor: &mut Editor) -> Vec<QueryMatch> {
    let Ok(mut doc) = editor.document() else {
        return Vec::new();
    };
    doc.definitions().unwrap_or_default()
}

/// The last line of a block whose span ends at `span_end` — a thematic break,
/// a page break — as the two offsets the rest of the crate means by it: where
/// the line's text ends, and the home past it — past the newline that ends the
/// line, the start of the line under it (or the text's end, with no newline to
/// pass). A rule's row ends at that home.
///
/// Read off the span less the newline djot's span takes in, since Markdown's
/// stops before it. Measured from djot's span end, the home after a rule landed
/// past the blank line under it, on the next block's first character.
pub(crate) fn block_line(source: &str, span_end: usize) -> (usize, usize) {
    let span_end = span_end.min(source.len());
    let text_end = source[..span_end]
        .strip_suffix('\n')
        .map_or(span_end, |s| s.strip_suffix('\r').unwrap_or(s).len());
    let rest = &source[text_end..];
    let newline = if rest.starts_with("\r\n") {
        2
    } else {
        usize::from(rest.starts_with('\n'))
    };
    (text_end, text_end + newline)
}

/// The label of the footnote definition starting at `start` — the `1` in
/// `[^1]: …`. twig gives the `footnote` node no label of its own (no `text`, no
/// `name`), and the bytes that spell it belong to no child node either — the
/// body `para` starts its *content* past them — so the source is the only place
/// to read it from. `None` when what's there isn't a definition after all.
pub(crate) fn footnote_label(source: &str, start: usize) -> Option<&str> {
    let rest = source.get(start..)?.strip_prefix("[^")?;
    let end = rest.find("]:")?;
    Some(&rest[..end])
}

/// Where the body of the footnote definition spanning `span` sits in `source` —
/// everything past the `[^1]:` marker, which is the part a reader actually wants
/// when they follow a reference.
///
/// Source bytes, verbatim but for the whitespace trimmed off each end: a note
/// that says `see *later*` answers with the asterisks in. Rendering that body is
/// a frontend's business the same way painting a [`Role`] is, and a caller that
/// wants it laid out already has the definition on screen where it was written.
///
/// The trim is what makes the common case read right — `[^1]: text` has a space
/// after the colon that belongs to the marker, not the note, and a definition's
/// span runs to the newline ending it.
///
/// The span is taken at its word, which it has only been safe to do since twig
/// 3.1: a djot definition's span used to run *past* its own last line, through
/// the blank line separating it from the next block and into that block's first
/// byte, so `[^2a]: a note.` came back as `"a note.\n\n["` and the offsets named
/// the following note's rows as well as this one's — a reader asking about one
/// footnote was shown two. leaf measured the body itself to get around that, and
/// paid for it: the scan stopped at the first blank line, so a note with a second
/// indented paragraph lost it. Both halves go away with the fix, since a blank
/// line *inside* a definition was always interior to the span and still is.
///
/// A range rather than a slice because "go to note" needs the *position* as much
/// as the text, and it needs the position of the body specifically: a
/// definition's `[^1]:` marker is decoration the caret can't occupy (the rich
/// view draws it as `[1] ` and gives it no stop), so aiming a caret at the
/// definition's first byte lands it on the nearest real stop instead — which is
/// up in the paragraph *above* the note. The body's first byte is a stop, and is
/// where a reader following a reference wants to arrive anyway.
pub(crate) fn footnote_body_span(source: &str, span: Range<usize>) -> Option<Range<usize>> {
    let rest = source.get(span.clone())?.strip_prefix("[^")?;
    let marker = rest.find("]:")?;
    // `span.start` + `[^` + the label + `]:`.
    let after_marker = span.start + 2 + marker + 2;
    let raw = source.get(after_marker..span.end)?;
    // Written as a start plus a length so an all-whitespace body lands on an
    // empty range at the end rather than an inverted one.
    let start = after_marker + (raw.len() - raw.trim_start().len());
    Some(start..start + raw.trim().len())
}

/// The label of the footnote *reference* spanning `span` — the `1` in `[^1]`.
///
/// The peer of [`footnote_label`] for the other half of the pair, and needed for
/// the same reason: a reference whose node carries neither a `content_span` nor
/// a `text` still spells its label plainly in the source. `None` when the bytes
/// aren't a reference after all.
pub(crate) fn footnote_reference_label(source: &str, span: Range<usize>) -> Option<&str> {
    let rest = source.get(span)?.strip_prefix("[^")?;
    let end = rest.find(']')?;
    Some(&rest[..end])
}

/// Where a heading's *content* starts — past the `#`s and the space the rich
/// view hides, for an ATX heading; the block's own start for a setext one (which
/// has no leading marker) and for a format that spells headings some other way.
///
/// Only an empty heading needs asking: with any content at all, the row ends on
/// its last glyph. Bounded to the heading's own first line so a marker-less
/// heading can't scan into the text under it.
pub(super) fn heading_content_start(source: &str, span: &Range<usize>) -> usize {
    let end = span.end.min(source.len());
    let Some(line) = source.get(span.start..end) else {
        return span.start;
    };
    let line = line.split('\n').next().unwrap_or("");
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 {
        return span.start;
    }
    let after = &line[hashes..];
    span.start + hashes + (after.len() - after.trim_start_matches([' ', '\t']).len())
}
