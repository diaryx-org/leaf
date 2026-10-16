//! leaf-core's rows, wrapped at a pixel width and positioned — the part of
//! `leaf-gpui` (`wrap_logical`, `RowLayout`) a Ply frontend has to bring itself,
//! since Ply hands out no per-character geometry.
//!
//! The output is a flat list of [`Line`]s in content coordinates. Every glyph
//! carries its x, its advance, and the source byte it came from, so drawing,
//! caret placement, selection and hit-testing all read the same numbers.

use leaf_core::{Doc, Glyph, Role, Style, Token, View};

use crate::fonts::{Face, Fonts};

/// Theme numbers the layout needs.
#[derive(Clone, Copy)]
pub struct Metrics {
    pub body_size: u16,
    /// Row height over font size.
    pub line_ratio: f32,
    /// A between-blocks gap row's height, as a fraction of a body row.
    pub gap_scale: f32,
    pub heading_scale: [f32; 6],
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            body_size: 18,
            line_ratio: 1.5,
            gap_scale: 0.5,
            heading_scale: [1.75, 1.45, 1.25, 1.1, 1.0, 0.95],
        }
    }
}

/// A run of adjacent glyphs drawn as one Ply text element.
#[derive(Clone, Debug)]
pub struct Run {
    pub text: String,
    pub face: Face,
    pub size: u16,
    pub style: Style,
    /// Left edge, relative to the line.
    pub x: f32,
    pub width: f32,
    /// This face's top-to-baseline at this size: Ply draws a text element's
    /// baseline that far below the element's top.
    pub ascent: f32,
}

/// One positioned character.
#[derive(Clone, Copy, Debug)]
pub struct Char {
    pub x: f32,
    pub advance: f32,
    pub src: usize,
    pub stop: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prose,
    /// A fenced/indented code line: tinted, never wrapped.
    Code,
    /// A table row. leaf-core spells tables with box-drawing glyphs, which
    /// only line up in a monospace face, so the spike draws them in one — the
    /// TUI's picture. A real frontend builds a grid from `TableInfo`, as
    /// leaf-gpui and leaf-web do.
    Table,
    /// A row the caret never rests on: block gaps, table rules.
    Decoration,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub y: f32,
    pub height: f32,
    /// From the line's top to its baseline.
    pub baseline: f32,
    pub kind: Kind,
    pub runs: Vec<Run>,
    pub chars: Vec<Char>,
    /// The caret stop just past the last character.
    pub end_src: usize,
    /// The line ends at a soft wrap, not a break: its `end_src` is the next
    /// line's first stop.
    pub wrapped: bool,
    pub end_x: f32,
}

impl Line {
    pub fn holds_caret(&self) -> bool {
        self.kind != Kind::Decoration
    }
}

pub fn heading_scale(level: u8, m: &Metrics) -> f32 {
    level
        .checked_sub(1)
        .and_then(|i| m.heading_scale.get(i as usize))
        .copied()
        .unwrap_or(1.0)
}

fn face_of(style: &Style, kind: Kind) -> Face {
    if style.role == Role::Code || kind == Kind::Table {
        return Face::Mono;
    }
    let heading = matches!(style.role, Role::Heading(_));
    let italic = style.italic || (style.role == Role::Code && style.token == Some(Token::Comment));
    Face::styled(style.bold || heading, italic)
}

/// A logical line: one unwrapped row (WYSIWYG) or one source line.
struct Logical {
    glyphs: Vec<Glyph>,
    end_src: usize,
    kind: Kind,
    heading: Option<u8>,
}

fn gather(doc: &Doc) -> Vec<Logical> {
    match doc.view {
        View::Source => {
            let mut out = Vec::new();
            let mut start = 0;
            for line in doc.source.split('\n') {
                let glyphs = line
                    .char_indices()
                    .map(|(i, ch)| Glyph {
                        ch,
                        style: Style::default(),
                        src: start + i,
                        stop: true,
                    })
                    .collect();
                out.push(Logical {
                    glyphs,
                    end_src: start + line.len(),
                    kind: Kind::Prose,
                    heading: None,
                });
                start += line.len() + 1;
            }
            out
        }
        View::Wysiwyg => {
            let in_table = |r: usize| doc.vmap.tables.iter().any(|t| t.rows_span.contains(&r));
            doc.vmap
                .rows
                .iter()
                .enumerate()
                .map(|(r, row)| Logical {
                    glyphs: row.glyphs.clone(),
                    end_src: row.end_src,
                    kind: if row.decoration && !in_table(r) {
                        Kind::Decoration
                    } else if in_table(r) {
                        Kind::Table
                    } else if row.code {
                        Kind::Code
                    } else {
                        Kind::Prose
                    },
                    heading: row.heading,
                })
                .collect()
        }
    }
}

