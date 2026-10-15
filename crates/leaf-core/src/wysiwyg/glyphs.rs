//! Glyph helpers: fitting columns, wrapping, pushing text and code, and the
//! synthetic glyphs — bullets, ordinals, labels — a block wears.

use super::*;

/// The narrowest a column may be squeezed. Below a few characters a column
/// stops carrying text and just shreds it one letter per line, which is worse
/// than letting the grid run wide.
const MIN_COL_WIDTH: usize = 3;

/// Shrink `widths` until the grid fits `avail` screen columns, taking from the
/// widest column each time so the loss is shared out rather than falling on
/// whichever column happens to be last. No column goes below
/// [`MIN_COL_WIDTH`]; a table with more columns than the surface has room for
/// still overflows, which is the honest outcome — there's nothing left to give.
pub(super) fn fit_widths(widths: &mut [usize], avail: usize) {
    // Chrome: each column is its content plus a gutter either side, and every
    // column is closed by a `│` — with one more opening the row.
    let budget = avail.saturating_sub(3 * widths.len() + 1);
    while widths.iter().sum::<usize>() > budget {
        let Some(w) = widths.iter_mut().filter(|w| **w > MIN_COL_WIDTH).max() else {
            return;
        };
        *w -= 1;
    }
}

/// Word-wrap `glyphs` into lines of at most `width` columns, hard-breaking any
/// single word too long to fit.
///
/// Unlike a paragraph — where an overlong word just trails off the end of the
/// line — a table column is a hard boundary: a glyph past it lands on top of
/// the border, or on the next cell. So the width here is a promise, and a word
/// that won't keep it is broken.
///
/// The space at a break is dropped rather than hung past the edge. Its offset
/// isn't lost: the caller gives every line an end stop just past its last
/// glyph, which is exactly where that space was.
///
/// `width` is in display columns, and a break only ever falls between grapheme
/// clusters. Both matter to more than the picture: the caller anchors each
/// line's end stop just past its last glyph, so a line cut mid-cluster would
/// put a caret stop inside a character — reachable by Down or a click, and the
/// next Backspace would take the cluster apart from the middle.
///
/// An explicit in-cell break (a `\n` glyph, from a `<br>`) is a hard boundary:
/// each run between the breaks wraps on its own and the results stack. The break
/// glyphs are dropped — the caller's per-line end stop already sits exactly where
/// each break was, so no offset is lost.
pub(super) fn wrap_glyphs(glyphs: &[Glyph], width: usize) -> Vec<Vec<Glyph>> {
    if glyphs.iter().any(|g| g.ch == '\n') {
        return glyphs
            .split(|g| g.ch == '\n')
            .flat_map(|seg| wrap_segment(seg, width))
            .collect();
    }
    wrap_segment(glyphs, width)
}

/// [`wrap_glyphs`] for a run with no explicit breaks — the word-wrap proper.
fn wrap_segment(glyphs: &[Glyph], width: usize) -> Vec<Vec<Glyph>> {
    let width = width.max(1);
    // Words are maximal non-space runs, each carrying the space that followed it
    // — which survives only if the next word joins it on this line.
    let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
    let mut word: Vec<Glyph> = Vec::new();
    for g in glyphs {
        if g.ch == ' ' {
            words.push((std::mem::take(&mut word), Some(g.clone())));
        } else {
            word.push(g.clone());
        }
    }
    if !word.is_empty() {
        words.push((word, None));
    }

    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    let mut line: Vec<Glyph> = Vec::new();
    let mut used = 0usize;
    let mut gap: Option<Glyph> = None;
    for (word, space) in words {
        for chunk in hard_break(&word, width) {
            let sep = gap.is_some() as usize;
            let cells = glyphs_width(chunk);
            if !line.is_empty() && used + sep + cells > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
                gap = None; // the break swallows the space
            }
            if let Some(sp) = gap.take() {
                line.push(sp);
                used += 1;
            }
            line.extend_from_slice(chunk);
            used += cells;
        }
        gap = space;
    }
    // An empty cell is still one (empty) line — it has an end the caret can
    // sit at, which is how you type into it.
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Break a single word into pieces of at most `width` columns, cutting only
/// between grapheme clusters — the replacement for slicing it into fixed runs
/// of glyphs, which measures a wide character as one column and can cut an
/// emoji in half.
///
/// A cluster wider than the whole column still gets a piece to itself: there is
/// nowhere legal to cut it, and overflowing by a cell is better than splitting a
/// character. An empty word yields no pieces at all, which is what keeps a
/// double space from opening a line of its own.
pub(super) fn hard_break(word: &[Glyph], width: usize) -> Vec<&[Glyph]> {
    let mut out = Vec::new();
    if word.is_empty() {
        return out;
    }
    let (mut start, mut used) = (0usize, 0usize);
    for c in clusters(word) {
        if used > 0 && used + c.cells > width {
            out.push(&word[start..c.glyph]);
            start = c.glyph;
            used = 0;
        }
        used += c.cells;
    }
    out.push(&word[start..]);
    out
}

