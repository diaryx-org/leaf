//! The editors leaf runs itself for a region of a document, behind one
//! interface, chosen by the region's kind.
//!
//! A session answers keys, typing, a paste and clicks with splices on the
//! region's content — the bytes a frontend's `RegionView` names — and draws
//! its picture with its caret in it. The frontend applies the splices to the
//! document as one undo step, so the document stays the source of truth. The
//! bindings export [`open`] once, as `EditSession`, so a second editor
//! written in Rust needs no new binding: it implements [`Editor`] and joins
//! [`open`] and [`KINDS`].
//!
//! Only `math` has an editor here today, [`leaf_math_edit`]. A region a host
//! edits in a view of its own does not come through this crate.

use std::ops::Range;

/// The kinds of region a session can be opened for.
pub const KINDS: &[&str] = &["math"];

/// A key a session understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A typed character.
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Backspace,
    /// Forward delete.
    Delete,
    /// ⇧← and ⇧→: move the caret, extending the selection.
    ExtendLeft,
    ExtendRight,
    /// The whole region.
    SelectAll,
}

/// One splice a session made: replace the bytes `range` of its content, as
/// the splice before this one left it, with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// How a session draws its picture: `size` pixels (or points) per em, the
/// ink, and the caret's colour, as RGBA bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub size: f64,
    pub ink: [u8; 4],
    pub accent: [u8; 4],
}

/// A session's picture: a standalone SVG, and its advance, its rise above
/// the baseline and its reach below it, in em of the size it was drawn at.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
}

/// Why a session could not open, take new content, or draw: what went wrong,
/// and the byte offset into the content where, when it can say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub at: Option<usize>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// One region open in an editor leaf runs.
pub trait Editor: Send {
    /// Apply a key, returning the splices it made, in order.
    fn key(&mut self, key: Key) -> Vec<Splice>;
    /// Paste `text` at the caret, over the selection.
    fn paste(&mut self, text: &str) -> Vec<Splice>;
    /// The selection, as the content's own text — what a copy takes — or
    /// `None` with nothing selected.
    fn selected(&self) -> Option<String>;
    /// The caret to the first stop, as → into the region from in front.
    fn home(&mut self);
    /// The caret to the last stop, as ← into the region from behind.
    fn end(&mut self);
    /// Whether ← would leave the region.
    fn at_home(&self) -> bool;
    /// Whether → would leave it.
    fn at_end(&self) -> bool;
    /// The caret to the stop nearest `(x, y)`, in em from the picture's top
    /// left.
    fn click(&mut self, x: f64, y: f64);
    /// Take `content` as the region's after an undo or an outside change.
    fn reload(&mut self, content: &str) -> Result<(), Error>;
    /// The content as the session holds it.
    fn content(&self) -> String;
    /// The region with its caret and selection drawn in.
    fn picture(&self, style: &Style) -> Result<Picture, Error>;
}

/// Open `content`, a region of `kind`, for editing, the caret at its end.
/// `display` is a formula's style. An error for a kind no editor here edits,
/// or content its editor cannot read — a frontend leaves that region to be
/// edited as its source.
pub fn open(kind: &str, content: &str, display: bool) -> Result<Box<dyn Editor>, Error> {
    match kind {
        "math" => Ok(Box::new(Math::open(content, display)?)),
        _ => Err(Error {
            message: format!("no editor for regions of kind {kind:?}"),
            at: None,
        }),
    }
}

/// A formula edited as structure, by [`leaf_math_edit`].
struct Math(leaf_math_edit::edit::Session);

impl Math {
    fn open(tex: &str, display: bool) -> Result<Self, Error> {
        let mut session = leaf_math_edit::edit::Session::new(tex).map_err(parse_error)?;
        session.display = display;
        Ok(Math(session))
    }
}

fn parse_error(e: leaf_math_edit::tree::ParseError) -> Error {
    Error {
        message: e.message,
        at: Some(e.at),
    }
}

