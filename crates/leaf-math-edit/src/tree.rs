//! A TeX formula read as a tree of rows and atoms, every node carrying the
//! byte span it was read from.
//!
//! The tree is a *view*: nothing here writes TeX. An edit is a splice on the
//! source (see [`crate::edit`]) followed by a reparse, the way twig's editor
//! works, so the bytes an edit does not name are never rewritten.
//!
//! The grammar is the math subset a structural editor can offer slots for —
//! fractions, roots, scripts, delimiters, one-argument wrappers like `\hat` —
//! and everything else is an [`AtomKind::Opaque`] atom: kept whole, moved
//! whole, deleted whole, never entered.

use std::ops::Range;

/// A horizontal run of atoms: a formula's top level, a fraction's numerator,
/// a script, a root's body.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The row's interior: between its braces when it has them, the one
    /// token it is otherwise. An empty braced row has an empty `content`
    /// sitting between `{` and `}`.
    pub content: Range<usize>,
    /// Whether the row is written in braces. An unbraced row (`x^2`,
    /// `\frac12`) is one token; inserting a second one has to add braces.
    pub braced: bool,
    pub atoms: Vec<Atom>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Atom {
    /// The atom's whole source, delimiters and arguments included.
    pub span: Range<usize>,
    pub kind: AtomKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AtomKind {
    /// One character: a letter, digit, operator or delimiter.
    Char(char),
    /// A control sequence that takes no argument: `\alpha`, `\cdot`, `\sin`,
    /// `\{`. `\limits`/`\nolimits` after it are folded into its span.
    Symbol(String),
    /// Horizontal space: `\,`, `\quad`, `~`. Has no ink.
    Space,
    /// `^…` — attaches to the atom before it, as in TeX.
    Sup(Row),
    /// `_…`.
    Sub(Row),
    /// `{…}` used for grouping.
    Group(Row),
    /// A command with math-mode arguments the editor can enter: `\frac`
    /// (two), `\sqrt` (an optional index and one), `\hat` (one).
    Command {
        name: String,
        opt: Option<Row>,
        args: Vec<Row>,
    },
    /// `\left( … \right)`.
    LeftRight { body: Row },
    /// Anything the editor does not open: `\text{…}`, environments, unknown
    /// constructs. Kept verbatim.
    Opaque,
}

impl Atom {
    /// The rows inside this atom, in source order — the order the caret
    /// visits them.
    pub fn rows(&self) -> Vec<&Row> {
        match &self.kind {
            AtomKind::Sup(r) | AtomKind::Sub(r) | AtomKind::Group(r) => vec![r],
            AtomKind::Command { opt, args, .. } => opt.iter().chain(args.iter()).collect(),
            AtomKind::LeftRight { body } => vec![body],
            _ => vec![],
        }
    }

    pub fn is_script(&self) -> bool {
        matches!(self.kind, AtomKind::Sup(_) | AtomKind::Sub(_))
    }

    /// Whether this atom ends a run of operands: what `/` stops at when it
    /// gathers a numerator.
    pub fn is_operator(&self) -> bool {
        match &self.kind {
            AtomKind::Char(c) => "+-=<>,;:!?([|*".contains(*c) || *c == '−',
            AtomKind::Symbol(s) => OPERATOR_SYMBOLS.contains(&s.as_str()),
            AtomKind::Space => true,
            _ => false,
        }
    }
}

const OPERATOR_SYMBOLS: &[&str] = &[
    "cdot",
    "times",
    "div",
    "pm",
    "mp",
    "le",
    "leq",
    "ge",
    "geq",
    "ne",
    "neq",
    "to",
    "approx",
    "equiv",
    "sim",
    "in",
    "subset",
    "cup",
    "cap",
    "sum",
    "prod",
    "int",
    "lim",
    "rightarrow",
    "leftarrow",
    "Rightarrow",
    "iff",
    "implies",
];

/// Commands whose arguments are math rows the caret can enter, with how many.
fn math_arity(name: &str) -> Option<usize> {
    Some(match name {
        "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" | "overset"
        | "underset" | "stackrel" => 2,
        "sqrt" | "hat" | "widehat" | "bar" | "overline" | "underline" | "vec" | "tilde"
        | "widetilde" | "dot" | "ddot" | "acute" | "grave" | "check" | "breve" | "mathring"
        | "overbrace" | "underbrace" | "overrightarrow" | "overleftarrow" | "boxed" | "cancel"
        | "phantom" => 1,
        _ => return None,
    })
}

/// Commands whose argument is not math (text, a name, a colour) — the whole
/// construct is one opaque atom.
fn opaque_arity(name: &str) -> Option<usize> {
    Some(match name {
        "text" | "textrm" | "textbf" | "textit" | "textsf" | "texttt" | "mathrm"
        // A font reaches its argument through the parser, which a probe
        // wrapper inside the argument would cut off.
        | "mathbf" | "mathit" | "mathbb" | "mathcal" | "mathfrak" | "mathsf" | "mathscr"
        | "boldsymbol" | "bm" | "pmb" | "mathtt"
        | "operatorname" | "mbox" | "hbox" | "label" | "tag" | "ce" | "pu" => 1,
        "textcolor" | "colorbox" | "href" => 2,
        _ => return None,
    })
}

fn is_infix(a: &Atom) -> bool {
    matches!(&a.kind, AtomKind::Symbol(s) if matches!(s.as_str(), "over" | "choose" | "atop" | "above" | "brace" | "brack"))
}

fn is_space_command(name: &str) -> bool {
    matches!(
        name,
        "," | ";"
            | ":"
            | "!"
            | " "
            | "quad"
            | "qquad"
            | "enspace"
            | "thinspace"
            | "medspace"
            | "thickspace"
    )
}

/// A parse failure: the formula is not TeX this reader can make a tree of.
/// The editor falls back to source editing for it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub at: usize,
}

