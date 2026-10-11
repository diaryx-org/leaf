//! Keystrokes as splices on a formula's TeX.
//!
//! Every command reads the [`Formula`] and the [`Caret`] and returns a
//! [`Splice`]: one byte range of the source to replace, and where the caret
//! goes in the source that results. Applying it is a string splice and a
//! reread ([`Formula::apply`]), or, for a formula inside a document, a twig
//! `edit_range` at the formula's offset ([`crate::doc`]). Nothing outside the
//! range is touched, so spacing, macros and comments the author wrote stay
//! as written.

use crate::{Caret, Formula, Kind};
use std::ops::Range;

/// One edit to a formula's source.
#[derive(Clone, Debug, PartialEq)]
pub struct Splice {
    /// The bytes replaced, in the current source.
    pub range: Range<usize>,
    pub text: String,
    /// Where the caret lands, as an offset into the source after the edit…
    pub caret: usize,
    /// …and the content start of the row it lands on there, which
    /// [`Formula::caret_at`] needs to tell nested rows ending together apart.
    pub row: usize,
}

/// What a keystroke did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The source changed.
    Edit(Splice),
    /// Only the caret moved.
    Move(Caret),
    /// Nothing to do.
    None,
}

/// Names typed as letters that become a command, as MathQuill's
/// auto-commands do: `sqrt` opens a root, `pi` becomes π. A name is left as
/// letters while what has been typed could still be the start of a longer
/// one (`epsi` → `epsilon`, not `e\psi`).
const AUTO: &[&str] = &[
    "sqrt", "pi", "theta", "alpha", "beta", "gamma", "delta", "epsilon", "lambda", "sigma",
    "omega", "phi", "rho", "infty", "sum", "prod", "int", "sin", "cos", "tan", "log", "ln", "lim",
    "exp", "cdot", "times", "pm", "Delta", "Sigma", "Omega", "Gamma", "Pi", "Phi", "partial",
    "nabla", "to", "leq", "geq", "neq", "approx", "in",
];

/// Commands that take a slot, written with an empty one for the caret.
fn auto_slots(name: &str) -> usize {
    match name {
        "sqrt" => 1,
        _ => 0,
    }
}

/// TeX for a typed character that TeX would read as markup.
fn escape(c: char) -> String {
    match c {
        '{' | '}' | '#' | '$' | '%' | '&' => format!("\\{c}"),
        '\\' => "\\backslash".into(),
        '~' => "\\sim".into(),
        '*' => "\\cdot".into(),
        _ => c.to_string(),
    }
}

fn ends_with_control_word(s: &str) -> bool {
    let t = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    t.len() < s.len() && t.ends_with('\\') && !t[..t.len() - 1].ends_with('\\')
}

impl Formula {
    /// Replace `range` — which lies inside `row`'s content — with `text`. An
    /// unbraced row gains braces, since it can only hold one token. A space
    /// is added where the replacement would run a control word into a
    /// letter (`\alpha` then `b`). Returns the splice with the caret placed
    /// `caret_in_text` bytes into `text`, and that position's row start.
    fn replace_in_row(
        &self,
        row: usize,
        range: Range<usize>,
        text: &str,
        caret_in_text: usize,
    ) -> Splice {
        let r = &self.rows[row];
        let src = &self.src;
        let before = &src[..range.start];
        let after = &src[range.end..];
        let mut lead = "";
        if ends_with_control_word(before) && text.starts_with(|c: char| c.is_ascii_alphabetic()) {
            lead = " ";
        }
        let mut trail = "";
        if ends_with_control_word(&format!("{before}{text}"))
            && after.starts_with(|c: char| c.is_ascii_alphabetic())
        {
            trail = " ";
        }
        let new = format!("{lead}{text}{trail}");
        let caret_rel = lead.len() + caret_in_text;
        if r.braced || r.parent.is_none() {
            return Splice {
                range: range.clone(),
                caret: range.start + caret_rel,
                row: r.content.start,
                text: new,
            };
        }
        // `x^2` + `a` in the script: `x^{2a}`.
        let inner = format!(
            "{}{new}{}",
            &src[r.content.start..range.start],
            &src[range.end..r.content.end]
        );
        Splice {
            range: r.content.clone(),
            caret: r.content.start + 1 + (range.start - r.content.start) + caret_rel,
            row: r.content.start + 1,
            text: format!("{{{inner}}}"),
        }
    }

