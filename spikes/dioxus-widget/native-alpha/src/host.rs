//! The gestures [`input`](crate::input) hands back as [`Host`]: the
//! clipboard, which a DOM widget cannot reach from a key event, and saving.
//!
//! A webview would need the `paste` event's `DataTransfer` for these. Native
//! has no such restriction — this process owns the clipboard outright — so
//! they go straight to the system clipboard through `arboard`.
//!
//! Both flavors travel, as leaf-swift publishes them: the source slice as
//! `text/plain`, and [`Doc::selection_html`] as `text/html`, so a paste into
//! Mail or Docs keeps its formatting. A paste prefers the HTML and falls back
//! to the text, which is [`Doc::paste_html`]'s own contract.

use leaf_core::Doc;

use crate::input::Host;

/// Do what the host owns. Answers whether the document changed, so the
/// caller knows to repaint.
pub fn run(doc: &mut Doc, gesture: Host) -> bool {
    match gesture {
        Host::Copy => {
            copy(doc);
            false
        }
        Host::Cut => {
            if copy(doc) {
                doc.backspace();
                true
            } else {
                false
            }
        }
        Host::Paste => paste(doc, true),
        Host::PastePlain => paste(doc, false),
        Host::Save => {
            doc.save();
            // `save` reports through `status` either way; the status line
            // shows it, so this is a repaint.
            true
        }
    }
}

/// Publish the selection; `false` if there was none to publish.
fn copy(doc: &mut Doc) -> bool {
    let Some(text) = doc.selected_text().map(str::to_owned) else {
        return false;
    };
    let html = doc.selection_html();
    let Ok(mut board) = arboard::Clipboard::new() else {
        doc.status = Some("copy failed: no clipboard".into());
        return false;
    };
    let wrote = match html {
        Some(html) => board.set_html(html, Some(text)),
        None => board.set_text(text),
    };
    if let Err(e) = wrote {
        doc.status = Some(format!("copy failed: {e}"));
        return false;
    }
    true
}

fn paste(doc: &mut Doc, rich: bool) -> bool {
    let Ok(mut board) = arboard::Clipboard::new() else {
        return false;
    };
    if rich
        && let Ok(html) = board.get().html()
        && doc.paste_html(&html)
    {
        return true;
    }
    match board.get_text() {
        Ok(text) if !text.is_empty() => {
            doc.paste(&text);
            true
        }
        _ => false,
    }
}
