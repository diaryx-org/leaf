//! New, Open, Save As, and the question an unsaved document asks first.
//!
//! The dialogs are rfd's *async* ones, which on macOS are sheets on the
//! window, answered through a completion handler. A modal dialog would spin a
//! nested event loop from inside a Dioxus task, and dioxus-native polls tasks
//! while Blitz holds its document borrowed: the first window event delivered
//! inside that loop would be the same "RefCell already borrowed" panic as
//! scrolling from a task (see `main.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use dioxus_native::winit::window::Window;
use rfd::{AsyncFileDialog, AsyncMessageDialog, MessageButtons, MessageDialogResult, MessageLevel};

/// The formats leaf opens, for the dialogs' filters.
const MARKDOWN: &[&str] = &["md", "markdown", "mdown", "mkd"];
const DJOT: &[&str] = &["dj", "djot"];

/// Ask for a document to open.
pub async fn pick_open(window: &Arc<dyn Window>) -> Option<PathBuf> {
    let file = AsyncFileDialog::new()
        .add_filter("Documents", &[MARKDOWN, DJOT].concat())
        .add_filter("Markdown", MARKDOWN)
        .add_filter("Djot", DJOT)
        .set_parent(&**window)
        .pick_file()
        .await?;
    Some(file.path().to_path_buf())
}

/// Ask where to save, suggesting `name`.
pub async fn pick_save(window: &Arc<dyn Window>, name: &str) -> Option<PathBuf> {
    let file = AsyncFileDialog::new()
        .add_filter("Markdown", MARKDOWN)
        .set_file_name(name)
        .set_parent(&**window)
        .save_file()
        .await?;
    Some(file.path().to_path_buf())
}

/// What to do with unsaved changes before the document is replaced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unsaved {
    Save,
    Discard,
    Cancel,
}

/// The Mac's question, in its words.
pub async fn ask_unsaved(window: &Arc<dyn Window>, name: &str) -> Unsaved {
    let answer = AsyncMessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title(format!("Do you want to save the changes you made to “{name}”?"))
        .set_description("Your changes will be lost if you don’t save them.")
        .set_buttons(MessageButtons::YesNoCancelCustom(
            "Save".into(),
            "Don’t Save".into(),
            "Cancel".into(),
        ))
        .set_parent(&**window)
        .show()
        .await;
    match answer {
        MessageDialogResult::Yes => Unsaved::Save,
        MessageDialogResult::No => Unsaved::Discard,
        MessageDialogResult::Custom(label) if label == "Save" => Unsaved::Save,
        MessageDialogResult::Custom(label) if label.starts_with("Don") => Unsaved::Discard,
        _ => Unsaved::Cancel,
    }
}

/// What a file command does.
#[derive(Clone, Copy, Debug)]
pub enum FileAction {
    New,
    Open,
    SaveAs,
}

impl crate::editor::Editor {
    /// New, Open, Save As: each a sheet on the window, so a task.
    pub fn file(&self, action: FileAction) {
        let ed = self.clone();
        dioxus::prelude::spawn(async move {
            match action {
                FileAction::SaveAs => {
                    save_as(&ed).await;
                    ed.repaint();
                }
                FileAction::New | FileAction::Open => {
                    if !settle_unsaved(&ed).await {
                        ed.repaint();
                        return;
                    }
                    let new = match action {
                        FileAction::New => leaf_core::Doc::blank().expect("a blank document"),
                        _ => {
                            let Some(path) = pick_open(&ed.st.window).await else { return };
                            match leaf_core::Doc::open(path.clone()) {
                                Ok(d) => d,
                                Err(e) => {
                                    ed.st.doc.borrow_mut().status = Some(format!("{}: {e}", path.display()));
                                    ed.repaint();
                                    return;
                                }
                            }
                        }
                    };
                    ed.install(new);
                }
            }
        });
    }
}

/// Name a file for the document and save it there; answers whether it saved.
async fn save_as(ed: &crate::editor::Editor) -> bool {
    let name = match ed.st.doc.borrow().is_untitled() {
        true => "Untitled.md".to_owned(),
        false => ed.st.doc.borrow().file_name(),
    };
    let Some(path) = pick_save(&ed.st.window, &name).await else { return false };
    let mut d = ed.st.doc.borrow_mut();
    d.save_as(path);
    !d.dirty
}

/// Before the document is replaced: save it, drop its changes, or stay —
/// answers whether to go ahead.
async fn settle_unsaved(ed: &crate::editor::Editor) -> bool {
    if !ed.st.doc.borrow().dirty {
        return true;
    }
    let name = ed.st.doc.borrow().file_name();
    match ask_unsaved(&ed.st.window, &name).await {
        Unsaved::Discard => true,
        Unsaved::Cancel => false,
        Unsaved::Save if ed.st.doc.borrow().is_untitled() => save_as(ed).await,
        Unsaved::Save => {
            let mut d = ed.st.doc.borrow_mut();
            d.save();
            !d.dirty
        }
    }
}