    fn insert(&self, c: Caret, text: &str, caret_in_text: usize) -> Splice {
        let at = self.offset(c);
        self.replace_in_row(c.row, at..at, text, caret_in_text)
    }

    /// Apply `s` and reread, returning the new formula and caret.
    pub fn apply(&self, s: &Splice) -> Result<(Formula, Caret), crate::ParseError> {
        let mut src = self.src.clone();
        src.replace_range(s.range.clone(), &s.text);
        let f = Formula::new(src)?;
        let c = f.caret_at(s.caret, s.row);
        Ok((f, c))
    }

    /// A typed character.
    pub fn type_char(&self, c: Caret, ch: char) -> Outcome {
        match ch {
            '^' => self.script(c, Kind::Sup),
            '_' => self.script(c, Kind::Sub),
            '/' => Outcome::Edit(self.fraction(c)),
            ' ' => self.exit_slot(c),
            _ => {
                let t = escape(ch);
                Outcome::Edit(self.insert(c, &t, t.len()))
            }
        }
    }

    /// After a letter is typed: if the letters before the caret end in an
    /// auto-command's name, the splice that turns them into it.
    pub fn auto_command(&self, c: Caret) -> Option<Splice> {
        let row = &self.rows[c.row];
        let mut start = c.index;
        while start > 0 {
            match self.atoms[row.atoms[start - 1]].kind {
                Kind::Char(ch) if ch.is_ascii_alphabetic() => start -= 1,
                _ => break,
            }
        }
        let letters: String = row.atoms[start..c.index]
            .iter()
            .map(|&a| match self.atoms[a].kind {
                Kind::Char(ch) => ch,
                _ => unreachable!(),
            })
            .collect();
        for k in 0..letters.len() {
            let tail = &letters[k..];
            // Could still become something longer: wait.
            if AUTO
                .iter()
                .any(|n| n.len() > tail.len() && n.starts_with(tail))
            {
                return None;
            }
            if AUTO.contains(&tail) {
                let first = row.atoms[start + k];
                let last = row.atoms[c.index - 1];
                let range = self.atoms[first].span.start..self.atoms[last].span.end;
                let slots = auto_slots(tail);
                let text = format!("\\{tail}{}", "{}".repeat(slots));
                let caret = if slots > 0 {
                    tail.len() + 2
                } else {
                    text.len()
                };
                return Some(self.replace_in_row(c.row, range, &text, caret));
            }
        }
        None
    }

    /// `^` or `_`: step into the script already there, or open one.
    fn script(&self, c: Caret, kind: Kind) -> Outcome {
        let row = &self.rows[c.row];
        // A script already on this base, after the caret or just before it.
        let candidates = [c.index.checked_sub(1), Some(c.index)];
        for i in candidates.into_iter().flatten() {
            if let Some(&a) = row.atoms.get(i)
                && self.atoms[a].kind == kind
            {
                let slot = self.atoms[a].rows[0];
                return Outcome::Move(Caret {
                    row: slot,
                    index: self.rows[slot].atoms.len(),
                });
            }
        }
        let mark = if kind == Kind::Sup { "^{}" } else { "_{}" };
        Outcome::Edit(self.insert(c, mark, 2))
    }

    /// `/`: the run of operands before the caret becomes a numerator, and
    /// the caret goes to the denominator; with no operand before it, an
    /// empty fraction with the caret in the numerator.
    fn fraction(&self, c: Caret) -> Splice {
        let row = &self.rows[c.row];
        let mut start = c.index;
        while start > 0 && !self.is_operator(row.atoms[start - 1]) {
            start -= 1;
        }
        // A script cannot lead a numerator without its base.
        while start < c.index && self.atoms[row.atoms[start]].kind.is_script() {
            start += 1;
        }
        if start == c.index {
            return self.insert(c, "\\frac{}{}", 6);
        }
        let range =
            self.atoms[row.atoms[start]].span.start..self.atoms[row.atoms[c.index - 1]].span.end;
        let num = &self.src[range.clone()];
        let text = format!("\\frac{{{num}}}{{}}");
        let caret = text.len() - 1;
        self.replace_in_row(c.row, range, &text, caret)
    }

