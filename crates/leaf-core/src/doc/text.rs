//! Free helpers over source text: grapheme and word boundaries, line ranges,
//! byte-offset ⇄ (row, col), attributes, and format detection.

use super::*;

// ── byte-offset ⇄ (row, col) helpers ─────────────────────────────────────────

// Left/right motion and backspace/delete step by *grapheme cluster*, not
// codepoint, so an emoji (a ZWJ sequence) or a base letter plus its combining
// marks moves and deletes as the single character a user sees. Grapheme
// boundaries are a superset of char boundaries, so the caret stays valid for twig.

/// How an insert of `text` groups for undo: a single typed character folds into
/// the run of typing around it, while a newline or a multi-character insert is a
/// step of its own.
pub(super) fn typed_edit_kind(text: &str) -> EditKind {
    if text.chars().take(2).count() == 1 && text != "\n" {
        EditKind::Insert
    } else {
        EditKind::Other
    }
}

pub(super) fn prev_boundary(s: &str, i: usize) -> usize {
    let mut cursor = GraphemeCursor::new(i, s.len(), true);
    cursor.prev_boundary(s, 0).ok().flatten().unwrap_or(0)
}

pub(super) fn next_boundary(s: &str, i: usize) -> usize {
    let mut cursor = GraphemeCursor::new(i, s.len(), true);
    cursor.next_boundary(s, 0).ok().flatten().unwrap_or(s.len())
}

// ── word boundaries ──────────────────────────────────────────────────────────
// The shared primitive behind word-wise motion, word deletion, and
// double-click-to-select-a-word. A "word" is a maximal run of one character
// class; whitespace and punctuation are their own classes, so motion skips
// cleanly between them the way native text fields do.

#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum Class {
    Word,
    Space,
    Other,
}

/// The source range of an inline node's own visible text — the part of it a
/// WYSIWYG caret can reach, as against the delimiters that only spell it.
/// `None` for a node with no interior to empty (a `str`, a break).
///
/// twig reports no `content_span` for `verbatim`/`inline_math`, whose text sits
/// one delimiter in from the span — the same place the renderer maps it to. A
/// longer fence (`` ``a`` ``) breaks that assumption, so the guess is checked
/// against the source rather than trusted: a range guessed wrong here is text
/// deleted wrong.
pub(super) fn inline_content_span(n: &FlatNode, source: &str) -> Option<std::ops::Range<usize>> {
    if let Some(span) = n.content_span.clone() {
        return Some(span);
    }
    match n.kind.as_str() {
        "verbatim" | "inline_math" => {
            let text = n.text.as_ref()?;
            let start = n.span.start + 1;
            let range = start..start + text.len();
            (source.get(range.clone()) == Some(text.as_str())).then_some(range)
        }
        _ => None,
    }
}

/// The `id` a node declares, or `None` for one that declares none — the
/// attribute djot writes for a `{#v1}` and mints for a heading.
///
/// A bare attribute (`{#v1 hidden}`'s `hidden`) has no value, and a bare `id`
/// names nothing, so it reads as absent rather than as the empty string.
pub(super) fn declared_id(n: &FlatNode) -> Option<&str> {
    n.attrs.iter().find(|(k, _)| k == "id")?.1.as_deref()
}

/// A heading's words reduced to the form a link fragment spells them in:
/// lowercase, runs of anything else collapsed to a single `-`, with none left
/// dangling at either end. `## Some Heading Here` → `some-heading-here`.
///
/// The rule every Markdown renderer follows, and applied to djot's own auto-ids
/// too so that `#some-heading-here` and `#Some-Heading-Here` are one question.
/// Unicode-aware (`is_alphanumeric`, not an ASCII test), because a heading in
/// any other language is still a heading someone will link to. Underscores
/// survive for the same reason they do on the web: they are word characters
/// wherever identifiers are written.
pub(super) fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            if pending && !out.is_empty() {
                out.push('-');
            }
            pending = false;
            out.extend(c.to_lowercase());
        } else {
            pending = true;
        }
    }
    out
}