/// Read `src` as a formula. The root row's content is the whole string.
pub fn parse(src: &str) -> Result<Row, ParseError> {
    let mut p = Parser { src, pos: 0 };
    let atoms = p.atoms(None)?;
    if p.pos < src.len() {
        return Err(p.err("unmatched `}`"));
    }
    if atoms.iter().any(is_infix) {
        let span = 0..src.len();
        let atoms = vec![Atom {
            span,
            kind: AtomKind::Opaque,
        }];
        return Ok(Row {
            content: 0..src.len(),
            braced: false,
            atoms,
        });
    }
    Ok(Row {
        content: 0..src.len(),
        braced: false,
        atoms,
    })
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, message: &str) -> ParseError {
        ParseError {
            message: message.into(),
            at: self.pos,
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.pos += c.len_utf8(),
                Some('%') => {
                    let rest = &self.src[self.pos..];
                    self.pos += rest.find('\n').map_or(rest.len(), |i| i + 1);
                }
                _ => break,
            }
        }
    }

    /// A control sequence's name, the backslash already consumed.
    fn control_name(&mut self) -> Result<String, ParseError> {
        let rest = &self.src[self.pos..];
        let len = rest
            .char_indices()
            .find(|(_, c)| !c.is_ascii_alphabetic())
            .map_or(rest.len(), |(i, _)| i);
        if len > 0 {
            self.pos += len;
            return Ok(rest[..len].to_string());
        }
        let c = rest.chars().next().ok_or_else(|| self.err("lone `\\`"))?;
        self.pos += c.len_utf8();
        Ok(c.to_string())
    }

    /// Atoms up to `close` (`}`, or `\right` when `close` is `None` inside a
    /// `\left`, or the end of input).
    fn atoms(&mut self, close: Option<char>) -> Result<Vec<Atom>, ParseError> {
        let mut atoms = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => {
                    if close.is_some() {
                        return Err(self.err("missing `}`"));
                    }
                    return Ok(atoms);
                }
                Some('}') => return Ok(atoms),
                Some(_)
                    if self.src[self.pos..].starts_with("\\right") && !self.at_word("\\right") =>
                {
                    return Ok(atoms);
                }
                Some(_) if self.src[self.pos..].starts_with("\\end") && !self.at_word("\\end") => {
                    return Err(self.err("`\\end` without `\\begin`"));
                }
                Some(_) => atoms.push(self.atom()?),
            }
        }
    }

    /// Whether the control word at `pos` is longer than `word` (so
    /// `\rightarrow` is not `\right`).
    fn at_word(&self, word: &str) -> bool {
        self.src[self.pos + word.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    }

    fn atom(&mut self) -> Result<Atom, ParseError> {
        let start = self.pos;
        let c = self.peek().unwrap();
        let kind = match c {
            '{' => {
                let row = self.braced_row()?;
                // `{n \choose k}`: an infix command splits its group in two,
                // which no slot model here can stand for.
                if row.atoms.iter().any(is_infix) {
                    AtomKind::Opaque
                } else {
                    AtomKind::Group(row)
                }
            }
            '^' | '_' => {
                self.pos += 1;
                let row = self.arg()?;
                if c == '^' {
                    AtomKind::Sup(row)
                } else {
                    AtomKind::Sub(row)
                }
            }
            '~' => {
                self.pos += 1;
                AtomKind::Space
            }
            '\\' => {
                self.pos += 1;
                let name = self.control_name()?;
                self.command(name)?
            }
            _ => {
                self.pos += c.len_utf8();
                AtomKind::Char(c)
            }
        };
        let mut end = self.pos;
        // `\sum\limits` is one base: the modifier belongs to the operator it
        // follows, and splitting them would leave `\limits` with nothing to
        // modify.
        if matches!(kind, AtomKind::Symbol(_)) {
            loop {
                let save = self.pos;
                self.skip_trivia();
                let rest = &self.src[self.pos..];
                let m = ["\\limits", "\\nolimits"]
                    .into_iter()
                    .find(|m| rest.starts_with(m) && !self.at_word(m));
                match m {
                    Some(m) => {
                        self.pos += m.len();
                        end = self.pos;
                    }
                    None => {
                        self.pos = save;
                        break;
                    }
                }
            }
        }
        Ok(Atom {
            span: start..end,
            kind,
        })
    }

    fn command(&mut self, name: String) -> Result<AtomKind, ParseError> {
        if is_space_command(&name) {
            return Ok(AtomKind::Space);
        }
        if name == "left" {
            self.delimiter()?;
            let atoms = self.atoms(None)?;
            // Bounded once both delimiters are known: see `fix_left_right`.
            let content = self.pos..self.pos;
            if !self.src[self.pos..].starts_with("\\right") {
                return Err(self.err("`\\left` without `\\right`"));
            }
            self.pos += "\\right".len();
            self.delimiter()?;
            return Ok(AtomKind::LeftRight {
                body: Row {
                    content,
                    braced: false,
                    atoms,
                },
            });
        }
        if name == "begin" {
            return self.environment();
        }
        if let Some(n) = math_arity(&name) {
            let mut opt = None;
            if name == "sqrt" {
                self.skip_trivia();
                if self.peek() == Some('[') {
                    opt = Some(self.bracket_row()?);
                }
            }
            let mut args = Vec::with_capacity(n);
            for _ in 0..n {
                args.push(self.arg()?);
            }
            return Ok(AtomKind::Command { name, opt, args });
        }
        if let Some(n) = opaque_arity(&name) {
            for _ in 0..n {
                self.skip_trivia();
                self.skip_balanced_arg()?;
            }
            return Ok(AtomKind::Opaque);
        }
        Ok(AtomKind::Symbol(name))
    }

    fn delimiter(&mut self) -> Result<(), ParseError> {
        self.skip_trivia();
        match self.peek() {
            Some('\\') => {
                self.pos += 1;
                self.control_name()?;
            }
            Some(c) => self.pos += c.len_utf8(),
            None => return Err(self.err("missing delimiter")),
        }
        Ok(())
    }

    fn environment(&mut self) -> Result<AtomKind, ParseError> {
        self.skip_trivia();
        let name_start = self.pos;
        self.skip_balanced_arg()?;
        let env = self.src[name_start..self.pos].to_string();
        let end_tag = format!("\\end{env}");
        // Environments nest; count `\begin{env}`/`\end{env}` pairs.
        let begin_tag = format!("\\begin{env}");
        let mut depth = 1;
        while depth > 0 {
            let rest = &self.src[self.pos..];
            let next_end = rest
                .find(&end_tag)
                .ok_or_else(|| self.err("missing `\\end`"))?;
            match rest.find(&begin_tag) {
                Some(b) if b < next_end => {
                    depth += 1;
                    self.pos += b + begin_tag.len();
                }
                _ => {
                    depth -= 1;
                    self.pos += next_end + end_tag.len();
                }
            }
        }
        Ok(AtomKind::Opaque)
    }

    fn skip_balanced_arg(&mut self) -> Result<(), ParseError> {
        match self.peek() {
            Some('{') => {
                let mut depth = 0usize;
                let bytes = self.src.as_bytes();
                while self.pos < bytes.len() {
                    match bytes[self.pos] {
                        b'\\' => self.pos += 1,
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1;
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                Err(self.err("missing `}`"))
            }
            Some('\\') => {
                self.pos += 1;
                self.control_name().map(|_| ())
            }
            Some(c) => {
                self.pos += c.len_utf8();
                Ok(())
            }
            None => Err(self.err("missing argument")),
        }
    }

    fn braced_row(&mut self) -> Result<Row, ParseError> {
        debug_assert_eq!(self.peek(), Some('{'));
        self.pos += 1;
        let start = self.pos;
        let atoms = self.atoms(Some('}'))?;
        if self.peek() != Some('}') {
            return Err(self.err("missing `}`"));
        }
        let content = start..self.pos;
        self.pos += 1;
        Ok(Row {
            content,
            braced: true,
            atoms,
        })
    }

    fn bracket_row(&mut self) -> Result<Row, ParseError> {
        self.pos += 1;
        let start = self.pos;
        let mut atoms = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(']') => break,
                None => return Err(self.err("missing `]`")),
                Some(_) => atoms.push(self.atom()?),
            }
        }
        let content = start..self.pos;
        self.pos += 1;
        Ok(Row {
            content,
            // Brackets do the braces' job: the row can hold many atoms.
            braced: true,
            atoms,
        })
    }

    /// A command's or script's argument: a braced row, or one token.
    fn arg(&mut self) -> Result<Row, ParseError> {
        self.skip_trivia();
        match self.peek() {
            Some('{') => self.braced_row(),
            Some('}') | None => Err(self.err("missing argument")),
            Some(_) => {
                let start = self.pos;
                let atom = self.single_token()?;
                Ok(Row {
                    content: start..self.pos,
                    braced: false,
                    atoms: vec![atom],
                })
            }
        }
    }

    /// One token as an atom, without the arguments a command would take:
    /// TeX's `\frac\alpha2` reads `\alpha` as the numerator.
    fn single_token(&mut self) -> Result<Atom, ParseError> {
        let start = self.pos;
        let c = self.peek().unwrap();
        let kind = if c == '\\' {
            self.pos += 1;
            let name = self.control_name()?;
            if is_space_command(&name) {
                AtomKind::Space
            } else {
                AtomKind::Symbol(name)
            }
        } else {
            self.pos += c.len_utf8();
            AtomKind::Char(c)
        };
        Ok(Atom {
            span: start..self.pos,
            kind,
        })
    }
}