    /// Space: leave the slot the caret is in, to just after its atom.
    fn exit_slot(&self, c: Caret) -> Outcome {
        match self.rows[c.row].parent {
            Some(a) => {
                let outer = self.atoms[a].row;
                let index = self.rows[outer].atoms.iter().position(|&x| x == a).unwrap() + 1;
                Outcome::Move(Caret { row: outer, index })
            }
            None => Outcome::None,
        }
    }

    fn all_slots_empty(&self, a: usize) -> bool {
        self.atoms[a]
            .rows
            .iter()
            .all(|&r| self.rows[r].atoms.is_empty())
    }

    /// Backspace.
    pub fn backspace(&self, c: Caret) -> Outcome {
        let row = &self.rows[c.row];
        if let Some(i) = c.index.checked_sub(1) {
            let a = row.atoms[i];
            let atom = &self.atoms[a];
            if atom.rows.is_empty() || self.all_slots_empty(a) {
                return Outcome::Edit(self.replace_in_row(c.row, atom.span.clone(), "", 0));
            }
            // Into the structure, at the end of its last slot, as MathQuill
            // does: a second backspace starts deleting inside it.
            let last = *atom.rows.last().unwrap();
            return Outcome::Move(Caret {
                row: last,
                index: self.rows[last].atoms.len(),
            });
        }
        let Some(p) = row.parent else {
            return Outcome::None;
        };
        let parent = &self.atoms[p];
        let outer = parent.row;
        if self.all_slots_empty(p) {
            return Outcome::Edit(self.replace_in_row(outer, parent.span.clone(), "", 0));
        }
        let slot = parent.rows.iter().position(|&r| r == c.row).unwrap();
        if slot > 0 {
            let prev = parent.rows[slot - 1];
            return Outcome::Move(Caret {
                row: prev,
                index: self.rows[prev].atoms.len(),
            });
        }
        // At the start of the first slot: take the structure apart, its
        // slots' contents left in its place.
        let text: Vec<&str> = parent
            .rows
            .iter()
            .map(|&r| &self.src[self.rows[r].content.clone()])
            .collect();
        let text = text.join(" ").trim().to_string();
        Outcome::Edit(self.replace_in_row(outer, parent.span.clone(), &text, 0))
    }
}

impl Formula {
    /// Delete forward — Backspace's mirror. The atom after the caret goes if
    /// it has no slots or only empty ones; one with something in it is
    /// entered, at the start of its first slot. At the end of a slot the
    /// caret steps into the next one, and at the end of the last the
    /// structure is taken apart, its slots' contents left in its place.
    pub fn delete(&self, c: Caret) -> Outcome {
        let row = &self.rows[c.row];
        if let Some(&a) = row.atoms.get(c.index) {
            let atom = &self.atoms[a];
            if atom.rows.is_empty() || self.all_slots_empty(a) {
                return Outcome::Edit(self.replace_in_row(c.row, atom.span.clone(), "", 0));
            }
            return Outcome::Move(Caret {
                row: atom.rows[0],
                index: 0,
            });
        }
        let Some(p) = row.parent else {
            return Outcome::None;
        };
        let parent = &self.atoms[p];
        let outer = parent.row;
        if self.all_slots_empty(p) {
            return Outcome::Edit(self.replace_in_row(outer, parent.span.clone(), "", 0));
        }
        let slot = parent.rows.iter().position(|&r| r == c.row).unwrap();
        if let Some(&next) = parent.rows.get(slot + 1) {
            return Outcome::Move(Caret {
                row: next,
                index: 0,
            });
        }
        let text: Vec<&str> = parent
            .rows
            .iter()
            .map(|&r| &self.src[self.rows[r].content.clone()])
            .collect();
        let text = text.join(" ").trim().to_string();
        let len = text.len();
        Outcome::Edit(self.replace_in_row(outer, parent.span.clone(), &text, len))
    }

