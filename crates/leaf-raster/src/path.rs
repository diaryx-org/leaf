//! Which image destinations a synchronous local loader handles.

use std::path::{Path, PathBuf};

/// Resolve an image destination to a readable local path, or `None` when it's
/// not one a synchronous loader handles: a remote URL, a `data:` URI, a
/// protocol-relative `//host/…`, or a relative path with no document directory
/// to anchor it. The one policy every frontend that decodes local files eagerly
/// shares — the rest stay a placeholder.
pub fn resolve_image_path(dest: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || dest.starts_with("//")
    {
        return None;
    }
    let raw = dest.strip_prefix("file://").unwrap_or(dest);
    let path = Path::new(raw);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        doc_dir.map(|d| d.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_remote_and_anchors_relative() {
        let dir = Path::new("/docs");
        assert_eq!(
            resolve_image_path("pics/cat.png", Some(dir)),
            Some(PathBuf::from("/docs/pics/cat.png"))
        );
        assert_eq!(
            resolve_image_path("/abs/cat.png", Some(dir)),
            Some(PathBuf::from("/abs/cat.png"))
        );
        assert_eq!(resolve_image_path("https://x.dev/a.png", Some(dir)), None);
        assert_eq!(
            resolve_image_path("data:image/png;base64,AAAA", Some(dir)),
            None
        );
        // A relative path with no document directory can't be anchored.
        assert_eq!(resolve_image_path("cat.png", None), None);
    }
}