/// Lay the whole document out at `width` pixels.
pub fn layout(doc: &mut Doc, fonts: &mut Fonts, m: &Metrics, width: f32) -> Vec<Line> {
    if doc.view == View::Wysiwyg {
        // Formulas and images stay as text (TeX, `🖼 alt`): pictures are out of
        // this spike's scope.
        doc.set_inline_pictures(false);
        doc.build_visual_unwrapped();
    }
    let mut lines = Vec::new();
    let mut y = 0.0;
    for logical in gather(doc) {
        y = wrap(logical, fonts, m, width, y, &mut lines);
    }
    lines
}

/// The size a logical line is drawn at: the body size, scaled if it is a
/// heading. One size per line, as in leaf-gpui.
fn line_size(l: &Logical, m: &Metrics) -> u16 {
    let level = l.heading.or_else(|| {
        l.glyphs.iter().find_map(|g| match g.style.role {
            Role::Heading(n) => Some(n),
            _ => None,
        })
    });
    let scale = level.map_or(1.0, |n| heading_scale(n, m));
    (m.body_size as f32 * scale).round() as u16
}

/// Greedy word wrap, the same rule as leaf-gpui's `wrap_logical`: break before
/// a word when the line up to that word's end would overflow. Pushes the
/// resulting lines and returns the y below them.
fn wrap(
    l: Logical,
    fonts: &mut Fonts,
    m: &Metrics,
    width: f32,
    y: f32,
    out: &mut Vec<Line>,
) -> f32 {
    let size = line_size(&l, m);
    let body_h = m.body_size as f32 * m.line_ratio;
    let height = match l.kind {
        Kind::Decoration if l.glyphs.is_empty() => body_h * m.gap_scale,
        _ => size as f32 * m.line_ratio,
    };

    let faces: Vec<Face> = l.glyphs.iter().map(|g| face_of(&g.style, l.kind)).collect();
    let adv: Vec<f32> = l
        .glyphs
        .iter()
        .zip(&faces)
        .map(|(g, &f)| fonts.advance(f, size, g.drawn()))
        .collect();

    // Glyph index each visual line starts at.
    let mut starts = vec![0usize];
    if matches!(l.kind, Kind::Prose) {
        let n = l.glyphs.len();
        let mut line_start = 0;
        let mut x_at_start = 0.0;
        let mut x = 0.0;
        let mut j = 0;
        while j < n {
            if l.glyphs[j].ch == ' ' {
                x += adv[j];
                j += 1;
                continue;
            }
            let word_start = j;
            let x_word = x;
            while j < n && l.glyphs[j].ch != ' ' {
                x += adv[j];
                j += 1;
            }
            if x - x_at_start > width && word_start > line_start {
                starts.push(word_start);
                line_start = word_start;
                x_at_start = x_word;
            }
        }
    }

    let mut y = y;
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(l.glyphs.len());
        // A wrapped line's end stop is where the next line begins: the caret
        // past its last glyph is the same offset as before the next one's first.
        let end_src = match starts.get(k + 1) {
            Some(&next) => l.glyphs[next].src,
            None => l.end_src,
        };
        let mut line = Line {
            y,
            height,
            baseline: 0.0,
            kind: l.kind,
            runs: Vec::new(),
            chars: Vec::with_capacity(e - s),
            end_src,
            wrapped: k + 1 < starts.len(),
            end_x: 0.0,
        };
        let mut x = 0.0;
        let mut ascent: f32 = 0.0;
        let mut text_h: f32 = 0.0;
        for i in s..e {
            let g = &l.glyphs[i];
            line.chars.push(Char {
                x,
                advance: adv[i],
                src: g.src,
                stop: g.stop,
            });
            let same = line
                .runs
                .last()
                .is_some_and(|r: &Run| r.style == g.style && r.face == faces[i]);
            if !same {
                let mt = fonts.metrics(faces[i], size);
                ascent = ascent.max(mt.ascent);
                text_h = text_h.max(mt.height);
                line.runs.push(Run {
                    text: String::new(),
                    face: faces[i],
                    size,
                    style: g.style,
                    x,
                    width: 0.0,
                    ascent: mt.ascent,
                });
            }
            let run = line.runs.last_mut().unwrap();
            run.text.push(g.drawn());
            run.width += adv[i];
            x += adv[i];
        }
        if line.runs.is_empty() {
            let mt = fonts.metrics(Face::Regular, size);
            ascent = mt.ascent;
            text_h = mt.height;
        }
        line.end_x = x;
        // Centre the tallest face's box in the row; every run then sits on
        // that one baseline, whatever its own face's ascent.
        line.baseline = ((height - text_h) / 2.0).max(0.0) + ascent;
        y += height;
        out.push(line);
    }
    y
}

