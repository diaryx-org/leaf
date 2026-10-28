//! The document as a PDF, set by twig-export: what a host's Export as PDF
//! writes and Print prints, and the pages show (`pages.rs`).
//!
//! twig-export lays the document out with Typst, the typesetter Diaryx sets
//! every paged format with, so the PDF leaf writes is the one the CLI and
//! the other apps write from the same source. The tree is the one the
//! editor reads (`leaf_core::parse_tree`, with leaf's features), and
//! pictures resolve against the document's folder. The page is Typst's, not
//! the editor's: lines and pages break where Typst breaks them, and what
//! twig-export does not draw yet (math, highlights, the presentation CSS in
//! `style`) is not on it. See docs/proposals/what-you-see-is-what-twig-export-sets.md.
//!
//! Where it is set is `typeset.rs`'s: in this process, which reads the
//! system's fonts when it is first asked, and not again; or, in a template
//! not twig-export's own, wherever the host's [`crate::Elsewhere`] sets it.

use std::path::Path;

use leaf_core::Format;
use twig_export::config::Config;

use crate::typeset::{Job, Laid};

/// A sheet and its margin, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    pub margin: f32,
}

/// What a document is set on: a sheet of paper, or one page `width` points
/// wide and as tall as its text, for a view that reflows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sheet {
    Paper(Paper),
    Reflow { width: f32 },
}

/// How a document is set: the template, which decides how every element
/// looks; the body size, in points, which every style is relative to; and
/// whether the ink is light, for a dark ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub template: crate::Template,
    pub size: f32,
    pub dark: bool,
}

/// The document `source` in `format`, at `path` for its pictures, set in
/// `ty` on `paper`, as a PDF.
pub fn render(
    source: &str,
    format: Format,
    path: Option<&Path>,
    ty: &Type,
    paper: Paper,
) -> Result<Vec<u8>, String> {
    crate::typeset::pdf(&job(source, format, path, ty, Sheet::Paper(paper)))
}

/// The same document laid out once: as the PDF [`render`] writes, as SVG
/// pages, and as each page's glyphs with the bytes of the source each was
/// set from — what the page view draws, and draws the caret on.
pub fn layout(
    source: &str,
    format: Format,
    path: Option<&Path>,
    ty: &Type,
    sheet: Sheet,
    known: &std::collections::HashSet<u128>,
) -> Result<Laid, String> {
    crate::typeset::layout(&job(source, format, path, ty, sheet), known)
}

fn job(source: &str, format: Format, path: Option<&Path>, ty: &Type, sheet: Sheet) -> Job {
    Job {
        source: source.to_owned(),
        format,
        path: path.map(Path::to_path_buf),
        ty: ty.clone(),
        sheet,
    }
}

