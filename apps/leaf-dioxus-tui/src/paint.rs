//! Blitz's laid-out page, painted into character cells.
//!
//! The walk is blitz-paint's (`render.rs`'s `render_element` and
//! `draw_children`): each element's box at its parent's origin plus its
//! taffy location, its children shifted back by its scroll, hoisted children
//! placed by their stacking context, and text read off the parley layout of
//! each inline root. What differs is the surface: a box's background fills
//! the cells whose centres it covers, a border is a line of box-drawing
//! characters, and a glyph cluster is written into the cell its left edge
//! rounds to. With a monospace face whose advance is [`Metrics::cw`], every
//! glyph lands in a cell of its own.
//!
//! The one element read by name is the editor's caret, `.caret-in`, a
//! zero-width box with a 2px border: drawn as cells it would be nothing or a
//! whole column, so it becomes the terminal's own cursor instead.

use std::collections::HashMap;

use blitz_dom::{BaseDocument, Node, NodeData, NodeId};
use parley::PositionedLayoutItem;
use style::color::AbsoluteColor;
use style::computed_values::visibility::T as Visibility;
use style::properties::ComputedValues;
use style::values::computed::Overflow;
use unicode_width::UnicodeWidthStr;

/// One character cell, in CSS pixels.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub cw: f32,
    pub ch: f32,
}

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// The cluster, or `""` for the second half of a wide one.
    pub text: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Cell {
    fn blank(fg: Rgb, bg: Rgb) -> Self {
        Self {
            text: " ".into(),
            fg,
            bg,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    /// Where the caret is, if it is on screen.
    pub cursor: Option<(u16, u16)>,
    /// The pictures, formulas and diagrams, for `pictures.rs` to paint over
    /// the frames drawn for them.
    pub pictures: Vec<Picture>,
}

/// An `img`'s box in cells. Its whole box, which may run off the screen,
/// and the rows of it in view, which a box drawn later may have covered.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub src: String,
    pub col: i32,
    pub row: i32,
    pub cols: u16,
    pub rows: u16,
    /// The rows in view, `top..bottom`; empty when none is.
    pub shown: (i32, i32),
}

impl Grid {
    pub fn new(cols: u16, rows: u16, fg: Rgb, bg: Rgb) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(fg, bg); cols as usize * rows as usize],
            cursor: None,
            pictures: Vec::new(),
        }
    }

    pub fn get(&self, c: u16, r: u16) -> &Cell {
        &self.cells[r as usize * self.cols as usize + c as usize]
    }

    fn at(&mut self, c: i32, r: i32) -> Option<&mut Cell> {
        if c < 0 || r < 0 || c >= self.cols as i32 || r >= self.rows as i32 {
            return None;
        }
        Some(&mut self.cells[r as usize * self.cols as usize + c as usize])
    }

    /// The grid's text, a line per row, trailing spaces dropped, and `▏`
    /// before the cell the caret is in.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for r in 0..self.rows {
            let mut line = String::new();
            for c in 0..self.cols {
                if self.cursor == Some((c, r)) {
                    line.push('▏');
                }
                line.push_str(&self.get(c, r).text);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

impl Grid {
    /// The grid as an HTML `<pre>`, each cell in its colours, the caret's
    /// cell outlined.
    pub fn html(&self) -> String {
        let esc = |t: &str| {
            t.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let hex = |(r, g, b): Rgb| format!("#{r:02x}{g:02x}{b:02x}");
        let mut out = String::from(
            "<pre style=\"font: 14px/1.2 Menlo, monospace; display: inline-block; margin: 0\">",
        );
        for r in 0..self.rows {
            // Cells of one style go in one span.
            let mut run: Option<(String, String)> = None;
            for c in 0..self.cols {
                let cell = self.get(c, r);
                if cell.text.is_empty() {
                    continue;
                }
                let mut style = format!("color:{};background:{}", hex(cell.fg), hex(cell.bg));
                if cell.bold {
                    style.push_str(";font-weight:bold");
                }
                if cell.italic {
                    style.push_str(";font-style:italic");
                }
                if cell.underline {
                    style.push_str(";text-decoration:underline");
                }
                if self.cursor == Some((c, r)) {
                    style.push_str(";box-shadow:inset 2px 0 #4a9eff");
                }
                match &mut run {
                    Some((st, text)) if *st == style => text.push_str(&cell.text),
                    _ => {
                        if let Some((st, text)) = run.take() {
                            out.push_str(&format!("<span style=\"{st}\">{}</span>", esc(&text)));
                        }
                        run = Some((style, cell.text.clone()));
                    }
                }
            }
            if let Some((st, text)) = run {
                out.push_str(&format!("<span style=\"{st}\">{}</span>", esc(&text)));
            }
            out.push('\n');
        }
        out.push_str("</pre>");
        out
    }
}

/// A rectangle in CSS pixels: what a box covers, or what may be drawn on.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn intersect(self, o: Rect) -> Rect {
        Rect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

struct Painter<'a> {
    doc: &'a BaseDocument,
    m: Metrics,
    grid: &'a mut Grid,
    root: NodeId,
    /// Where each inline box's line put it: its cell and its width in pixels.
    inline_cells: HashMap<NodeId, (i32, i32, f32)>,
}

/// Paint `doc`, already resolved, into a fresh grid of `cols` × `rows`.
pub fn paint(doc: &BaseDocument, m: Metrics, cols: u16, rows: u16) -> Grid {
    let (fg, bg) = page_colors(doc);
    let mut grid = Grid::new(cols, rows, fg, bg);
    let Some(root) = doc.try_root_element() else {
        return grid;
    };
    let scroll = doc.viewport_scroll();
    let clip = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: cols as f32 * m.cw,
        y1: rows as f32 * m.ch,
    };
    let mut p = Painter {
        doc,
        m,
        grid: &mut grid,
        root: root.id,
        inline_cells: HashMap::new(),
    };
    p.element(root.id, (-scroll.x as f32, -scroll.y as f32), clip);
    grid
}

