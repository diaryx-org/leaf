//! Reading a built map's rows back into the spans it publishes — caret stops,
//! mark ends, code blocks, media, directives, math — and the fence and
//! list-marker arithmetic they lean on.

use super::*;

/// Collect the caret stops of a laid-out grid: every stop glyph's offset plus
/// every row's end, ascending and deduplicated. Duplicates are the norm rather
/// than the exception — a wrapped line's end is the same offset as the next
/// line's first glyph — and collapsing them is what makes one press of Left or
/// Right cross exactly one stop.
pub(super) fn collect_stops(rows: &[VRow]) -> Vec<usize> {
    let mut stops: Vec<usize> = rows
        .iter()
        .filter(|r| !r.decoration)
        .flat_map(|r| {
            r.glyphs
                .iter()
                .filter(|g| g.stop)
                .map(|g| g.src)
                .chain(std::iter::once(r.end_src))
        })
        .collect();
    stops.sort_unstable();
    stops.dedup();
    stops
}

/// Collect every row's [`VRow::mark_ends`] into one ascending, deduplicated
/// table — the peer of [`collect_stops`] for the caret's second home at the
/// end of a hidden mark. A mark that closes at a row's end coincides with the
/// row's own end stop; that offset is in both tables, and harmlessly so.
pub(super) fn collect_mark_ends(rows: &[VRow]) -> Vec<usize> {
    let mut ends: Vec<usize> = rows
        .iter()
        .filter(|r| !r.decoration)
        .flat_map(|r| r.mark_ends.iter().copied())
        .collect();
    ends.sort_unstable();
    ends.dedup();
    ends
}

/// Group the rows tagged [`VRow::code`] into one [`CodeBlockInfo`] per maximal
/// run — the block-level view a frontend needs to box and scroll each code
/// block. Two code blocks are always parted by the blank separator row a block
/// boundary is spelled with (never itself a code row), so a contiguous run is
/// exactly one block. Derived from the final rows rather than tracked through
/// the builder so it comes out right no matter how [`build_cached`] and
/// [`build_spliced`] shuffle rows around.
pub(super) fn code_block_spans(rows: &[VRow]) -> Vec<CodeBlockInfo> {
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        match (row.code, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                blocks.push(CodeBlockInfo {
                    rows_span: s..i,
                    lang: rows[s].code_lang.clone(),
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        blocks.push(CodeBlockInfo {
            rows_span: s..rows.len(),
            lang: rows[s].code_lang.clone(),
        });
    }
    blocks
}

/// The value of `node`'s `key` attribute, if it carries one *with* a value. A
/// bare attribute (`controls`, `muted`) has a `None` value and so reads as
/// absent here — a caller wanting presence-not-value tests the list directly.
/// Shared by the media element and `<source>` readers.
pub(super) fn attr_of(node: &FlatNode, key: &str) -> Option<String> {
    node.attrs
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.clone())
}

/// Collect one [`MediaInfo`] per row carrying a [`VRow::media`] mark — the
/// block-level view a frontend needs to replace each placeholder row with a real
/// picture. The mark rides the block's *first* row and names how many rows the
/// media reserves ([`MediaMark::rows`]); the rows below it are blank
/// [`decoration`](VRow::decoration) fillers that hold the vertical space and no
/// caret. So the span runs from the marked row across those fillers. Derived from
/// the final rows rather than tracked through the builder so it survives however
/// [`build_cached`] and [`build_spliced`] shuffle rows around.
pub(super) fn media_spans(rows: &[VRow]) -> Vec<MediaInfo> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.media.as_ref().map(|m| MediaInfo {
                rows_span: i..i + m.rows.max(1),
                kind: m.kind,
                destination: m.destination.clone(),
                sources: m.sources.clone(),
                alt: m.alt.clone(),
                poster: m.poster.clone(),
            })
        })
        .collect()
}

