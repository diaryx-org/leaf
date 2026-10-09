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

/// A formula being edited: the source, the caret, and the steps that move
/// them. What a frontend holds.
#[derive(Clone, Debug)]
pub struct Session {
    pub formula: Formula,
    pub caret: Caret,
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
}

impl Session {
    pub fn new(tex: &str) -> Result<Self, crate::ParseError> {
        let formula = Formula::new(tex)?;
        let caret = formula.end();
        Ok(Session { formula, caret })
    }

    /// Apply a key, returning the splices it made to the source in order
    /// (each against the source the one before it left), for a host that
    /// mirrors them into a document.
    pub fn key(&mut self, key: Key) -> Vec<Splice> {
        let f = &self.formula;
        let outcome = match key {
            Key::Char(ch) => f.type_char(self.caret, ch),
            Key::Backspace => f.backspace(self.caret),
            Key::Left => Outcome::Move(f.left(self.caret)),
            Key::Right => Outcome::Move(f.right(self.caret)),
            Key::Up | Key::Down => match f.layout(false) {
                Ok(l) => l
                    .vertical(f, self.caret, key == Key::Up)
                    .map_or(Outcome::None, Outcome::Move),
                Err(_) => Outcome::None,
            },
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
