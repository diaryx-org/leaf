//! A dialog — a title, a message, perhaps one line of text to type, and two
//! buttons — and that line's field.
//!
//! The field is the page's own, not a Blitz `<input>`: an `<input>` edits
//! only while it has DOM focus, and on dioxus-native a page cannot give it
//! focus from code (`MountedData::set_focus` from a task finds Blitz's
//! document borrowed), and loses it on any click that is not on a form
//! control. So the field keeps its own text and caret, and the host hands
//! it keys and input-method text through the layer, as it hands them to
//! the editor otherwise. On a phone those are the soft keyboard's.

use dioxus::html::{Key, Modifiers, NamedKey};

/// One line of text being typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub text: String,
    /// The caret, as a byte offset at a character boundary.
    pub caret: usize,
    /// Whether everything is selected: ⌘A, and a field's starting text, so
    /// that typing replaces it.
    pub all: bool,
    /// Shown greyed while the field is empty.
    pub placeholder: String,
}

/// What a key did to a field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKey {
    Ignored,
    Handled,
    /// ⌘V: the host is asked for the clipboard's text.
    Paste,
    /// ⌘C: the host is given the text.
    Copy(String),
}

impl Field {
    pub fn new(text: impl Into<String>, placeholder: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            caret: text.len(),
            all: !text.is_empty(),
            text,
            placeholder: placeholder.into(),
        }
    }

    /// Type `s` at the caret, over the selection if everything is selected.
    /// A newline ends nothing here: a field is one line.
    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        if self.all {
            self.text.clear();
            self.caret = 0;
            self.all = false;
        }
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    pub fn key(&mut self, key: &Key, mods: Modifiers) -> FieldKey {
        let cmd = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
        match key {
            Key::Named(NamedKey::Backspace) => {
                if self.all {
                    self.text.clear();
                    self.caret = 0;
                    self.all = false;
                } else if self.caret > 0 {
                    let from = if cmd { 0 } else { self.prev() };
                    self.text.replace_range(from..self.caret, "");
                    self.caret = from;
                }
                FieldKey::Handled
            }
            Key::Named(NamedKey::Delete) => {
                if self.all {
                    self.text.clear();
                    self.caret = 0;
                    self.all = false;
                } else {
                    let to = self.next();
                    self.text.replace_range(self.caret..to, "");
                }
                FieldKey::Handled
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.caret = if self.all || cmd { 0 } else { self.prev() };
                self.all = false;
                FieldKey::Handled
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.caret = if self.all || cmd {
                    self.text.len()
                } else {
                    self.next()
                };
                self.all = false;
                FieldKey::Handled
            }
            Key::Named(NamedKey::Home | NamedKey::ArrowUp) => {
                self.caret = 0;
                self.all = false;
                FieldKey::Handled
            }
            Key::Named(NamedKey::End | NamedKey::ArrowDown) => {
                self.caret = self.text.len();
                self.all = false;
                FieldKey::Handled
            }
            Key::Character(c) if cmd => match c.to_lowercase().as_str() {
                "a" => {
                    self.all = !self.text.is_empty();
                    FieldKey::Handled
                }
                "v" => FieldKey::Paste,
                "c" => FieldKey::Copy(self.text.clone()),
                "x" => {
                    let t = std::mem::take(&mut self.text);
                    self.caret = 0;
                    self.all = false;
                    FieldKey::Copy(t)
                }
                _ => FieldKey::Ignored,
            },
            Key::Character(c) if !mods.contains(Modifiers::ALT) || c.chars().count() == 1 => {
                self.insert(c);
                FieldKey::Handled
            }
            _ => FieldKey::Ignored,
        }
    }

    /// The text either side of the caret, for drawing it there.
    pub fn split(&self) -> (&str, &str) {
        self.text.split_at(self.caret)
    }
}

/// A dialog's content.
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub title: String,
    pub message: Option<String>,
    pub field: Option<Field>,
    /// The default button's label: Return presses it.
    pub ok: String,
    /// The other button's: Escape presses it.
    pub cancel: String,
}

impl Dialog {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: None,
            field: None,
            ok: "OK".into(),
            cancel: "Cancel".into(),
        }
    }

    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    /// A line to type, starting as `text`.
    pub fn field(mut self, text: impl Into<String>, placeholder: impl Into<String>) -> Self {
        self.field = Some(Field::new(text, placeholder));
        self
    }

    pub fn buttons(mut self, ok: impl Into<String>, cancel: impl Into<String>) -> Self {
        self.ok = ok.into();
        self.cancel = cancel.into();
        self
    }
}

/// How a dialog ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The default button, with the field's text (empty with no field).
    Ok(String),
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut Field, k: Key) -> FieldKey {
        f.key(&k, Modifiers::empty())
    }

    #[test]
    fn typing_over_a_starting_text_replaces_it() {
        let mut f = Field::new("https://old", "");
        assert!(f.all);
        press(&mut f, Key::Character("n".into()));
        assert_eq!(f.text, "n");
        assert_eq!(f.caret, 1);
    }

    #[test]
    fn the_caret_moves_and_deletes_by_character() {
        let mut f = Field::new("", "");
        f.insert("née");
        press(&mut f, Key::Named(NamedKey::ArrowLeft));
        press(&mut f, Key::Named(NamedKey::ArrowLeft));
        assert_eq!(f.split(), ("n", "ée"));
        press(&mut f, Key::Named(NamedKey::Delete));
        assert_eq!(f.text, "ne");
        press(&mut f, Key::Named(NamedKey::Backspace));
        assert_eq!((f.text.as_str(), f.caret), ("e", 0));
    }

    #[test]
    fn the_clipboard_is_the_hosts() {
        let mut f = Field::new("a link", "");
        assert_eq!(
            f.key(&Key::Character("v".into()), Modifiers::META),
            FieldKey::Paste
        );
        assert_eq!(
            f.key(&Key::Character("c".into()), Modifiers::META),
            FieldKey::Copy("a link".into())
        );
        f.insert("pasted\nline");
        assert_eq!(f.text, "pastedline");
    }
}