/// A table rule spanning `widths`, e.g. `┌──────┬─────┐`. Each column is its
/// content width plus the one-space gutter on either side.
pub(super) fn rule_text(widths: &[usize], left: char, mid: char, right: char) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        for _ in 0..w + 2 {
            s.push('─');
        }
    }
    s.push(right);
    s
}

/// Push real document text: each glyph maps to its own source byte, and the one
/// that opens a grapheme cluster is the caret stop for the whole cluster.
///
/// Per cluster rather than per codepoint because a cluster is the character the
/// user sees, and it's the unit backspace and delete already step by. A stop
/// inside 👨‍👩‍👧 — five codepoints strung together with joiners — is a caret
/// parked in the middle of a character: one press of Right lands there, and the
/// next Backspace severs a joiner from what it joined, leaving a dangling ZWJ in
/// the source. The rest of the cluster still gets its glyph (it has to be
/// drawn); it just isn't somewhere to stand.
pub(super) fn push_text(out: &mut Vec<Glyph>, text: &str, base_src: usize, style: Style) {
    for (gi, cluster) in text.grapheme_indices(true) {
        for (ci, ch) in cluster.char_indices() {
            out.push(Glyph {
                ch,
                style,
                src: base_src + gi + ci,
                stop: ci == 0,
            });
        }
    }
}

/// [`push_text`] for one line of a highlighted code block: the same glyphs at
/// the same offsets, each additionally carrying the [`Token`] of the span it
/// falls in — `spans` being the line's entry from [`code_tokens`], ascending
/// byte ranges *into `text`*. A byte between spans keeps `style` as it is.
///
/// Offsets are what matters here: a token changes how a glyph is painted and
/// nothing about where it is or which source byte it stands on, so a caret
/// walks a highlighted block exactly as it walks an unhighlighted one.
pub(super) fn push_code_text(
    out: &mut Vec<Glyph>,
    text: &str,
    base_src: usize,
    style: Style,
    spans: &[(Range<usize>, Token)],
) {
    let mut spans = spans.iter().peekable();
    for (gi, cluster) in text.grapheme_indices(true) {
        // Spans are ascending, so the one covering this cluster's first byte
        // is at or after the one that covered the last; step past those ended.
        while spans.peek().is_some_and(|(r, _)| r.end <= gi) {
            spans.next();
        }
        let token = spans
            .peek()
            .filter(|(r, _)| r.contains(&gi))
            .map(|(_, t)| *t);
        // A cluster is classed whole, by its first byte: a grammar that split
        // an emoji's scalars between two tokens would otherwise split the
        // glyph, and no grammar means to.
        let style = style.token(token);
        for (ci, ch) in cluster.char_indices() {
            out.push(Glyph {
                ch,
                style,
                src: base_src + gi + ci,
                stop: ci == 0,
            });
        }
    }
}

/// One line's highlighting — `crate::syntax::LineTokens`, spelled here so the
/// shape exists whether or not the feature that fills it does.
type LineTokens = Vec<(Range<usize>, Token)>;

/// The syntax highlighting for a code block's lines, or `None` when the fence's
/// language is not one the grammars know. Without the `syntax` feature nothing
/// is known, and every code glyph draws in the plain code colour.
#[cfg(feature = "syntax")]
pub(super) fn code_tokens(lang: &str, lines: &[&str]) -> Option<Vec<LineTokens>> {
    crate::syntax::highlight(lang, lines)
}