/// The page's colour and background: `html`'s, else `body`'s, as
/// blitz-paint picks the canvas colour.
fn page_colors(doc: &BaseDocument) -> (Rgb, Rgb) {
    let mut fg = (220, 220, 220);
    let mut bg = (0, 0, 0);
    let Some(root) = doc.try_root_element() else {
        return (fg, bg);
    };
    let body = root
        .children
        .iter()
        .filter_map(|id| doc.get_node(*id))
        .find(|n| {
            n.data
                .is_element_with_tag_name(&blitz_dom::local_name!("body"))
        });
    for node in [Some(root), body].into_iter().flatten() {
        if let Some(s) = node.primary_styles() {
            let cc = s.clone_color();
            fg = rgb(&cc).0;
            let (b, a) = rgb(&s.clone_background_color().resolve_to_absolute(&cc));
            if a > 0.5 {
                bg = b;
            }
        }
    }
    (fg, bg)
}

fn rgb(c: &AbsoluteColor) -> (Rgb, f32) {
    let [r, g, b, a] = *c
        .to_color_space(style::color::ColorSpace::Srgb)
        .raw_components();
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    ((q(r), q(g), q(b)), a)
}

fn blend(over: Rgb, a: f32, under: Rgb) -> Rgb {
    let m = |o: u8, u: u8| (o as f32 * a + u as f32 * (1.0 - a)).round() as u8;
    (m(over.0, under.0), m(over.1, under.1), m(over.2, under.2))
}

