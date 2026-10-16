//! The editor widget: a `leaf_core::Doc`, the layout of it, and the two
//! directions across the Ply boundary — input events in, Ply elements out.

use leaf_core::{Doc, InlineKind, Role, View, VisualKey};
use ply_engine::prelude::*;

use crate::fonts::Fonts;
use crate::layout::{self, Kind, Line, Metrics};

/// An input event, already in the order it happened. Collected from
/// macroquad's input-subscriber stream rather than its per-frame polls:
/// `get_char_pressed` pops a `Vec` from the end, so two characters typed in one
/// frame come out reversed, and `is_key_pressed` drops the OS key repeat.
#[derive(Clone, Debug)]
pub enum Ev {
    Char(char, Mods),
    Key(KeyCode, Mods),
    MouseDown(f32, f32, Mods),
    MouseMove(f32, f32),
    MouseUp,
    Wheel(f32),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Mods {
    /// The command modifier: ⌘ on macOS, Ctrl elsewhere — either is taken.
    fn cmd(self) -> bool {
        self.ctrl || self.logo
    }
}

/// What the host owns: the editor reports it, the host acts.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Quit,
}

const PAGE: &str = "leaf-page";
const COLUMN: f32 = 720.0;
const MARGIN: f32 = 32.0;

pub struct Theme {
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub link: u32,
    pub caret: u32,
    pub selection: (u8, u8, u8, u8),
    pub code_bg: u32,
    pub mark_bg: u32,
    pub mark_colors: [u32; 7],
    pub syntax: [u32; 8],
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            background: 0xFFFFFF,
            text: 0x1F2328,
            muted: 0x8C959F,
            link: 0x0969DA,
            caret: 0x2F6FEB,
            selection: (47, 111, 235, 64),
            code_bg: 0xF3F4F6,
            mark_bg: 0xFFF1A8,
            mark_colors: [
                0xFFD6D6, 0xFFE3C2, 0xFFF1A8, 0xD4F5D4, 0xD3E7FF, 0xE6D9FF, 0xF2F2F2,
            ],
            // punctuation, keyword, entity, support, constant, string, comment, invalid
            syntax: [
                0x57606A, 0xCF222E, 0x8250DF, 0x0550AE, 0x0550AE, 0x0A3069, 0x6E7781, 0xCF222E,
            ],
        }
    }
}

pub struct Editor {
    pub doc: Doc,
    fonts: Fonts,
    metrics: Metrics,
    theme: Theme,
    lines: Vec<Line>,
    /// What `lines` was laid out from; a different key relayouts.
    key: Option<(u64, View, u32, Option<VisualKey>)>,
    scroll: f32,
    /// The page's box as Ply laid it out last frame.
    page: (f32, f32, f32, f32),
    dragging: bool,
    last_click: Option<(f64, usize)>,
    /// Bring the caret into view after this frame's layout.
    reveal: bool,
    blink_epoch: f64,
    /// Up/Down's sticky x, valid while the caret is still where it left it.
    goal: Option<(f32, usize)>,
    pub last_layout_ms: f64,
    /// The last time leaf-core's map build took over a millisecond.
    pub last_core_ms: f64,
}

impl Editor {
    pub fn new(mut doc: Doc, fonts: Fonts) -> Editor {
        // A fresh document's caret is byte 0, which may be inside front matter
        // that no row shows; the model's own doc-start puts it on the first stop.
        doc.move_doc_start(false);
        Editor {
            doc,
            fonts,
            metrics: Metrics::default(),
            theme: Theme::default(),
            lines: Vec::new(),
            key: None,
            scroll: 0.0,
            page: (0.0, 0.0, 800.0, 600.0),
            dragging: false,
            last_click: None,
            reveal: false,
            blink_epoch: 0.0,
            goal: None,
            last_layout_ms: 0.0,
            last_core_ms: 0.0,
        }
    }

    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    fn column_width(&self) -> f32 {
        (self.page.2 - 2.0 * MARGIN).clamp(120.0, COLUMN)
    }

    /// The column's left edge and the top of the document, in window pixels.
    pub fn origin(&self) -> (f32, f32) {
        let (px, py, pw, _) = self.page;
        (
            px + (pw - self.column_width()) / 2.0,
            py + MARGIN - self.scroll,
        )
    }