/// Fix up `\left…\right` bodies, whose content the parser can only bound
/// once it has seen both ends: from the end of the opening delimiter to the
/// start of `\right`.
pub(crate) fn fix_left_right(src: &str, row: &mut Row) {
    for atom in &mut row.atoms {
        if let AtomKind::LeftRight { body } = &mut atom.kind {
            let after_left = atom.span.start + "\\left".len();
            let s = &src[after_left..];
            let ws = s.len() - s.trim_start().len();
            let mut p = after_left + ws;
            if src[p..].starts_with('\\') {
                p += 1;
                let rest = &src[p..];
                let n = rest
                    .char_indices()
                    .find(|(_, c)| !c.is_ascii_alphabetic())
                    .map_or(rest.len(), |(i, _)| i);
                p += if n == 0 {
                    rest.chars().next().map_or(0, char::len_utf8)
                } else {
                    n
                };
            } else {
                p += src[p..].chars().next().map_or(0, char::len_utf8);
            }
            body.content = match (body.atoms.first(), body.atoms.last()) {
                (Some(f), Some(l)) => f.span.start..l.span.end,
                _ => p..p,
            };
        }
        for r in atom_rows_mut(atom) {
            fix_left_right(src, r);
        }
    }
}

fn atom_rows_mut(atom: &mut Atom) -> Vec<&mut Row> {
    match &mut atom.kind {
        AtomKind::Sup(r) | AtomKind::Sub(r) | AtomKind::Group(r) => vec![r],
        AtomKind::Command { opt, args, .. } => opt.iter_mut().chain(args.iter_mut()).collect(),
        AtomKind::LeftRight { body } => vec![body],
        _ => vec![],
    }
}

