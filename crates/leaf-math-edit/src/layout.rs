//! Where every atom, row and caret stop of a [`Formula`] lands on the page.
//!
//! RaTeX's display list carries no source positions, so they are smuggled in
//! as colour. The formula is printed again as *probe TeX*: each atom wrapped
//! in `\htmlStyle{color: #id}{…}`, each empty row drawn as a `\square` in its
//! row's colour. `\htmlStyle` lays out as its contents — RaTeX classifies it
//! by its body, so binary-operator cancellation and inter-atom spacing come
//! out identical (`tests::the_probe_does_not_move_anything`) — and its colour
//! reaches every glyph, rule and path inside it, with an inner wrapper's
//! colour winning. Reading the colours back gives each atom its own ink; an
//! atom's box is that ink plus its slots' boxes.
//!
//! The probe list is then recoloured to the theme and is the picture: what
//! the editor shows is exactly what was measured.

use crate::{Caret, Formula, Kind};
use ratex_font::FontId;
use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_svg::{SvgColorSyntax, SvgOptions, render_to_svg_with_color_syntax};
use ratex_types::color::Color;
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::math_style::MathStyle;
use ratex_types::path_command::PathCommand;

/// An axis-aligned box in em, y down, the formula's top at 0 and its
/// baseline at `DisplayList::height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    fn merge(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        (self.x0..=self.x1).contains(&x) && (self.y0..=self.y1).contains(&y)
    }

    fn area(&self) -> f64 {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }
}

/// The caret drawn at a stop: a vertical bar at `x` from `y0` to `y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub x: f64,
    pub y0: f64,
    pub y1: f64,
}

/// A typeset formula and where its parts are.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The display list, recoloured: what to draw.
    pub list: DisplayList,
    /// Each atom's box, ink and slots together. `None` for an atom with no
    /// ink (a space).
    pub atom_box: Vec<Option<Rect>>,
    /// Each row's box: its atoms', or its placeholder's when empty.
    pub row_box: Vec<Option<Rect>>,
    /// The probe TeX that was typeset, for debugging.
    pub probe: String,
}

/// Atom ids are colours `1..`; a row's placeholder is `ROW_BIT | row + 1`.
const ROW_BIT: u32 = 1 << 23;

fn atom_colour(id: usize) -> String {
    format!("#{:06x}", id as u32 + 1)
}

fn row_colour(id: usize) -> String {
    format!("#{:06x}", ROW_BIT | (id as u32 + 1))
}

/// Switches that act on what follows them in their group. Wrapping one would
/// end its scope at the wrapper, so they are written bare.
fn is_switch(name: &str) -> bool {
    matches!(
        name,
        "displaystyle"
            | "textstyle"
            | "scriptstyle"
            | "scriptscriptstyle"
            | "rm"
            | "bf"
            | "it"
            | "sf"
            | "tt"
            | "cal"
            | "tiny"
            | "small"
            | "large"
            | "Large"
            | "LARGE"
            | "huge"
            | "Huge"
            | "normalsize"
            | "\\"
            | "cr"
            | "newline"
    )
}

impl Formula {
    /// The formula as probe TeX: the same layout, with every atom's ink in a
    /// colour that names it.
    pub fn probe_tex(&self) -> String {
        let mut out = String::new();
        self.probe_row(0, &mut out);
        out
    }

    fn probe_row(&self, row: usize, out: &mut String) {
        let atoms = &self.rows[row].atoms;
        if atoms.is_empty() {
            out.push_str(&format!(
                "\\htmlStyle{{color: {}}}{{\\square}}",
                row_colour(row)
            ));
            return;
        }
        let mut i = 0;
        while i < atoms.len() {
            let a = atoms[i];
            let atom = &self.atoms[a];
            // A base and the scripts after it are one unit to TeX; they are
            // wrapped together in the base's colour, each script's own atoms
            // in theirs.
            let mut j = i + 1;
            if !atom.kind.is_script() {
                while j < atoms.len() && self.atoms[atoms[j]].kind.is_script() {
                    j += 1;
                }
            }
            let bare = match &atom.kind {
                Kind::Symbol(s) => is_switch(s),
                Kind::Char('&') => true,
                _ => false,
            };
            if bare || atom.kind.is_script() {
                // A script with no base is written as it stands; its row's
                // atoms carry the colour.
                for &s in &atoms[i..j] {
                    self.probe_atom(s, out);
                }
            } else {
                out.push_str(&format!("\\htmlStyle{{color: {}}}{{", atom_colour(a)));
                for &s in &atoms[i..j] {
                    self.probe_atom(s, out);
                }
                out.push('}');
            }
            i = j;
        }
    }

