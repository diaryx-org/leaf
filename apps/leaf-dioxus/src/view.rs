//! The page: the stylesheet, and the markup of the rows in view, the blocks
//! among them, the find bar, the settings panel and the status line.

use dioxus::html::geometry::WheelDelta;
use dioxus::prelude::*;
use crate::bridge::style::STYLESHEET;

use crate::blocks;
use crate::editor::Editor;
use crate::findbar;
use crate::row::{self, Row};
use crate::table::{PAD_X, PAD_Y};
use crate::prefs;
use crate::settings::Appearance;
use crate::text::Type;
use crate::trace::trace;

/// What this binary adds to the bridge's stylesheet: proportional body text
/// with code and tables in cells, in the type `text.rs` measures with;
/// `.leaf` as a viewport the editor scrolls itself; a known left edge for
/// [`row::PAD`]; the inline caret `row.rs` draws; tables; the find bar, the
/// settings panel and the status line.
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
//   window's bottom over it, in a stacking layer of its own.
pub fn css(ty: &Type) -> String {
    let Type { body, mono, size, leading } = ty;
    let mut css = format!(
        "html, body {{ margin: 0; overflow: hidden; height: 100vh; background: var(--leaf-bg); }}
body {{ position: relative; }}
.leaf {{ padding: 0 16px; box-sizing: border-box; font-family: {body}; font-size: {size}px; line-height: {leading}; }}
.leaf .row.code, .leaf .row.table, .leaf .row.source, .leaf .code {{ font-family: {mono}; }}
.leaf .row.source .g {{ font-size: 1em; vertical-align: baseline; }}
.leaf .row, .leaf .line {{ min-height: {leading}em; }}
.leaf .ime {{ text-decoration: underline; text-decoration-color: var(--leaf-accent); }}
.leaf .table {{ box-sizing: border-box; }}
.leaf .trow {{ display: flex; box-sizing: border-box; }}
.leaf .trow.head .tcell {{ background: var(--leaf-table-head); }}
.leaf .trow:first-child .tcell {{ border-top: 1px solid var(--leaf-separator); }}
.leaf .tprefix {{ flex: none; white-space: pre; }}
.leaf .tcell {{ flex: none; box-sizing: border-box; padding: {PAD_Y}px {PAD_X}px;
                border-right: 1px solid var(--leaf-separator); border-bottom: 1px solid var(--leaf-separator); }}
.leaf .tprefix + .tcell {{ border-left: 1px solid var(--leaf-separator); }}
.leaf .tcell.center {{ text-align: center; }}
.leaf .tcell.end {{ text-align: right; }}
{CHROME}{REST}"
    );
    if std::env::var_os("LEAF_OUTLINE").is_some() {
        css.push_str(".leaf .line { outline: 1px solid #f00; } .leaf .line > span { outline: 1px solid #0a0; }");
    }
    css
}

/// The window chrome's colours — the status line, the find bar, the settings
/// panel — beside the document's in the bridge's stylesheet, and like them
/// set once per appearance.
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

const CHROME: &str = "
:root {
  --chrome-bg: #f5f5f5;
  --chrome-text: #262626;
  --field-bg: #ffffff;
  --field-border: #c8c8c8;
  --chip-on: #007aff;
  --chip-on-text: #ffffff;
}
@media (prefers-color-scheme: dark) {
  :root {
    --chrome-bg: #2a2a2a;
    --chrome-text: #dfdfdf;
    --field-bg: #1e1e1e;
    --field-border: #4d4d4d;
    --chip-on: #0a84ff;
    --chip-on-text: #ffffff;
  }
}
";

const REST: &str = "
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid var(--leaf-accent); margin-right: -2px;
                  vertical-align: text-bottom; }
.status { position: absolute; left: 0; right: 0; bottom: 0; z-index: 1;
          height: 24px; line-height: 24px; padding: 0 16px; box-sizing: border-box;
          font: 12px ui-monospace, monospace; color: var(--leaf-secondary); background: var(--chrome-bg);
          border-top: 1px solid var(--leaf-separator); display: flex; gap: 16px; white-space: pre; }
.status .grow { flex: 1; }
.find { position: absolute; left: 0; right: 0; top: 0; z-index: 1; box-sizing: border-box;
        padding: 0 16px; background: var(--chrome-bg); border-bottom: 1px solid var(--leaf-separator);
        font: 13px system-ui, sans-serif; color: var(--chrome-text); }
.find .find-row { display: flex; align-items: center; gap: 8px; height: 34px; }
.find .label { width: 56px; color: var(--leaf-secondary); }
.find .field { flex: 1; height: 22px; line-height: 22px; padding: 0 6px; white-space: pre;
               background: var(--field-bg); border: 1px solid var(--field-border); border-radius: 5px; overflow: hidden; }
.find .field.focused { border-color: var(--leaf-accent); }
.find .count { min-width: 72px; color: var(--leaf-secondary); text-align: right; }
.find .btn, .prefs .btn { height: 22px; line-height: 22px; padding: 0 9px; background: var(--field-bg);
             border: 1px solid var(--field-border); border-radius: 5px; }
.find .caret-in { display: inline-block; width: 0; height: 16px; vertical-align: text-bottom;
                  border-left: 2px solid var(--leaf-accent); margin-right: -2px; }
.prefs { position: absolute; top: 12px; right: 12px; width: 400px; z-index: 2; box-sizing: border-box;
         padding: 10px 14px 12px; background: var(--chrome-bg); color: var(--chrome-text);
         border: 1px solid var(--field-border); border-radius: 10px;
         font: 13px system-ui, sans-serif; line-height: 1.3; white-space: normal; }
