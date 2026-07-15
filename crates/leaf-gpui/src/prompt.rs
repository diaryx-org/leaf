//! A minimal, reusable single-line text prompt: a caller-supplied label and
//! initial value, edited with Backspace and printable characters, Enter to
//! confirm and Esc to cancel. `Editor` (`lib.rs`) opens it for ⌘K's link
//! destination and ⌘⇧S's file name; nothing here names either, and a third
//! caller adds a [`PromptAction`] variant rather than a second prompt type.
//!
//! The prompt owns a `FocusHandle` distinct from the document's own. Moving
//! window focus onto it (`Editor::open_prompt`) is what keeps keystrokes out
//! of the document while it's up: `lib.rs`'s `EntityInputHandler` impl (the
//! IME hookup) is only ever wired to whichever focus handle currently holds
//! focus (see `TextElement::paint`'s `window.handle_input` call), so once
//! focus moves here the document's input handler simply goes quiet — the
//! same way it would for any other gpui focusable stealing focus. This prompt
//! doesn't implement that trait itself, so it has no IME composition of its
//! own (no dead-key/CJK candidate window); `Editor::prompt_key_down` reads
//! each keystroke's already-resolved `key_char` instead, which is enough for
//! typing a URL but not for composing text.

use gpui::{FocusHandle, SharedString};

use crate::Pending;

/// What a confirmed prompt does with the string it collected. `Editor::confirm_prompt`
/// matches on this; a new caller adds a variant and an arm there rather than
/// teaching this module about itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptAction {
    /// Link the selection, or the block at the caret, to the entered destination.
    Link,
    /// Write the document to the entered path (`Doc::save_as`). `then` is what
    /// the Save As was on the way to: an untitled document reaches this from a
    /// plain ⌘S (nothing follows), but also from the quit dialog's Save, and
    /// naming the file is the only reason that quit hasn't happened yet.
    SaveAs { then: Pending },
}

/// One open prompt's live state.
pub(crate) struct TextPrompt {
    pub label: SharedString,
    pub value: String,
    /// Byte offset into `value` (always on a char boundary) the caret sits at.
    pub caret: usize,
    pub focus_handle: FocusHandle,
    pub action: PromptAction,
}

impl TextPrompt {
    /// Start a prompt over `value` with the caret parked at its end — the
    /// natural place to keep typing when `value` is a prefill (a re-edited
    /// link's current destination) as much as when it's empty.
    pub fn new(
        label: impl Into<SharedString>,
        value: String,
        action: PromptAction,
        focus_handle: FocusHandle,
    ) -> Self {
        let caret = value.len();
        TextPrompt { label: label.into(), value, caret, focus_handle, action }
    }

    /// Insert `text` (one keystroke's `key_char`) at the caret.
    pub fn insert(&mut self, text: &str) {
        insert_at(&mut self.value, &mut self.caret, text);
    }

    /// Delete the character before the caret, if any — a no-op at the start.
    pub fn backspace(&mut self) {
        backspace_at(&mut self.value, &mut self.caret);
    }
}

/// [`TextPrompt::insert`]'s logic, split out (the way `lib.rs` splits
/// `locate_caret_core` out of `locate_caret`) so it's unit-testable without a
/// live `FocusHandle`.
fn insert_at(value: &mut String, caret: &mut usize, text: &str) {
    value.insert_str(*caret, text);
    *caret += text.len();
}

/// [`TextPrompt::backspace`]'s logic — see [`insert_at`].
fn backspace_at(value: &mut String, caret: &mut usize) {
    let Some(prev) = value[..*caret].char_indices().next_back().map(|(i, _)| i) else {
        return;
    };
    value.replace_range(prev..*caret, "");
    *caret = prev;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_lands_at_the_caret_and_advances_past_it() {
        let mut value = "ab".to_string();
        let mut caret = 1;
        insert_at(&mut value, &mut caret, "X");
        assert_eq!(value, "aXb");
        assert_eq!(caret, 2);
    }

    #[test]
    fn backspace_removes_one_char_before_the_caret() {
        let mut value = "aXb".to_string();
        let mut caret = 2;
        backspace_at(&mut value, &mut caret);
        assert_eq!(value, "ab");
        assert_eq!(caret, 1);
    }

    #[test]
    fn backspace_at_the_start_is_a_no_op() {
        let mut value = "ab".to_string();
        let mut caret = 0;
        backspace_at(&mut value, &mut caret);
        assert_eq!(value, "ab");
        assert_eq!(caret, 0);
    }

    #[test]
    fn backspace_removes_a_whole_multibyte_char_not_one_byte_of_it() {
        let mut value = "a€b".to_string(); // € is 3 bytes wide
        let mut caret = 4; // just past €, before b
        backspace_at(&mut value, &mut caret);
        assert_eq!(value, "ab");
        assert_eq!(caret, 1);
    }
}
