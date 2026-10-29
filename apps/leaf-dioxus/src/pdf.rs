//! The document as a PDF, set by twig-export through leaf-pages
//! (`leaf_pages::pdf`): what Export as PDF writes and Print prints, on the
//! paper the display choices name, in their template.
//!
//! twig-export lays the document out with Typst, the typesetter Diaryx sets
//! every paged format with, so the PDF leaf writes is the one the CLI and
//! the other apps write from the same source. The page is Typst's, not the
//! editor's: lines and pages break where Typst breaks them, and what
//! twig-export does not draw yet (math, highlights, the presentation CSS in
//! `style`) is not on it. See docs/proposals/what-you-see-is-what-twig-export-sets.md.

pub use leaf_pages::pdf::{Paper, Type, render};

/// The paper the display choices name, at an inch's margin: what Export as
/// PDF writes on, and the page view shows.
pub fn paper_of(settings: &crate::settings::Settings) -> Paper {
    let (width, height) = settings.paper.size();
    Paper {
        width,
        height,
        margin: 72.0,
    }
}

/// The display choices' template and size, a CSS pixel printed at three
/// quarters of a point, as a browser prints: 16px text is 12pt. Dark ink
/// never: a sheet is paper.
pub fn type_of(settings: &crate::settings::Settings) -> Type {
    Type {
        template: settings.template.clone(),
        size: settings.size.px() * 0.75,
        dark: false,
    }
}

/// The document in `page`, in the display choices, on `paper`, as a PDF;
/// `Err` if the editor has it borrowed or Typst will not set it.
pub fn of_page(page: &crate::page::Page, paper: Paper) -> Result<Vec<u8>, String> {
    let host = page.editor().map(|ed| ed.host());
    let (source, format, path, supply) = page
        .with_doc(|d| {
            // The pictures where the editor finds them; no room for a view
            // the host draws on the screen, which paper has none of.
            let pictures = host
                .as_ref()
                .map(|h| leaf_pages::gather(d, &**h, false).0.pictures)
                .unwrap_or_default();
            let supply = leaf_pages::typeset::Supply {
                pictures,
                rooms: Default::default(),
            };
            (d.source.clone(), d.format, d.path.clone(), supply)
        })
        .ok_or("the document is busy")?;
    let ty = type_of(&crate::app::settings());
    render(
        &source,
        format,
        Some(&path),
        &ty,
        paper,
        std::sync::Arc::new(supply),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::Format;

    /// The sample: formulas, a highlight, a table, a picture beside it and
    /// one written into it as a `data:` URL.
    #[test]
    fn the_sample_sets() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/resources");
        let src = std::fs::read_to_string(format!("{dir}/Sample.md")).unwrap();
        let path = std::path::PathBuf::from(format!("{dir}/Media/Untitled.md"));
        let ty = Type {
            template: crate::settings::Template::default(),
            size: 12.0,
            dark: false,
        };
        let paper = Paper {
            width: 612.0,
            height: 792.0,
            margin: 72.0,
        };
        let pdf = render(
            &src,
            Format::Markdown,
            Some(&path),
            &ty,
            paper,
            Default::default(),
        )
        .expect("a pdf");
        assert!(pdf.starts_with(b"%PDF"), "not a PDF");
    }
}