/// Re-label the drawn block boundaries either side of a block-level media
/// placeholder, so the pair a frontend spaces by names the picture.
///
/// [`BlockClass::from_node_kind`] classifies the node the walk is standing on,
/// and a block image is never a node of its own: [`Builder::media_only`] promotes
/// its *wrapper* — a `paragraph`, or the `container` a `<video>`/`<audio>`
/// arrives as — so the gap above a picture reported [`BlockClass::Paragraph`] and
/// the gap above a movie reported [`BlockClass::Directive`], the class a frontend
/// paints a tinted panel for. [`BlockClass::Media`] was unreachable in
/// consequence: the vocabulary named a kind no frontend could ever be told about.
///
/// Done as a pass over the finished rows rather than inside the walk because
/// only the rows know. The incremental top-level walk carries no node arena at
/// all (`nodes: &[]`) and can classify by kind alone, so teaching the wrapper
/// promotion to the whole-arena walk would label the full and incremental builds
/// differently — the exact drift that walk's own comment forbids. Both builds
/// emit the same [`VRow::media`] marks, so both reach the same answer here. The
/// [`media_spans`] / [`code_block_spans`] pattern.
///
/// One gap can be spelled with several rows — [`Builder::emit_separators_before`]
/// draws the row that closes the block above and the row that opens the block
/// below, with any extra blank source lines navigable between them. Only where
/// the two drawn rows meet do both border the picture; past a navigable blank
/// line the gap borders that empty paragraph instead. So the walk relabels the
/// drawn rows next to the picture and stops at the first row that is not one.
pub(super) fn label_media_boundaries(rows: &mut [VRow]) {
    // A display formula's placeholder is promoted from its paragraph exactly
    // as a picture is, and its gaps are relabelled the same way, as `Math`.
    let spans: Vec<(Range<usize>, BlockClass)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            if let Some(m) = &row.media {
                Some((i..i + m.rows.max(1), BlockClass::Media))
            } else {
                row.math
                    .iter()
                    .find(|m| m.glyph.is_none())
                    .map(|m| (i..i + m.rows.max(1), BlockClass::Math))
            }
        })
        .collect();
    // A drawn boundary beside the picture. Anything else ends the run.
    fn in_gap(row: &VRow) -> bool {
        row.boundary.is_some()
    }
    for (span, class) in spans {
        for i in (0..span.start).rev() {
            if !in_gap(&rows[i]) {
                break;
            }
            if let Some(b) = rows[i].boundary.as_mut() {
                b.below = class;
            }
        }
        for row in rows.iter_mut().skip(span.end) {
            if !in_gap(row) {
                break;
            }
            if let Some(b) = row.boundary.as_mut() {
                b.above = class;
            }
        }
    }
}

/// Collect one [`DirectiveInfo`] per row carrying a [`VRow::leaf_directive`]
/// mark — the block-level view a frontend needs to replace each placeholder row
/// with whatever the directive means to it. The peer of [`media_spans`], derived
/// from the final rows for the same reason: it survives however [`build_cached`]
/// and [`build_spliced`] shuffle rows around.
pub(super) fn directive_spans(rows: &[VRow]) -> Vec<DirectiveInfo> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.leaf_directive.as_ref().map(|m| DirectiveInfo {
                rows_span: i..i + m.rows.max(1),
                name: m.name.clone(),
                attrs: m.attrs.clone(),
                label: m.label.clone(),
            })
        })
        .collect()
}

/// Where `tex` sits inside the node at `span`, as an offset from its start.
/// twig hands a formula's text and not its range, so the text is found in the
/// node's bytes; the delimiters before it are never TeX, so the first match is
/// the text. `0` if it is somehow not there, which only misplaces an edit.
pub(super) fn tex_offset(source: &str, span: Range<usize>, tex: &str) -> usize {
    source.get(span).and_then(|s| s.find(tex)).unwrap_or(0)
}