    fn relayout(&mut self) {
        let width = self.column_width();
        // The rendered map depends on the caret too (markup is revealed around
        // it), so it is rebuilt — core caches that on its own key, which is
        // what `visual_key` names — and the wrap is redone only when that key,
        // the text, or the width moved.
        let t = std::time::Instant::now();
        let visual = (self.doc.view == View::Wysiwyg).then(|| {
            self.doc.set_inline_pictures(false);
            self.doc.build_visual_unwrapped();
            self.doc.visual_key()
        });
        let core_ms = t.elapsed().as_secs_f64() * 1000.0;
        if core_ms > 1.0 {
            self.last_core_ms = core_ms;
        }
        let key = (self.doc.revision(), self.doc.view, width.to_bits(), visual);
        if self.key.as_ref() != Some(&key) {
            let t = std::time::Instant::now();
            self.lines = layout::layout(&mut self.doc, &mut self.fonts, &self.metrics, width);
            self.last_layout_ms = t.elapsed().as_secs_f64() * 1000.0;
            self.key = Some(key);
        }
    }

    fn offset_at(&self, wx: f32, wy: f32) -> Option<usize> {
        let (ox, oy) = self.origin();
        layout::hit(&self.lines, wx - ox, wy - oy)
    }

    pub fn handle(&mut self, events: &[Ev], now: f64) -> Outcome {
        for ev in events {
            if let Some(out) = self.handle_one(ev, now) {
                return out;
            }
        }
        Outcome::Continue
    }

    fn handle_one(&mut self, ev: &Ev, now: f64) -> Option<Outcome> {
        // Anything that moves the caret restarts its blink and scrolls to it.
        let mut moved = true;
        match *ev {
            Ev::Char(ch, m) => {
                if ch.is_control() || m.cmd() {
                    return None;
                }
                self.doc.insert(ch.encode_utf8(&mut [0; 4]));
            }
            Ev::Key(k, m) => {
                let ext = m.shift;
                let word = m.alt || (m.ctrl && !m.logo);
                match k {
                    KeyCode::Left if word => self.doc.move_word_left(ext),
                    KeyCode::Right if word => self.doc.move_word_right(ext),
                    KeyCode::Left => self.doc.move_left(ext),
                    KeyCode::Right => self.doc.move_right(ext),
                    KeyCode::Up => self.vertical(-1, ext),
                    KeyCode::Down => self.vertical(1, ext),
                    KeyCode::Home if m.cmd() => self.doc.move_doc_start(ext),
                    KeyCode::End if m.cmd() => self.doc.move_doc_end(ext),
                    KeyCode::Home => self.doc.move_home(ext),
                    KeyCode::End => self.doc.move_end(ext),
                    KeyCode::Backspace => self.doc.backspace(),
                    KeyCode::Delete => self.doc.delete_forward(),
                    KeyCode::Enter | KeyCode::KpEnter => self.doc.newline(),
                    KeyCode::Tab if m.shift => self.doc.outdent(),
                    KeyCode::Tab => self.doc.indent(),
                    KeyCode::PageUp => {
                        self.scroll -= self.page.3 * 0.9;
                        moved = false;
                    }
                    KeyCode::PageDown => {
                        self.scroll += self.page.3 * 0.9;
                        moved = false;
                    }
                    _ if m.cmd() => return self.command(k, m),
                    _ => moved = false,
                }
            }
            Ev::MouseDown(x, y, m) => {
                if let Some(off) = self.offset_at(x, y) {
                    // Two presses on the same spot inside 400ms select a word.
                    let double = self
                        .last_click
                        .is_some_and(|(t, o)| now - t < 0.4 && o == off);
                    if double {
                        self.doc.select_word_at(off);
                        self.last_click = None;
                    } else {
                        self.doc.place_caret(off, m.shift);
                        self.last_click = Some((now, off));
                        self.dragging = true;
                    }
                }
            }
            Ev::MouseMove(x, y) => {
                moved = false;
                if self.dragging
                    && let Some(off) = self.offset_at(x, y)
                {
                    self.doc.place_caret(off, true);
                    moved = true;
                }
            }
            Ev::MouseUp => {
                self.dragging = false;
                moved = false;
            }
            Ev::Wheel(dy) => {
                self.scroll -= dy * 40.0;
                moved = false;
            }
        }
        if moved {
            self.reveal = true;
            self.blink_epoch = now;
        }
        None
    }

    /// Up/Down over *visual* lines. leaf-core's `move_up`/`move_down` step
    /// through its own rows, and `build_visual_unwrapped` makes a whole
    /// paragraph one row, so a frontend that wraps by pixels moves vertically
    /// itself — as leaf-gpui does in `vertical_target`.
    fn vertical(&mut self, dir: i32, extend: bool) {
        let caret = self.doc.caret;
        let x = match self.goal {
            Some((x, at)) if at == caret => x,
            _ => match layout::caret_at(&self.lines, caret) {
                Some((_, x)) => x,
                None => return,
            },
        };
        match layout::vertical(&self.lines, caret, dir, x) {
            Some(off) => self.doc.place_caret(off, extend),
            // Off either end: the model's own edge move.
            None if dir < 0 => self.doc.move_doc_start(extend),
            None => self.doc.move_doc_end(extend),
        }
        self.goal = Some((x, self.doc.caret));
    }