    /// The atoms a selection from `a` to `b` covers, as a row and a range of
    /// its atom indices — `None` when it covers nothing. Both ends are lifted
    /// to the innermost row that holds them both, and an end inside an atom's
    /// slot takes in that whole atom: a selection never splits a fraction.
    pub fn selection(&self, a: Caret, b: Caret) -> Option<(usize, Range<usize>)> {
        // Each end's way out to the top: (row, index, through an atom).
        let path = |c: Caret| {
            let mut out = vec![(c.row, c.index, false)];
            let mut row = c.row;
            while let Some(p) = self.rows[row].parent {
                let outer = self.atoms[p].row;
                let index = self.rows[outer].atoms.iter().position(|&x| x == p).unwrap();
                out.push((outer, index, true));
                row = outer;
            }
            out
        };
        let (pa, pb) = (path(a), path(b));
        let (ea, eb) = pa
            .iter()
            .find_map(|ea| pb.iter().find(|eb| eb.0 == ea.0).map(|eb| (*ea, *eb)))?;
        let bounds = |(_, i, through): (usize, usize, bool)| (i, if through { i + 1 } else { i });
        let ((sa, fa), (sb, fb)) = (bounds(ea), bounds(eb));
        let range = sa.min(sb)..fa.max(fb);
        (!range.is_empty()).then_some((ea.0, range))
    }

    /// The source bytes the atoms `range` of `row` span.
    pub fn atoms_span(&self, row: usize, range: Range<usize>) -> Range<usize> {
        let atoms = &self.rows[row].atoms[range];
        self.atoms[atoms[0]].span.start..self.atoms[*atoms.last().unwrap()].span.end
    }
}

/// A formula being edited: the source, the caret, and the steps that move
/// them. What a frontend holds.
#[derive(Clone, Debug)]
pub struct Session {
    pub formula: Formula,
    pub caret: Caret,
    /// Where a selection started, while there is one being made: the caret
    /// is its other end, and [`Session::selection`] the atoms between.
    pub anchor: Option<Caret>,
    /// Display style, which ↑ and ↓ measure the formula in.
    pub display: bool,
}

/// A key a session understands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Backspace,
    /// Forward delete.
    Delete,
    /// ⇧← and ⇧→: move the caret, extending the selection.
    ShiftLeft,
    ShiftRight,
    /// The whole formula.
    SelectAll,
}

impl Session {
    pub fn new(tex: &str) -> Result<Self, crate::ParseError> {
        let formula = Formula::new(tex)?;
        let caret = formula.end();
        Ok(Session {
            formula,
            caret,
            anchor: None,
            display: false,
        })
    }

    /// The first stop: where → from in front of the formula enters it.
    pub fn home(&mut self) {
        self.caret = self.formula.stops[0];
        self.anchor = None;
    }

    /// The last stop: where ← from behind the formula enters it.
    pub fn end(&mut self) {
        self.caret = self.formula.end();
        self.anchor = None;
    }

    /// The formula drawn with its caret and its selection, in `style`.
    pub fn picture(&self, style: &crate::layout::Style) -> Result<crate::layout::Picture, String> {
        let l = self.formula.layout(self.display)?;
        Ok(l.picture_selecting(&self.formula, style, Some(self.caret), self.selection()))
    }

    /// The selected atoms, as a row and a range of its atom indices.
    pub fn selection(&self) -> Option<(usize, Range<usize>)> {
        self.formula.selection(self.anchor?, self.caret)
    }

    /// The selected atoms' TeX, as written — what a copy takes.
    pub fn selected_tex(&self) -> Option<&str> {
        let (row, range) = self.selection()?;
        Some(&self.formula.src[self.formula.atoms_span(row, range)])
    }

    /// Paste `tex` at the caret, over the selection if there is one. Written
    /// as it is when the formula still reads with it in place; otherwise
    /// typed a character at a time, as though it had been typed, so that
    /// `1/2` from plain text still makes a fraction.
    pub fn paste(&mut self, tex: &str) -> Vec<Splice> {
        let mut splices = self.delete_selection();
        let at = self.formula.offset(self.caret);
        let s = self
            .formula
            .replace_in_row(self.caret.row, at..at, tex, tex.len());
        if !tex.is_empty() && self.commit(&s) {
            splices.push(s);
            return splices;
        }
        for ch in tex.chars().filter(|c| !c.is_control()) {
            splices.extend(self.key(Key::Char(ch)));
        }
        splices
    }