fn splices(list: Vec<leaf_math_edit::edit::Splice>) -> Vec<Splice> {
    list.into_iter()
        .map(|s| Splice {
            range: s.range,
            text: s.text,
        })
        .collect()
}

impl Editor for Math {
    fn key(&mut self, key: Key) -> Vec<Splice> {
        use leaf_math_edit::edit::Key as K;
        let k = match key {
            Key::Char(c) => K::Char(c),
            Key::Left => K::Left,
            Key::Right => K::Right,
            Key::Up => K::Up,
            Key::Down => K::Down,
            Key::Backspace => K::Backspace,
            Key::Delete => K::Delete,
            Key::ExtendLeft => K::ShiftLeft,
            Key::ExtendRight => K::ShiftRight,
            Key::SelectAll => K::SelectAll,
        };
        splices(self.0.key(k))
    }

    fn paste(&mut self, text: &str) -> Vec<Splice> {
        splices(self.0.paste(text))
    }

    fn selected(&self) -> Option<String> {
        self.0.selected_tex().map(str::to_string)
    }

    fn home(&mut self) {
        self.0.home();
    }

    fn end(&mut self) {
        self.0.end();
    }

    fn at_home(&self) -> bool {
        self.0.at_home()
    }

    fn at_end(&self) -> bool {
        self.0.at_end()
    }

    fn click(&mut self, x: f64, y: f64) {
        self.0.click(x, y);
    }

    fn reload(&mut self, content: &str) -> Result<(), Error> {
        self.0.reload(content).map_err(parse_error)
    }

    fn content(&self) -> String {
        self.0.tex().to_string()
    }

    /// Empty slots are drawn in a third of the ink.
    fn picture(&self, style: &Style) -> Result<Picture, Error> {
        let ink = style.ink;
        let style = leaf_math_edit::layout::Style {
            size: style.size,
            pad: 0.0,
            ink,
            faint: [ink[0], ink[1], ink[2], ink[3] / 3],
            accent: style.accent,
        };
        let p = self
            .0
            .picture(&style)
            .map_err(|message| Error { message, at: None })?;
        Ok(Picture {
            svg: p.svg,
            width: p.width,
            height: p.height,
            depth: p.depth,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: Style = Style {
        size: 16.0,
        ink: [0, 0, 0, 255],
        accent: [0, 0, 255, 255],
    };

    #[test]
    fn a_math_session_answers_keys_with_splices_on_its_content() {
        let mut s = open("math", "x", false).unwrap();
        let sp = s.key(Key::Char('/'));
        assert_eq!(sp.len(), 1);
        assert_eq!(
            (sp[0].range.clone(), sp[0].text.as_str()),
            (0..1, "\\frac{x}{}")
        );
        assert_eq!(s.content(), "\\frac{x}{}");
        s.end();
        assert!(s.at_end() && !s.at_home());
        assert!(
            s.picture(&STYLE).unwrap().svg.contains("<rect"),
            "the caret is drawn"
        );
        // A selection: copied as TeX, pasted over, shaded in the picture.
        assert!(s.key(Key::SelectAll).is_empty());
        assert_eq!(s.selected().as_deref(), Some("\\frac{x}{}"));
        assert!(s.picture(&STYLE).unwrap().svg.contains("fill-opacity"));
        assert_eq!(
            s.paste("y").len(),
            2,
            "the selection's removal, then the paste"
        );
        assert_eq!(s.content(), "y");
        assert!(s.selected().is_none());
        s.home();
        assert_eq!(s.key(Key::Delete).len(), 1);
        assert_eq!(s.content(), "");
        s.reload("a+b").unwrap();
        assert_eq!(s.content(), "a+b");
        assert!(s.reload("\\frac{").is_err());
    }

    #[test]
    fn a_kind_with_no_editor_or_content_it_cannot_read_does_not_open() {
        let e = open("fence:mermaid", "graph LR", false).err().unwrap();
        assert!(e.message.contains("fence:mermaid"));
        let e = open("math", "\\frac{", false).err().unwrap();
        assert!(e.at.is_some());
        assert_eq!(KINDS, &["math"]);
    }
}
