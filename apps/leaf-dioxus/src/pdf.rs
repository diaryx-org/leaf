//! The document as a PDF, set by twig-export: what Export as PDF writes,
//! Print prints, and the preview shows (`preview.rs`).
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
//! One [`Typesetter`] sets every document, for the life of the app: it reads
//! the system's fonts when it is first asked, which is most of the time a
//! short document takes, and not again.

use std::path::Path;
use std::sync::OnceLock;

use leaf_core::Format;
use twig_export::config::Config;
use twig_export::pdf::{Layout, Typesetter};

/// A sheet and its margin, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
    pub margin: f32,
}

impl Paper {
    /// The paper the display choices name, at an inch's margin: what Export
    /// as PDF writes on, and the preview shows.
    pub fn of(settings: &crate::settings::Settings) -> Paper {
        let (width, height) = settings.paper.size();
        Paper {
            width,
            height,
            margin: 72.0,
        }
    }
}

/// The display choices a PDF follows: the body face, by its family name,
/// and the body size, in points.
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub face: String,
    pub size: f32,
}

impl Type {
    /// The display choices' face and size, a CSS pixel printed at three
    /// quarters of a point, as a browser prints: 16px text is 12pt.
    pub fn of(settings: &crate::settings::Settings) -> Type {
        Type {
            face: settings.body.clone(),
            size: settings.size.px() * 0.75,
        }
    }
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
    layout(source, format, path, ty, paper)?
        .pdf()
        .map_err(|e| format!("{e:#}"))
}

/// The same document laid out once: as the PDF [`render`] writes, as SVG
/// pages, and as each page's glyphs with the bytes of the source each was
/// set from — what the page view draws, and draws the caret on.
pub fn layout(
    source: &str,
    format: Format,
    path: Option<&Path>,
    ty: &Type,
    paper: Paper,
) -> Result<Layout, String> {
    let tree = leaf_core::parse_tree(source, format).map_err(|e| e.to_string())?;
    let words = twig_export::render_words(tree, source, &config(ty, paper))
        .map_err(|e| format!("{e:#}"))?;
    typesetter()
        .lay_out(&words, root(path))
        .map_err(|e| format!("{e:#}"))
}

/// The app's typesetter, made by the first document set.
fn typesetter() -> &'static Typesetter {
    static TYPESETTER: OnceLock<Typesetter> = OnceLock::new();
    TYPESETTER.get_or_init(Typesetter::new)
}

/// The folder a document at `path` finds its pictures in.
fn root(path: Option<&Path>) -> &Path {
    path.and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// The document in `page`, in the display choices, on `paper`, as a PDF;
/// `Err` if the editor has it borrowed or Typst will not set it.
pub fn of_page(page: &crate::page::Page, paper: Paper) -> Result<Vec<u8>, String> {
    let (source, format, path) = page
        .with_doc(|d| (d.source.clone(), d.format, d.path.clone()))
        .ok_or("the document is busy")?;
    let ty = Type::of(&crate::app::settings());
    render(&source, format, Some(&path), &ty, paper)
}

/// twig-export's defaults, on `paper` and in `ty`. The sheet is set by its
/// size, not a name, since Print's comes from the print panel.
fn config(ty: &Type, paper: Paper) -> Config {
    let mut cfg = Config::default();
    let d = &mut cfg.document;
    d.paper = None;
    d.margin = Some(format!("{}pt", paper.margin));
    d.font = Some(ty.face.clone());
    d.size = Some(format!("{}pt", ty.size));
    // A run with nowhere to break that is wider than a line (a URL, a string
    // of punctuation) breaks where it must, rather than run off the page:
    // the text is whatever the writer typed, at whatever width.
    d.overflow_wrap = Some("break-word".into());
    let page = format!(
        "#set page(width: {}pt, height: {}pt)",
        paper.width, paper.height
    );
    d.raw_preamble = Some(match d.raw_preamble.take() {
        Some(p) => format!("{page}\n{p}"),
        None => page,
    });
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
            face: "Iowan Old Style".into(),
            size: 12.0,
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

    /// The sample: formulas, a highlight, a table, a picture beside it and
    /// one written into it as a `data:` URL.
    #[test]
    fn the_sample_sets() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/resources");
        let src = std::fs::read_to_string(format!("{dir}/Sample.md")).unwrap();
        let path = std::path::PathBuf::from(format!("{dir}/Media/Untitled.md"));
        let pdf = render(&src, Format::Markdown, Some(&path), &ty(), US_LETTER).expect("a pdf");
        assert!(pdf.starts_with(b"%PDF"), "not a PDF");
    }

    /// A run longer than a line with nowhere to break stays on the page.
    #[test]
    fn a_run_too_long_for_a_line_stays_on_the_page() {
        let src = format!("This is a test{} and more words.\n", "!".repeat(300));
        let layout = layout(&src, Format::Markdown, None, &ty(), US_LETTER).expect("a layout");
        let pages = layout.pages();
        let right = pages[0]
            .glyphs
            .iter()
            .map(|g| g.x + g.width)
            .fold(0.0, f64::max);
        assert!(right <= f64::from(US_LETTER.width), "a glyph at {right}");
    }

    #[test]
    fn the_sheet_is_the_paper_given() {
        let cfg = config(&ty(), US_LETTER);
        let pre = cfg.document.raw_preamble.unwrap();
        assert!(pre.contains("width: 612pt, height: 792pt"), "{pre}");
        assert_eq!(cfg.document.margin.as_deref(), Some("72pt"));
        assert_eq!(cfg.document.size.as_deref(), Some("12pt"));
        assert_eq!(cfg.document.paper, None);
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