    fn command(&mut self, k: KeyCode, m: Mods) -> Option<Outcome> {
        match k {
            KeyCode::Z if m.shift => {
                self.doc.redo();
            }
            KeyCode::Z => {
                self.doc.undo();
            }
            KeyCode::Y => {
                self.doc.redo();
            }
            KeyCode::B => self.doc.toggle(InlineKind::Strong),
            KeyCode::I => self.doc.toggle(InlineKind::Emph),
            KeyCode::E => self.doc.toggle_view(),
            KeyCode::A => self.doc.select_all(),
            KeyCode::S => self.doc.save(),
            KeyCode::Q => return Some(Outcome::Quit),
            KeyCode::Key1 => self.doc.toggle_heading(1),
            KeyCode::Key2 => self.doc.toggle_heading(2),
            KeyCode::Key3 => self.doc.toggle_heading(3),
            // Plain text only: miniquad's clipboard has no HTML flavor, so the
            // rich paste leaf-gpui gets through arboard isn't reachable here.
            KeyCode::C | KeyCode::X => {
                if let Some(text) = self.doc.selected_text() {
                    macroquad::miniquad::window::clipboard_set(text);
                }
                if k == KeyCode::X {
                    self.doc.backspace();
                }
            }
            KeyCode::V => {
                if let Some(text) = macroquad::miniquad::window::clipboard_get() {
                    self.doc.paste(&text);
                }
            }
            _ => {}
        }
        None
    }

    /// Lay out (if anything moved) and emit this frame's elements. Call between
    /// `ply.begin()` and `ui.show()`.
    pub fn ui(&mut self, ui: &mut Ui, now: f64) {
        if let Some(b) = ui.bounding_box(PAGE) {
            self.page = (b.x, b.y, b.width, b.height);
        }
        self.relayout();

        let content_h = layout::content_height(&self.lines) + 2.0 * MARGIN;
        if self.reveal {
            if let Some((i, _)) = layout::caret_at(&self.lines, self.doc.caret) {
                let l = &self.lines[i];
                let (top, bottom) = (l.y + MARGIN, l.y + l.height + MARGIN);
                if top < self.scroll {
                    self.scroll = top - MARGIN;
                } else if bottom > self.scroll + self.page.3 {
                    self.scroll = bottom - self.page.3 + MARGIN;
                }
            }
            self.reveal = false;
        }
        self.scroll = self.scroll.clamp(0.0, (content_h - self.page.3).max(0.0));

        let theme = &self.theme;
        let lines = &self.lines;
        let (ox, oy) = self.origin();
        // Offsets are relative to the page element, which floats its children.
        let (lx, ly) = (ox - self.page.0, oy - self.page.1);
        let view_h = self.page.3;
        let col_w = self.column_width();
        let sel = self.doc.selection();
        let caret = layout::caret_at(lines, self.doc.caret);
        let blink_on = (((now - self.blink_epoch) / 0.5) as u64).is_multiple_of(2);

        let status = format!(
            "{}  ·  {}  ·  {} lines  ·  layout {:.1} ms  ·  Ctrl+E view · Ctrl+B/I · Ctrl+Z undo · Ctrl+S save · Ctrl+Q quit",
            self.doc
                .path
                .file_name()
                .map_or("untitled".into(), |n| n.to_string_lossy()),
            if self.doc.view == View::Wysiwyg {
                "rendered"
            } else {
                "source"
            },
            lines.len(),
            self.last_layout_ms,
        );

        ui.element()
            .width(grow!())
            .height(grow!())
            .background_color(theme.background)
            .layout(|l| l.direction(TopToBottom))
            .children(|ui| {
                ui.element()
                    .id(PAGE)
                    .width(grow!())
                    .height(grow!())
                    .overflow(|o| o.clip())
                    .children(|ui| {
                        // Code blocks' tint first, then text, then selection and caret.
                        for l in visible(lines, ly, view_h) {
                            if l.kind == Kind::Code {
                                rect(
                                    ui,
                                    lx - 10.0,
                                    ly + l.y,
                                    col_w + 20.0,
                                    l.height,
                                    theme.code_bg,
                                );
                            }
                        }
                        for l in visible(lines, ly, view_h) {
                            draw_line(ui, &self.fonts, theme, l, lx, ly);
                        }
                        if let Some((s, e)) = sel {
                            for l in visible(lines, ly, view_h) {
                                for (x, w) in selection_spans(l, s, e) {
                                    rect(ui, lx + x, ly + l.y, w, l.height, theme.selection);
                                }
                            }
                        }
                        if let Some((i, x)) = caret.filter(|_| blink_on) {
                            let l = &lines[i];
                            let h = l.height * 0.8;
                            rect(
                                ui,
                                lx + x - 1.0,
                                ly + l.y + (l.height - h) / 2.0,
                                2.0,
                                h,
                                theme.caret,
                            );
                        }
                    });
                ui.element()
                    .width(grow!())
                    .height(fixed!(28.0))
                    .background_color(0xF6F8FA)
                    .layout(|l| l.padding((6, 12, 6, 12)).align(Left, CenterY))
                    .children(|ui| {
                        ui.text(&status, |t| {
                            t.font_size(13).color(theme.muted).wrap_mode(WrapMode::None)
                        });
                    });
            });
    }
}

