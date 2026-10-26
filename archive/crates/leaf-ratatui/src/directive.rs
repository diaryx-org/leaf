//! A host's directives: the ones it names, inserts and draws.
//!
//! leaf reads every leaf directive (`::embed{src=…}`) a document carries and
//! draws each as a one-row `⧉ name` placeholder, because the vocabulary is the
//! host's and leaf knows how to draw none of it. This module is the terminal's
//! two halves of the host's side, as `docs/proposals/host-directives.md` lays
//! them out:
//!
//! - **The drawing.** A [`DirectiveRenderer`], installed with
//!   [`EditorState::set_directive_renderer`](crate::EditorState::set_directive_renderer),
//!   is asked for the lines to draw for each directive at the width it has. The
//!   surface reports each answer's line count to core through
//!   [`Doc::set_directive_rows`], the way a picture's height goes through
//!   `set_media_rows`, so the placeholder is laid out that tall, and draws the
//!   lines over the rows it reserved. The lines hold no caret: the caret steps
//!   over the directive in one keypress each way, as it does over the
//!   placeholder. An answer of `None` leaves the placeholder as it is.
//! - **The catalogue.** A [`DirectiveItem`] is one row a host offers to insert —
//!   its id, the name written, the title and glyph a menu shows, and either the
//!   label and attributes to write or a question to ask the author first.
//!   Where it is offered is the host's: leaf-tui lists its items in the command
//!   palette, dimmed where [`Capabilities::directives`](leaf_core::Capabilities)
//!   is false.
//!
//! `::page-break` is leaf's own name and never reaches a renderer: the surface
//! draws it itself, as the dashed rule it has always been.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leaf_core::{DirectiveInfo, DirectiveKey, Doc};
use ratatui::text::Line;

use crate::style::Theme;

/// Draws a host's leaf directives for the terminal.
///
/// Asked once per frame for each distinct directive the rich view lays out —
/// distinct by [`DirectiveKey`], its name, label and attributes, so two
/// directives spelled alike are asked about once and drawn alike — with the
/// columns it may fill and the palette the surface is painting with. The answer
/// is the lines to draw, one per row, or `None` for a directive this host does
/// not draw, which keeps leaf's `⧉ name` placeholder. An empty answer is taken
/// as `None`.
///
/// Asked every frame rather than once: a drawing that depends on something the
/// host learns later (a title fetched for an embed) is then simply the next
/// frame's answer, and a height that changes with it is reported and laid out
/// on that frame. So a drawing should be cheap to produce, or cached by the
/// host. What reaches core is only the line count, and core rebuilds only when
/// one changes — a width change, a new directive, a different answer.
///
/// Lines wider than `width` are clipped. The drawing is display-only as far as
/// leaf is concerned: it holds no caret, and a click on it lands where a click
/// on the placeholder does.
///
/// A closure `FnMut(&DirectiveInfo, u16, &Theme) -> Option<Vec<Line<'static>>>`
/// is a renderer.
pub trait DirectiveRenderer {
    /// The lines to draw for `directive` in `width` columns, or `None`.
    fn draw(
        &mut self,
        directive: &DirectiveInfo,
        width: u16,
        theme: &Theme,
    ) -> Option<Vec<Line<'static>>>;
}

impl<F> DirectiveRenderer for F
where
    F: FnMut(&DirectiveInfo, u16, &Theme) -> Option<Vec<Line<'static>>>,
{
    fn draw(
        &mut self,
        directive: &DirectiveInfo,
        width: u16,
        theme: &Theme,
    ) -> Option<Vec<Line<'static>>> {
        self(directive, width, theme)
    }
}

