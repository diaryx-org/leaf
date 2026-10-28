//! The page: the stylesheet, and the markup of the rows in view, the blocks
//! among them, the find bar and the status line.

use std::collections::BTreeSet;

use crate::bridge::style::STYLESHEET;
use dioxus::prelude::*;

use crate::blocks::{self, Block};
use crate::editor::{Editor, Status};
use crate::findbar;
use crate::row::{self, Row};
use crate::table::Rules;
use crate::text::{Grid, Type};
use crate::trace::trace;

/// Light or dark: the system's, or one of the two whatever the system says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// What the widget adds to the bridge's stylesheet: proportional body text
/// with code and tables in cells, in the type `text.rs` measures with;
/// `.leaf` as a viewport the editor scrolls itself; a known left edge for
/// [`row::PAD`]; the inline caret and a selection's grab handles
/// `row.rs` draws; tables; the find bar and
/// the status line.
///
/// A function of the [`Type`] because the faces, the size and the leading
/// are `text.rs`'s, and the two must never disagree: a face Blitz draws in
/// and parley does not measure is every click landing a few glyphs off. A
/// settings change or a zoom writes a new one, and the rows are laid out
/// again for it in the same repaint.
//
// Two Blitz behaviours shape this, both found the hard way:
//
// - It paints the first frame of an `overflow: hidden` element and not the
//   changes after it, so the clip is on `body`, never on `.leaf`.
// - It skips painting a child that hangs past its parent's box, so `.leaf`
//   has no fixed height: its box is as tall as the rows in it, shifted up by
//   the scroll, and the window cuts it off. The status line is pinned to the
//   bottom of the editor's box over it, and the find bar to its top, each in
//   a stacking layer of its own (the box is the window's, unless the host
//   gives it another: `Bounds`).
pub fn css(ty: &Type) -> String {
    let Type {
        body,
        mono,
        size,
        leading,
        grid,
        // Files, not rules: the host gives Blitz's collection the same ones.
        faces: _,
    } = ty;
    let quote = ty.quote_indent();
    let Rules {
        pad_x,
        pad_y,
        across,
        down,
        ..
    } = Rules::of(ty);
    let mut css = format!(
        "html, body {{ margin: 0; overflow: hidden; height: 100vh; background: var(--leaf-bg); }}
body {{ position: relative; }}
.leaf {{ padding: 0 16px; box-sizing: border-box; font-family: {body}; font-size: {size}px; line-height: {leading}; }}
.leaf .row.code, .leaf .row.table, .leaf .row.source, .leaf .code {{ font-family: {mono}; }}
.leaf .row.source .g {{ font-size: 1em; vertical-align: baseline; }}
.leaf .row, .leaf .line {{ min-height: {leading}em; }}
.leaf .qbar {{ display: inline-block; width: {quote}px; vertical-align: top; box-sizing: border-box;
               border-left: 3px solid currentColor; }}
.leaf .ghost {{ color: transparent; background: none; }}
.leaf .ime {{ text-decoration: underline; text-decoration-color: var(--leaf-accent); }}
.leaf .miss {{ text-decoration: underline wavy; text-decoration-color: var(--leaf-misspelled); }}
.leaf .table {{ box-sizing: border-box; }}
.leaf .trow {{ display: flex; box-sizing: border-box; }}
.leaf .trow.head .tcell {{ background: var(--leaf-table-head); }}
.leaf .trow:first-child .tcell {{ border-top: {down}px solid var(--leaf-separator); }}
.leaf .tprefix {{ flex: none; white-space: pre; }}
.leaf .tcell {{ flex: none; box-sizing: border-box; padding: {pad_y}px {pad_x}px;
                border-right: {across}px solid var(--leaf-separator); border-bottom: {down}px solid var(--leaf-separator); }}
.leaf .tprefix + .tcell {{ border-left: {across}px solid var(--leaf-separator); }}
.leaf .tcell.center {{ text-align: center; }}
.leaf .tcell.end {{ text-align: right; }}
{CHROME}{REST}"
    );
    if let Some(Grid {
        width: w,
        height: h,
    }) = grid
    {
        // Every run at the body size, as `text.rs` measures them; a rule
        // under a table's head and its last row only (`Rules::ruled`); and
        // the find bar and the status line in whole cells, a row each, in the
        // body's face, with no hairline, which would take a row of its own.
        let label = 7.0 * w;
        let count = 8.0 * w;
        css.push_str(&format!(
            ".leaf .h1, .leaf .h2, .leaf .h3, .leaf .sup, .leaf .sub {{ font-size: 1em; vertical-align: baseline; }}
.leaf .tcell {{ border-bottom-width: 0; }}
.leaf .trow.head .tcell, .leaf .trow:last-child .tcell {{ border-bottom-width: {down}px; }}
.status {{ height: {h}px; line-height: {h}px; padding: 0 {w}px; gap: {w}px; font: {size}px {mono}; border-top: 0; }}
.find {{ padding: 0 {w}px; font: {size}px {mono}; border-bottom: 0; }}
.find .find-row {{ height: {h}px; gap: {w}px; }}
.find .label {{ width: {label}px; }}
.find .count {{ min-width: {count}px; }}
.find .field, .find .btn {{ height: {h}px; line-height: {h}px; padding: 0 {w}px; border: 0; border-radius: 0; }}"
        ));
    }
    if std::env::var_os("LEAF_OUTLINE").is_some() {
        css.push_str(".leaf .line { outline: 1px solid #f00; } .leaf .line > span { outline: 1px solid #0a0; }");
    }
    css
}