fn visible(lines: &[Line], ly: f32, view_h: f32) -> impl Iterator<Item = &Line> {
    lines
        .iter()
        .filter(move |l| ly + l.y + l.height >= 0.0 && ly + l.y <= view_h)
}

fn rect(ui: &mut Ui, x: f32, y: f32, w: f32, h: f32, color: impl Into<ply_engine::Color>) {
    ui.element()
        .width(fixed!(w.max(0.0)))
        .height(fixed!(h.max(0.0)))
        .floating(|f| {
            f.attach_parent()
                .offset((x, y))
                .clip_by_parent()
                .passthrough()
        })
        .background_color(color)
        .empty();
}

fn draw_line(ui: &mut Ui, fonts: &Fonts, theme: &Theme, l: &Line, lx: f32, ly: f32) {
    for run in &l.runs {
        // A list item's later lines are indented by the marker's own glyphs,
        // measured but never seen.
        if run.style.role == Role::ListIndent {
            continue;
        }
        let x = lx + run.x;
        let top = ly + l.y;
        // Run backgrounds: highlights and inline code pills.
        let bg = match run.style.role {
            Role::Mark(Some(c)) => Some(theme.mark_colors[c.index()]),
            Role::Mark(None) => Some(theme.mark_bg),
            Role::Code if l.kind != Kind::Code => Some(theme.code_bg),
            _ => None,
        };
        if let Some(bg) = bg {
            rect(ui, x, top + l.height * 0.12, run.width, l.height * 0.76, bg);
        }
        let color = match (run.style.role, run.style.token) {
            (Role::Code, Some(t)) => theme.syntax[t.index()],
            (Role::Link, _) => theme.link,
            (Role::ListMarker | Role::QuoteGutter | Role::Rule | Role::Delimiter, _) => theme.muted,
            _ => theme.text,
        };
        // Ply draws the baseline at `element top + this face's ascent`; put
        // the element where that lands on the line's one shared baseline.
        let y = top + l.baseline - run.ascent;
        let asset = fonts.asset(run.face);
        let text = run.text.clone();
        let size = run.size;
        ui.element()
            .width(fit!())
            .height(fit!())
            .floating(|f| {
                f.attach_parent()
                    .offset((x, y))
                    .clip_by_parent()
                    .passthrough()
            })
            .children(|ui| {
                ui.text(&text, |t| {
                    t.font(asset)
                        .font_size(size)
                        .color(color)
                        .wrap_mode(WrapMode::None)
                });
            });
        if run.style.role == Role::Link || run.style.underline {
            rect(ui, x, top + l.baseline + 2.0, run.width, 1.0, color);
        }
        if run.style.strikethrough {
            rect(
                ui,
                x,
                top + l.baseline - run.ascent * 0.35,
                run.width,
                1.0,
                color,
            );
        }
    }
}

/// The x spans a selection `[s, e)` covers on a line: each selected character's
/// box, merged where they touch.
fn selection_spans(l: &Line, s: usize, e: usize) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    for c in &l.chars {
        if c.src >= s && c.src < e {
            match out.last_mut() {
                Some((x, w)) if (*x + *w - c.x).abs() < 0.5 => *w += c.advance,
                _ => out.push((c.x, c.advance)),
            }
        }
    }
    // A selection running on past the line's end shows a sliver there, so an
    // empty line inside it still reads as selected.
    if l.holds_caret() && !l.wrapped && l.end_src >= s && l.end_src < e {
        out.push((l.end_x, 6.0));
    }
    out
}
