use super::*;

// ── display columns ──────────────────────────────────────────────────────

#[test]
fn a_table_column_is_as_wide_as_its_cells_are_drawn() {
    // A column sized by counting characters is drawn narrower than the text
    // it has to hold — `你好` is two characters in four cells — and the cell
    // spills over the border it is supposed to sit inside, taking the whole
    // grid out of square with it. Squareness is the property: every row of a
    // grid is drawn to the same column, whatever its cells are spelled with.
    for src in [
        "| A | B |\n|---|---|\n| 你好 | y |\n",
        "| A | B |\n|---|---|\n| a👨‍👩‍👧b | y |\n",
        "| A | 漢字 |\n|---|---|\n| x | y |\n",
    ] {
        let m = map(src);
        let widths: Vec<usize> = m.rows.iter().map(|r| r.width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged grid {widths:?} for {src:?}:\n{}",
            rendered(&m)
        );
    }
}

#[test]
fn a_cell_wrapped_narrow_never_breaks_inside_a_character() {
    // A column too narrow for its cell hard-breaks the text, and every line
    // of it is given an end stop just past its last glyph. Broken into runs
    // of four glyphs, the first line of this cell ends between `👨‍👩` and the
    // joiner holding `👧` on — so its end stop lands inside a character,
    // where a click or Down can reach it and the next Backspace takes the
    // cluster apart from the middle.
    let src = "| A |\n|---|\n| 👨‍👩‍👧👨‍👩‍👧 |\n";
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    let m = build_t(&ed.nodes().unwrap(), src, Some(8));
    let boundaries: Vec<usize> = src
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(src.len()))
        .collect();
    for off in (0..=src.len()).filter(|&o| m.is_stop(o)) {
        assert!(
            boundaries.contains(&off),
            "stop at {off} is inside a character:\n{}",
            rendered(&m)
        );
    }
}

#[test]
fn a_wrapped_cell_keeps_every_line_inside_its_column() {
    // The width is a promise in a table, where a glyph past the column lands
    // on the border or in the next cell — and it is a promise about cells,
    // which is not what a count of glyphs measures.
    let src = "| A |\n|---|\n| 你好世界漢字 |\n";
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    let m = build_t(&ed.nodes().unwrap(), src, Some(14));
    for r in &m.rows {
        assert_eq!(r.width(), 14, "{:?} is not drawn to the grid", rendered(&m));
    }
}

#[test]
fn a_hard_break_falls_between_clusters_and_measures_in_cells() {
    let glyphs = |s: &str| {
        let mut out = Vec::new();
        push_text(&mut out, s, 0, Style::default());
        out
    };
    let piece = |p: &[Glyph]| p.iter().map(|g| g.ch).collect::<String>();

    // Six cells of CJK broken at four: two characters, then one — never
    // between the two cells of `好`.
    let w = glyphs("你好世");
    let pieces: Vec<String> = hard_break(&w, 4).iter().map(|p| piece(p)).collect();
    assert_eq!(pieces, ["你好", "世"]);

    // A character wider than the column has nowhere legal to break, so it
    // keeps its cells rather than being cut in half.
    let w = glyphs("你好");
    let pieces: Vec<String> = hard_break(&w, 1).iter().map(|p| piece(p)).collect();
    assert_eq!(pieces, ["你", "好"]);

    // An empty word yields no pieces at all — a double space stays a space.
    assert!(hard_break(&[], 4).is_empty());
}

#[test]
fn an_empty_list_item_still_gets_a_bulleted_row_with_a_caret_home() {
    // Pressing Enter at the end of a list item opens a new, empty item —
    // a childless `list_item`. Without a row of its own the new bullet
    // wouldn't appear until something was typed into it (the caret would be
    // stranded on an offset no row draws). It now renders as one prefixed
    // row whose end is a caret stop, so the bullet shows and the caret lands
    // just past the marker.
    let m = map("- item\n- \n");
    assert_eq!(m.num_rows(), 2, "the empty second item needs its own row");
    assert_eq!(
        m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "• ",
        "the empty item draws just its bullet",
    );
    // Its end is the caret home (past the `- ` marker), and it's a real stop.
    assert!(
        m.is_stop(m.rows[1].end_src),
        "the empty item's caret home is not a stop"
    );
    assert_eq!(
        m.pos_of_offset(m.rows[1].end_src),
        (1, 2),
        "caret sits after '• '"
    );
}

#[test]
fn bullets_change_shape_with_nesting_depth_and_come_round_again() {
    let m = map("- a\n  - b\n    - c\n      - d\n- e\n");
    let rows = row_texts(&m);
    assert_eq!(rows[0], "• a");
    assert!(rows[1].ends_with("◦ b"), "{rows:?}");
    assert!(rows[2].ends_with("▪ c"), "{rows:?}");
    assert!(
        rows[3].ends_with("• d"),
        "the fourth level starts over: {rows:?}"
    );
    assert_eq!(rows[4], "• e", "back at the top level");
}