    fn probe_atom(&self, a: usize, out: &mut String) {
        let atom = &self.atoms[a];
        let src = &self.src;
        match &atom.kind {
            Kind::Sup | Kind::Sub => {
                out.push(if atom.kind == Kind::Sup { '^' } else { '_' });
                out.push('{');
                self.probe_row(atom.rows[0], out);
                out.push('}');
            }
            Kind::Group => {
                out.push('{');
                self.probe_row(atom.rows[0], out);
                out.push('}');
            }
            Kind::Command(name) => {
                out.push('\\');
                out.push_str(name);
                let mut slots = atom.rows.iter();
                if name == "sqrt" && atom.rows.len() == 2 {
                    out.push('[');
                    self.probe_row(*slots.next().unwrap(), out);
                    out.push(']');
                }
                for &r in slots {
                    out.push('{');
                    self.probe_row(r, out);
                    out.push('}');
                }
            }
            Kind::LeftRight => {
                let body = &self.rows[atom.rows[0]];
                out.push_str(&src[atom.span.start..body.content.start]);
                out.push(' ');
                self.probe_row(atom.rows[0], out);
                out.push(' ');
                out.push_str(&src[body.content.end..atom.span.end]);
            }
            // Separated from what follows, so `\alpha` before a wrapper's
            // `}` or a letter cannot run on.
            _ => {
                out.push_str(&src[atom.span.clone()]);
                if !matches!(atom.kind, Kind::Char(_)) {
                    out.push(' ');
                }
            }
        }
    }

    /// Typeset the formula and measure it. `display` picks display style.
    pub fn layout(&self, display: bool) -> Result<Layout, String> {
        let probe = self.probe_tex();
        let ast = ratex_parser::parse(&probe).map_err(|e| e.message)?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let opts = LayoutOptions::default().with_style(style);
        let mut list = to_display_list(&layout(&ast, &opts));

        let mut own: Vec<Option<Rect>> = vec![None; self.atoms.len()];
        let mut placeholder: Vec<Option<Rect>> = vec![None; self.rows.len()];
        for item in &mut list.items {
            let (colour, bounds) = item_bounds(item);
            let code = colour_code(colour);
            if code & ROW_BIT != 0 {
                let r = (code & !ROW_BIT) as usize;
                if let Some(slot) = r.checked_sub(1).and_then(|r| placeholder.get_mut(r)) {
                    *slot = Rect::merge(*slot, Some(bounds));
                }
            } else if let Some(slot) = (code as usize).checked_sub(1).and_then(|a| own.get_mut(a)) {
                *slot = Rect::merge(*slot, Some(bounds));
            }
        }

        let mut atom_box = vec![None; self.atoms.len()];
        let mut row_box = vec![None; self.rows.len()];
        self.measure_row(0, &own, &placeholder, &mut atom_box, &mut row_box);
        Ok(Layout {
            list,
            atom_box,
            row_box,
            probe,
        })
    }

    fn measure_row(
        &self,
        row: usize,
        own: &[Option<Rect>],
        placeholder: &[Option<Rect>],
        atom_box: &mut [Option<Rect>],
        row_box: &mut [Option<Rect>],
    ) -> Option<Rect> {
        let mut b = placeholder[row];
        for &a in &self.rows[row].atoms {
            let mut ab = own[a];
            for &r in &self.atoms[a].rows {
                ab = Rect::merge(ab, self.measure_row(r, own, placeholder, atom_box, row_box));
            }
            atom_box[a] = ab;
            b = Rect::merge(b, ab);
        }
        row_box[row] = b;
        b
    }
}

