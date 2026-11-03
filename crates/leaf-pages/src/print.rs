//! Export as PDF, set exactly as the page view sets it: the one place a
//! host's PDF and the page view on its screen are told what to set, so that
//! what prints is what the reader saw.
//!
//! The page view ([`crate::Pages::update`]) and a host's Export as PDF used
//! to say each for itself what the document is set in — the template, the
//! body size, the paper, where its pictures are, the room under a card the
//! host draws — and a host that said one thing in one place and another in
//! the other printed pages that broke elsewhere than the ones on its screen.
//! Both now ask here: [`on_paper`] for the type and the sheet, and [`job`]
//! for the whole of a setting. `parity.rs` holds the two to the same pages.

use std::sync::Arc;

use leaf_core::Doc;

use crate::Choices;
use crate::pdf::{Paper, Sheet, Type};
use crate::typeset::{Job, Supply};

/// The margin the page view sets a sheet at, in points: an inch.
pub const MARGIN: f32 = 72.0;

/// What the page view sets a document in, as the host's `choices` say: its
/// template, its body size (a CSS pixel printed at three quarters of a
/// point, as a browser prints: 16 pixels is 12 points), dark ink never,
/// since a sheet is paper; on the choices' paper at an inch's margin.
pub fn on_paper(choices: &Choices) -> (Type, Paper) {
    let (width, height) = choices.paper;
    (
        Type {
            template: choices.template.clone(),
            size: choices.size * 0.75,
            dark: false,
        },
        Paper {
            width,
            height,
            margin: MARGIN,
        },
    )
}

/// The document `d` set as the page view sets it, in `choices`: its source
/// as the editor holds it, its pictures where the editor finds them
/// (through `host`; without one, beside the document), and the room the page
/// view leaves under each card the host draws, so that every line and every
/// page break falls where it falls on the screen.
pub fn job(d: &mut Doc, host: Option<&dyn leaf_dioxus::Host>, choices: &Choices) -> Job {
    let (ty, paper) = on_paper(choices);
    let supply = match host {
        Some(host) => crate::gather(d, host, ty.dark).0,
        None => Supply::default(),
    };
    Job {
        source: d.source.clone(),
        format: d.format,
        path: Some(d.path.clone()),
        ty,
        sheet: Sheet::Paper(paper),
        supply: Arc::new(supply),
    }
}

/// The document `d` as a PDF, set as the page view sets it ([`job`]).
pub fn print(
    d: &mut Doc,
    host: Option<&dyn leaf_dioxus::Host>,
    choices: &Choices,
) -> Result<Vec<u8>, String> {
    crate::typeset::pdf(&job(d, host, choices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Template;

    fn choices(size: f32) -> Choices {
        Choices {
            template: Template::default(),
            size,
            zoom: 1.5,
            paper: (595.0, 842.0),
        }
    }

    /// Paper is set at the body size a browser prints the choices' at, in
    /// light ink, on their paper; the window's text zoom is the reflowed
    /// view's, not paper's.
    #[test]
    fn a_sheet_is_set_as_the_choices_print() {
        let (ty, paper) = on_paper(&choices(16.0));
        assert_eq!(ty.size, 12.0);
        assert!(!ty.dark);
        assert_eq!(
            (paper.width, paper.height, paper.margin),
            (595.0, 842.0, 72.0)
        );
    }

    /// The editor's source is what prints, as the page view sets it: not
    /// the file's, front matter and all.
    #[test]
    fn what_prints_is_the_editors_source() {
        let mut d = Doc::from_source("Words.\n".into(), leaf_core::Format::Markdown).unwrap();
        let job = job(&mut d, None, &choices(18.0));
        assert_eq!(job.source, "Words.\n");
        assert_eq!(job.ty.size, 13.5);
        assert!(matches!(job.sheet, Sheet::Paper(_)));
    }
}
