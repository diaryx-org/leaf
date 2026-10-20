//! The context menu, and the Link prompt — both drawn in the page's overlay
//! ([`unit::Unit`]), so they are the same on every platform.
//!
//! A right-click (or a ⌃-click, or a long press) places the caret unless it
//! lands in the selection, then asks for the menu (`Host::context_menu`). It
//! offers, in the SwiftUI Mac app's order: the spelling checker's guesses
//! for a misspelled word under the caret; Go to Note or Back to Reference;
//! Open, Edit and Copy Link; Cut and Copy with a selection; Paste, Paste
//! and Match Style and Select All; and Look Up.

use std::rc::Rc;

use dioxus::html::Code;
use leaf_dioxus::{Context, Footnote};
use unit::{Answer, Dialog, Entry, Item};

use crate::app::{self, AppEvent};
use crate::commands::{ALT, CMD, Command, SHIFT};
use crate::page::Page;
use crate::platform;
use crate::text;

/// What the menu offers for `cx`, at the caret as it now stands: (label,
/// command) pairs, a `None` label a separator and a `None` command an item
/// that can't be chosen.
pub fn items(page: &Page, cx: &Context) -> Vec<(Option<String>, Option<Command>)> {
    let mut v: Vec<(Option<String>, Option<Command>)> = Vec::new();
    let sep = |v: &mut Vec<(Option<String>, Option<Command>)>| {
        if v.last().is_some_and(|(l, _)| l.is_some()) {
            v.push((None, None));
        }
    };
    if !cx.selection {
        for (label, c) in platform::spelling::context_items(page) {
            // An empty label is the checker's separator.
            v.push(((!label.is_empty()).then_some(label), c));
        }
    }
    sep(&mut v);
    match cx.footnote {
        Some(Footnote::GoToNote) => {
            v.push((Some("Go to Note".into()), Some(Command::FollowFootnote)))
        }
        Some(Footnote::BackToReference) => v.push((
            Some("Back to Reference".into()),
            Some(Command::FollowFootnote),
        )),
        None => {}
    }
    if cx.link {
        v.push((Some("Open Link".into()), Some(Command::OpenLink)));
        v.push((Some("Edit Link…".into()), Some(Command::Link)));
        v.push((Some("Copy Link".into()), Some(Command::CopyLink)));
    }
    sep(&mut v);
    let chord = |ch: &str, code, mods| Some(Command::chord(ch, code, mods));
    if cx.selection {
        v.push((Some("Cut".into()), chord("x", Code::KeyX, CMD)));
        v.push((Some("Copy".into()), chord("c", Code::KeyC, CMD)));
    }
    v.push((Some("Paste".into()), chord("v", Code::KeyV, CMD)));
    v.push((
        Some("Paste and Match Style".into()),
        chord("v", Code::KeyV, CMD | SHIFT | ALT),
    ));
    v.push((Some("Select All".into()), chord("a", Code::KeyA, CMD)));
    if let Some((label, c)) = look_up(page, cx) {
        v.push((None, None));
        v.push((Some(label), Some(c)));
    }
    v
}

/// Look Up the selection, or the word the right-click put the caret in.
fn look_up(page: &Page, cx: &Context) -> Option<(String, Command)> {
    page.with_doc(|d| {
        let visible: Vec<u16> = text::visible(d).encode_utf16().collect();
        if cx.selection {
            let (a, b) = d.selection()?;
            let (lo, hi) = (text::utf16_of(d, a), text::utf16_of(d, b));
            return Some(("Look Up".into(), Command::LookUp(lo, hi.saturating_sub(lo))));
        }
        let at = text::utf16_of(d, d.caret);
        let (loc, len) = platform::lookup::word_at(&String::from_utf16_lossy(&visible), at)?;
        let word = String::from_utf16_lossy(&visible[loc..loc + len]);
        Some((format!("Look Up “{word}”"), Command::LookUp(loc, len)))
    })
    .flatten()
}

/// Open the context menu at the pointer.
pub fn show(page: &Page, cx: &Context) {
    let Some(unit) = page.unit() else { return };
    let items = items(page, cx);
    let mut commands = Vec::new();
    let entries: Vec<Entry> = items
        .into_iter()
        .map(|(label, c)| match (label, c) {
            (None, _) => Entry::Separator,
            (Some(label), Some(c)) => {
                commands.push(c);
                Item::new(commands.len() - 1, label).into()
            }
            (Some(label), None) => Item::new(usize::MAX, label).enabled(false).into(),
        })
        .collect();
    let commands = Rc::new(commands);
    let at = page.pointer().unwrap_or((40.0, 80.0));
    let id = page.window.id();
    page.in_scope(|| {
        unit.open_menu(entries, at, 0.0, move |i| {
            if let Some(c) = commands.get(i).cloned() {
                app::post(AppEvent::Command(id, c));
            }
        })
    });
}

/// Format ▸ Link…: ask for a destination, seeded with the link at the
/// caret, and make or change the link.
pub fn prompt_link(page: &Page) {
    let Some(unit) = page.unit() else { return };
    let current = page
        .with_doc(|d| d.link_destination_at_caret())
        .flatten()
        .unwrap_or_default();
    let id = page.window.id();
    let dialog = Dialog::new("Link")
        .message("The address the selected text links to.")
        .field(current, "https://")
        .buttons("OK", "Cancel");
    page.in_scope(|| {
        unit.open_dialog(dialog, move |answer| {
            if let Answer::Ok(dest) = answer
                && !dest.trim().is_empty()
            {
                let dest = dest.trim().to_string();
                app::on_turn(move || {
                    if let Some(p) = app::page(id) {
                        p.with_editor(|ed, _| {
                            ed.doc().insert_link(&dest);
                            ed.refresh();
                        });
                    }
                });
            }
        })
    });
}