/// Ask `renderer` for every directive in the map `doc` last built, at `width`
/// columns, and report the line counts to core. Returns the drawings by key for
/// the paint pass; the caller rebuilds the map afterwards, which is a cache hit
/// unless a count changed.
///
/// `page-break` is skipped: it is leaf's to draw. A directive inside a quote or
/// a list is asked about at the width its placeholder has after the gutter —
/// the first occurrence's, for a key that stands in more than one place.
pub(crate) fn reserve(
    doc: &mut Doc,
    renderer: Option<&mut Box<dyn DirectiveRenderer>>,
    width: usize,
    theme: &Theme,
) -> HashMap<DirectiveKey, Vec<Line<'static>>> {
    let mut drawings: HashMap<DirectiveKey, Vec<Line<'static>>> = HashMap::new();
    if let Some(renderer) = renderer {
        let mut asked: HashSet<DirectiveKey> = HashSet::new();
        for info in &doc.vmap.directives {
            if info.name == leaf_core::PAGE_BREAK {
                continue;
            }
            let key = info.key();
            if !asked.insert(key.clone()) {
                continue;
            }
            let indent = indent_of(doc, info);
            let cols = width.saturating_sub(indent) as u16;
            if let Some(lines) = renderer.draw(info, cols, theme)
                && !lines.is_empty()
            {
                drawings.insert(key, lines);
            }
        }
    }
    doc.set_directive_rows(
        drawings
            .iter()
            .map(|(key, lines)| (key.clone(), lines.len()))
            .collect(),
    );
    drawings
}

/// The columns in front of a directive's placeholder label on its first row —
/// a quote's gutter, a list's indent — which a drawing is placed after, so the
/// structure it sits in still shows. Core writes the label as `⧉ ` and the
/// shown text, every glyph of it at the directive's start, after whatever
/// prefix the block it is nested in carries.
pub(crate) fn indent_of(doc: &Doc, info: &DirectiveInfo) -> usize {
    let Some(row) = doc.vmap.rows.get(info.rows_span.start) else {
        return 0;
    };
    let shown = if info.label.is_empty() {
        &info.name
    } else {
        &info.label
    };
    let label = 2 + shown.chars().count();
    row.glyphs.len().saturating_sub(label)
}

/// What the author is asked for, and what their answer writes. See
/// [`DirectiveFill::Ask`].
pub type DirectiveAnswer = Arc<dyn Fn(&str) -> Option<DirectiveContent> + Send + Sync>;

/// What a catalogue row writes: a label (none for most) and attributes in the
/// order they are written, a `None` value a bare attribute — the arguments of
/// [`Doc::insert_directive`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectiveContent {
    pub label: Option<String>,
    pub attrs: Vec<(String, Option<String>)>,
}

/// Where a catalogue row's label and attributes come from.
#[derive(Clone)]
pub enum DirectiveFill {
    /// Written as given, the moment the row is chosen.
    Fixed(DirectiveContent),
    /// Asked of the author first: the host puts up a one-line prompt titled
    /// `prompt`, and `answer` turns what was typed into what is written — or
    /// into `None`, which writes nothing (an empty URL, say).
    Ask {
        prompt: String,
        answer: DirectiveAnswer,
    },
}

/// One directive a host offers to insert — the terminal's catalogue row.
#[derive(Clone)]
pub struct DirectiveItem {
    /// Stable across releases, as a tool id is (`"directive.embed"`), so a
    /// host's saved arrangement of its rows survives.
    pub id: String,
    /// The directive's name — what [`Doc::insert_directive`] writes.
    pub name: String,
    /// The row's text, in the host's language (`"Embed…"`).
    pub title: String,
    /// A glyph for the row, where the host's menu shows one.
    pub glyph: String,
    /// What the row writes.
    pub fill: DirectiveFill,
}

impl DirectiveItem {
    /// A row that writes `content` as soon as it is chosen.
    pub fn fixed(
        id: impl Into<String>,
        name: impl Into<String>,
        title: impl Into<String>,
        glyph: impl Into<String>,
        content: DirectiveContent,
    ) -> Self {
        DirectiveItem {
            id: id.into(),
            name: name.into(),
            title: title.into(),
            glyph: glyph.into(),
            fill: DirectiveFill::Fixed(content),
        }
    }

    /// A row that asks the author `prompt` and writes what `answer` makes of
    /// the reply.
    pub fn ask(
        id: impl Into<String>,
        name: impl Into<String>,
        title: impl Into<String>,
        glyph: impl Into<String>,
        prompt: impl Into<String>,
        answer: impl Fn(&str) -> Option<DirectiveContent> + Send + Sync + 'static,
    ) -> Self {
        DirectiveItem {
            id: id.into(),
            name: name.into(),
            title: title.into(),
            glyph: glyph.into(),
            fill: DirectiveFill::Ask {
                prompt: prompt.into(),
                answer: Arc::new(answer),
            },
        }
    }

