//! Insert ▸ Image…: a picture chosen in the system's dialog, put in at the
//! caret as the document's link to it — beside the document, a path from
//! its folder, so the two move together; anywhere else, or in a document
//! not saved yet, the picture's whole path.

use std::path::{Component, Path, PathBuf};

use crate::page::Page;
use crate::platform;

/// The pictures the dialog offers: what the editor draws and Typst sets.
pub const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// Ask for a picture, and put it in at the caret of `page`'s document.
pub fn image(page: &Page) {
    let folder = page
        .with_doc(|d| d.path.parent().map(Path::to_path_buf))
        .flatten()
        .filter(|f| !f.as_os_str().is_empty());
    platform::pick_picture(page, folder, put);
}

/// Files dropped on `page`'s window: each picture among them put in at
/// the caret, one after another; anything else is not the editor's.
pub fn dropped(page: &Page, paths: Vec<PathBuf>) {
    for path in paths.into_iter().filter(|p| is_picture(p)) {
        put(page, path);
    }
}

/// Whether the file at `path` is a picture the dialog would offer.
fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PICTURES.contains(&e.to_ascii_lowercase().as_str()))
}

/// Put the picture at `path` in at the caret.
fn put(page: &Page, path: PathBuf) {
    page.with_editor(|ed, _| {
        {
            let mut d = ed.doc();
            let dest = destination(&path, &d.path);
            d.insert_image(&dest, "");
        }
        ed.refresh();
    });
}

/// The link to the picture at `picture` from a document at `document`: a
/// path from the document's folder, with `/` between its parts, where the
/// picture is in it; otherwise the picture's whole path.
pub fn destination(picture: &Path, document: &Path) -> String {
    let folder = document.parent().filter(|f| !f.as_os_str().is_empty());
    let inside = folder.and_then(|f| picture.strip_prefix(f).ok());
    match inside {
        Some(rel) if rel.components().all(|c| matches!(c, Component::Normal(_))) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => picture.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_beside_the_document_is_linked_from_its_folder() {
        let doc = Path::new("/notes/today.md");
        assert_eq!(destination(Path::new("/notes/a.png"), doc), "a.png");
        assert_eq!(
            destination(Path::new("/notes/Media/my photo.jpg"), doc),
            "Media/my photo.jpg"
        );
        assert_eq!(
            destination(Path::new("/elsewhere/b.png"), doc),
            "/elsewhere/b.png"
        );
        // Not saved yet: no folder to be beside.
        assert_eq!(
            destination(Path::new("/x/c.png"), Path::new("")),
            "/x/c.png"
        );
    }
}
