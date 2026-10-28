//! Where a document is set: in this process, in one of twig-export's own
//! templates; wherever the host says ([`crate::Elsewhere`]), in any other.
//!
//! A template is Typst, and Typst has no limit on how long it runs or how
//! much it takes: a `while true {}`, or a layout that never settles, would
//! hold the host, and a fault in Typst itself would take it down. So a host
//! that can sets a template not twig-export's somewhere else: Leaf's desktop
//! app, in a typesetter of its own, another process, whose answers it sets
//! through [`layout_here`] and [`pdf_here`]. A host with nowhere else sets it
//! here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use leaf_core::Format;
use twig_export::pdf::{Layout, Packages, Page, Typesetter};

use crate::Template;
use crate::pdf::{Sheet, Type};

/// A document laid out, here or elsewhere: each page's size
/// and glyphs, and its pieces, twig-export's tiles, each with its picture
/// unless the window has that picture already.
pub struct Laid {
    pub pages: Vec<Page>,
    pub tiles: Vec<Tiled>,
}

/// A piece of a page as laid out: where it is on its page, in points, what
/// it draws as a key, and its SVG if the window asking has none of that key.
#[derive(Clone, Debug, PartialEq)]
pub struct Tiled {
    pub page: usize,
    pub top: f64,
    pub height: f64,
    pub width: f64,
    pub key: u128,
    pub svg: Option<String>,
}

/// `layout` as pages and pieces, a picture made for each piece whose key is
/// not `known`, once.
pub fn laid(layout: &Layout, known: &HashSet<u128>) -> Laid {
    let mut made = HashSet::new();
    let tiles = layout
        .tiles()
        .iter()
        .map(|t| Tiled {
            page: t.page,
            top: t.top,
            height: t.height,
            width: t.width,
            key: t.key,
            svg: (!known.contains(&t.key) && made.insert(t.key)).then(|| layout.tile_svg(t)),
        })
        .collect();
    Laid {
        pages: layout.pages(),
        tiles,
    }
}

/// What a setting is asked of: a document, how it is set, and on what.
#[derive(Clone, Debug)]
pub struct Job {
    pub source: String,
    pub format: Format,
    pub path: Option<PathBuf>,
    pub ty: Type,
    pub sheet: Sheet,
}

/// `job` laid out, with a picture for each piece whose key is not `known`.
pub fn layout(job: &Job, known: &HashSet<u128>) -> Result<Laid, String> {
    match crate::setup().elsewhere.as_ref() {
        Some(e) if !is_ours(&job.ty.template) => e.layout(job, known),
        _ => layout_here(job, known),
    }
}

/// `job` as a PDF.
pub fn pdf(job: &Job) -> Result<Vec<u8>, String> {
    match crate::setup().elsewhere.as_ref() {
        Some(e) if !is_ours(&job.ty.template) => e.pdf(job),
        _ => pdf_here(job),
    }
}

/// `job` laid out in this process, whatever its template.
pub fn layout_here(job: &Job, known: &HashSet<u128>) -> Result<Laid, String> {
    here(job).map(|l| laid(&l, known))
}

/// `job` as a PDF, set in this process, whatever its template.
pub fn pdf_here(job: &Job) -> Result<Vec<u8>, String> {
    here(job)?.pdf().map_err(|e| format!("{e:#}"))
}

/// Check, in this process, that the template `key` sets a sample in each
/// of `layouts`: what makes an installed template one the views can set.
pub fn check_here(key: &str, layouts: &[String]) -> Result<(), String> {
    typesetter()
        .check(key, layouts)
        .map_err(|e| format!("{e:#}"))
}

/// Whether a template is twig-export's own, and set in this process.
fn is_ours(t: &Template) -> bool {
    twig_export::templates::builtin(t.key()).is_some()
}

/// The process's typesetter: the host's folder of templates, if it has
/// one, and today, which many templates date a document by.
fn typesetter() -> &'static Typesetter {
    static TYPESETTER: OnceLock<Typesetter> = OnceLock::new();
    TYPESETTER.get_or_init(|| {
        let setup = crate::setup();
        if !setup.fonts.is_empty() {
            leaf_dioxus::trace(|| {
                format!(
                    "typeset: {} fonts of the host's not taken: twig-export takes none yet",
                    setup.fonts.len()
                )
            });
        }
        let packages = match &setup.packages {
            Some(dir) => Packages::new([dir.clone()]),
            None => Packages::default(),
        };
        Typesetter::new()
            .with_packages(packages)
            .with_today(Some(today()))
    })
}

/// `job` laid out in this process.
pub fn here(job: &Job) -> Result<Layout, String> {
    let tree = leaf_core::parse_tree(&job.source, job.format).map_err(|e| e.to_string())?;
    let cfg = crate::pdf::config(&job.ty, job.sheet);
    let words = twig_export::render_words(tree, &job.source, &cfg).map_err(|e| format!("{e:#}"))?;
    here_words(&words, root(job.path.as_deref()))
}

/// Words already rendered, laid out in this process.
pub fn here_words(words: &twig_export::Words, root: &Path) -> Result<Layout, String> {
    typesetter()
        .lay_out(words, root)
        .map_err(|e| format!("{e:#}"))
}

/// The folder a document at `path` finds its pictures in.
pub fn root(path: Option<&Path>) -> &Path {
    path.and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Today's date, in UTC, as (year, month, day).
fn today() -> (i32, u8, u8) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}