impl Painter<'_> {
    fn node(&self, id: NodeId) -> &Node {
        &self.doc.tree()[id]
    }

    fn render(&mut self, id: NodeId, origin: (f32, f32), clip: Rect) {
        match &self.node(id).data {
            NodeData::Element(_) | NodeData::AnonymousBlock(_) => self.element(id, origin, clip),
            _ => {}
        }
    }

    fn element(&mut self, id: NodeId, origin: (f32, f32), clip: Rect) {
        let doc = self.doc;
        let node = &doc.tree()[id];
        if matches!(node.taffy_display(), taffy::Display::None) {
            return;
        }
        let Some(styles) = node.primary_styles() else {
            return;
        };
        let styles: &ComputedValues = &styles;
        if styles.get_inherited_box().visibility != Visibility::Visible
            || styles.get_effects().opacity == 0.0
        {
            return;
        }
        let l = node.final_layout();
        let x = origin.0 + l.location.x;
        let y = origin.1 + l.location.y;
        let bbox = Rect {
            x0: x,
            y0: y,
            x1: x + l.size.width,
            y1: y + l.size.height,
        };

        if node
            .attr(blitz_dom::local_name!("class"))
            .is_some_and(|c| c.split_ascii_whitespace().any(|c| c == "caret-in"))
        {
            let (cx, cy) = (x, y + l.size.height / 2.0);
            if clip.contains(cx, cy) {
                let (c, r) = match self.inline_cells.get(&id) {
                    Some(&(c, r, _)) => (c, r),
                    None => (
                        (cx / self.m.cw).round() as i32,
                        (cy / self.m.ch).floor() as i32,
                    ),
                };
                if c >= 0 && r >= 0 && c < self.grid.cols as i32 && r < self.grid.rows as i32 {
                    self.grid.cursor = Some((c as u16, r as u16));
                }
            }
            return;
        }

        self.background(styles, bbox.intersect(clip));
        self.borders(styles, bbox, l, clip);
        if node
            .data
            .is_element_with_tag_name(&blitz_dom::local_name!("img"))
        {
            let src = node.attr(blitz_dom::local_name!("src")).unwrap_or("");
            // A file by its name; a formula or diagram (an SVG `data:` URL)
            // by its alt text, if it has one.
            let name = match src.strip_prefix("data:") {
                Some(_) => node
                    .attr(blitz_dom::local_name!("alt"))
                    .unwrap_or("picture"),
                None => src.rsplit('/').next().unwrap_or(src),
            };
            let fg = rgb(&styles.clone_color()).0;
            self.frame(bbox, clip, fg, &format!("🖼 {name}"));
            self.picture(src, bbox, clip);
        }

        let ov = styles.get_box();
        let clips = !matches!(ov.overflow_x, Overflow::Visible)
            || !matches!(ov.overflow_y, Overflow::Visible);
        let pb_left = l.padding.left + l.border.left;
        let pb_top = l.padding.top + l.border.top;
        let inner_clip = if clips {
            clip.intersect(Rect {
                x0: x + l.border.left,
                y0: y + l.border.top,
                x1: bbox.x1 - l.border.right,
                y1: bbox.y1 - l.border.bottom,
            })
        } else {
            clip
        };
        let scroll = node.scroll_offset();
        let (sx, sy) = (scroll.x as f32, scroll.y as f32);

        if node.flags.is_inline_root()
            && let Some(tl) = node
                .element_data()
                .and_then(|e| e.inline_layout_data.as_ref())
        {
            let tx = x + pb_left - sx;
            let ty = y + pb_top - sy + tl.block_offset;
            self.text(tl, (tx, ty), inner_clip);
        }

        // Children draw from this box's origin, less its scroll.
        let base = (x - sx, y - sy);
        let scroll_v = doc.viewport_scroll();
        if let Some(sc) = &node.stacking_context {
            for h in sc.neg_z_hoisted_children() {
                let p = h.position(node.tree(), id);
                self.render(h.node_id, (base.0 + p.x, base.1 + p.y), inner_clip);
            }
        }
        let kids: Vec<NodeId> = match &*node.paint_children.borrow() {
            Some(k) => k.iter().copied().collect(),
            None => Vec::new(),
        };
        for kid in kids {
            let fixed =
                self.node(kid).taffy_position() == taffy::Position::Fixed && id == self.root;
            let o = if fixed {
                (base.0 + scroll_v.x as f32, base.1 + scroll_v.y as f32)
            } else {
                base
            };
            self.render(kid, o, inner_clip);
        }
        if let Some(sc) = &node.stacking_context {
            for h in sc.pos_z_hoisted_children() {
                let p = h.position(node.tree(), id);
                self.render(h.node_id, (base.0 + p.x, base.1 + p.y), inner_clip);
            }
        }
    }

    /// Record an `img` at `b`, drawn to cells as [`Self::frame`] does.
    fn picture(&mut self, src: &str, b: Rect, clip: Rect) {
        let m = self.m;
        let col = (b.x0 / m.cw).round() as i32;
        let row = (b.y0 / m.ch).round() as i32;
        let cols = ((b.x1 / m.cw).round() as i32 - col).max(1);
        let rows = ((b.y1 / m.ch).round() as i32 - row).max(1);
        let top = row.max((clip.y0 / m.ch).round() as i32).max(0);
        let bottom = (row + rows)
            .min((clip.y1 / m.ch).round() as i32)
            .min(self.grid.rows as i32);
        self.grid.pictures.push(Picture {
            src: src.to_string(),
            col,
            row,
            cols: cols as u16,
            rows: rows as u16,
            shown: (top, bottom.max(top)),
        });
    }

    /// A placeholder for what cells cannot show: a box drawn round `b`,
    /// `label` on its top edge.
    fn frame(&mut self, b: Rect, clip: Rect, fg: Rgb, label: &str) {
        let m = self.m;
        let c0 = (b.x0 / m.cw).round() as i32;
        let c1 = ((b.x1 / m.cw).round() as i32 - 1).max(c0);
        let r0 = (b.y0 / m.ch).round() as i32;
        let r1 = ((b.y1 / m.ch).round() as i32 - 1).max(r0);
        let visible =
            |c: i32, r: i32| clip.contains((c as f32 + 0.5) * m.cw, (r as f32 + 0.5) * m.ch);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let ch = match (r == r0, r == r1, c == c0, c == c1) {
                    (true, _, true, _) => "╭",
                    (true, _, _, true) => "╮",
                    (_, true, true, _) => "╰",
                    (_, true, _, true) => "╯",
                    (true, _, _, _) | (_, true, _, _) => "─",
                    (_, _, true, _) | (_, _, _, true) => "│",
                    _ => continue,
                };
                if visible(c, r)
                    && let Some(cell) = self.grid.at(c, r)
                {
                    cell.text = ch.into();
                    cell.fg = fg;
                }
            }
        }
        let mut c = c0 + 2;
        for g in unicode_segmentation::UnicodeSegmentation::graphemes(label, true) {
            let w = UnicodeWidthStr::width(g).max(1) as i32;
            if c + w > c1 - 1 {
                break;
            }
            if visible(c, r0) {
                if let Some(cell) = self.grid.at(c, r0) {
                    cell.text = g.into();
                    cell.fg = fg;
                }
                if w == 2
                    && let Some(next) = self.grid.at(c + 1, r0)
                {
                    next.text = String::new();
                }
            }
            c += w;
        }
    }

    /// Fill the cells whose centres `area` covers.
    fn background(&mut self, styles: &ComputedValues, area: Rect) {
        let cc = styles.clone_color();
        let (c, a) = rgb(&styles.clone_background_color().resolve_to_absolute(&cc));
        if a < 0.05 {
            return;
        }
        self.fill(area, c, a);
    }

    fn fill(&mut self, area: Rect, c: Rgb, a: f32) {
        let m = self.m;
        let c0 = (area.x0 / m.cw - 0.5).ceil() as i32;
        let c1 = (area.x1 / m.cw - 0.5).ceil() as i32;
        let r0 = (area.y0 / m.ch - 0.5).ceil() as i32;
        let r1 = (area.y1 / m.ch - 0.5).ceil() as i32;
        // An opaque box across a picture's whole width covers the rows of
        // it that it spans: the status line over a picture at the bottom.
        if a > 0.5 && r0 < r1 {
            for p in &mut self.grid.pictures {
                if c0 > p.col || c1 < p.col + p.cols as i32 {
                    continue;
                }
                let (top, bottom) = p.shown;
                if r0 <= top && r1 > top {
                    p.shown.0 = r1.min(bottom);
                } else if r0 < bottom && r1 >= bottom {
                    p.shown.1 = r0.max(top);
                }
            }
        }
        for r in r0..r1 {
            for col in c0..c1 {
                if a > 0.5 && self.grid.cursor == Some((col as u16, r as u16)) {
                    self.grid.cursor = None;
                }
                if let Some(cell) = self.grid.at(col, r) {
                    // An opaque box covers what was drawn under it.
                    if a > 0.5 {
                        *cell = Cell::blank(cell.fg, cell.bg);
                    }
                    cell.bg = blend(c, a, cell.bg);
                }
            }
        }
    }

    /// Each side with a visible border, as a line of box-drawing characters
    /// along the cells that side runs through.
    fn borders(&mut self, styles: &ComputedValues, b: Rect, l: &taffy::Layout, clip: Rect) {
        use style::values::specified::BorderStyle;
        let bs = styles.get_border();
        let cc = styles.clone_color();
        let m = self.m;
        let side = |w: f32, st: BorderStyle, col: &style::values::computed::Color| {
            if w <= 0.0 || matches!(st, BorderStyle::None | BorderStyle::Hidden) {
                return None;
            }
            let (c, a) = rgb(&col.resolve_to_absolute(&cc));
            (a > 0.05).then_some((c, a))
        };
        let col_of = |x: f32| (x / m.cw).floor() as i32;
        let row_of = |y: f32| (y / m.ch).floor() as i32;
        let (c0, c1) = (col_of(b.x0), col_of((b.x1 - 0.01).max(b.x0)));
        let (r0, r1) = (row_of(b.y0), row_of((b.y1 - 0.01).max(b.y0)));
        let put = |g: &mut Grid, c: i32, r: i32, ch: &str, (fg, a): (Rgb, f32)| {
            let (px, py) = ((c as f32 + 0.5) * m.cw, (r as f32 + 0.5) * m.ch);
            if !clip.contains(px, py) {
                return;
            }
            if let Some(cell) = g.at(c, r) {
                cell.text = ch.into();
                cell.fg = blend(fg, a, cell.bg);
            }
        };
        // A horizontal border gets a row only when the box is tall enough to
        // give it one: a table cell a line high keeps its text, not a rule
        // drawn through it.
        let content_top = row_of(b.y0 + l.border.top + l.padding.top + 0.01);
        let content_bottom = row_of((b.y1 - l.border.bottom - l.padding.bottom - 0.01).max(b.y0));
        if let Some(fg) = side(l.border.top, bs.border_top_style, &bs.border_top_color)
            .filter(|_| r0 < content_top)
        {
            for c in c0..=c1 {
                put(self.grid, c, r0, "─", fg);
            }
        }
        if let Some(fg) = side(
            l.border.bottom,
            bs.border_bottom_style,
            &bs.border_bottom_color,
        )
        .filter(|_| r1 > content_bottom)
        {
            for c in c0..=c1 {
                put(self.grid, c, r1, "─", fg);
            }
        }
        if let Some(fg) = side(l.border.left, bs.border_left_style, &bs.border_left_color) {
            for r in r0..=r1 {
                put(self.grid, c0, r, "│", fg);
            }
        }
        if let Some(fg) = side(
            l.border.right,
            bs.border_right_style,
            &bs.border_right_color,
        ) {
            for r in r0..=r1 {
                put(self.grid, c1, r, "│", fg);
            }
        }
    }

    /// An inline root's text, its origin at `o`. A line starts in the cell
    /// its left edge rounds to, and each cluster after takes the next cell
    /// (two for a wide one), so text set larger than a cell — a heading —
    /// reads as text rather than spaced-out letters. A gap between runs, and
    /// an inline box, take the cells their width rounds to; an inline box's
    /// cell is kept for [`Self::element`], which is how the caret finds its
    /// column in a heading.
    fn text(&mut self, tl: &blitz_dom::node::TextLayout, o: (f32, f32), clip: Rect) {
        let m = self.m;
        for line in tl.layout.lines() {
            let lm = line.metrics();
            let mid = o.1 + (lm.block_min_coord + lm.block_max_coord) / 2.0;
            let r = (mid / m.ch).floor() as i32;
            // How many of each run's clusters earlier glyph runs drew.
            let mut taken: HashMap<usize, usize> = HashMap::new();
            // The next free cell, and the x where the last item ended.
            let mut col: Option<i32> = None;
            let mut end_x = 0.0f32;
            let place = |x: f32, col: &mut Option<i32>, end_x: f32| -> i32 {
                match *col {
                    None => ((o.0 + x) / m.cw).round() as i32,
                    Some(c) => c + ((x - end_x) / m.cw).round().max(0.0) as i32,
                }
            };
            for item in line.items() {
                let gr = match item {
                    PositionedLayoutItem::GlyphRun(gr) => gr,
                    PositionedLayoutItem::InlineBox(b) => {
                        let c = place(b.x, &mut col, end_x);
                        self.inline_cells
                            .insert(NodeId::from_u64(b.id), (c, r, b.width));
                        col = Some(c + (b.width / m.cw).round() as i32);
                        end_x = b.x + b.width;
                        continue;
                    }
                };
                let mut c = place(gr.offset(), &mut col, end_x);
                end_x = gr.offset() + gr.advance();
                let st = self
                    .doc
                    .get_node(gr.style().brush.id)
                    .and_then(|n| n.primary_styles());
                let Some(st) = st else {
                    col = Some(c);
                    continue;
                };
                let cc = st.clone_color();
                let (fg, fa) = rgb(&cc);
                let (bg, ba) = rgb(&st.clone_background_color().resolve_to_absolute(&cc));
                let bold = st.get_font().font_weight.value() >= 600.0;
                let italic = st.get_font().font_style != style::values::computed::FontStyle::NORMAL;
                let underline = st
                    .get_text()
                    .text_decoration_line
                    .contains(style::values::specified::TextDecorationLine::UNDERLINE);
                // A parley run can hold several spans of one face; the glyph
                // run is the stretch of it in one style. Its clusters are the
                // run's next ones in that style.
                let key = gr.run().text_range().start;
                let skip = *taken.get(&key).unwrap_or(&0);
                let style = gr.style_index();
                let clusters: Vec<_> = gr
                    .run()
                    .visual_clusters()
                    .skip(skip)
                    .take_while(|c| c.style_index() == style)
                    .collect();
                taken.insert(key, skip + clusters.len());
                for cluster in clusters {
                    let s = &tl.text[cluster.text_range()];
                    let s = if s.chars().all(char::is_whitespace) {
                        " "
                    } else {
                        s
                    };
                    let w = UnicodeWidthStr::width(s).max(1) as i32;
                    let at = c;
                    c += w;
                    let cx = (at as f32 + 0.5) * m.cw;
                    if !clip.contains(cx, mid) {
                        continue;
                    }
                    let Some(cell) = self.grid.at(at, r) else {
                        continue;
                    };
                    if ba > 0.05 {
                        cell.bg = blend(bg, ba, cell.bg);
                    }
                    // Transparent text (a ghost) keeps its place and draws
                    // nothing.
                    if fa < 0.3 {
                        continue;
                    }
                    cell.text = s.to_string();
                    cell.fg = blend(fg, fa, cell.bg);
                    cell.bold = bold;
                    cell.italic = italic;
                    cell.underline = underline;
                    if w == 2
                        && let Some(next) = self.grid.at(at + 1, r)
                    {
                        next.text = String::new();
                    }
                }
                col = Some(c);
            }
        }
    }
}