#[cfg(not(feature = "syntax"))]
pub(super) fn code_tokens(_lang: &str, _lines: &[&str]) -> Option<Vec<LineTokens>> {
    None
}

/// Emit an inline `str`/`smart_punctuation` run, mapping every visible char back
/// to its *true* source byte even when the source carries backslash escapes the
/// parsed `text` dropped (`\*` → `*`). The naive `span.start + text_offset`
/// mapping [`push_text`] uses drifts by one byte after each escape, so a caret or
/// click past an escaped `*` would land on the wrong character; walking the text
/// against its source keeps them aligned, and the hidden escape backslash gets no
/// glyph of its own (it is a spelling artefact, not something the caret lands on).
pub(super) fn push_escaped_text(
    out: &mut Vec<Glyph>,
    text: &str,
    span: Range<usize>,
    source: &str,
    style: Style,
) {
    let end = span.end.min(source.len());
    let src = source.get(span.start..end).unwrap_or("");
    // Fast path — no dropped bytes, so text and source align 1:1 (the common
    // case: prose with no escapes). Byte lengths equal ⇒ no backslash was eaten.
    if src.len() == text.len() {
        push_text(out, text, span.start, style);
        return;
    }
    // Slow path: some `\` was consumed. Walk char-by-char, skipping a backslash
    // in the source exactly when it escapes the next visible char (a real escape),
    // never when it is a literal backslash the parse kept (that case has equal
    // lengths and takes the fast path above).
    let sb = src.as_bytes();
    let mut si = 0usize;
    'text: for (_, cluster) in text.grapheme_indices(true) {
        for (ci, ch) in cluster.char_indices() {
            // The text outlasted the source it is being mapped onto. In a
            // consistent document that cannot happen on this path: the slow path
            // is only entered when the two lengths differ, and everything that
            // makes them differ makes the *source* the longer one — an escape
            // backslash the parse ate, or source folded into a neighbouring node.
            // A `smart_punctuation` node reports its canonical ASCII spelling
            // (`--`, `...`, `"`), which is never longer than what was written.
            //
            // So reaching here means `span` was measured against a document that
            // `source` is no longer, and there is no honest offset left to give
            // the remaining characters. Stop: the row comes out short, which is
            // a wrong picture of a document that is already inconsistent. The
            // alternative was `si` stepping past the end and the slice below
            // panicking — which is what it did, in a paint loop.
            if si >= sb.len() {
                break 'text;
            }
            // Advance to the source character this one came from, stepping over
            // whatever the parse dropped on the way. An escape backslash is the
            // common case, but not the only one: a span can cover source that
            // was folded into a neighbouring node (smart punctuation next to a
            // bracket gives `text: "]"` over a source span of `"…]"`). Advancing
            // by the *text* character's length assumed escapes were the only
            // divergence, so one dropped multi-byte character desynchronized
            // every glyph after it — placing `]` inside the `…` before it.
            while si < sb.len() && !src[si..].starts_with(ch) {
                si += src[si..].chars().next().map_or(1, char::len_utf8);
            }
            out.push(Glyph {
                ch,
                style,
                src: span.start + si.min(src.len()),
                stop: ci == 0,
            });
            si += src[si..]
                .chars()
                .next()
                .map_or(ch.len_utf8(), char::len_utf8);
        }
    }
}

/// How many ems deeper than its own line a verse line's turnover hangs.
pub const VERSE_HANG: usize = 2;

/// How many ems a verse stands in from the prose around it.
pub const VERSE_INSET: usize = 2;

/// The byte length of the em space (U+2003) `source` spells at `at`, raw or as
/// an entity, or `None` — the spellings twig's verse pass reads as one step of
/// a line's indent.
pub fn em_space_at(source: &str, at: usize) -> Option<usize> {
    let rest = source.get(at..)?;
    ["&#x2003;", "&#X2003;", "&#8195;", "&emsp;", "\u{2003}"]
        .into_iter()
        .find(|s| rest.starts_with(s))
        .map(str::len)
}

/// The bullet a list `depth` levels deep draws: `•`, `◦`, `▪`, then round again.
pub(super) fn bullet(depth: usize) -> char {
    ['•', '◦', '▪'][depth % 3]
}