    /// Delete the selection, if there is one, and drop it.
    fn delete_selection(&mut self) -> Vec<Splice> {
        let Some((row, range)) = self.selection() else {
            self.anchor = None;
            return Vec::new();
        };
        self.anchor = None;
        let span = self.formula.atoms_span(row, range);
        let s = self.formula.replace_in_row(row, span, "", 0);
        if self.commit(&s) { vec![s] } else { Vec::new() }
    }

    pub fn at_home(&self) -> bool {
        self.caret == self.formula.stops[0]
    }

    pub fn at_end(&self) -> bool {
        self.caret == self.formula.end()
    }

    /// Take `tex` as the formula's source — an undo, or a change from
    /// elsewhere — keeping the caret at the same offset where it can.
    pub fn reload(&mut self, tex: &str) -> Result<(), crate::ParseError> {
        if tex == self.formula.src {
            return Ok(());
        }
        let at = self.formula.offset(self.caret);
        let row = self.formula.rows[self.caret.row].content.start;
        self.formula = Formula::new(tex)?;
        self.caret = self.formula.caret_at(at.min(tex.len()), row);
        Ok(())
    }

    /// Apply a key, returning the splices it made to the source in order
    /// (each against the source the one before it left), for a host that
    /// mirrors them into a document.
    pub fn key(&mut self, key: Key) -> Vec<Splice> {
        match key {
            Key::ShiftLeft | Key::ShiftRight => {
                let anchor = *self.anchor.get_or_insert(self.caret);
                self.caret = if key == Key::ShiftLeft {
                    self.formula.left(self.caret)
                } else {
                    self.formula.right(self.caret)
                };
                if self.caret == anchor {
                    self.anchor = None;
                }
                return Vec::new();
            }
            Key::SelectAll => {
                self.anchor = Some(Caret { row: 0, index: 0 });
                self.caret = self.formula.end();
                return Vec::new();
            }
            _ => {}
        }
        if let Some((row, range)) = self.selection() {
            match key {
                // An arrow collapses the selection to its edge.
                Key::Left | Key::Right => {
                    let index = if key == Key::Left {
                        range.start
                    } else {
                        range.end
                    };
                    self.caret = Caret { row, index };
                    self.anchor = None;
                    return Vec::new();
                }
                Key::Backspace | Key::Delete => return self.delete_selection(),
                // The selection becomes the numerator, as MathQuill makes it.
                Key::Char('/') => {
                    self.anchor = None;
                    let span = self.formula.atoms_span(row, range);
                    let text = format!("\\frac{{{}}}{{}}", &self.formula.src[span.clone()]);
                    let caret = text.len() - 1;
                    let s = self.formula.replace_in_row(row, span, &text, caret);
                    return if self.commit(&s) { vec![s] } else { Vec::new() };
                }
                Key::Char(_) => {
                    let mut splices = self.delete_selection();
                    splices.extend(self.key(key));
                    return splices;
                }
                _ => self.anchor = None,
            }
        }
        self.anchor = None;
        let f = &self.formula;
        let outcome = match key {
            Key::Char(ch) => f.type_char(self.caret, ch),
            Key::Backspace => f.backspace(self.caret),
            Key::Delete => f.delete(self.caret),
            Key::Left => Outcome::Move(f.left(self.caret)),
            Key::Right => Outcome::Move(f.right(self.caret)),
            Key::Up | Key::Down => match f.layout(self.display) {
                Ok(l) => l
                    .vertical(f, self.caret, key == Key::Up)
                    .map_or(Outcome::None, Outcome::Move),
                Err(_) => Outcome::None,
            },
            Key::ShiftLeft | Key::ShiftRight | Key::SelectAll => Outcome::None,
        };
        let mut splices = Vec::new();
        match outcome {
            Outcome::Move(c) => self.caret = c,
            Outcome::Edit(s) => {
                if self.commit(&s) {
                    splices.push(s);
                    if matches!(key, Key::Char(ch) if ch.is_ascii_alphabetic())
                        && let Some(s) = self.formula.auto_command(self.caret)
                        && self.commit(&s)
                    {
                        splices.push(s);
                    }
                }
            }
            Outcome::None => {}
        }
        splices
    }