/// Collect one [`MathInfo`] per [`VRow::math`] mark — the peer of
/// [`media_spans`] and [`directive_spans`], derived from the final rows for the
/// same reason. A block mark spans its fillers; an atom spans its own row.
pub(super) fn math_spans(rows: &[VRow]) -> Vec<MathInfo> {
    rows.iter()
        .enumerate()
        .flat_map(|(i, row)| {
            row.math.iter().map(move |m| {
                let src = math_src(row, m);
                MathInfo {
                    rows_span: i..i + m.rows.max(1),
                    row: i,
                    glyph: m.glyph,
                    tex: m.tex.clone(),
                    display: m.display,
                    src,
                }
            })
        })
        .collect()
}

/// Where formula `m` on `row` starts: its atom's offset, or a block's
/// placeholder glyphs'.
pub(super) fn math_src(row: &VRow, m: &MathMark) -> usize {
    match m.glyph {
        Some(g) => row.glyphs.get(g).map_or(row.end_src, |g| g.src),
        None => row
            .glyphs
            .iter()
            .find(|g| g.style.role == Role::Math)
            .map_or(row.end_src, |g| g.src),
    }
}

/// The source range of a fenced code block's info string — everything on the
/// opening line past the fence (`` ```rust `` → the `rust`). `block_start` is the
/// code block node's `span.start`. `None` for an indented code block, which
/// opens with no fence to carry one. The range is empty for a fence written
/// bare (`` ``` `` alone), which is exactly where a language would be inserted.
///
/// A block inside a quote or a list item starts at its line's start, with the
/// container's marker in front of the fence — `> ```rust`, `- ```rust` — so
/// the marker is skipped first, and the fence's three-space indent allowance
/// is measured from where the container's content begins: past the marker on
/// the fence's own line, or, on a continuation line, from the content column
/// of the list item above. That is what keeps an *indented* code block inside
/// a list item answering `None`.
///
/// Shared by the WYSIWYG builder (to label the box) and [`crate::Doc`] (to edit
/// the label through a prompt), so the two agree on where the language lives.
pub fn code_info_span(source: &str, block_start: usize) -> Option<Range<usize>> {
    let rest = source.get(block_start..)?;
    let line_len = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_len];
    let (depth, quoted) = strip_quote_markers(line);
    let mut content = quoted;
    let mut listed = false;
    while let Some(n) = list_marker_len(&line[content..], 3) {
        content += n;
        listed = true;
    }
    let lead = leading_spaces(&line[content..]);
    // No marker of its own: a continuation line, whose leading spaces include
    // the enclosing item's indent. Only when the block owns its line — a span
    // that starts mid-line has had its container prefix measured off already.
    let at_line_start = block_start == 0 || source.as_bytes()[block_start - 1] == b'\n';
    let base = if listed || !at_line_start {
        0
    } else {
        item_content_column(&source[..block_start], depth, lead)
    };
    // A fence may be indented up to three spaces; past that it opens with a run
    // of the same fence character.
    if lead - base > 3 {
        return None;
    }
    let at = content + lead;
    let fence = line[at..].chars().next()?;
    if fence != '`' && fence != '~' {
        return None; // an indented block, not a fenced one
    }
    let fence_len = line[at..].chars().take_while(|&c| c == fence).count();
    let info_start = block_start + at + fence_len;
    Some(info_start..block_start + line_len)
}

/// The spaces a line opens with.
fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

/// A line's block-quote markers — each `>` behind up to three spaces, with the
/// one space after it — as the quote depth and the byte offset past them.
fn strip_quote_markers(line: &str) -> (usize, usize) {
    let b = line.as_bytes();
    let (mut depth, mut i) = (0, 0);
    loop {
        let j = i + leading_spaces(&line[i..]);
        if j - i > 3 || b.get(j) != Some(&b'>') {
            return (depth, i);
        }
        depth += 1;
        i = j + 1 + usize::from(b.get(j + 1) == Some(&b' '));
    }
}