/// How item `n` of an ordered list `depth` levels deep is numbered: decimal,
/// then lower-alpha, then lower-roman, then round again — CSS's `decimal`,
/// `lower-alpha` and `lower-roman`, down to their fallbacks. Alpha counts on
/// past `z` to `aa`; roman has no zero and nothing past 3999, so those (and an
/// alpha zero) fall back to decimal, as a browser does.
pub(super) fn ordinal(n: usize, depth: usize) -> String {
    match depth % 3 {
        1 if n > 0 => {
            let mut out = Vec::new();
            let mut n = n;
            while n > 0 {
                n -= 1;
                out.push(b'a' + (n % 26) as u8);
                n /= 26;
            }
            out.reverse();
            String::from_utf8(out).unwrap_or_default()
        }
        2 if (1..4000).contains(&n) => {
            const NUMERALS: [(usize, &str); 13] = [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ];
            let mut out = String::new();
            let mut n = n;
            for (value, numeral) in NUMERALS {
                while n >= value {
                    out.push_str(numeral);
                    n -= value;
                }
            }
            out
        }
        _ => n.to_string(),
    }
}

/// Build synthetic decoration glyphs (a bullet, a gutter) all pointing at `src`,
/// each carrying `role` so the frontend can style it (`Role::Body` for plain
/// padding). Synthetic glyphs are never caret stops — they share one offset, so
/// the caret steps over them (a click still lands at `src`).
pub(super) fn synth(text: &str, role: Role, src: usize) -> Vec<Glyph> {
    let style = Style::default().role(role);
    text.chars()
        .map(|ch| Glyph {
            ch,
            style,
            src,
            stop: false,
        })
        .collect()
}

pub(super) fn concat(a: &[Glyph], b: &[Glyph]) -> Vec<Glyph> {
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    v
}

/// The columns a row's prefix (a bullet, a quote gutter, an indent) takes up
/// before the text it introduces — what the wrap budget has left to spend.
pub(super) fn prefix_width(prefix: &[Glyph]) -> usize {
    glyphs_width(prefix)
}

/// The label shown for an image with no alt text: the final path segment of its
/// destination (`img/cat.png` → `cat.png`), the whole destination when it has no
/// separator, and `"image"` when it's empty. A `data:` URI (which has no useful
/// tail) shows its scheme so the placeholder isn't a wall of base64.
pub(super) fn media_label(dest: &str) -> String {
    if dest.is_empty() {
        return "image".to_string();
    }
    if dest.starts_with("data:") {
        return "data:…".to_string();
    }
    // Trim a query/fragment so a URL's `?v=2#frag` doesn't ride along.
    let clean = dest.split(['?', '#']).next().unwrap_or(dest);
    let tail = clean
        .trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(clean);
    if tail.is_empty() {
        dest.to_string()
    } else {
        tail.to_string()
    }
}

/// A directive's attributes read as a human label — what a frontend puts on a
/// container's tinted panel, and what an attribute-bearing inline directive
/// shows in its chip.
///
/// Reads BOTH conventions diaryx content actually uses: twig's own dot-prefixed
/// classes (`{.public .family}`, arriving as one combined `class` attr) and bare
/// pandoc-style words with no leading dot (`{public family}` — what
/// `diaryx_core::visibility`'s publish-time filter and apps/web's directive
/// serializer both write, and which twig parses as one valueless attribute
/// each). Reading only `.class` would leave every *existing* diaryx `:::vis{…}`
/// block unlabeled. A `key=value` attr is configuration rather than a name, so
/// it contributes nothing. `None` when nothing readable is left.
pub(super) fn directive_attr_label(attrs: &[(String, Option<String>)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in attrs {
        if k == "class" {
            if let Some(v) = v
                && !v.is_empty()
            {
                parts.push(v.clone());
            }
        } else if v.as_deref().unwrap_or("").is_empty() {
            parts.push(k.clone());
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub(super) fn heading_style(level: u32) -> Style {
    // Just the role — a frontend decides how a heading of this level *looks*
    // (the terminal cycles a color and bolds it, the GUI scales the font). The
    // author wrote no emphasis here, so core records none. `level as u8` is safe:
    // Markdown/Djot cap headings at 6.
    Style::default().role(Role::Heading(level.min(255) as u8))
}