/// [`parse`], with every span settled.
pub fn read(src: &str) -> Result<Row, ParseError> {
    let mut row = parse(src)?;
    fix_left_right(src, &mut row);
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<String> {
        read(src)
            .unwrap()
            .atoms
            .iter()
            .map(|a| src[a.span.clone()].to_string())
            .collect()
    }

    #[test]
    fn atoms_cover_their_source() {
        assert_eq!(kinds(r"a+b"), ["a", "+", "b"]);
        assert_eq!(kinds(r"x^2_i"), ["x", "^2", "_i"]);
        assert_eq!(
            kinds(r"\frac{a}{b} + \alpha"),
            [r"\frac{a}{b}", "+", r"\alpha"]
        );
        assert_eq!(
            kinds(r"\sum\limits_{i=1}^n"),
            [r"\sum\limits", "_{i=1}", "^n"]
        );
        assert_eq!(kinds(r"\text{if } x"), [r"\text{if }", "x"]);
        assert_eq!(kinds(r"\left( a \right)"), [r"\left( a \right)"]);
        assert_eq!(
            kinds(r"\begin{pmatrix} a & b \end{pmatrix}"),
            [r"\begin{pmatrix} a & b \end{pmatrix}"]
        );
        assert_eq!(kinds(r"\sqrt[3]{x}\,y"), [r"\sqrt[3]{x}", r"\,", "y"]);
    }

    #[test]
    fn rows_know_their_interiors() {
        let src = r"\frac12 + \frac{}{b}";
        let row = read(src).unwrap();
        let AtomKind::Command { args, .. } = &row.atoms[0].kind else {
            panic!()
        };
        assert_eq!(&src[args[0].content.clone()], "1");
        assert!(!args[0].braced);
        let AtomKind::Command { args, .. } = &row.atoms[2].kind else {
            panic!()
        };
        assert!(args[0].content.is_empty() && args[0].braced);
        assert_eq!(&src[args[1].content.clone()], "b");
    }

    #[test]
    fn a_left_right_body_is_between_its_delimiters() {
        let src = r"\left\{ a+b \right\}";
        let row = read(src).unwrap();
        let AtomKind::LeftRight { body } = &row.atoms[0].kind else {
            panic!()
        };
        assert_eq!(&src[body.content.clone()], "a+b");
        assert_eq!(body.atoms.len(), 3);
    }

    #[test]
    fn right_arrow_is_not_right() {
        assert_eq!(kinds(r"a \rightarrow b"), ["a", r"\rightarrow", "b"]);
    }

    #[test]
    fn broken_tex_is_an_error_not_a_panic() {
        for s in [r"\frac{a", "}", r"\left( a", "x^", r"\begin{matrix} a"] {
            assert!(read(s).is_err(), "{s}");
        }
    }
}
