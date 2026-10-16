use super::*;

// ── block boundaries ─────────────────────────────────────────────────────

#[test]
fn a_boundary_says_which_blocks_it_divides() {
    use BlockClass::*;
    let m = map("one\n\ntwo\n\n# Head\n\ntail\n\n> quoted\n\n```\ncode\n```\n\n");
    assert_eq!(
        boundaries(&m),
        vec![
            (Paragraph, Paragraph),
            (Paragraph, Heading),
            (Heading, Paragraph),
            (Paragraph, Quote),
            (Quote, Code),
            // The blank line the document trails off with is a boundary too
            // — it closes the last block above the empty paragraph the caret
            // rests on. See `emit_trailing_blank_lines`.
            (Code, Paragraph),
        ],
        "each gap names the pair it falls between, in document order"
    );
}

#[test]
fn a_closing_fence_at_the_end_of_the_document_opens_no_phantom_row() {
    // The code block's last row ends at its last line of code; the closing
    // fence under it has no row. Counted from the row, the fence's line read
    // as a trailing blank line and opened an empty paragraph whose offset
    // was inside the fence — typing on it wrote into the backticks.
    let m = map("para\n\n```\ncode\n```\n");
    let texts: Vec<String> = m
        .rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert_eq!(texts, vec!["para", "", "code"], "a row under the fence");
    assert!(
        m.rows.last().is_some_and(|r| r.code),
        "the last row is the code"
    );
    // One more newline is the real empty paragraph, past the fence.
    let m = map("para\n\n```\ncode\n```\n\n");
    let last = m.rows.last().expect("a trailing row");
    assert_eq!(last.end_src, 20, "the trailing row stands past the fence");
    assert!(!last.decoration, "the trailing row is somewhere to type");
    // A setext underline is the same shape: markup under the last row.
    let m = map("Head\n====\n");
    assert_eq!(
        m.rows.len(),
        1,
        "a row under the underline: {:?}",
        m.rows.len()
    );
}

/// Each row as whether it is drawn-only and where it ends — the shape of
/// the blank lines around a block, which is what its text can't show.
fn row_shape(m: &VisualMap) -> Vec<(bool, usize)> {
    m.rows.iter().map(|r| (r.decoration, r.end_src)).collect()
}

#[test]
fn the_lines_above_the_first_block_draw_as_the_lines_under_the_last_do() {
    // No row was drawn above the first block, so an empty paragraph opened
    // there drew nothing. As at the document's end, one blank line is only
    // the gap, and every line above that gap is somewhere to type.
    assert_eq!(row_shape(&map("\nb\n")), [(false, 2)]);
    let m = map("\n\nb\n");
    assert_eq!(row_shape(&m), [(false, 0), (true, 1), (false, 3)]);
    assert_eq!(m.content_start, 0, "the caret can reach the empty line");
    assert_eq!(m.pos_of_offset(0), (0, 0));
    assert_eq!(map("b\n").content_start, 0);
    assert_eq!(
        row_shape(&map("\n\n\n# H\n")),
        [(false, 0), (false, 1), (true, 2), (false, 6)]
    );

    // Past frontmatter, the line under it is the frontmatter's.
    let fm = "---\nt: x\n---\n";
    let m = map(&format!("{fm}\nb\n"));
    assert_eq!(row_shape(&m), [(false, 15)]);
    assert_eq!(m.content_start, 14);
    let m = map(&format!("{fm}\n\nb\n"));
    assert_eq!(row_shape(&m), [(false, 13), (true, 14), (false, 16)]);
    assert_eq!(m.content_start, 13);

    // Preserve flow draws every line, but the frontmatter's.
    let preserve = |src: &str| {
        let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
        build(
            &ed.nodes().unwrap(),
            src,
            Some(80),
            true,
            &Surface::default(),
            None,
        )
    };
    assert_eq!(row_shape(&preserve("\nb\n")), [(false, 0), (false, 2)]);
    assert_eq!(row_shape(&preserve(&format!("{fm}\nb\n"))), [(false, 15)]);
    assert_eq!(
        row_shape(&preserve(&format!("{fm}\n\nb\n"))),
        [(false, 14), (false, 16)]
    );

    // Past a comment, counted from the line after it.
    assert_eq!(row_shape(&map("<!-- c -->\n\nb\n")), [(false, 13)]);
    assert_eq!(
        row_shape(&map("<!-- c -->\n\n\nb\n")),
        [(false, 11), (true, 12), (false, 14)]
    );
}

#[test]
fn the_lines_under_a_rule_are_counted_from_the_rule() {
    // A rule's row ends at the caret's home past the newline under it, and
    // the counts of the blank lines below used to start from there too, so
    // every gap under a rule came up a line short: the empty paragraph
    // Enter opens beneath a rule drew as two gaps and no line, and the
    // caret put on it was drawn on the next block. They count from the
    // rule itself, as under any other block's last line.
    let src = "a\n\n---\n\n\n\nb\n";
    let m = map(src);
    assert_eq!(
        row_shape(&m),
        [
            (false, 1),
            (true, 2),
            (false, 7), // the rule, its home past the newline under it
            (true, 7),
            (false, 8), // the empty paragraph
            (true, 9),
            (false, 11),
        ]
    );
    assert_eq!(m.pos_of_offset(8), (4, 0), "the caret on the empty line");
    // The one blank line under a rule is still the one gap, not two.
    assert_eq!(
        row_shape(&map("a\n\n---\n\nb\n")),
        [(false, 1), (true, 2), (false, 7), (true, 7), (false, 9)]
    );

    // Closing the document, the line under the gap is somewhere to type.
    let m = map("a\n\n---\n\n");
    assert_eq!(
        row_shape(&m),
        [(false, 1), (true, 2), (false, 7), (true, 7), (false, 8)]
    );
    assert_eq!(m.pos_of_offset(8), (4, 0));
    // And a lone newline after the rule is only the rule's own.
    assert_eq!(
        row_shape(&map("a\n\n---\n")),
        [(false, 1), (true, 2), (false, 7)]
    );

    // A quote's own trailing lines under a rule: each a row, the first too.
    let m = map("> ---\n>\n> ");
    assert_eq!(row_shape(&m), [(false, 6), (false, 7), (false, 10)]);
}

#[test]
fn the_home_past_a_djot_rule_is_the_line_under_it() {
    // djot's span takes in the newline that ends the rule, and the home
    // past the rule was measured from that end — one newline further on,
    // across the blank line, to the next block's first character. A caret
    // left after the rule was drawn on that block.
    let m = map_djot("a\n\n* * *\n\nb\n");
    assert_eq!(
        row_shape(&m),
        [(false, 1), (true, 2), (false, 9), (true, 9), (false, 11)]
    );
    assert_eq!(m.pos_of_offset(9).0, 2, "beside the rule, not on `b`");
    let m = map_djot("a\n\n* * *\n\n\n\nb\n");
    assert_eq!(m.pos_of_offset(10), (4, 0), "the caret on the empty line");
}
