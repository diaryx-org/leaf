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
/// template, its body size on paper (`paper_size`, else `size`: a CSS pixel
/// printed at three quarters of a point, as a browser prints: 16 pixels is
/// 12 points), dark ink never, since a sheet is paper; on the choices'
/// paper at an inch's margin.
pub fn on_paper(choices: &Choices) -> (Type, Paper) {
    let (width, height) = choices.paper;
    (
        Type {
            template: choices.template.clone(),
            size: choices.paper_size.unwrap_or(choices.size) * 0.75,
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
/// page break falls where it falls on the screen. In each room, the card as
/// the host prints it ([`leaf_dioxus::Host::print_directive`]), as wide as
/// the text and as tall as the room.
pub fn job(d: &mut Doc, host: Option<&dyn leaf_dioxus::Host>, choices: &Choices) -> Job {
    let (ty, paper) = on_paper(choices);
    let supply = match host {
        Some(host) => {
            let (mut supply, asked) = crate::gather(d, host, ty.dark);
            let width = paper.width - 2.0 * paper.margin;
            for (at, directive) in asked {
                let Some(&height) = supply.rooms.get(&at.start) else {
                    continue;
                };
                if let Some(card) = host.print_directive(d, &directive, width, height) {
                    supply.drawings.insert(at.start, card);
                }
            }
            supply
        }
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
            paper_size: None,
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
    /// Paper is set at its own size where the choices name one, whatever
    /// the reflowed view's.
    #[test]
    fn paper_has_a_size_of_its_own() {
        let c = Choices {
            paper_size: Some(16.0),
            ..choices(18.0)
        };
        assert_eq!(on_paper(&c).0.size, 12.0);
    }

    /// A host that draws a card 66 pixels tall under every `::page`, and
    /// prints `card` in it.
    struct Carding {
        card: Option<&'static str>,
    }

    impl leaf_dioxus::Host for Carding {
        fn directive_height(&self, _doc: &Doc, d: &leaf_dioxus::Directive) -> Option<f32> {
            (d.name == "page").then_some(66.0)
        }

        fn print_directive(
            &self,
            _doc: &Doc,
            _d: &leaf_dioxus::Directive,
            width: f32,
            height: f32,
        ) -> Option<Arc<[u8]>> {
            assert_eq!((width, height), (595.0 - 144.0, 49.5));
            self.card.map(|c| Arc::from(c.as_bytes()))
        }
    }

    /// The card a host prints is drawn in the room it leaves under the
    /// directive, which is the room the page view leaves there.
    #[test]
    fn a_card_is_printed_in_its_room() {
        const CARD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="451pt" height="49.5pt"><rect width="451" height="49.5" fill="none" stroke="black"/></svg>"#;
        let source = "Before.\n\n::page{src=\"Other.md\"}\n\nAfter.\n";
        let mut d = Doc::from_source(source.into(), leaf_core::Format::Markdown).unwrap();
        let blank = job(&mut d, Some(&Carding { card: None }), &choices(16.0));
        let carded = job(&mut d, Some(&Carding { card: Some(CARD) }), &choices(16.0));
        assert_eq!(blank.supply.rooms, carded.supply.rooms);
        assert_eq!(
            blank.supply.rooms.values().copied().collect::<Vec<_>>(),
            [49.5]
        );
        assert!(blank.supply.drawings.is_empty());
        assert_eq!(carded.supply.drawings.len(), 1);
        let (blank, carded) = (
            crate::typeset::pdf(&blank).unwrap(),
            crate::typeset::pdf(&carded).unwrap(),
        );
        assert_ne!(blank, carded, "the card is drawn");
    }

    #[test]
    fn what_prints_is_the_editors_source() {
        let mut d = Doc::from_source("Words.\n".into(), leaf_core::Format::Markdown).unwrap();
        let job = job(&mut d, None, &choices(18.0));
        assert_eq!(job.source, "Words.\n");
        assert_eq!(job.ty.size, 13.5);
        assert!(matches!(job.sheet, Sheet::Paper(_)));
    }
}