    fn commit(&mut self, s: &Splice) -> bool {
        match self.formula.apply(s) {
            // Commands keep TeX's rules where they know them; this catches
            // the rest (`x^{}'` is a double superscript) by refusing the
            // keystroke rather than writing TeX nothing can typeset.
            Ok((f, _)) if ratex_parser::parse(&f.src).is_err() => false,
            Ok((f, c)) => {
                self.formula = f;
                self.caret = c;
                true
            }
            // A splice that leaves TeX this reader rejects is a bug in a
            // command, not something to show the author; drop it.
            Err(_) => false,
        }
    }

    pub fn tex(&self) -> &str {
        &self.formula.src
    }
}

/// Parse a key script for tests and demos: characters type themselves, and
/// `←→↑↓⌫` are the arrows and backspace.
pub fn keys(script: &str) -> Vec<Key> {
    script
        .chars()
        .map(|c| match c {
            '←' => Key::Left,
            '→' => Key::Right,
            '↑' => Key::Up,
            '↓' => Key::Down,
            '⌫' => Key::Backspace,
            '⌦' => Key::Delete,
            '⇠' => Key::ShiftLeft,
            '⇢' => Key::ShiftRight,
            c => Key::Char(c),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(start: &str, script: &str) -> Session {
        let mut s = Session::new(start).unwrap();
        for k in keys(script) {
            s.key(k);
        }
        s
    }

    fn typed(script: &str) -> String {
        run("", script).tex().to_string()
    }

    /// The session after `script`, from `start` with the caret at its end.
    fn after(start: &str, script: &str) -> String {
        run(start, script).tex().to_string()
    }

    #[test]
    fn delete_forward_mirrors_backspace() {
        // Before an atom: it goes.
        let mut s = Session::new("ab").unwrap();
        s.home();
        s.key(Key::Delete);
        assert_eq!(s.tex(), "b");
        // Before a fraction with something in it: into its first slot, then
        // the numerator's letter goes, then the empty fraction comes apart
        // around what is left.
        let mut s = Session::new(r"\frac{a}{b}c").unwrap();
        s.home();
        s.key(Key::Delete);
        assert!(s.selection().is_none());
        assert_eq!(s.tex(), r"\frac{a}{b}c", "entering wrote something");
        s.key(Key::Delete);
        assert_eq!(s.tex(), r"\frac{}{b}c");
        // At the end of the numerator: into the denominator.
        s.key(Key::Delete);
        assert_eq!(s.tex(), r"\frac{}{b}c");
        // At the end of the last slot: the structure comes apart.
        s.key(Key::Right);
        s.key(Key::Delete);
        assert_eq!(s.tex(), "bc");
        // At the very end: nothing.
        assert_eq!(after("x", "⌦"), "x");
    }

    #[test]
    fn a_selection_takes_whole_structures_and_its_tex_is_what_was_written() {
        let mut s = Session::new(r"1+\frac{a}{b}+2").unwrap();
        s.end();
        // ⇧← twice: the `2`, then the `+`.
        s.key(Key::ShiftLeft);
        s.key(Key::ShiftLeft);
        assert_eq!(s.selected_tex(), Some("+2"));
        // Into the fraction's denominator: the selection lifts to the whole
        // fraction, never half of one.
        s.key(Key::ShiftLeft);
        s.key(Key::ShiftLeft);
        assert_eq!(s.selected_tex(), Some(r"\frac{a}{b}+2"));
        // Back to where it started: no selection.
        for _ in 0..4 {
            s.key(Key::ShiftRight);
        }
        assert_eq!(s.selection(), None);
        // ← with a selection collapses it to its start, and writes nothing.
        s.key(Key::ShiftLeft);
        s.key(Key::Left);
        assert_eq!(s.selection(), None);
        assert_eq!(s.formula.offset(s.caret), r"1+\frac{a}{b}+".len());
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut s = Session::new("a+b+c").unwrap();
        s.end();
        s.key(Key::ShiftLeft);
        s.key(Key::ShiftLeft);
        let sp = s.key(Key::Char('x'));
        assert_eq!(s.tex(), "a+bx");
        assert_eq!(sp.len(), 2, "the delete and the typing, each a splice");
        // Backspace and Delete take it whole.
        s.key(Key::SelectAll);
        assert_eq!(s.selected_tex(), Some("a+bx"));
        s.key(Key::Backspace);
        assert_eq!(s.tex(), "");
        // `/` makes the selection a numerator.
        let mut s = Session::new("a+bc").unwrap();
        s.end();
        s.key(Key::ShiftLeft);
        s.key(Key::ShiftLeft);
        s.key(Key::Char('/'));
        assert_eq!(s.tex(), r"a+\frac{bc}{}");
        s.key(Key::Char('2'));
        assert_eq!(s.tex(), r"a+\frac{bc}{2}");
    }

    #[test]
    fn paste_writes_tex_that_reads_and_types_what_does_not() {
        let mut s = Session::new("x").unwrap();
        s.end();
        s.paste(r"+\sqrt{2}");
        assert_eq!(s.tex(), r"x+\sqrt{2}");
        // Not TeX the formula reads in place: typed, so `/` still makes a
        // fraction.
        let mut s = Session::new("").unwrap();
        s.paste("1/2}");
        assert_eq!(s.tex(), r"\frac{1}{2\}}");
        // Over a selection.
        let mut s = Session::new("a+b").unwrap();
        s.key(Key::SelectAll);
        let sp = s.paste("c");
        assert_eq!(s.tex(), "c");
        assert_eq!(sp.len(), 2);
    }

    #[test]
    fn the_selection_is_shaded_in_the_picture() {
        let mut s = Session::new(r"a+\frac{1}{2}").unwrap();
        let style = crate::layout::Style::default();
        let plain = s.picture(&style).unwrap().svg;
        assert!(!plain.contains("fill-opacity"));
        s.key(Key::SelectAll);
        let shaded = s.picture(&style).unwrap().svg;
        assert!(shaded.contains(r#"fill-opacity="0.25""#), "{shaded}");
    }

    #[test]
    fn plain_typing_is_plain_tex() {
        assert_eq!(typed("a+b=c"), "a+b=c");
    }

    #[test]
    fn slash_makes_a_fraction_of_the_term_before_it() {
        assert_eq!(typed("1/2"), r"\frac{1}{2}");
        assert_eq!(typed("a+12/x"), r"a+\frac{12}{x}");
        assert_eq!(typed("/"), r"\frac{}{}");
        assert_eq!(typed("x^2/3"), r"x^{\frac{2}{3}}");
    }

    #[test]
    fn caret_and_space_open_and_leave_a_script() {
        assert_eq!(typed("x^2 +1"), "x^{2}+1");
        assert_eq!(typed("x_i^2"), "x_{i^{2}}");
        assert_eq!(typed("x_i →^2"), "x_{i}^{2}");
    }

    #[test]
    fn names_become_commands() {
        assert_eq!(typed("2pi"), r"2\pi");
        assert_eq!(typed("sqrtx"), r"\sqrt{x}");
        assert_eq!(typed("epsilon"), r"\epsilon");
        assert_eq!(typed("theta"), r"\theta");
        assert_eq!(typed("pix"), r"\pi x");
    }

    #[test]
    fn arrows_walk_through_slots() {
        // ← from the end of `\frac{1}{2}` goes into the denominator.
        assert_eq!(typed("1/2 ←3"), r"\frac{1}{23}");
        assert_eq!(typed("1/2 ←←←4"), r"\frac{14}{2}");
    }

    #[test]
    fn up_and_down_switch_between_numerator_and_denominator() {
        assert_eq!(typed("a/b↑c↓d"), r"\frac{ac}{bd}");
    }

    #[test]
    fn backspace_deletes_atoms_and_dissolves_structure() {
        assert_eq!(typed("ab⌫"), "a");
        // Into the fraction, then emptying it, then removing it.
        assert_eq!(typed("1/2 ⌫"), r"\frac{1}{2}");
        assert_eq!(typed("1/2 ⌫⌫"), r"\frac{1}{}");
        assert_eq!(typed("1/2 ⌫⌫⌫⌫"), r"\frac{}{}");
        assert_eq!(typed("1/2 ⌫⌫⌫⌫⌫"), "");
        // At the start of a numerator, the fraction comes apart.
        assert_eq!(run(r"\frac{a}{b}", "←←←←⌫").tex(), "a b");
    }

    #[test]
    fn hand_written_tex_around_an_edit_survives() {
        let src = r"\frac12 \;+\; \alpha % note
  x";
        let mut s = Session::new(src).unwrap();
        // To the start, then type.
        for _ in 0..20 {
            s.key(Key::Left);
        }
        s.key(Key::Char('y'));
        assert_eq!(s.tex(), format!("y{src}"));
    }

    #[test]
    fn an_unbraced_slot_gains_braces_when_it_grows() {
        let s = run("x^2", "←b");
        // ← from the end goes into the superscript's end.
        assert_eq!(s.tex(), "x^{2b}");
        let s = run(r"\frac12", "←←←3");
        assert_eq!(s.tex(), r"\frac{13}2");
    }

    #[test]
    fn a_display_block_s_newlines_stay_outside_the_formula() {
        // `$$\n…\n$$`: typing at the end lands before the closing newline.
        let s = run("\n\\int x\n", "y");
        assert_eq!(s.tex(), "\n\\int xy\n");
        let mut s = Session::new("\n\n").unwrap();
        s.key(Key::Char('a'));
        assert_eq!(s.tex(), "\na\n");
        s.home();
        s.key(Key::Char('b'));
        assert_eq!(s.tex(), "\nba\n");
    }

    #[test]
    fn a_control_word_is_kept_apart_from_a_letter() {
        let s = run(r"\alpha", "b");
        assert_eq!(s.tex(), r"\alpha b");
    }

    /// Whatever is typed, the source stays TeX that RaTeX reads.
    #[test]
    fn every_step_typesets() {
        let scripts = [
            "1/2+x^2 -sqrtab ←←⌫⌫⌫/3↑4",
            "a_1^2/b ⌫⌫⌫⌫⌫⌫⌫⌫⌫⌫",
            "sum_i=1 ^n i^2 =n(n+1)(2n+1)/6",
            "lim_x→0 sinx/x",
            "{}#$%&\\~*",
        ];
        for script in scripts {
            let mut s = Session::new("").unwrap();
            for k in keys(script) {
                s.key(k);
                ratex_parser::parse(s.tex())
                    .unwrap_or_else(|e| panic!("{script}: `{}`: {}", s.tex(), e.message));
                s.formula.layout(false).expect("probe typesets");
            }
        }
    }
}

#[cfg(test)]
mod fuzz {
    use super::*;
    use ratex_layout::{LayoutOptions, layout, to_display_list};

    /// A tiny LCG: reproducible without a dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as usize
        }
    }

    /// Random keystrokes, from empty and from hand-written TeX: after every
    /// one the source is TeX RaTeX reads, the tree reads, and the probe lays
    /// out exactly as the plain formula does.
    #[test]
    fn random_typing_keeps_every_invariant() {
        let alphabet: Vec<Key> = keys("abxyz12+-=()/^_ ←→↑↓⌫⌫sqrtpi,'").into_iter().collect();
        let starts = [
            "",
            r"\frac12 + \alpha",
            r"\left( x_i^2 \right) \cdot \sqrt[3]{y}",
            r"\sum_{k=0}^{n} \binom{n}{k} \text{ ways}",
        ];
        let mut rng = Rng(7);
        for round in 0..200 {
            let mut s = Session::new(starts[round % starts.len()]).unwrap();
            let mut trail = String::new();
            for _ in 0..30 {
                let k = alphabet[rng.next() % alphabet.len()];
                trail.push_str(&format!("{k:?} "));
                s.key(k);
                let tex = s.tex();
                let ctx = || format!("round {round}: `{tex}` after {trail}");
                let ast =
                    ratex_parser::parse(tex).unwrap_or_else(|e| panic!("{}: {}", ctx(), e.message));
                let opts =
                    LayoutOptions::default().with_style(ratex_types::math_style::MathStyle::Text);
                let plain = to_display_list(&layout(&ast, &opts));
                let probed = s
                    .formula
                    .layout(false)
                    .unwrap_or_else(|e| panic!("{}: {e}", ctx()));
                // Empty slots draw a placeholder the plain formula has not
                // got, so only compare formulas without one.
                if s.formula.rows.iter().all(|r| !r.atoms.is_empty()) {
                    assert_eq!(
                        plain.width,
                        probed.list.width,
                        "{}\n{}",
                        ctx(),
                        probed.probe
                    );
                    assert_eq!(plain.items.len(), probed.list.items.len(), "{}", ctx());
                }
                assert!(s.formula.stops.contains(&s.caret), "{}", ctx());
            }
        }
    }
}