#[test]
fn ordered_items_count_in_decimal_then_alpha_then_roman() {
    let m = map("1. a\n   1. b\n   2. c\n      1. d\n      2. e\n      3. f\n      4. g\n");
    let rows = row_texts(&m);
    assert_eq!(rows[0], "1. a");
    assert!(rows[1].ends_with("a. b"), "{rows:?}");
    assert!(rows[2].ends_with("b. c"), "{rows:?}");
    assert!(rows[3].ends_with("i. d"), "{rows:?}");
    assert!(rows[4].ends_with("ii. e"), "{rows:?}");
    assert!(rows[5].ends_with("iii. f"), "{rows:?}");
    assert!(rows[6].ends_with("iv. g"), "{rows:?}");
    // Display only: the source keeps its own markers.
    assert!(
        !m.rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .any(|g| g.stop && g.ch == 'i')
    );
}

#[test]
fn an_ordered_list_counts_from_its_first_items_number() {
    let rows = row_texts(&map("5. a\n6. b\n"));
    assert_eq!(rows, ["5. a", "6. b"]);
    // The source's later numbers don't matter, only where it starts.
    let rows = row_texts(&map("3. a\n3. b\n"));
    assert_eq!(rows, ["3. a", "4. b"]);
    // A nested list starting at 3 is lettered from `c`.
    let rows = row_texts(&map("1. a\n   3. b\n   4. c\n"));
    assert!(
        rows[1].ends_with("c. b") && rows[2].ends_with("d. c"),
        "{rows:?}"
    );
}

#[test]
fn mixed_lists_share_one_depth_and_task_boxes_stay_boxes() {
    let rows = row_texts(&map("1. a\n   - b\n     - [ ] c\n       1. d\n"));
    assert_eq!(rows[0], "1. a");
    assert!(
        rows[1].ends_with("◦ b"),
        "a bullet under a number is level two: {rows:?}"
    );
    assert!(rows[2].ends_with("☐ c"), "{rows:?}");
    assert!(
        rows[3].ends_with("1. d"),
        "level four comes back round to decimal: {rows:?}"
    );
}

#[test]
fn a_quote_between_lists_neither_resets_nor_adds_to_the_depth() {
    let rows = row_texts(&map("- a\n\n  > - b\n  >   - c\n"));
    let b = rows.iter().find(|r| r.ends_with('b')).unwrap();
    let c = rows.iter().find(|r| r.ends_with('c')).unwrap();
    assert!(b.contains("│") && b.ends_with("◦ b"), "{rows:?}");
    assert!(c.ends_with("▪ c"), "{rows:?}");
}

#[test]
fn a_wide_marker_hangs_its_continuation_rows_under_the_text() {
    // A roman `viii. ` is wider than `• `; the item's later rows must still
    // start under its text, which they do by wearing the marker itself as
    // their (invisible) indent.
    let src = "1. a\n   1. b\n      1. c\n      2. c\n      3. c\n      4. c\n      5. c\n      6. c\n      7. c\n      8. one\n\n         two\n";
    let m = map(src);
    let row = |needle: &str| {
        m.rows
            .iter()
            .find(|r| {
                r.glyphs
                    .iter()
                    .map(|g| g.ch)
                    .collect::<String>()
                    .ends_with(needle)
            })
            .unwrap()
    };
    let first = row("viii. one");
    let second = row("two");
    let marker = |r: &VRow, role: Role| -> String {
        r.glyphs
            .iter()
            .filter(|g| g.style.role == role)
            .map(|g| g.ch)
            .collect()
    };
    assert_eq!(marker(first, Role::ListMarker), "viii. ");
    assert_eq!(marker(second, Role::ListIndent), "1. a. viii. ");
    let width = |r: &VRow| r.glyphs.iter().take_while(|g| !g.stop).count();
    assert_eq!(width(first), width(second), "the continuation lines up");
}

#[test]
fn ordinals_count_past_z_and_into_big_numerals() {
    assert_eq!(ordinal(1, 1), "a");
    assert_eq!(ordinal(26, 1), "z");
    assert_eq!(ordinal(27, 1), "aa");
    assert_eq!(ordinal(52, 1), "az");
    assert_eq!(ordinal(703, 1), "aaa");
    assert_eq!(ordinal(0, 1), "0", "no letter for zero");
    assert_eq!(ordinal(4, 2), "iv");
    assert_eq!(ordinal(1994, 2), "mcmxciv");
    assert_eq!(ordinal(3999, 2), "mmmcmxcix");
    assert_eq!(ordinal(4000, 2), "4000", "past the numerals, decimal");
    assert_eq!(ordinal(0, 2), "0");
    assert_eq!(ordinal(7, 3), "7", "the fourth level is decimal again");
}

