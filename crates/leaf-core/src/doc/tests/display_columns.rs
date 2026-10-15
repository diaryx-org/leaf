use super::*;

// ── display columns ──────────────────────────────────────────────────────
// A `col` is a terminal cell, not a character. The two are the same number
// for the ASCII the fixtures above are written in, which is how they came
// apart in the first place: `你` is one character drawn in two cells, so a
// column counted in characters names a cell the text isn't in — one earlier
// for every wide character to its left.

#[test]
fn a_wide_character_is_two_columns_wide() {
    // The reproduction: `你` is one char and two cells, so the caret just
    // past it drew at column 1 — inside the character it had already left.
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("wide_col_{tag}"), "你好\n");
        d.caret = "你".len();
        assert_eq!(d.caret_pos(), (0, 2), "{tag}: caret drew inside 你");
        d.caret = "你好".len();
        assert_eq!(d.caret_pos(), (0, 4), "{tag}");
    }
}

#[test]
fn a_cluster_is_as_wide_as_it_is_drawn_not_as_its_codepoints_measure() {
    // `👨‍👩‍👧` is five codepoints — two-cell, joiner, two-cell, joiner,
    // two-cell — measuring six cells one at a time, but the character they
    // spell is drawn in two. Width belongs to the cluster, not the glyph,
    // and the frontends measure it the same way.
    let family = "👨‍👩‍👧";
    for (view, tag) in VIEWS {
        let src = format!("a{family}b\n");
        let mut d = doc_in(view, &format!("wide_cluster_{tag}"), &src);
        d.caret = 1 + family.len();
        assert_eq!(
            d.caret_pos(),
            (0, 3),
            "{tag}: 'a' is one cell, the family two"
        );
    }
}

#[test]
fn both_cells_of_a_wide_character_mean_the_character() {
    // Clicking the far half of `好` is still clicking `好`: half a character
    // is not a place the caret can be, so it comes to rest at the
    // character's start — the column it would have been drawn at anyway.
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("wide_click_{tag}"), "你好\n");
        for col in [2, 3] {
            d.caret = 0;
            d.click(0, col, false);
            assert_eq!(d.caret, "你".len(), "{tag}: click at col {col}");
            assert_eq!(d.caret_pos(), (0, 2), "{tag}: click at col {col}");
        }
        // Past the last cell is the line's end, as it is for ASCII.
        d.click(0, 9, false);
        assert_eq!(d.caret, "你好".len(), "{tag}: click past the end");
    }
}

#[test]
fn every_offset_survives_the_trip_out_to_a_column_and_back() {
    // The mapping is only a mapping if it inverts: the cell the caret is
    // drawn in has to be the cell that brings it back to the same offset.
    // Over a fixture where a character may be one cell or two, and one
    // codepoint or five.
    use unicode_segmentation::UnicodeSegmentation;

    let src = "ab 你好 c\n\n👨‍👩‍👧 e\u{0301}x 漢字\n\nplain ascii\n";

    let mut d = doc_in(View::Source, "roundtrip_source", src);
    // Every offset the source view's caret can occupy: it steps by grapheme
    // cluster, so those are its boundaries.
    for (off, _) in src
        .grapheme_indices(true)
        .chain(std::iter::once((src.len(), "")))
    {
        d.caret = off;
        let (row, col) = d.caret_pos();
        d.click(row, col, false);
        assert_eq!(d.caret, off, "source: {off} → ({row}, {col}) → {}", d.caret);
    }

    // And in WYSIWYG, where the offsets the caret can occupy are the map's
    // stops rather than every boundary.
    let mut d = doc_in(View::Wysiwyg, "roundtrip_wysiwyg", src);
    let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
    assert!(stops.len() > 20, "fixture should have plenty of stops");
    for off in stops {
        d.caret = off;
        let (row, col) = d.caret_pos();
        d.click(row, col, false);
        assert_eq!(
            d.caret, off,
            "wysiwyg: {off} → ({row}, {col}) → {}",
            d.caret
        );
    }
}

#[test]
fn vertical_motion_aims_at_a_column_the_reader_can_see() {
    // Down from under `世` lands under the glyph in that cell, not two
    // characters further along the line. The goal is a column, so a line of
    // wide characters and a line of ASCII line up the way they're drawn.
    //
    // The gap differs by view: a bare newline inside a paragraph is a soft
    // break, which WYSIWYG draws as a space on a single row. The views share
    // a grid only where the source's lines are the renderer's rows too.
    for (view, tag) in VIEWS {
        let gap = if view == View::Source { "\n" } else { "\n\n" };
        let src = format!("你好世{gap}abcdef\n");
        let mut d = doc_in(view, &format!("goal_wide_{tag}"), &src);
        d.caret = "你好".len();
        assert_eq!(d.caret_pos().1, 4, "{tag}: `世` is drawn at column 4");
        d.move_down(false);
        assert_eq!(d.caret_pos().1, 4, "{tag}: goal column lost");
        assert!(
            d.source[d.caret..].starts_with('e'),
            "{tag}: landed on the wrong glyph"
        );
    }
}

#[test]
fn a_goal_column_landing_inside_a_wide_character_lands_on_it() {
    // Down from column 3 onto `你好`, whose characters start at columns 0
    // and 2: column 3 is the *second* cell of `好`. There is nowhere to be
    // between the cells of one character, so the caret rests on it — and on
    // its start, which is the only offset there that is a caret stop.
    for (view, tag) in VIEWS {
        let gap = if view == View::Source { "\n" } else { "\n\n" };
        let src = format!("abcdef{gap}你好\n");
        let mut d = doc_in(view, &format!("goal_inside_{tag}"), &src);
        let line = src.find('你').unwrap();
        d.caret = 3;
        d.move_down(false);
        assert_eq!(d.caret, line + "你".len(), "{tag}: landed off `好`'s start");
        assert_eq!(d.caret_pos().1, 2, "{tag}: drew between `好`'s cells");
    }
}

#[test]
fn a_caret_in_a_table_cell_of_wide_text_draws_where_the_text_is() {
    // The column the cell's text is laid out in is measured in cells, so the
    // caret walking that text has to be too — the two agreeing is the whole
    // point of the grid staying square.
    let mut d = wysiwyg_doc("table_wide", "| A | B |\n|---|---|\n| 你好 | y |\n");
    let at = d.source.find("你").unwrap();
    d.caret = at;
    let (row, col) = d.caret_pos();
    // `│ ` opens the row, so the cell's text starts at column 2; `好` is two
    // cells further along.
    assert_eq!(col, 2, "the cell's first character");
    d.move_right(false);
    assert_eq!(
        d.caret_pos(),
        (row, 4),
        "`好` is drawn past `你`'s two cells"
    );
    assert_eq!(d.caret, at + "你".len());
}