/// Every rule the editor's page is drawn with, for `ty`: the bridge's and
/// [`css`]'s. What [`Palette::of`] reads to check a host's own chrome
/// against the custom properties the editor sets.
pub fn stylesheet(ty: &Type) -> String {
    format!("{STYLESHEET}{}", css(ty))
}

/// The stylesheet for an appearance. `System` leaves the dark palette to
/// `prefers-color-scheme`, which Blitz answers from winit's theme. A forced
/// one cannot go that way: winit sends no `ThemeChanged` for the app's own
/// `Window::set_theme` (its appearance observer ignores a window whose
/// appearance is set), so Blitz never hears of it — the dark block is
/// switched on or off here instead.
pub fn themed(css: &str, appearance: Appearance) -> String {
    const DARK: &str = "@media (prefers-color-scheme: dark)";
    match appearance {
        Appearance::System => css.to_string(),
        Appearance::Dark => css.replace(DARK, "@media all"),
        Appearance::Light => css.replace(DARK, "@media not all"),
    }
}

/// The window chrome's colours — the status line, the find bar, and a
/// host's own panels if it likes — beside the document's in the bridge's
/// stylesheet, and like them set once per appearance.
const CHROME: &str = "
:root {
  --chrome-bg: #f5f5f5;
  --chrome-text: #262626;
  --field-bg: #ffffff;
  --field-border: #c8c8c8;
}
@media (prefers-color-scheme: dark) {
  :root {
    --chrome-bg: #2a2a2a;
    --chrome-text: #dfdfdf;
    --field-bg: #1e1e1e;
    --field-border: #4d4d4d;
  }
}
";

const REST: &str = "
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid var(--leaf-accent); margin-right: -2px;
                  vertical-align: text-bottom; }
.leaf .grip { display: inline-block; width: 10px; margin: 0 -5px; vertical-align: text-bottom; }
.leaf .grip.start { margin-top: -10px; }
.leaf .grip.end { margin-bottom: -10px; }
.leaf .grip .stem { display: block; width: 2px; height: 1.2em; margin-left: 4px; background: var(--leaf-accent); }
.leaf .grip .knob { display: block; width: 10px; height: 10px; border-radius: 5px; background: var(--leaf-accent); }
.status { position: absolute; z-index: 1;
          height: 24px; line-height: 24px; padding: 0 16px; box-sizing: border-box;
          font: 12px ui-monospace, monospace; color: var(--leaf-secondary); background: var(--chrome-bg);
          border-top: 1px solid var(--leaf-separator); display: flex; gap: 16px; white-space: pre; }