#[test]
fn a_notes_row_range_stops_at_the_note_even_when_it_ends_in_a_link() {
    // The peek bug: a note whose body ends in a link has its last byte
    // inside the hidden destination, so mapping `end - 1` through
    // `pos_of_offset` snapped *forward* — past its own row, past the drawn
    // gap, and onto the next note's row. The popover then drew both notes.
    let src =
        "A[^1] B[^2].\n\n[^1]: bare text\n\n[^2]: [title](https://example.com/x)\n\n[^3]: last\n";
    let m = map(src);
    let body = src.find("[title]").unwrap();
    let end = src.find("\n\n[^3]").unwrap();

    let (first, last) = m.row_range_for(body..end);
    assert_eq!(
        first, last,
        "a one-block note is one row, not a span onto the next"
    );

    // The old arithmetic, kept here as the thing that must stay wrong: it
    // is what this method exists instead of.
    assert_ne!(
        m.pos_of_offset(end - 1).0,
        last,
        "the forward snap still leaves the note's row — that is the whole point",
    );

    // A note ending in *visible* text was never broken, and still isn't:
    // both readings agree there, which is why the original test missed it.
    let plain = src.find("bare text").unwrap();
    let plain_end = src.find("\n\n[^2]").unwrap();
    let (pf, pl) = m.row_range_for(plain..plain_end);
    assert_eq!(pf, pl);
    assert_eq!(m.pos_of_offset(plain_end - 1).0, pl);
}

#[test]
fn a_row_range_covers_every_row_of_a_block_that_spans_several() {
    // The range is a span, not a point: a quote of two paragraphs covers its
    // gap row and both of its text rows, so a peek draws the whole thing.
    let src = "> one\n>\n> two\n\nafter\n";
    let m = map(src);
    let (first, last) = m.row_range_for(0..src.find("\n\nafter").unwrap());
    assert_eq!((first, last), (0, 2));

    // And a range with no visible byte at all still covers the row it opened
    // on, rather than collapsing to nothing.
    let (f, l) = m.row_range_for(0..1);
    assert_eq!((f, l), (0, 0));
}

#[test]
fn an_empty_block_quote_still_gets_a_gutter_row_with_a_caret_home() {
    // The peer of the empty list item, and the case that made an empty line
    // in a quote draw as plain body text: a childless `block_quote` — a bare
    // `> `, which is what the toolbar's Quote button leaves on a blank line —
    // has no inner block to carry the gutter, so the whole quote used to
    // render as *nothing*. It didn't merely lose its bar; the row went away
    // and the caret had no home on it.
    let m = map("a\n\n> \n\nb\n");
    assert_eq!(
        m.rows[2].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ ",
        "the empty quote draws just its gutter",
    );
    assert!(
        m.rows[2]
            .glyphs
            .iter()
            .all(|g| g.style.role == Role::QuoteGutter)
    );
    assert!(
        !m.rows[2].decoration,
        "it is a line text can go on, not a drawn gap"
    );
    assert!(
        m.is_stop(m.rows[2].end_src),
        "the empty quote's caret home is not a stop"
    );
    assert_eq!(
        m.pos_of_offset(m.rows[2].end_src),
        (2, 2),
        "caret sits after '│ '"
    );

    // And a document that is *only* an empty quote still renders a row — it
    // used to render none at all, leaving the caret nowhere to stand.
    let m = map("> \n");
    assert_eq!(m.num_rows(), 1);
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ "
    );
}

