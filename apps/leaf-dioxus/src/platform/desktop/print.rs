//! The document on paper: File ▸ Export as PDF…, through the system's save
//! dialog, then the PDF `pdf.rs` has twig-export set, on the paper Settings
//! names. No Print: the PDF goes to a printer from the viewer it opens in.

use std::path::Path;

use crate::page::Page;
use crate::pdf::Paper;

/// File ▸ Export as PDF…: the save dialog, then the PDF.
pub fn export(page: &Page) {
    let id = page.window.id();
    let Some(shell) = page.shell() else { return };
    let document = &shell.document;
    let title = document.title();
    let stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
    let folder = document
        .file()
        .and_then(|f| f.parent().map(Path::to_path_buf));
    super::save_dialog(
        &format!("{stem}.pdf"),
        folder,
        ("PDF", &["pdf"]),
        move |path| {
            let Some(page) = crate::app::page(id) else {
                return;
            };
            let (w, h) = crate::app::settings().paper.size();
            let paper = Paper {
                width: w,
                height: h,
                margin: 72.0,
            };
            match crate::pdf::of_page(&page, paper) {
                Ok(bytes) => match std::fs::write(&path, bytes) {
                    Ok(()) => page.say(&format!("Exported {}", path.display())),
                    Err(e) => page.say(&format!("Export failed: {e}")),
                },
                Err(e) => page.say(&format!("Export failed: {e}")),
            }
        },
    );
}
