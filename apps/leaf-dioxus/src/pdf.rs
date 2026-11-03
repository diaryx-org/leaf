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

pub use leaf_pages::pdf::Paper;

/// The paper the display choices name, at an inch's margin: what Export as
/// PDF writes on, and the page view shows.
pub fn paper_of(settings: &crate::settings::Settings) -> Paper {
    let (width, height) = settings.paper.size();
    Paper {
        width,
        height,
        margin: leaf_pages::print::MARGIN,
    }
}

/// The display choices, on paper `size`, as the page view is given them
/// (`pages.rs`), at no window's zoom: paper does not zoom.
fn choices_of(settings: &crate::settings::Settings, size: (f32, f32)) -> leaf_pages::Choices {
    leaf_pages::Choices {
        template: settings.template.clone(),
        size: settings.size.px(),
        zoom: 1.0,
        paper: size,
        paper_size: None,
    }
}

/// The document in `page`, in the display choices, on `paper`, as a PDF:
/// set as the page view sets it ([`leaf_pages::print`]), its pictures where
/// the editor finds them. `Err` if the editor has it borrowed or Typst will
/// not set it.
pub fn of_page(page: &crate::page::Page, paper: Paper) -> Result<Vec<u8>, String> {
    let host = page.editor().map(|ed| ed.host());
    let choices = choices_of(&crate::app::settings(), (paper.width, paper.height));
    let job = page
        .with_doc(|d| leaf_pages::print::job(d, host.as_deref(), &choices))
        .ok_or("the document is busy")?;
    leaf_pages::typeset::pdf(&job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::Format;
    use leaf_pages::pdf::{Type, render};

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