#[test]
fn a_quotes_own_trailing_marker_lines_stay_inside_the_quote() {
    // Enter at the end of `> a` writes `> a\n>\n> \n`. Those last two lines
    // hold no block — a quote's `content_span` stops at its last child — so
    // the children walk never reaches them, and they used to fall through to
    // the document-level trailing pass, which knows no prefix: the gutter
    // stopped and the writer's new line drew as plain prose. Fixable only
    // since twig 3.2.0, where the quote's *span* covers its own marker lines
    // (`0..3` before, `0..8` now) and there is finally a node saying they
    // are the quote's.
    let m = map("> a\n>\n> \n");
    assert_eq!(m.num_rows(), 3, "one row per line the quote spells");
    for (i, row) in m.rows.iter().enumerate() {
        let text = row.glyphs.iter().map(|g| g.ch).collect::<String>();
        assert!(text.starts_with("│ "), "row {i} lost the gutter: {text:?}");
        assert!(
            !row.decoration,
            "row {i} is a line to type on, not a drawn gap"
        );
        assert!(m.is_stop(row.end_src), "row {i} has no caret home");
    }
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ a"
    );
    // Distinct offsets, so ↑/↓ between them moves the caret rather than
    // landing twice on the same byte.
    assert!(m.rows[0].end_src < m.rows[1].end_src);
    assert!(m.rows[1].end_src < m.rows[2].end_src);

    // A blank line *after* the quote is not the quote's: it is spelled with
    // no marker, so it stays an ordinary boundary and the gutter ends.
    let m = map("> a\n\nb\n");
    assert_eq!(m.num_rows(), 3);
    assert_eq!(
        m.rows[2].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "b"
    );
    assert!(
        !m.rows[1]
            .glyphs
            .iter()
            .any(|g| g.style.role == Role::QuoteGutter)
    );

    // Nesting is the case this could get wrong, and the depth has to come
    // from which quote's span the line falls in rather than from the row
    // above it. A trailing `>` under `> > a` matches only the OUTER quote,
    // so it wears one gutter; spell it `> >` and it wears two.
    let m = map("> > a\n>\n");
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ │ a"
    );
    assert_eq!(
        m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ "
    );
    let m = map("> > a\n> >\n");
    assert_eq!(
        m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "│ │ "
    );

    // And a marker line BETWEEN two quoted paragraphs is untouched: that is
    // the boundary `emit_separators_before` spells, and it stays a drawn gap
    // rather than becoming a line to type on.
    let m = map("> a\n>\n> b\n");
    assert_eq!(m.num_rows(), 3);
    assert!(
        m.rows[1].decoration,
        "the gap between two quoted blocks is still a gap"
    );
}

#[test]
fn an_empty_ordered_item_gets_its_number_and_a_caret_home() {
    let m = map("1. item\n2. \n");
    assert_eq!(m.num_rows(), 2);
    assert_eq!(
        m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "2. "
    );
    assert!(m.is_stop(m.rows[1].end_src));
    assert_eq!(
        m.pos_of_offset(m.rows[1].end_src),
        (1, 3),
        "caret sits after '2. '"
    );
}

#[test]
fn an_empty_headings_caret_home_is_past_its_hidden_marker() {
    // The toolbar's H1 on a blank line writes `# ` and nothing else. The row
    // it renders is empty (the marker is hidden), so its end *is* its only
    // caret stop — and it has to be the offset past the `# `, where typing
    // continues the heading. Anchored at the block's start instead, the caret
    // drew in front of the hashes and the first character typed there landed
    // before them (`x# `), which isn't a heading at all.
    let m = map("# \n");
    assert_eq!(m.num_rows(), 1);
    assert!(m.rows[0].glyphs.is_empty(), "the `# ` marker is hidden");
    assert_eq!(m.rows[0].end_src, 2, "the caret home is past the marker");
    assert!(m.is_stop(2), "the empty heading's caret home is not a stop");
}

#[test]
fn a_headings_rows_carry_its_level_even_with_nothing_typed_in_it() {
    // The row-level fact a proportional frontend sizes a whole line by. An
    // empty heading has no glyph to read a `Role::Heading` off, so a renderer
    // scanning glyphs drew `# ` (and its caret) at body height until the
    // first character landed.
    let m = map("# \n");
    assert_eq!(
        m.rows[0].heading,
        Some(1),
        "the empty heading knows its level"
    );

    // Every row of one that wraps, not just the first — and nothing else.
    let m = map_at(
        "## a heading long enough to wrap over two rows\n\nbody\n",
        Some(20),
    );
    let heads: Vec<Option<u8>> = m.rows.iter().map(|r| r.heading).collect();
    assert!(
        heads.iter().filter(|h| **h == Some(2)).count() >= 2,
        "got {heads:?}"
    );
    assert_eq!(
        m.rows.last().and_then(|r| r.heading),
        None,
        "the paragraph under it is not a heading",
    );
}

#[test]
fn an_empty_heading_leaves_the_rows_under_it_at_their_own_offsets() {
    // The row's end is also what the *next* row's separator is measured from,
    // so an empty heading that under-reported it shifted every offset below —
    // and the blank line under the heading then claimed the same offset as the
    // heading's own end. `pos_of_offset` resolves such a tie downstream (a
    // soft wrap belongs to the row below), so the caret at the end of the
    // heading was drawn two rows lower, on the blank line.
    // `text\n\n# \n\n`: the heading's content opens at 8, and the two rows
    // under it end at 9 and 10 — the blank line and the document's end.
    let m = map("text\n\n# \n\n");
    let end = m.rows.last().expect("a trailing blank row").end_src;
    assert_eq!(end, 10, "the trailing rows must end at their real offsets");
    // The heading's caret home is its own row's, not one shared with a row
    // below — the tie that drew the caret two rows down.
    assert_eq!(m.pos_of_offset(8), (2, 0), "the empty heading's own row");
    assert!(
        m.rows[3..].iter().all(|r| r.end_src > 8),
        "rows below own later offsets"
    );
}
