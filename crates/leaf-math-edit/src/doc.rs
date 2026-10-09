//! Formulas in a document, found and edited through twig.
//!
//! twig reads `$…$` and `$$…$$` (Markdown with the `math` extension, as leaf
//! parses it) and djot's `` $`…` `` and `` $$`…` `` as `inline_math` and
//! `display_math` nodes whose text is the TeX. A formula's splices are moved
//! by its TeX's offset in the document and applied with
//! [`twig::Editor::edit_range`], so the document is edited the way every
//! other leaf keystroke edits it — losslessly, reparsed after each splice.

use crate::edit::Splice;
use std::ops::Range;
use twig::{Editor, Format, MarkdownExtensions};

/// A formula's place in a document.
#[derive(Clone, Debug, PartialEq)]
pub struct FormulaRef {
    /// The whole node, delimiters included.
    pub span: Range<usize>,
    /// The TeX between the delimiters.
    pub tex: Range<usize>,
    pub display: bool,
}

/// An editor over `src` with the math extension on.
pub fn editor(src: &str, format: Format) -> Result<Editor, twig::Error> {
    Editor::new_ext(
        src.as_bytes(),
        format,
        MarkdownExtensions {
            math: true,
            ..Default::default()
        },
    )
}

/// Every formula in the document, in source order.
pub fn formulas(ed: &mut Editor) -> Result<Vec<FormulaRef>, twig::Error> {
    let source = ed.source_str()?;
    let mut out = Vec::new();
    for n in ed.nodes()? {
        let display = match n.kind.as_str() {
            "inline_math" => false,
            "display_math" => true,
            _ => continue,
        };
        let tex = match (&n.content_span, &n.text) {
            (Some(c), _) => c.clone(),
            // twig gives the text and not its range; find it in the span,
            // checked against the source rather than assumed.
            (None, Some(text)) => {
                let within = &source[n.span.clone()];
                let at = within.find(text.as_str()).unwrap_or(0);
                n.span.start + at..n.span.start + at + text.len()
            }
            (None, None) => continue,
        };
        out.push(FormulaRef {
            span: n.span.clone(),
            tex,
            display,
        });
    }
    Ok(out)
}

/// Apply a formula's splice to the document holding it, at `tex_start`, the
/// formula's TeX's offset. Returns the TeX's new range.
pub fn apply(
    ed: &mut Editor,
    tex: &Range<usize>,
    splice: &Splice,
) -> Result<Range<usize>, twig::Error> {
    let start = tex.start + splice.range.start;
    let end = tex.start + splice.range.end;
    let change = ed.edit_range(start, end, &splice.text)?;
    let end = (tex.end as isize + change.delta()) as usize;
    Ok(tex.start..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Session, keys};

    #[test]
    fn formulas_are_found_in_markdown_and_djot() {
        let md = "Euler: $e^{i\\pi}+1=0$.\n\n$$\n\\int_0^1 x\\,dx\n$$\n";
        let mut ed = editor(md, Format::Markdown).unwrap();
        let fs = formulas(&mut ed).unwrap();
        assert_eq!(fs.len(), 2);
        assert_eq!(&md[fs[0].tex.clone()], "e^{i\\pi}+1=0");
        assert!(!fs[0].display);
        assert!(fs[1].display);
        assert_eq!(md[fs[1].tex.clone()].trim(), "\\int_0^1 x\\,dx");

        let dj = "Euler: $`e^{i\\pi}`.\n";
        let mut ed = editor(dj, Format::Djot).unwrap();
        let fs = formulas(&mut ed).unwrap();
        assert_eq!(&dj[fs[0].tex.clone()], "e^{i\\pi}");
    }

    #[test]
    fn keystrokes_edit_the_formula_in_place_and_nothing_else() {
        let md = "# Notes\n\nThe area is $r^2$, roughly.\n\n- item, *kept*  \n";
        let mut ed = editor(md, Format::Markdown).unwrap();
        let f = formulas(&mut ed).unwrap().remove(0);
        let mut tex = f.tex.clone();
        let mut s = Session::new(&md[tex.clone()]).unwrap();
        // Make it π r².
        for _ in 0..4 {
            s.key(crate::edit::Key::Left);
        }
        for k in keys("pi") {
            for sp in s.key(k) {
                tex = apply(&mut ed, &tex, &sp).unwrap();
            }
        }
        let out = ed.source_str().unwrap();
        assert_eq!(
            out,
            "# Notes\n\nThe area is $\\pi r^2$, roughly.\n\n- item, *kept*  \n"
        );
        assert_eq!(&out[tex], s.tex());
        // twig still reads it as one formula.
        let fs = formulas(&mut ed).unwrap();
        assert_eq!(fs.len(), 1);
    }
}
