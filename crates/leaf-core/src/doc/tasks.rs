//! Task list items: reading and toggling a checkbox.

use super::*;

impl Doc {
    // ── Task list items ──────────────────────────────────────────────────────
    // The checkbox in `- [x] done`. twig owns all three gestures: the box is
    // inline content of the item's first paragraph rather than part of its
    // marker, so adding or removing one must leave the item's continuation
    // indentation alone, and an item inside a quote is found past the quote
    // markers. leaf names the gesture and the offset; the spelling is twig's.

    /// Whether the list item at the caret carries a checkbox, and which way it
    /// faces — `Some(true)` ticked, `Some(false)` empty, `None` for a plain list
    /// item or no item at all. What a toolbar reads to light its checkbox button.
    pub fn task_checked_at_caret(&mut self) -> Option<bool> {
        self.task_checked_at(self.caret)
    }

    /// [`task_checked_at_caret`](Self::task_checked_at_caret) for an arbitrary
    /// offset — what a frontend asks before deciding a click landed on a box.
    pub fn task_checked_at(&mut self, offset: usize) -> Option<bool> {
        let offset = offset.min(self.source.len());
        // Out of every list, which the walk to the offset says without the
        // marshal of the whole tree the item's own state needs.
        let in_list = self
            .nodes_at(offset)
            .iter()
            .any(|m| m.kind == Kind::ListItem || m.kind == Kind::TaskListItem);
        if !in_list {
            return None;
        }
        self.innermost_list_item(offset)?.checked
    }

    /// Tick or untick the task item at the caret (the checkbox's keyboard half).
    /// A no-op with a reported reason when the caret is in no task item — minting
    /// a box here is [`toggle_task_item`](Self::toggle_task_item)'s job.
    pub fn toggle_task_checked(&mut self) {
        self.toggle_task_at(self.caret);
    }

    /// Tick or untick the task item covering `offset` — what a *click* on a
    /// rendered checkbox is. Separate from the caret form because a click carries
    /// its own offset and must not first move the caret there: ticking a box
    /// three paragraphs away should not take the cursor with it.
    pub fn toggle_task_at(&mut self, offset: usize) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("task", Gesture::ToggleTaskChecked) {
            return;
        }
        let offset = offset.min(self.source.len());
        self.record_caret();
        match self.editor.toggle_task_checked(offset) {
            Ok(_) => self.after_task_edit(),
            Err(e) => self.status = Some(format!("task: {e}")),
        }
    }

    /// Give the list item at the caret a checkbox, or take its checkbox away —
    /// the gesture that converts between a plain bullet and a task. A new box
    /// arrives unticked.
    ///
    /// Outside a list the caret's block becomes a bullet first, and the box
    /// goes on the new item — one undo step. twig's gesture only converts an
    /// item that is already there, and a Checklist button that did nothing on
    /// a paragraph read as broken.
    pub fn toggle_task_item(&mut self) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("task", Gesture::ToggleTaskItem) {
            return;
        }
        if self
            .innermost_list_item(self.caret.min(self.source.len()))
            .is_none()
        {
            if self.refuse_unsupported(
                "task",
                Gesture::ToggleBlockContainer(BlockContainerKind::BulletList),
            ) {
                return;
            }
            self.begin_undo_group();
            self.toggle_list(false);
            if self
                .innermost_list_item(self.caret.min(self.source.len()))
                .is_some()
            {
                self.box_item_at_caret();
            }
            self.end_undo_group();
            return;
        }
        self.box_item_at_caret();
    }

    /// twig's half of [`toggle_task_item`](Self::toggle_task_item): the box on
    /// the item the caret is already in.
    fn box_item_at_caret(&mut self) {
        let caret = self.caret.min(self.source.len());
        self.record_caret();
        match self.editor.toggle_task_item(caret) {
            Ok(_) => self.after_task_edit(),
            Err(e) => self.status = Some(format!("task: {e}")),
        }
    }

    /// Settle after a task gesture. The caret rides its old byte offset and is
    /// clamped back in: a box is three or four bytes on the item's first line, so
    /// text after it shifts by that much at most, and `clamp_caret` lands it on a
    /// real stop either way.
    fn after_task_edit(&mut self) {
        self.last_edit_kind = None;
        self.refresh();
        self.anchor = None;
        self.dirty = self.source != self.clean_source;
        self.status = None;
        self.clamp_caret();
        self.record_caret();
    }
}