    /// The question the host must put to the author before this row can
    /// write anything, or `None` for a row that writes as it stands.
    pub fn prompt(&self) -> Option<&str> {
        match &self.fill {
            DirectiveFill::Fixed(_) => None,
            DirectiveFill::Ask { prompt, .. } => Some(prompt),
        }
    }

    /// Choose the row. A [`Fixed`](DirectiveFill::Fixed) row writes its
    /// directive at the caret and returns `None`; an
    /// [`Ask`](DirectiveFill::Ask) row writes nothing and returns its prompt,
    /// for the host to put up and hand the reply to
    /// [`answer`](Self::answer).
    ///
    /// Core refuses, with a status, where the format cannot spell the
    /// directive or the document is read-only.
    pub fn insert(&self, doc: &mut Doc) -> Option<&str> {
        match &self.fill {
            DirectiveFill::Fixed(content) => {
                self.write(doc, content);
                None
            }
            DirectiveFill::Ask { prompt, .. } => Some(prompt),
        }
    }

    /// Write this row's directive from the author's `reply` to its prompt. A
    /// fixed row ignores the reply and writes what it always does.
    pub fn answer(&self, doc: &mut Doc, reply: &str) {
        match &self.fill {
            DirectiveFill::Fixed(content) => self.write(doc, content),
            DirectiveFill::Ask { answer, .. } => {
                if let Some(content) = answer(reply) {
                    self.write(doc, &content);
                }
            }
        }
    }

    fn write(&self, doc: &mut Doc, content: &DirectiveContent) {
        doc.insert_directive(&self.name, content.label.as_deref(), &content.attrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::Format;

    fn url(reply: &str) -> Option<DirectiveContent> {
        let reply = reply.trim();
        (!reply.is_empty()).then(|| DirectiveContent {
            label: None,
            attrs: vec![("src".into(), Some(reply.into()))],
        })
    }

    /// A fixed row writes as soon as it is chosen; an asking row writes
    /// nothing until it is answered, and nothing at all for an answer its
    /// callback turns down.
    #[test]
    fn a_fixed_row_writes_at_once_and_an_asking_row_waits_for_its_answer() {
        let toc = DirectiveItem::fixed(
            "directive.toc",
            "toc",
            "Contents",
            "≡",
            DirectiveContent::default(),
        );
        let mut doc = Doc::from_source("a\n".into(), Format::Markdown).unwrap();
        doc.caret = 1;
        assert_eq!(toc.insert(&mut doc), None);
        assert_eq!(doc.source, "a\n\n::toc\n\n");

        let embed = DirectiveItem::ask("directive.embed", "embed", "Embed…", "⧉", "Embed URL", url);
        let mut doc = Doc::from_source("a\n".into(), Format::Markdown).unwrap();
        doc.caret = 1;
        assert_eq!(embed.insert(&mut doc), Some("Embed URL"));
        assert_eq!(doc.source, "a\n", "asking writes nothing");
        embed.answer(&mut doc, "  ");
        assert_eq!(doc.source, "a\n", "an answer turned down writes nothing");
        embed.answer(&mut doc, "https://x.org");
        assert_eq!(doc.source, "a\n\n::embed{src=\"https://x.org\"}\n\n");
    }

    /// Where the format cannot spell a host's directive, core refuses with a
    /// status and the document is untouched.
    #[test]
    fn a_row_in_a_format_without_directives_writes_nothing() {
        let embed = DirectiveItem::ask("directive.embed", "embed", "Embed…", "⧉", "Embed URL", url);
        let mut doc = Doc::from_source("<p>a</p>\n".into(), Format::Html).unwrap();
        assert!(!doc.capabilities().directives);
        doc.caret = 4;
        embed.answer(&mut doc, "https://x.org");
        assert_eq!(doc.source, "<p>a</p>\n");
        assert!(doc.status.is_some());
    }
}