.prefs .title { font-weight: 700; margin-bottom: 4px; }
.prefs .section { margin-top: 10px; color: var(--leaf-secondary); font-size: 11px; }
.prefs .prow { display: flex; gap: 8px; margin-top: 6px; }
.prefs .plabel { flex: none; width: 92px; text-align: right; padding-top: 3px; }
.prefs .chips { flex: 1; display: flex; flex-wrap: wrap; gap: 4px; }
.prefs .chip { padding: 2px 7px; border-radius: 5px; background: var(--field-bg);
               border: 1px solid var(--field-border); white-space: nowrap; }
.prefs .chip.on { background: var(--chip-on); color: var(--chip-on-text); border-color: var(--chip-on); }
.prefs .foot { display: flex; gap: 8px; margin-top: 12px; }
.prefs .foot .grow { flex: 1; }";

/// The page, for the rows in view.
pub fn render(ed: &Editor) -> Element {
    let sig = ed.sig;
    let bar = sig.bar.read().clone();
    let bar_h = bar.height();
    let layout = sig.layout.read();
    let height = *sig.height.read();
    let top = *sig.top.read();
    let (first, last) = layout.window(top, height - bar_h);
    trace(|| format!("render rows {first}..{last} of {}, top {top:.0}, height {height:.0}", layout.rows()));
    // The first drawn row is `top - above` above the viewport's top edge.
    let shift = layout.top(first) - top + bar_h;
    let rows = sig.rows.read();
    let lays = sig.lays.read();
    let kinds = sig.kinds.read();
    let blocks = sig.blocks.read();
    let column = *sig.width.read() + 2.0 * row::PAD as f32;
    let status = sig.status.read().clone();
    let words = *sig.words.read();
    let ty = sig.ty.read().clone();
    let prefs = sig.prefs.read().clone();
    let (wheel, past_end) = (ed.clone(), ed.clone());

    rsx! {
        style { {themed(STYLESHEET, prefs.settings.appearance)} }
        style { {themed(&css(&ty), prefs.settings.appearance)} }
        // The mouse's default actions are cancelled: they are Blitz's own
        // text selection, drawn over leaf's, and the focus changes that made
        // `onkeydown` unreliable (see `keys.rs`). Not `onpointerdown`:
        // cancelling the pointer event stops Blitz dispatching `mousedown`.
        div { class: "leaf", style: "width: {column}px",
            // The wheel and the trackpad, in CSS pixels: a line is 20.
            onwheel: move |e: WheelEvent| {
                e.prevent_default();
                wheel.scroll(match e.delta() {
                    WheelDelta::Pixels(v) => v.y,
                    WheelDelta::Lines(v) => v.y * 20.0,
                    WheelDelta::Pages(v) => v.y * height,
                });
            },
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            onclick: move |e: MouseEvent| e.prevent_default(),
            div { style: "height: 0; margin-top: {shift}px" }
            for i in first..last {
                match blocks.iter().find(|b| b.rows().contains(&i)) {
                    Some(b) if b.first() == i => blocks::render(i, b),
                    // A block's other rows: the block stands for them.
                    Some(_) => rsx! {},
                    None => rsx! { Row { key: "{i}", index: i, row: rows[i].clone(), layout: lays[i].clone(), kind: kinds[i] } },
                }
            }
            // Room under the last row, as a page has, so there is somewhere
            // to click past the end.
            div { style: "height: {height / 2.0}px",
                onmousedown: move |_: MouseEvent| past_end.past_end(),
            }
        }
        {findbar::render(ed, &bar)}
        {prefs::render(ed, &prefs)}
        div { class: "status",
            "{status}"
            span { class: "grow" }
            match words {
                Some((_, n)) => rsx! { "{n} words" },
                None => rsx! { "counting…" },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The custom properties a stylesheet uses, and those it defines before
    /// its dark block and inside it.
    fn properties(css: &str) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
        let names = |s: &str, after: &str| -> BTreeSet<String> {
            s.split(after)
                .skip(1)
                .map(|t| t.split(|c: char| !(c.is_alphanumeric() || c == '-')).next().unwrap_or("").to_string())
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
                if k > 0 && n == 0 { &mut dark } else { &mut light }.extend(defined);
            }
        }
        (used, light, dark)
    }

    #[test]
    fn every_colour_is_set_for_both_appearances() {
        let all = format!("{}{}", crate::bridge::style::STYLESHEET, css(&Type::default()));
        let (used, light, dark) = properties(&all);
        assert!(used.len() > 20, "{used:?}");
        assert_eq!(light, dark, "light and dark set different properties");
        let missing: Vec<_> = used.difference(&light).collect();
        assert!(missing.is_empty(), "used but never set: {missing:?}");
        // And no colour is spelled outside the two palettes, but the
        // outline instrument's.
        let rules: String = all
            .split(":root {")
            .enumerate()
            .map(|(i, s)| if i == 0 { s } else { s.split_once('}').map_or("", |(_, after)| after) })
            .collect();
        assert!(!rules.contains('#'), "a literal colour outside the palettes");
    }

    #[test]
    fn the_stylesheet_sets_the_type_text_rs_measures_with() {
        let ty = Type { body: "Charter, serif".into(), mono: "Monaco, monospace".into(), size: 19.0, leading: 1.75 };
        let css = css(&ty);
        assert!(css.contains("font-family: Charter, serif; font-size: 19px; line-height: 1.75;"), "{css}");
        assert!(css.contains("font-family: Monaco, monospace;"));
        assert!(css.contains("min-height: 1.75em;"));
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
