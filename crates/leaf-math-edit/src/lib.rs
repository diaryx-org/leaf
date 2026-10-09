//! Spike: a structural math editor whose file format stays TeX.
//!
//! A [`Formula`] is TeX source plus the tree [`tree::read`] makes of it,
//! flattened into rows and atoms with stable ids for one revision. The caret
//! lives on a row, between two atoms. A keystroke ([`edit`]) becomes one or
//! two byte splices on the TeX, the formula is reread, and the caret is found
//! again by where it landed in the new source — so an edit never rewrites a
//! byte it did not name, and the TeX an author wrote by hand survives the
//! editor.
//!
//! Geometry ([`layout`]) comes from RaTeX without patching it: the formula is
//! typeset once with every atom wrapped in `\htmlStyle{color: #id}`, a wrapper
//! RaTeX lays out as its contents (same math class, same spacing), and the
//! display list's colours are read back as atom ids.
//!
//! [`doc`] finds formulas in a document through twig and applies a formula's
//! splices to the document.

#[cfg(feature = "doc")]
pub mod doc;
pub mod edit;
pub mod layout;
pub mod tree;

use std::ops::Range;
use tree::{AtomKind, Row};

pub use tree::ParseError;

/// One row of the flattened tree.
#[derive(Clone, Debug)]
pub struct FlatRow {
    pub content: Range<usize>,
    pub braced: bool,
    /// Atom ids, left to right.
    pub atoms: Vec<usize>,
    /// The atom this row is a slot of; `None` for the root.
    pub parent: Option<usize>,
}

/// One atom of the flattened tree.
#[derive(Clone, Debug)]
pub struct FlatAtom {
    pub span: Range<usize>,
    pub kind: Kind,
    /// The row this atom sits in.
    pub row: usize,
    /// The atom's slots, in source order.
    pub rows: Vec<usize>,
}

/// [`AtomKind`] without its children, which the flat form holds as ids.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Char(char),
    Symbol(String),
    Space,
    Sup,
    Sub,
    Group,
    Command(String),
    LeftRight,
    Opaque,
}

impl Kind {
    pub fn is_script(&self) -> bool {
        matches!(self, Kind::Sup | Kind::Sub | Kind::Char('\''))
    }
}

/// A caret position: on `row`, before the atom at `index` (`index ==
/// atoms.len()` is the row's end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub index: usize,
}

/// A formula's source and the tree read from it.
#[derive(Clone, Debug)]
pub struct Formula {
    pub src: String,
    pub rows: Vec<FlatRow>,
    pub atoms: Vec<FlatAtom>,
    /// Every caret position, in the order → visits them: a row's stops left
    /// to right, with each atom's slots visited between the stop before it
    /// and the stop after it.
    pub stops: Vec<Caret>,
    /// Kept for [`tree::Atom::is_operator`].
    tree_atoms: Vec<tree::Atom>,
}

impl Formula {
    pub fn new(src: impl Into<String>) -> Result<Self, ParseError> {
        let src = src.into();
        let root = tree::read(&src)?;
        let mut f = Formula {
            src,
            rows: Vec::new(),
            atoms: Vec::new(),
            stops: Vec::new(),
            tree_atoms: Vec::new(),
        };
        f.flatten_row(&root, None);
        f.stops = f.collect_stops(0);
        Ok(f)
    }

