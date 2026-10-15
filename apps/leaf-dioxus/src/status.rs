//! The status line and the word count.

use std::rc::Rc;
use std::time::Duration;

use dioxus::prelude::*;
use leaf_core::Doc;

use crate::editor::State;

/// How long the typing has to stop before the word count is redone.
const SETTLE: Duration = Duration::from_millis(400);

/// Count the words once the document has sat at `rev` for [`SETTLE`].
///
/// `counts()` reparses and lays out the whole document — 130 ms at 5 200 rows
/// — so it cannot run per keystroke, and core's own doc says to ask "when the
/// typing stops". Every edit schedules one of these; all but the last wake to
/// find the revision has moved on, and do nothing.
pub fn recount(st: Rc<State>, rev: u64, mut words: Signal<Option<(u64, usize)>>) {
    spawn(async move {
        tokio::time::sleep(SETTLE).await;
        let doc = st.doc.borrow();
        if doc.revision() == rev {
            words.set(Some((rev, doc.counts().words)));
        }
    });
}

/// The status line: the file, whether it is saved, the view, and whatever
/// core last had to say — a failed save, an untitled ⌘S. The word count is
/// its own signal, drawn beside this; see [`recount`].
pub fn line(doc: &Doc) -> String {
    let name = doc.file_name();
    let dirty = if doc.dirty { " •" } else { "" };
    let note = doc.status.as_deref().unwrap_or("");
    format!("{name}{dirty}   {} view   {note}", doc.view_name())
}