/// The length of a list marker opening `s` — its indent (at most `max_lead`
/// spaces), a bullet or an ordinal, and the one space the item's content
/// starts after — or `None` when `s` opens with none.
fn list_marker_len(s: &str, max_lead: usize) -> Option<usize> {
    let b = s.as_bytes();
    let lead = leading_spaces(s);
    if lead > max_lead {
        return None;
    }
    let mark = match b.get(lead)? {
        b'-' | b'*' | b'+' => 1,
        c if c.is_ascii_digit() => {
            let digits = b[lead..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 || !matches!(b.get(lead + digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    match b.get(lead + mark) {
        Some(b' ') => Some(lead + mark + 1),
        None => Some(lead + mark),
        _ => None,
    }
}

/// The content column of the list item a continuation line indented `lead`
/// spaces (past `depth` quote markers) sits in: the nearest item above whose
/// content starts at or before `lead`. `0` when a line at the margin, or a
/// change of quote depth, says there is no such item.
fn item_content_column(before: &str, depth: usize, lead: usize) -> usize {
    for line in before.strip_suffix('\n').unwrap_or(before).rsplit('\n') {
        let (d, q) = strip_quote_markers(line);
        let s = &line[q..];
        if s.trim().is_empty() {
            continue;
        }
        if d != depth {
            return 0;
        }
        let mut col = 0;
        while let Some(n) = list_marker_len(&s[col..], usize::MAX) {
            col += n;
        }
        if col > 0 && col <= lead {
            return col;
        }
        if col == 0 && leading_spaces(s) == 0 {
            return 0;
        }
    }
    0
}

/// A fenced code block's language for display: its info string, trimmed, or
/// `None` when there's no fence or the fence carries no language. The trimmed
/// text is what a frontend labels the box with; [`code_info_span`] is what an
/// edit replaces.
pub fn code_language(source: &str, block_start: usize) -> Option<String> {
    let span = code_info_span(source, block_start)?;
    let text = source.get(span)?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A horizontal rule's dash count when the map isn't wrapping to a column grid
/// (the GUI, which wraps at pixel width): a fixed, sane width the frontend can
/// paint or re-wrap, instead of a runaway count from an unbounded wrap width.
pub(super) const UNWRAPPED_RULE_WIDTH: usize = 40;

/// The one glyph an inline formula renders to on a surface that paints
/// pictures in a line — what a plain surface handed such a map would show.
/// One column wide, so a column-wrapped build's arithmetic still holds; the
/// frontend that asked for atoms draws the picture as wide as it is.
pub(super) const MATH_ATOM: char = '∑';

/// The label an owned fence's placeholder row starts with — see
/// [`Builder::block_region`].
pub(super) const REGION_ATOM: char = '▤';

/// The region kind every formula is: what a frontend names in
/// [`crate::Doc::set_owned_regions`] to edit formulas itself.
pub const MATH_REGION: &str = "math";

/// The prefix of a fenced code block's region kind, before its language:
/// `fence:mermaid`.
pub const FENCE_REGION: &str = "fence:";

/// The prefix of a leaf directive's region kind, before its name:
/// `directive:embed`. `directive:page-break` is never a region: the name is
/// leaf's, and a page break is a layout fact rather than a picture.
pub const DIRECTIVE_REGION: &str = "directive:";

/// The label a leaf directive's placeholder row starts with — see
/// [`Builder::block_directive`].
pub(super) const DIRECTIVE_ATOM: char = '⧉';

/// The content of fenced code block `span`: the lines between its opening
/// fence and its closing one, each with its newline — empty for a fence with
/// nothing in it, where it is the closing fence's start. An unclosed fence's
/// content runs to the block's end. `None` for a block with no opening line.
pub(super) fn fence_content(source: &str, span: Range<usize>) -> Option<Range<usize>> {
    let s = source.get(span.clone())?;
    let fence = s.trim_start().chars().next()?;
    let open = s.find('\n')? + 1;
    let body = &s[open..];
    let lines = body.strip_suffix('\n').unwrap_or(body);
    let last_at = lines.rfind('\n').map_or(0, |i| i + 1);
    let last = lines[last_at..].trim();
    let closed = last.len() >= 3 && last.chars().all(|c| c == fence);
    let len = if closed { last_at } else { body.len() };
    Some(span.start + open..span.start + open + len)
}