    fn flatten_row(&mut self, row: &Row, parent: Option<usize>) -> usize {
        let id = self.rows.len();
        self.rows.push(FlatRow {
            content: row.content.clone(),
            braced: row.braced,
            atoms: Vec::new(),
            parent,
        });
        for atom in &row.atoms {
            let aid = self.atoms.len();
            let kind = match &atom.kind {
                AtomKind::Char(c) => Kind::Char(*c),
                AtomKind::Symbol(s) => Kind::Symbol(s.clone()),
                AtomKind::Space => Kind::Space,
                AtomKind::Sup(_) => Kind::Sup,
                AtomKind::Sub(_) => Kind::Sub,
                AtomKind::Group(_) => Kind::Group,
                AtomKind::Command { name, .. } => Kind::Command(name.clone()),
                AtomKind::LeftRight { .. } => Kind::LeftRight,
                AtomKind::Opaque => Kind::Opaque,
            };
            self.atoms.push(FlatAtom {
                span: atom.span.clone(),
                kind,
                row: id,
                rows: Vec::new(),
            });
            self.tree_atoms.push(atom.clone());
            self.rows[id].atoms.push(aid);
            let slots: Vec<usize> = atom
                .rows()
                .into_iter()
                .map(|r| self.flatten_row(r, Some(aid)))
                .collect();
            self.atoms[aid].rows = slots;
        }
        id
    }

    fn collect_stops(&self, row: usize) -> Vec<Caret> {
        let mut out = Vec::new();
        let atoms = &self.rows[row].atoms;
        for i in 0..=atoms.len() {
            out.push(Caret { row, index: i });
            if let Some(&a) = atoms.get(i) {
                for &r in &self.atoms[a].rows {
                    out.extend(self.collect_stops(r));
                }
            }
        }
        out
    }

    /// The caret at the end of the formula.
    pub fn end(&self) -> Caret {
        Caret {
            row: 0,
            index: self.rows[0].atoms.len(),
        }
    }

    /// The source offset a caret stands at: the start of the atom after it,
    /// or the end of its row's content.
    pub fn offset(&self, c: Caret) -> usize {
        let row = &self.rows[c.row];
        match row.atoms.get(c.index) {
            Some(&a) => self.atoms[a].span.start,
            None => row.content.end,
        }
    }

    /// The caret standing at `offset` on the row whose content starts at
    /// `row_start` — how a caret is found again after an edit rewrites the
    /// tree. Falls back to any stop at `offset`, then to the nearest.
    pub fn caret_at(&self, offset: usize, row_start: usize) -> Caret {
        let at: Vec<Caret> = self
            .stops
            .iter()
            .copied()
            .filter(|&c| self.offset(c) == offset)
            .collect();
        if let Some(&c) = at
            .iter()
            .find(|c| self.rows[c.row].content.start == row_start)
        {
            return c;
        }
        if let Some(&c) = at.first() {
            return c;
        }
        *self
            .stops
            .iter()
            .min_by_key(|&&c| self.offset(c).abs_diff(offset))
            .unwrap()
    }

    pub fn is_operator(&self, atom: usize) -> bool {
        self.tree_atoms[atom].is_operator()
    }

    fn stop_index(&self, c: Caret) -> usize {
        self.stops.iter().position(|&s| s == c).unwrap_or(0)
    }

    /// The stop before `c` in → order, or `c` at the first.
    pub fn left(&self, c: Caret) -> Caret {
        let i = self.stop_index(c);
        self.stops[i.saturating_sub(1)]
    }

    /// The stop after `c` in → order, or `c` at the last.
    pub fn right(&self, c: Caret) -> Caret {
        let i = self.stop_index(c);
        self.stops[(i + 1).min(self.stops.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_visit_slots_between_the_stops_around_their_atom() {
        let f = Formula::new(r"\frac{a}{b}+1").unwrap();
        let offsets: Vec<usize> = f.stops.iter().map(|&c| f.offset(c)).collect();
        // before \frac, num start, after a, den start, after b, before +,
        // before 1, end.
        assert_eq!(offsets, [0, 6, 7, 9, 10, 11, 12, 13]);
    }

    #[test]
    fn a_caret_is_found_again_by_offset_and_row() {
        // `x^2`: the end of the superscript and the end of the formula share
        // offset 3; the row start tells them apart.
        let f = Formula::new("x^2").unwrap();
        let sup = f.caret_at(3, 2);
        assert_eq!(f.rows[sup.row].parent, Some(1));
        let end = f.caret_at(3, 0);
        assert_eq!(end, f.end());
    }
}
#[cfg(feature = "wasm")]
mod wasm;