fn colour_code(c: Color) -> u32 {
    let byte = |v: f32| (v * 255.0).round() as u32;
    (byte(c.r) << 16) | (byte(c.g) << 8) | byte(c.b)
}

fn item_bounds(item: &DisplayItem) -> (Color, Rect) {
    match item {
        DisplayItem::GlyphPath {
            x,
            y,
            scale,
            font,
            char_code,
            color,
        } => {
            let m = FontId::parse(font)
                .and_then(|f| ratex_font::get_char_metrics_with_fallback(f, *char_code));
            let (w, h, d) = m.map_or((0.5, 0.7, 0.0), |m| (m.width + m.italic, m.height, m.depth));
            let r = Rect {
                x0: *x,
                y0: y - h * scale,
                x1: x + w * scale,
                y1: y + d * scale,
            };
            (*color, r)
        }
        DisplayItem::Line {
            x,
            y,
            width,
            thickness,
            color,
            ..
        } => (
            *color,
            Rect {
                x0: *x,
                y0: y - thickness / 2.0,
                x1: x + width,
                y1: y + thickness / 2.0,
            },
        ),
        DisplayItem::Rect {
            x,
            y,
            width,
            height,
            color,
        } => (
            *color,
            Rect {
                x0: *x,
                y0: *y,
                x1: x + width,
                y1: y + height,
            },
        ),
        DisplayItem::Path {
            x,
            y,
            commands,
            color,
            ..
        } => {
            let mut r = Rect {
                x0: *x,
                y0: *y,
                x1: *x,
                y1: *y,
            };
            for c in commands {
                let pts: &[(f64, f64)] = &match *c {
                    PathCommand::MoveTo { x: px, y: py } | PathCommand::LineTo { x: px, y: py } => {
                        vec![(px, py)]
                    }
                    PathCommand::CubicTo {
                        x1,
                        y1,
                        x2,
                        y2,
                        x: px,
                        y: py,
                    } => {
                        vec![(x1, y1), (x2, y2), (px, py)]
                    }
                    PathCommand::QuadTo {
                        x1,
                        y1,
                        x: px,
                        y: py,
                    } => vec![(x1, y1), (px, py)],
                    PathCommand::Close => vec![],
                };
                for &(px, py) in pts {
                    r = r.union(Rect {
                        x0: x + px,
                        y0: y + py,
                        x1: x + px,
                        y1: y + py,
                    });
                }
            }
            (*color, r)
        }
    }
}

fn set_colour(item: &mut DisplayItem, c: Color) {
    match item {
        DisplayItem::GlyphPath { color, .. }
        | DisplayItem::Line { color, .. }
        | DisplayItem::Rect { color, .. }
        | DisplayItem::Path { color, .. } => *color = c,
    }
}

