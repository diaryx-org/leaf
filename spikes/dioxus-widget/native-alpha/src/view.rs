//! The page: the stylesheet, and the markup of the rows in view, the blocks
//! among them, the find bar and the status line.

use dioxus::html::geometry::WheelDelta;
use dioxus::prelude::*;
use dioxus_widget_spike::style::STYLESHEET;

use crate::blocks;
use crate::editor::Editor;
use crate::findbar;
use crate::row::{self, Row};
use crate::table::{PAD_X, PAD_Y};
use crate::text::{BODY, MONO, SIZE};
use crate::trace::trace;

/// What this binary adds to the parent's stylesheet: proportional body text
/// with code and tables in cells, in the families `text.rs` measures with;
/// `.leaf` as a viewport the editor scrolls itself; a known left edge for
/// [`row::PAD`]; the inline caret `row.rs` draws; tables; the find bar and
/// the status line.
///
/// A function only because the families are `text.rs`'s constants, and the
/// two must never disagree: a face Blitz draws in and parley does not
/// measure is every click landing a few glyphs off.
//
// Two Blitz behaviours shape this, both found the hard way:
//
// - It paints the first frame of an `overflow: hidden` element and not the
//   changes after it, so the clip is on `body`, never on `.leaf`.
// - It skips painting a child that hangs past its parent's box, so `.leaf`
//   has no fixed height: its box is as tall as the rows in it, shifted up by
//   the scroll, and the window cuts it off. The status line is pinned to the
//   window's bottom over it, in a stacking layer of its own.
fn css() -> String {
    let mut css = format!(
        "html, body {{ margin: 0; overflow: hidden; height: 100vh; }}
body {{ position: relative; }}
.leaf {{ padding: 0 16px; box-sizing: border-box; font-family: {BODY}; font-size: {SIZE}px; }}
.leaf .row.code, .leaf .row.table, .leaf .row.source, .leaf .code {{ font-family: {MONO}; }}
.leaf .row.source .g {{ font-size: 1em; vertical-align: baseline; }}
.leaf .line {{ min-height: 1.5em; }}
.leaf .ime {{ text-decoration: underline; text-decoration-color: #1a73e8; }}
.leaf .table {{ box-sizing: border-box; }}
.leaf .trow {{ display: flex; box-sizing: border-box; }}
.leaf .trow.head .tcell {{ background: #f6f8fa; }}
.leaf .trow:first-child .tcell {{ border-top: 1px solid #d0d7de; }}
.leaf .tprefix {{ flex: none; white-space: pre; }}
.leaf .tcell {{ flex: none; box-sizing: border-box; padding: {PAD_Y}px {PAD_X}px;
                border-right: 1px solid #d0d7de; border-bottom: 1px solid #d0d7de; }}
.leaf .tprefix + .tcell {{ border-left: 1px solid #d0d7de; }}
.leaf .tcell.center {{ text-align: center; }}
.leaf .tcell.end {{ text-align: right; }}
{REST}"
    );
    if std::env::var_os("LEAF_OUTLINE").is_some() {
        css.push_str(".leaf .line { outline: 1px solid #f00; } .leaf .line > span { outline: 1px solid #0a0; }");
    }
    css
}

const REST: &str = "
.leaf .caret-in { display: inline-block; width: 0; height: 1.2em;
                  border-left: 2px solid #1a73e8; margin-right: -2px;
                  vertical-align: text-bottom; }
.status { position: absolute; left: 0; right: 0; bottom: 0; z-index: 1;
          height: 24px; line-height: 24px; padding: 0 16px; box-sizing: border-box;
          font: 12px ui-monospace, monospace; color: #5f6368; background: #f6f8fa;
          border-top: 1px solid #e1e4e8; display: flex; gap: 16px; white-space: pre; }
.status .grow { flex: 1; }
.find { position: absolute; left: 0; right: 0; top: 0; z-index: 1; box-sizing: border-box;
        padding: 0 16px; background: #f6f8fa; border-bottom: 1px solid #e1e4e8;
        font: 13px system-ui, sans-serif; color: #24292f; }
.find .find-row { display: flex; align-items: center; gap: 8px; height: 34px; }
.find .label { width: 56px; color: #5f6368; }
.find .field { flex: 1; height: 22px; line-height: 22px; padding: 0 6px; white-space: pre;
               background: #fff; border: 1px solid #c8ccd0; border-radius: 5px; overflow: hidden; }
.find .field.focused { border-color: #1a73e8; }
.find .count { min-width: 72px; color: #5f6368; text-align: right; }
.find .btn { height: 22px; line-height: 22px; padding: 0 9px; background: #fff;
             border: 1px solid #c8ccd0; border-radius: 5px; }
.find .caret-in { display: inline-block; width: 0; height: 16px; vertical-align: text-bottom;
                  border-left: 2px solid #1a73e8; margin-right: -2px; }";

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
    let (wheel, past_end) = (ed.clone(), ed.clone());

    rsx! {
        style { {STYLESHEET} }
        style { {css()} }
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