/// twig-export's defaults, in `ty`'s template, laid out on `sheet`. A sheet
/// of paper is given by its size, not a name, since Print's comes from the
/// print panel; its margins are the template's. Reflowed, a template that
/// sets only paper is Classic. The template is given the whole of the
/// document's frontmatter, as data: its title, its authors.
pub fn config(ty: &Type, sheet: Sheet) -> Config {
    let mut cfg = Config::default();
    let d = &mut cfg.document;
    let reflow = matches!(sheet, Sheet::Reflow { .. });
    let template = match crate::templates::entry(ty.template.key()) {
        Some(e) if reflow && !e.reflows => crate::templates::FALLBACK.to_owned(),
        _ => ty.template.key().to_owned(),
    };
    d.template = Some(template);
    d.template_meta = Some(vec!["*".into()]);
    // A template sets its own title from the frontmatter, or none.
    d.title_page = Some(String::new());
    d.size = Some(format!("{}pt", ty.size));
    d.dark = Some(ty.dark);
    d.paper = None;
    d.margin = None;
    d.font = None;
    match sheet {
        Sheet::Paper(paper) => {
            d.layout = Some("paged".into());
            d.page_width = Some(format!("{}pt", paper.width));
            d.page_height = Some(format!("{}pt", paper.height));
        }
        Sheet::Reflow { width } => {
            d.layout = Some("reflow".into());
            d.page_width = Some(format!("{width}pt"));
        }
    }
    // A run with nowhere to break that is wider than a line (a URL, a string
    // of punctuation) breaks where it must, rather than run off the page:
    // the text is whatever the writer typed, at whatever width.
    d.overflow_wrap = Some("break-word".into());
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    const US_LETTER: Paper = Paper {
        width: 612.0,
        height: 792.0,
        margin: 72.0,
    };

    fn ty() -> Type {
        Type {
            template: crate::Template::default(),
            size: 12.0,
            dark: false,
        }
    }

    #[test]
    fn a_long_document_with_a_table_and_a_formula_sets() {
        let mut src = String::from(
            "# Printed\n\nEuler: $e^{i\\pi} + 1 = 0$, ==marked==.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n",
        );
        for i in 0..80 {
            src.push_str(&format!(
                "Paragraph {i} of a long document, to run onto more pages.\n\n"
            ));
        }
        let pdf = render(&src, Format::Markdown, None, &ty(), US_LETTER).expect("a pdf");
        assert!(pdf.starts_with(b"%PDF"), "not a PDF");
        if let Ok(out) = std::env::var("LEAF_PDF_OUT") {
            std::fs::write(out, &pdf).unwrap();
        }
        assert!(pdf.len() > 5_000, "{} bytes", pdf.len());
    }

    /// A run longer than a line with nowhere to break stays on the page.
    #[test]
    fn a_run_too_long_for_a_line_stays_on_the_page() {
        let src = format!("This is a test{} and more words.\n", "!".repeat(300));
        let layout = layout(
            &src,
            Format::Markdown,
            None,
            &ty(),
            Sheet::Paper(US_LETTER),
            &Default::default(),
        )
        .expect("a layout");
        let pages = layout.pages.clone();
        let right = pages[0]
            .glyphs
            .iter()
            .map(|g| g.x + g.width)
            .fold(0.0, f64::max);
        assert!(right <= f64::from(US_LETTER.width), "a glyph at {right}");
    }

    #[test]
    fn the_sheet_is_the_paper_given_to_the_template() {
        let d = config(&ty(), Sheet::Paper(US_LETTER)).document;
        assert_eq!(d.template.as_deref(), Some("classic"));
        assert_eq!(d.layout.as_deref(), Some("paged"));
        assert_eq!(
            (d.page_width.as_deref(), d.page_height.as_deref()),
            (Some("612pt"), Some("792pt"))
        );
        assert_eq!(d.size.as_deref(), Some("12pt"));
        assert_eq!((d.paper, d.margin, d.font), (None, None, None));
    }

    /// Reflowed, in every template, a document is one page as wide as the
    /// view asks, light ink or dark.
    #[test]
    fn a_reflowed_document_is_one_page_in_every_template() {
        use crate::Template;
        let src = "# A title\n\nA paragraph, long enough to wrap, about nothing much at all.\n";
        for name in twig_export::templates::NAMES {
            let template = Template::parse(name).unwrap();
            for dark in [false, true] {
                let ty = Type {
                    template: template.clone(),
                    size: 12.0,
                    dark,
                };
                let set = layout(
                    src,
                    Format::Markdown,
                    None,
                    &ty,
                    Sheet::Reflow { width: 400.0 },
                    &Default::default(),
                )
                .expect("a layout");
                let pages = set.pages.clone();
                assert_eq!(pages.len(), 1, "{template:?}");
                assert_eq!(pages[0].width, 400.0, "{template:?}");
            }
        }
    }

    #[test]
    fn a_picture_beside_the_document_is_found() {
        let dir = std::env::temp_dir().join(format!("leaf-pdf-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dot.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><circle cx="5" cy="5" r="4"/></svg>"#,
        )
        .unwrap();
        let path = dir.join("note.md");
        let pdf = render(
            "A dot:\n\n![a dot](dot.svg)\n",
            Format::Markdown,
            Some(&path),
            &ty(),
            US_LETTER,
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(pdf.expect("the picture is found").starts_with(b"%PDF"));
    }
}