/// The text of the inline run starting at `first` and its siblings, markup
/// stripped: each leaf's payload in order, a line break as a space. `nodes` is
/// the arena `Doc::nodes` returns, indexed by id.
pub(super) fn inline_text(nodes: &[FlatNode], first: Option<NodeId>, out: &mut String) {
    let mut next = first;
    while let Some(id) = next {
        let Some(n) = nodes.get(id.0 as usize) else {
            return;
        };
        match (&n.text, &n.kind) {
            (Some(text), _) => out.push_str(text),
            (None, Kind::SoftBreak | Kind::HardBreak) => out.push(' '),
            (None, _) => inline_text(nodes, n.first_child, out),
        }
        next = n.next_sibling;
    }
}

pub(super) fn is_block_container(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Doc
            | Kind::Section
            | Kind::BlockQuote
            | Kind::BulletList
            | Kind::OrderedList
            | Kind::TaskList
            | Kind::ListItem
            | Kind::TaskListItem
            // Every `container` — a directive in any of its three forms, or a
            // promoted HTML element. A *text* directive is really inline, so
            // claiming it here is a small overreach, and the deliberate one this
            // function's kind-only peer `is_inline_kind` documents: the pair is
            // consulted together, and answering "block container" for something
            // inline is what keeps an ancestor walk from stopping short of the
            // paragraph that actually holds it.
            | Kind::Container
    )
}

/// Whether `line` is a line of *text* — not blank, and not one of the lines a
/// verse is spelled with around its text: Markdown's `<div …>` and `</div>`,
/// djot's `:::` fence, AsciiDoc's `[verse…]` and `____`.
pub(super) fn is_text_line(line: &str) -> bool {
    let t = line.trim();
    !(t.is_empty()
        || t.starts_with("<div")
        || t == "</div>"
        || t.starts_with(":::")
        || t.starts_with("[verse")
        || (t.len() >= 4 && t.bytes().all(|b| b == b'_'))
        || (t.starts_with('{') && t.ends_with('}')))
}

/// The `[start, end)` byte range of the source line containing `off` (newline
/// excluded) — the fallback when `off` sits outside any AST block (e.g. a blank
/// line between paragraphs).
pub(super) fn source_line_range(s: &str, off: usize) -> std::ops::Range<usize> {
    let off = off.min(s.len());
    let start = s[..off].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let end = s[off..].find('\n').map(|p| off + p).unwrap_or(s.len());
    start..end
}

/// How many leading bytes an outdent takes off `line`: a whole indent level
/// where the line has one, and whatever it has where it has less.
///
/// A leading tab counts as a level on its own. It's indentation some other
/// editor wrote, and one tab is one level everywhere it came from — measuring it
/// in spaces it doesn't contain would leave it untouchable.
pub(super) fn outdent_width(line: &str, unit: usize) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes().take(unit).take_while(|b| *b == b' ').count()
}

/// A list marker found at the head of a line, together with everything before it
/// that a sibling line has to repeat.
///
/// The three offsets differ only inside a block quote, where `>   - b` opens with
/// a `> ` quote marker the line's own text doesn't own. Outside one they collapse:
/// `line_start == marker_start`, and `text` is the plain `"  - "`.
#[derive(Clone, Debug)]
pub(super) struct ListMarker {
    /// The line's first byte.
    pub(super) line_start: usize,
    /// Where the marker proper begins, past any quote prefix. The offset to hand
    /// the AST: a quoted item's span opens at its bullet, not at the `>`.
    pub(super) marker_start: usize,
    /// `line_start` through the marker's trailing space — quote prefix, indent
    /// and bullet together, which is what the next item's line opens with.
    pub(super) text: String,
}

impl ListMarker {
    /// Where the item's content starts — one past the marker's trailing space.
    pub(super) fn content_start(&self) -> usize {
        self.line_start + self.text.len()
    }
}