.status .grow { flex: 1; }
.find { position: absolute; z-index: 1; box-sizing: border-box;
        padding: 0 16px; background: var(--chrome-bg); border-bottom: 1px solid var(--leaf-separator);
        font: 13px system-ui, sans-serif; color: var(--chrome-text); }
.find .find-row { display: flex; align-items: center; gap: 8px; height: 34px; }
.find .label { width: 56px; color: var(--leaf-secondary); }
.find .field { flex: 1; height: 22px; line-height: 22px; padding: 0 6px; white-space: pre;
               background: var(--field-bg); border: 1px solid var(--field-border); border-radius: 5px; overflow: hidden; }
.find .field.focused { border-color: var(--leaf-accent); }
.find .count { min-width: 72px; color: var(--leaf-secondary); text-align: right; }
.find .btn { height: 22px; line-height: 22px; padding: 0 9px; background: var(--field-bg);
             border: 1px solid var(--field-border); border-radius: 5px; }
.find .caret-in { display: inline-block; width: 0; height: 16px; vertical-align: text-bottom;
                  border-left: 2px solid var(--leaf-accent); margin-right: -2px; }";

/// The custom properties a stylesheet uses, and those it sets for each
/// appearance — so a host with chrome of its own, drawn in the editor's
/// `var(--…)`s and its own, can check that every one is set for both.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub used: BTreeSet<String>,
    pub light: BTreeSet<String>,
    pub dark: BTreeSet<String>,
}