impl Layout {
    /// Where the caret at `c` is drawn.
    pub fn caret(&self, f: &Formula, c: Caret) -> CaretRect {
        let row = &f.rows[c.row];
        let rb = self.row_box[c.row].unwrap_or(Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: self.list.height,
        });
        // Prefer the right edge of the atom before: between a base and its
        // script that is the base's edge, not the script's.
        let before = c
            .index
            .checked_sub(1)
            .and_then(|i| (0..=i).rev().find_map(|j| self.atom_box[row.atoms[j]]));
        let after = row.atoms[c.index..].iter().find_map(|&a| self.atom_box[a]);
        let x = match (before, after) {
            (Some(b), _) => b.x1,
            (None, Some(a)) => a.x0,
            (None, None) => rb.x0,
        };
        // At least as tall as an x-height line, so a caret in an empty
        // script is still a visible bar.
        let (mut y0, mut y1) = (rb.y0, rb.y1);
        if y1 - y0 < 0.6 {
            let mid = (y0 + y1) / 2.0;
            y0 = mid - 0.3;
            y1 = mid + 0.3;
        }
        CaretRect { x, y0, y1 }
    }

    /// The stop nearest the point `(x, y)` in em: on the innermost row whose
    /// box holds the point, the stop with the nearest caret.
    pub fn hit(&self, f: &Formula, x: f64, y: f64) -> Caret {
        let row = (0..f.rows.len())
            .filter(|&r| self.row_box[r].is_some_and(|b| b.contains(x, y)))
            .min_by(|&a, &b| {
                self.row_box[a]
                    .unwrap()
                    .area()
                    .total_cmp(&self.row_box[b].unwrap().area())
            })
            .unwrap_or(0);
        f.stops
            .iter()
            .copied()
            .filter(|c| c.row == row)
            .min_by(|&a, &b| {
                let da = (self.caret(f, a).x - x).abs();
                let db = (self.caret(f, b).x - x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(f.end())
    }

    /// The stop above (`up`) or below the caret at `c`: among stops whose
    /// caret lies wholly on that side, the nearest, weighing horizontal
    /// distance over vertical. `None` when there is nothing there.
    pub fn vertical(&self, f: &Formula, c: Caret, up: bool) -> Option<Caret> {
        let here = self.caret(f, c);
        let mid = (here.y0 + here.y1) / 2.0;
        f.stops
            .iter()
            .copied()
            .filter_map(|s| {
                let r = self.caret(f, s);
                let m = (r.y0 + r.y1) / 2.0;
                let gap = if up { here.y0 - r.y1 } else { r.y0 - here.y1 };
                let beyond = if up { m < mid - 0.05 } else { m > mid + 0.05 };
                (beyond && gap > -0.25).then(|| (s, (r.x - here.x).abs() + gap.max(0.0) * 0.5))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(s, _)| s)
    }

    /// The formula as a standalone SVG at `size` px per em, with a 4 px
    /// margin and the demo's colours — what `demo/` draws.
    pub fn svg(&self, f: &Formula, size: f64, caret: Option<Caret>) -> String {
        let style = Style {
            size,
            pad: 4.0,
            ..Style::default()
        };
        self.picture(f, &style, caret).svg
    }

    /// The formula drawn for a host: ink, empty slots and the caret in the
    /// style's colours, with the metrics a host places the picture by —
    /// `leaf-math`'s `MathPicture` shape, so an editor swaps one for the
    /// other in place.
    pub fn picture(&self, f: &Formula, style: &Style, caret: Option<Caret>) -> Picture {
        self.picture_selecting(f, style, caret, None)
    }

    /// The formula drawn without its caret or selection, for a host that
    /// draws those itself: the picture, and where the caret at `caret` and
    /// the atoms `selection` names stand on it, in em from its top left,
    /// margin included. The caret is a zero-width rect at the stop, as tall
    /// as its row.
    pub fn picture_marked(
        &self,
        f: &Formula,
        style: &Style,
        caret: Caret,
        selection: Option<(usize, std::ops::Range<usize>)>,
    ) -> (Picture, Mark, Vec<Mark>) {
        let picture = self.picture_selecting(f, style, None, None);
        let pad = style.pad / style.size;
        let r = self.caret(f, caret);
        let caret = Mark {
            x: r.x + pad,
            y: r.y0 + pad,
            width: 0.0,
            height: r.y1 - r.y0,
        };
        let selected = selection
            .and_then(|(row, range)| {
                f.rows[row].atoms[range]
                    .iter()
                    .fold(None, |acc, &a| Rect::merge(acc, self.atom_box[a]))
            })
            .map(|b| Mark {
                x: b.x0 + pad,
                y: b.y0 + pad,
                width: b.x1 - b.x0,
                height: b.y1 - b.y0,
            });
        (picture, caret, selected.into_iter().collect())
    }

    /// [`picture`](Self::picture) with the atoms `selection` names — a row
    /// and a range of its atoms, [`Formula::selection`]'s answer — shaded in
    /// the accent behind the ink.
    pub fn picture_selecting(
        &self,
        f: &Formula,
        style: &Style,
        caret: Option<Caret>,
        selection: Option<(usize, std::ops::Range<usize>)>,
    ) -> Picture {
        let (size, pad) = (style.size, style.pad);
        let mut list = self.list.clone();
        let rgba = |c: [u8; 4]| {
            let [r, g, b, a] = c.map(|v| f32::from(v) / 255.0);
            Color::new(r, g, b, a)
        };
        let (ink, faint) = (rgba(style.ink), rgba(style.faint));
        for item in &mut list.items {
            let (c, _) = item_bounds(item);
            set_colour(
                item,
                if colour_code(c) & ROW_BIT != 0 {
                    faint
                } else {
                    ink
                },
            );
        }
        let mut svg = in_pixels(render_to_svg_with_color_syntax(
            &list,
            &SvgOptions {
                font_size: size,
                padding: pad,
                stroke_width: (size / 16.0).max(0.5),
                embed_glyphs: true,
                font_dir: String::new(),
            },
            SvgColorSyntax::Rgb,
        ));
        let shade = selection.and_then(|(row, range)| {
            f.rows[row].atoms[range]
                .iter()
                .fold(None, |acc, &a| Rect::merge(acc, self.atom_box[a]))
        });
        if let (Some(b), Some(head_end)) = (shade, svg.find('>')) {
            let [cr, cg, cb, _] = style.accent;
            let rect = format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="rgb({cr},{cg},{cb})" fill-opacity="0.25"/>"#,
                b.x0 * size + pad,
                b.y0 * size + pad,
                (b.x1 - b.x0) * size,
                (b.y1 - b.y0) * size,
            );
            svg.insert_str(head_end + 1, &rect);
        }
        if let Some(caret) = caret {
            let r = self.caret(f, caret);
            let w = (size * 0.06).max(1.5);
            let full = self.list.width * size + 2.0 * pad;
            // Inside the picture, so a caret at either end is not clipped by
            // a host that draws it without a margin.
            let x = (r.x * size + pad - w / 2.0).clamp(0.0, (full - w).max(0.0));
            let [cr, cg, cb, _] = style.accent;
            let bar = format!(
                r#"<rect x="{x:.2}" y="{:.2}" width="{w:.2}" height="{:.2}" fill="rgb({cr},{cg},{cb})"/>"#,
                r.y0 * size + pad,
                (r.y1 - r.y0) * size,
            );
            if let Some(i) = svg.rfind("</svg>") {
                svg = format!("{}{bar}</svg>", &svg[..i]);
            }
        }
        Picture {
            svg,
            width: self.list.width + 2.0 * pad / size,
            height: self.list.height + pad / size,
            depth: self.list.depth + pad / size,
        }
    }
}

/// How [`Layout::picture`] draws.
#[derive(Clone, Debug)]
pub struct Style {
    /// Pixels per em.
    pub size: f64,
    /// Margin on every side, in pixels.
    pub pad: f64,
    pub ink: [u8; 4],
    /// Empty slots' placeholders.
    pub faint: [u8; 4],
    /// The caret.
    pub accent: [u8; 4],
}

impl Default for Style {
    fn default() -> Self {
        Style {
            size: 16.0,
            pad: 0.0,
            ink: [26, 26, 31, 255],
            faint: [150, 150, 160, 255],
            accent: [37, 99, 235, 255],
        }
    }
}

/// A rect on a drawn formula — the caret, or a selection — in em from the
/// picture's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A drawn formula: a standalone SVG and, in em, its advance width and its
/// extent above and below the baseline — margin included.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}

/// `ratex-svg` sizes the root in `pt`; a host places the picture in pixels.
fn in_pixels(svg: String) -> String {
    let Some(head_end) = svg.find('>') else {
        return svg;
    };
    let (head, rest) = svg.split_at(head_end);
    let mut head = head.to_string();
    for attr in ["width=\"", "height=\""] {
        if let Some(i) = head.find(attr) {
            let v = i + attr.len();
            if let Some(q) = head[v..].find('"') {
                let value = head[v..v + q].trim_end_matches("pt").to_string();
                head.replace_range(v..v + q, &value);
            }
        }
    }
    head.push_str(rest);
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(list: &DisplayList) -> Vec<String> {
        let mut v: Vec<String> = list
            .items
            .iter()
            .map(|i| {
                let (_, r) = item_bounds(i);
                format!("{:.4} {:.4} {:.4} {:.4}", r.x0, r.y0, r.x1, r.y1)
            })
            .collect();
        v.push(format!(
            "{:.4} {:.4} {:.4}",
            list.width, list.height, list.depth
        ));
        v
    }

    /// The probe is only sound if it is invisible to layout: the wrapped
    /// formula must put every item exactly where the plain one does.
    #[test]
    fn the_probe_does_not_move_anything() {
        let corpus = [
            "a+b",
            "-x",
            "a=-b",
            "x^2",
            r"x_i^2",
            r"\frac{a}{b}+1",
            r"\frac12",
            r"\sin x",
            r"\sum_{i=1}^n i",
            r"\sum\limits_{i=1}^n i",
            r"\int_0^1 x\,dx",
            "f(x)",
            "a,b",
            r"\left( \frac{a}{b} \right)^2",
            r"\sqrt[3]{x+1}",
            r"\hat{x} \cdot \vec{v}",
            r"\text{if } x > 0",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"e^{i\pi} + 1 = 0",
            r"\alpha\beta\gamma",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
            r"{n \choose k}",
            r"x' + y''",
            r"\displaystyle \sum_i x_i",
            r"a \le b \pm c",
            r"\mathbb{R}^n",
            r"\binom{n}{k}",
            r"\operatorname{sinc} x",
        ];
        for display in [false, true] {
            for tex in corpus {
                let style = if display {
                    MathStyle::Display
                } else {
                    MathStyle::Text
                };
                let opts = LayoutOptions::default().with_style(style);
                let plain = to_display_list(&layout(&ratex_parser::parse(tex).unwrap(), &opts));
                let f = Formula::new(tex).unwrap();
                let probed = f.layout(display).unwrap();
                assert_eq!(
                    geometry(&plain),
                    geometry(&probed.list),
                    "{tex} (display {display})\nprobe: {}",
                    probed.probe
                );
            }
        }
    }

    #[test]
    fn every_inked_atom_gets_a_box() {
        let f = Formula::new(r"\frac{a}{b}+x^2").unwrap();
        let l = f.layout(false).unwrap();
        for (i, a) in f.atoms.iter().enumerate() {
            assert!(l.atom_box[i].is_some(), "{:?}", a);
        }
        // The numerator sits above the denominator.
        let (num, den) = (f.atoms[0].rows[0], f.atoms[0].rows[1]);
        assert!(l.row_box[num].unwrap().y1 <= l.row_box[den].unwrap().y0);
    }

    #[test]
    fn carets_advance_left_to_right_along_a_row() {
        let f = Formula::new("a+b=c").unwrap();
        let l = f.layout(false).unwrap();
        let xs: Vec<f64> = (0..=5)
            .map(|i| l.caret(&f, Caret { row: 0, index: i }).x)
            .collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    }

    #[test]
    fn up_and_down_cross_a_fraction_bar() {
        let f = Formula::new(r"\frac{ab}{c}").unwrap();
        let l = f.layout(false).unwrap();
        let (num, den) = (f.atoms[0].rows[0], f.atoms[0].rows[1]);
        let in_num = Caret { row: num, index: 1 };
        assert_eq!(l.vertical(&f, in_num, false).unwrap().row, den);
        let in_den = Caret { row: den, index: 0 };
        assert_eq!(l.vertical(&f, in_den, true).unwrap().row, num);
    }

    #[test]
    fn a_click_lands_in_the_slot_it_is_over() {
        let f = Formula::new(r"1+\frac{a}{b}").unwrap();
        let l = f.layout(false).unwrap();
        let den = f.atoms[2].rows[1];
        let b = l.row_box[den].unwrap();
        let c = l.hit(&f, b.x1 + 0.01, (b.y0 + b.y1) / 2.0);
        // Just right of `b`'s ink is still over the denominator row? No —
        // the row box ends at the ink; aim inside it instead.
        let c2 = l.hit(&f, b.x1 - 0.01, (b.y0 + b.y1) / 2.0);
        assert_eq!(c2.row, den);
        let _ = c;
    }
}