pub(super) fn classify(c: char) -> Class {
    if c == '_' || c.is_alphanumeric() {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// The offset at the end of the next word to the right of `i` (⌥→ / Ctrl+→):
/// skip any leading separators, then consume the following word run.
pub(super) fn next_word(s: &str, i: usize) -> usize {
    let mut off = i;
    let mut in_word = false;
    for c in s[i..].chars() {
        if classify(c) == Class::Word {
            in_word = true;
        } else if in_word {
            break;
        }
        off += c.len_utf8();
    }
    off
}

/// The offset at the start of the word to the left of `i` (⌥← / Ctrl+←):
/// skip separators walking left, then consume the preceding word run.
pub(super) fn prev_word(s: &str, i: usize) -> usize {
    let mut off = i;
    let mut in_word = false;
    for c in s[..i].chars().rev() {
        if classify(c) == Class::Word {
            in_word = true;
        } else if in_word {
            break;
        }
        off -= c.len_utf8();
    }
    off
}

/// The `[start, end)` run of same-class characters surrounding `off` — the
/// word (or whitespace/punctuation run) a double-click selects. At end-of-text
/// the run ending there is used.
pub(super) fn word_range_at(s: &str, off: usize) -> (usize, usize) {
    if s.is_empty() {
        return (0, 0);
    }
    let off = off.min(s.len());
    let reference = if off < s.len() {
        s[off..].chars().next()
    } else {
        s[..off].chars().next_back()
    };
    let Some(rc) = reference else {
        return (off, off);
    };
    let class = classify(rc);

    let mut start = off;
    for c in s[..start].chars().rev() {
        if classify(c) == class {
            start -= c.len_utf8();
        } else {
            break;
        }
    }
    let mut end = off;
    for c in s[end..].chars() {
        if classify(c) == class {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

/// `(row, col)` of byte offset `off`, `col` counted in *display columns* from
/// the line's start — terminal cells, not characters, so the column names the
/// cell the caret is drawn in even on a line of `你好` or emoji.
pub(super) fn offset_to_row_col(s: &str, off: usize) -> (usize, usize) {
    let off = off.min(s.len());
    let mut row = 0;
    let mut line_start = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if i >= off {
            break;
        }
        if b == b'\n' {
            row += 1;
            line_start = i + 1;
        }
    }
    (row, wysiwyg::text_width(&s[line_start..off]))
}

/// The byte offset at display column `col` of `row` (clamped to that line's
/// end) — the inverse of [`offset_to_row_col`], which it has to agree with.
///
/// A column landing *inside* a character — the second cell of `你`, or any cell
/// but the first of an emoji — resolves to that character's start, which is the
/// column the caret would have been drawn at to begin with. So both cells of a
/// wide character mean the character, and every offset survives the round trip
/// out to a column and back. The walk steps by grapheme cluster for the same
/// reason the caret does: a cluster is the character, and the cells belong to it
/// rather than to the codepoints spelling it.
pub(super) fn row_col_to_offset(s: &str, row: usize, col: usize) -> usize {
    let start = line_start(s, row);
    let end = line_end_from(s, start);
    let mut off = start;
    let mut at = 0; // the display column `off` sits at
    while off < end {
        let next = next_boundary(s, off).min(end);
        let cells = wysiwyg::text_width(&s[off..next]);
        if at + cells > col {
            break; // `col` is one of this cluster's own cells
        }
        at += cells;
        off = next;
    }
    off
}

pub(super) fn line_start(s: &str, row: usize) -> usize {
    if row == 0 {
        return 0;
    }
    let mut r = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b == b'\n' {
            r += 1;
            if r == row {
                return i + 1;
            }
        }
    }
    s.len()
}

pub(super) fn line_end_from(s: &str, start: usize) -> usize {
    s[start..].find('\n').map(|p| start + p).unwrap_or(s.len())
}

/// twig's node-kind name for an inline mark, back to the [`InlineKind`] a
/// frontend names when it calls [`Doc::toggle`] — the inverse of the mapping
/// twig applies writing the mark out, so the toolbar can light the same button
/// that made the node.
///
/// `None` for every other kind, including the inline nodes that aren't marks at
/// all (`str`, `link`, `image`, the math and break kinds): they're things a
/// caret stands in, not formatting a button toggles.
/// Whether a match from an ancestor chain is an inline run whose delimiters
/// the rich view draws nothing for — a mark (`**`, `_`, `==`), or an
/// attributed span: `<span data-size="large">…</span>`, djot's `[…]{…}`. The
/// span is a [`Kind::Container`], which the kind alone cannot tell from a
/// block `<div>`, so the chain's caller passes [`Doc::run_span_ids`] and the
/// answer is the node's own. Every delete and caret step that walks over a
/// `**` walks over a span's tags by this test; without it Backspace after
/// `</span>` took the `>` and left the paragraph unparseable.
pub(super) fn hides_delims(m: &QueryMatch, run_spans: &[NodeId]) -> bool {
    inline_kind(&m.kind).is_some() || run_spans.contains(&NodeId(m.node_id))
}

pub(super) fn inline_kind(kind: &Kind) -> Option<InlineKind> {
    Some(match kind {
        Kind::Strong => InlineKind::Strong,
        Kind::Emph => InlineKind::Emph,
        Kind::Verbatim => InlineKind::Verbatim,
        Kind::Mark => InlineKind::Mark,
        Kind::Superscript => InlineKind::Superscript,
        Kind::Subscript => InlineKind::Subscript,
        Kind::Insert => InlineKind::Insert,
        Kind::Delete => InlineKind::Delete,
        _ => return None,
    })
}

/// leaf's [`MarkColor`] as twig's — the palette twig writes as the emoji after
/// a highlight's opening `==`.
///
/// Two enums for one closed vocabulary, and the duplication is the boundary
/// working: core's is what a *frontend* names (`style::MarkColor`, beside the
/// [`Role`](crate::Role) that carries it into the glyph map) and twig's is what
/// the editor writes. Spelled as a match rather than routed through the two
/// crates' name strings so that a colour added on either side is a compile
/// error here, where the pairing is decided, rather than a runtime `None` that
/// would read as "clear the colour".
pub(super) fn twig_mark_color(color: MarkColor) -> twig::MarkColor {
    match color {
        MarkColor::Red => twig::MarkColor::Red,
        MarkColor::Orange => twig::MarkColor::Orange,
        MarkColor::Yellow => twig::MarkColor::Yellow,
        MarkColor::Green => twig::MarkColor::Green,
        MarkColor::Blue => twig::MarkColor::Blue,
        MarkColor::Purple => twig::MarkColor::Purple,
        MarkColor::Brown => twig::MarkColor::Brown,
    }
}

/// Where an offset lands after a splice it didn't make — twig's own rule, from
/// [`Change`]: shift anything at or past the replaced range's end by the length
/// the replacement gained or lost, and leave anything before it alone.
///
/// An offset *inside* the replaced range has no text of its own to ride any
/// more, and lands at the end of what replaced it: for
/// [`Doc::set_mark_color`] that is a caret standing on the colour prefix when
/// the prefix is cleared, which then sits where the highlighted text begins.
/// One node's attribute list, twig's own `(key, value)` pairs owned — what
/// every presentation gesture reads, edits one key of, and passes back whole.
pub(super) type Attrs = Vec<(String, Option<String>)>;

/// `attrs` with `key` set to `value`, or removed when `value` is `None`, and
/// every other attribute kept in its place — the read-edit-write half of twig's
/// replace-not-merge contract for a `data-` key.
///
/// **A key that is already there is rewritten where it stands**, and only a key
/// the node did not have goes on the end. That is what makes the proposal's
/// worked example true: `class="lead center" id="intro"
/// data-line-height="1.5"`, right-aligned, is `class="lead right" id="intro"
/// data-line-height="1.5"` — the same document with one token changed, and a
/// one-line diff. Removing the key and pushing it back would reorder the
/// author's attributes on every press, so a document that passed through the
/// editor came out shuffled even where nothing about it had changed.
///
/// A duplicate key — which no format leaf opens can spell, but twig reports
/// verbatim — collapses onto the first of its copies, since twig is handed one
/// value for one key either way.
pub(super) fn with_attr(
    attrs: &[(String, Option<String>)],
    key: &str,
    value: Option<&str>,
) -> Attrs {
    let mut out: Attrs = Vec::with_capacity(attrs.len() + 1);
    let mut written = false;
    for (k, v) in attrs {
        if k != key {
            out.push((k.clone(), v.clone()));
            continue;
        }
        if let Some(new) = value.filter(|_| !written) {
            out.push((k.clone(), Some(new.to_string())));
            written = true;
        }
    }
    if let Some(new) = value.filter(|_| !written) {
        out.push((key.to_string(), Some(new.to_string())));
    }
    out
}

/// [`with_attr`] for a `class` token: every token `mine` claims is removed, and
/// `token` added, with the rest of the list kept in order.
///
/// `class` is a space-separated token list, and leaf owns three of the tokens in
/// it. A paragraph that arrives as `class="lead center"` and is right-aligned
/// goes out as `class="lead right"`; one whose last owned token goes and which
/// carried nothing else loses the key, so a block that has lost its whole
/// vocabulary is spelled bare again. `class` itself keeps its place among the
/// attributes, because [`with_attr`] does the writing.
pub(super) fn with_class_token(
    attrs: &[(String, Option<String>)],
    mine: impl Fn(&str) -> bool,
    token: Option<&str>,
) -> Attrs {
    let kept: Vec<&str> = attrs
        .iter()
        .find(|(k, _)| k == "class")
        .and_then(|(_, v)| v.as_deref())
        .unwrap_or_default()
        .split_whitespace()
        .filter(|t| !mine(t))
        .collect();
    let class = kept.into_iter().chain(token).collect::<Vec<_>>().join(" ");
    with_attr(
        attrs,
        "class",
        (!class.is_empty()).then_some(class.as_str()),
    )
}

/// An owned attribute list as the borrowed pairs twig's two attribute ops take.
///
/// A **bare** attribute — one twig reports with no value, such as HTML's `<p
/// hidden>` — is passed back as an empty one. Twig refuses a `None` outright
/// (djot has no bare attribute, so no format reads one back everywhere), and
/// `hidden=""` is the same document where `hidden` is; dropping it instead
/// would lose what the author wrote, which is the one thing these gestures
/// promise not to do.
pub(super) fn attr_pairs(attrs: &[(String, Option<String>)]) -> Vec<(&str, Option<&str>)> {
    attrs
        .iter()
        .map(|(k, v)| (k.as_str(), Some(v.as_deref().unwrap_or_default())))
        .collect()
}

pub(super) fn reanchor(off: usize, change: &Change) -> usize {
    if off < change.old.start {
        return off;
    }
    if off < change.old.end {
        return change.new.end;
    }
    (off + change.new.end).saturating_sub(change.old.end)
}

/// [`reanchor`] for an edit that respells the markup *around* a block and
/// leaves the block's own bytes alone — which is every attribute gesture.
///
/// `block` is that block's content span before and after the splice, so an
/// offset standing in the text keeps its distance from the text's start and how
/// many bytes twig wrote above it never enters the arithmetic. That is the whole
/// rule, and it is why nothing here knows how long a `<div …>` is: a second key
/// on the same div lengthens the attribute line, clearing the last one takes the
/// div away entirely, and both are the same sum. `None` where the splice named
/// no block at either end, which is every djot case — the `{…}` line is written
/// above the block, and the block itself only shifts past it.
///
/// Anywhere else it is `reanchor`'s own answer: untouched before the splice,
/// shifted by its delta after it, and at the splice's end for an offset that
/// stood in markup being rewritten — a caret inside djot's `{…}` line has no
/// text to keep.
pub(super) fn reanchor_in_block(
    off: usize,
    change: &Change,
    block: Option<(&Range<usize>, &Range<usize>)>,
) -> usize {
    if let Some((was, now)) = block
        && was.start <= off
        && off <= was.end
    {
        return now.start + (off - was.start).min(now.end - now.start);
    }
    reanchor(off, change)
}

/// A watermark for a file's contents (see `Doc::disk_hash`).
///
/// `DefaultHasher` is not stable across Rust releases, which doesn't matter: a
/// watermark is compared only against one taken by the same process moments
/// earlier, and never outlives it. 64 bits leaves a collision — an external edit
/// that hashes to exactly what leaf wrote — at odds no filesystem race gets near.
pub(super) fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

#[cfg(feature = "fs")]
pub(super) fn detect_format(path: &Path) -> Result<Format> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    Ok(match ext.as_str() {
        "dj" | "djot" => Format::Djot,
        "md" | "markdown" => Format::Markdown,
        "xml" => Format::Xml,
        "html" | "htm" => Format::Html,
        other => return Err(anyhow!("unknown document extension: .{other}")),
    })
}