/// Where the caret at `offset` draws: (line index, x).
///
/// The rule is leaf-core's `VisualMap::pos_of_offset`, applied to our wrapped
/// lines: the caret shows at the first stop at or after it (a glyph, or a
/// line's end), and a tie goes to the later line — the only offset two lines
/// share is a wrap boundary, which belongs to the line below. An offset need
/// not be any glyph's `src`: 0 in `# Title` sits before the hidden `# ` and
/// shows on the `T`.
pub fn caret_at(lines: &[Line], offset: usize) -> Option<(usize, f32)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for (i, l) in lines.iter().enumerate() {
        if !l.holds_caret() {
            continue;
        }
        let cand = l
            .chars
            .iter()
            .find(|c| c.stop && c.src >= offset)
            .map(|c| (c.src, i, c.x))
            .or_else(|| (l.end_src >= offset).then_some((l.end_src, i, l.end_x)));
        if let Some(c) = cand
            && best.is_none_or(|b| c.0 <= b.0)
        {
            best = Some(c);
        }
    }
    // Past every stop (a trailing newline): the last line's end, as
    // `pos_of_offset` falls back to its last stop row.
    best.map(|(_, i, x)| (i, x)).or_else(|| {
        lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| l.holds_caret())
            .map(|(i, l)| (i, l.end_x))
    })
}

/// The source offset a point in content coordinates lands on: the nearest line
/// that can hold a caret, then the nearest stop by each character's midpoint.
pub fn hit(lines: &[Line], x: f32, y: f32) -> Option<usize> {
    let line = lines.iter().filter(|l| l.holds_caret()).min_by(|a, b| {
        let d = |l: &Line| {
            if y < l.y {
                l.y - y
            } else if y > l.y + l.height {
                y - l.y - l.height
            } else {
                0.0
            }
        };
        d(a).total_cmp(&d(b))
    })?;
    for c in &line.chars {
        if c.stop && x < c.x + c.advance / 2.0 {
            return Some(c.src);
        }
    }
    // Past the midpoint of every stop: the line's end. A stop past the last
    // drawn one (a grapheme continuation) never wins.
    Some(line.end_src)
}

/// The stop nearest `x` on one line — vertical motion's landing spot.
pub fn offset_in_line(line: &Line, x: f32) -> usize {
    for c in &line.chars {
        if c.stop && x < c.x + c.advance / 2.0 {
            return c.src;
        }
    }
    line.end_src
}

/// Where Up (`dir = -1`) or Down (`+1`) from the caret lands, keeping `x`:
/// the next line that way that can hold a caret, block gaps stepped over.
/// `None` off either end of the document.
pub fn vertical(lines: &[Line], caret: usize, dir: i32, x: f32) -> Option<usize> {
    let (i, _) = caret_at(lines, caret)?;
    let mut j = i as i32 + dir;
    while (0..lines.len() as i32).contains(&j) {
        let l = &lines[j as usize];
        if l.holds_caret() {
            return Some(offset_in_line(l, x));
        }
        j += dir;
    }
    None
}

pub fn content_height(lines: &[Line]) -> f32 {
    lines.last().map_or(0.0, |l| l.y + l.height)
}