impl Palette {
    /// Read off `css`, laid out as the editor's is: a light `:root { … }`,
    /// and a dark one first inside each `@media (prefers-color-scheme:
    /// dark)`.
    pub fn of(css: &str) -> Self {
        let names = |s: &str, after: &str| -> BTreeSet<String> {
            s.split(after)
                .skip(1)
                .map(|t| {
                    t.split(|c: char| !(c.is_alphanumeric() || c == '-'))
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        };
        let used = names(css, "var(--");
        let (mut light, mut dark) = (BTreeSet::new(), BTreeSet::new());
        // After each dark media query, the first `:root` block is dark;
        // every other is light.
        for (k, part) in css.split("@media (prefers-color-scheme: dark)").enumerate() {
            for (n, block) in part.split(":root {").skip(1).enumerate() {
                let body = block.split('}').next().unwrap_or("");
                let defined = body
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("--"))
                    .filter_map(|l| l.split(':').next())
                    .map(str::to_string);
                if k > 0 && n == 0 {
                    &mut dark
                } else {
                    &mut light
                }
                .extend(defined);
            }
        }
        Self { used, light, dark }
    }

    /// That both appearances set the same properties, and every property
    /// used is set.
    pub fn check(&self) -> Result<(), String> {
        if self.light != self.dark {
            let only: Vec<_> = self.light.symmetric_difference(&self.dark).collect();
            return Err(format!("set for one appearance only: {only:?}"));
        }
        let missing: Vec<_> = self.used.difference(&self.light).collect();
        if !missing.is_empty() {
            return Err(format!("used but never set: {missing:?}"));
        }
        Ok(())
    }
}

impl Editor {
    /// The editor's part of the page: its stylesheet, the rows in view, the
    /// find bar and the status line. A host draws its own beside it.
    pub fn render(&self) -> Element {
        render(self)
    }
}

/// The page, for the rows in view.
fn render(ed: &Editor) -> Element {
    let sig = ed.sig;
    let bar = sig.bar.read().clone();
    let bar_h = bar.height();
    let layout = sig.layout.read();
    let height = *sig.height.read();
    let top = *sig.top.read();
    let (first, last) = layout.window(top, height - bar_h);
    trace(|| {
        format!(
            "render rows {first}..{last} of {}, top {top:.0}, height {height:.0}",
            layout.rows()
        )
    });
    // The first drawn row is `top - above` above the viewport's top edge.
    let shift = layout.top(first) - top + bar_h;
    let rows = sig.rows.read();
    let lays = sig.lays.read();
    let kinds = sig.kinds.read();
    let blocks = sig.blocks.read();
    // What each drawn row is drawn as: itself, or the block it starts. A
    // block's other rows are left out: the block stands for them.
    let drawn: Vec<(usize, Option<&Block>)> = (first..last)
        .filter_map(|i| match blocks.iter().find(|b| b.rows().contains(&i)) {
            Some(b) if b.first() == i => Some((i, Some(b))),
            Some(_) => None,
            None => Some((i, None)),
        })
        .collect();
    let column = *sig.width.read() + 2.0 * row::PAD as f32;
    let status = sig.status.read().clone();
    let words = *sig.words.read();
    let aside = sig.aside.read().clone();
    let look = sig.look.read().clone();
    // The editor's box, which the column is centred in and the bars are
    // pinned to; and how far a finger has slid the page aside.
    let (bx, by) = *sig.origin.read();
    let box_w = *sig.win_w.read();
    let margin = bx + ((box_w - column) / 2.0).max(0.0) as f64;
    let slide = *sig.slide.read();
    let pinned = |top: f64| format!("left: {bx}px; top: {top}px; width: {box_w}px");
    let host = ed.st.host.clone();
    let (wheel, press_end, tap_end) = (ed.clone(), ed.clone(), ed.clone());
    // The scroll bar's: the viewport under the find bar, which ends where
    // the editor's own scroll ends.
    let bar_scroll = {
        let mut s = ed.scroller();
        s.view.1 -= bar_h;
        s.slack -= bar_h;
        s
    };
    let thumb_w = unit::components::scrollbar::BAR;
    let (bar_left, bar_top) = (bx + box_w as f64 - thumb_w, by + bar_h);
    let bar_len = (height - bar_h).max(0.0);
    let shown = if look.scroll_bar { "block" } else { "none" };
    let scroller = ed.clone();

    rsx! {
        style { {themed(STYLESHEET, look.appearance)} }
        style { {themed(&css(&look.ty), look.appearance)} }
        // The mouse's default actions are cancelled: they are Blitz's own
        // text selection, drawn over leaf's, and the focus changes that made
        // `onkeydown` unreliable (see `keys.rs`). Not `onpointerdown`:
        // cancelling the pointer event stops Blitz dispatching `mousedown`.
        // The element assistive tech is given the document's text as
        // (`access.rs`), by its id.
        div { class: "leaf", id: crate::access::TEXT_ID, style: "width: {column}px; margin-left: {margin}px; position: relative; left: {slide}px",
            // The wheel and the trackpad, in CSS pixels: a line is 20.
            onwheel: move |e: WheelEvent| {
                e.prevent_default();
                wheel.scroll(unit::scroll::wheel_delta(e.delta(), height).1);
            },
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            div { style: "height: 0; margin-top: {shift + by}px" }
            // Every child keyed, and a row's key never a block's: the Dioxus
            // alpha's keyed diff cannot place a child among unkeyed empty
            // ones, nor swap a row for the block that takes its key, and
            // panics — which leaves the page never rendering again.
            for (i, block) in drawn {
                match block {
                    Some(b) => blocks::render(i, b, &*host),
                    None => rsx! { Row { key: "r{i}", index: i, row: rows[i].clone(), layout: lays[i].clone(), kind: kinds[i] } },
                }
            }
            // Room under the last row, as a page has, so there is somewhere
            // to click past the end.
            div { style: "height: {height / 2.0}px",
                onmousedown: move |_: MouseEvent| press_end.past_end(false),
                onclick: move |_: MouseEvent| tap_end.past_end(true),
            }
        }
        {findbar::render(ed, &bar, &pinned(by))}
        // Always present, shown or hidden, as the find bar is: Blitz does not
        // lay out an absolutely positioned element inserted after the first
        // frame.
        // The scroll bar, down the box's right edge, under the find bar and
        // over the status line: over the rows, and under whatever a host
        // draws over the editor.
        div { style: "display: {shown}; position: fixed; left: {bar_left}px; top: {bar_top}px; width: {thumb_w}px; height: {bar_len}px; z-index: 1",
            unit::ScrollBar { axis: unit::Axis::Vertical, scroll: bar_scroll,
                label: "Document".to_string(),
                onscroll: move |y| scroller.scroll_to(y),
            }
        }
        div { class: "status",
            style: if look.status == Status::Hidden { "display: none".to_string() } else { pinned(by + height) },
            if look.status == Status::Full {
                "{status}"
            }
            span { class: "grow" }
            if let Some(a) = aside {
                span { class: "aside", "{a}" }
            }
            // In an element of its own: a flex row's gap is between its
            // elements, and Blitz leaves a bare text run out of it, which
            // ran a host's aside into the count ("177 pages66272 words").
            span { class: "words",
                match words {
                    Some((_, n)) => rsx! { "{n} words" },
                    None => rsx! { "counting…" },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_colour_is_set_for_both_appearances() {
        let all = stylesheet(&Type::default());
        let palette = Palette::of(&all);
        assert!(palette.used.len() > 20, "{:?}", palette.used);
        palette.check().expect("a whole palette");
        // And no colour is spelled outside the two palettes, but the
        // outline instrument's.
        let rules: String = all
            .split(":root {")
            .enumerate()
            .map(|(i, s)| {
                if i == 0 {
                    s
                } else {
                    s.split_once('}').map_or("", |(_, after)| after)
                }
            })
            .collect();
        assert!(
            !rules.contains('#'),
            "a literal colour outside the palettes"
        );
    }

    #[test]
    fn a_palette_set_for_one_appearance_only_is_caught() {
        let css = ":root {\n  --a: #fff;\n  --b: #000;\n}\n\
                   @media (prefers-color-scheme: dark) {\n  :root {\n    --a: #000;\n  }\n}\n\
                   .x { color: var(--a); background: var(--c); }";
        let p = Palette::of(css);
        assert!(p.check().is_err_and(|e| e.contains("one appearance")));
        let p = Palette {
            dark: p.light.clone(),
            ..p
        };
        assert!(
            p.check()
                .is_err_and(|e| e.contains("--c") || e.contains("\"c\""))
        );
    }

    #[test]
    fn the_stylesheet_sets_the_type_text_rs_measures_with() {
        let ty = Type {
            body: "Charter, serif".into(),
            mono: "Monaco, monospace".into(),
            size: 19.0,
            leading: 1.75,
            grid: None,
            faces: Default::default(),
        };
        let css = css(&ty);
        assert!(
            css.contains("font-family: Charter, serif; font-size: 19px; line-height: 1.75;"),
            "{css}"
        );
        assert!(css.contains("font-family: Monaco, monospace;"));
        assert!(css.contains("min-height: 1.75em;"));
        // A quote's level, as wide as `text.rs` measures it.
        assert!(css.contains(&format!("width: {}px;", ty.quote_indent())));
    }

    #[test]
    fn a_forced_appearance_switches_the_dark_block_itself() {
        let css = crate::bridge::style::STYLESHEET;
        assert_eq!(themed(css, Appearance::System), css);
        let dark = themed(css, Appearance::Dark);
        assert!(dark.contains("@media all") && !dark.contains("prefers-color-scheme"));
        let light = themed(css, Appearance::Light);
        assert!(light.contains("@media not all") && !light.contains("prefers-color-scheme"));
    }
}
