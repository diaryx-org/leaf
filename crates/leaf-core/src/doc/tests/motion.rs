use super::*;

#[test]
fn motion_and_delete_treat_an_emoji_as_one_character() {
    // 👨‍👩‍👧 is a single grapheme built from three emoji joined by ZWJ — 18
    // bytes, several codepoints. Right-arrow must clear it in one step, and
    // backspace must remove the whole cluster, not a stray joiner.
    let family = "👨‍👩‍👧";
    let mut d = doc_with("emoji", &format!("a{family}b\n"));
    d.caret = 1; // just after 'a', before the emoji
    d.move_right(false);
    assert_eq!(
        d.caret,
        1 + family.len(),
        "one step clears the whole cluster"
    );
    assert_eq!(&d.source[d.caret..d.caret + 1], "b");

    d.backspace(); // delete the emoji as a unit
    assert_eq!(d.source, "ab\n");
    assert_eq!(d.caret, 1);
}

#[test]
fn motion_handles_a_combining_accent_as_one_character() {
    // "e" + U+0301 (combining acute) renders as one é.
    let mut d = doc_with("combining", "e\u{0301}x\n");
    d.caret = 0;
    d.move_right(false);
    assert_eq!(
        d.caret,
        "e\u{0301}".len(),
        "steps past base + combining mark"
    );
}

// ── goal column ──────────────────────────────────────────────────────────

#[test]
fn vertical_motion_goal_column_survives_a_short_line() {
    // Regression: re-deriving the column from the clamped position on
    // every step permanently forgets it once a short line clamps it.
    // Down through "xy" (2 cols) and into "ghijkl" must return to col 4.
    let g = |m, f: fn(&mut Doc)| golden("goalcol", m, f);
    assert_eq!(
        g("abcd|ef\nxy\nghijkl\n", |d| {
            d.move_down(false); // clamps to end of "xy"
            d.move_down(false); // restores col 4 on the long line
        }),
        "abcdef\nxy\nghij|kl\n"
    );
}

#[test]
fn goal_column_state_is_set_by_vertical_motion_and_cleared_by_horizontal() {
    let mut d = doc_with("goalcol_state", "abcdef\nxy\nghijkl\n");
    assert_eq!(d.goal_col, None);
    d.caret = 4; // row 0, col 4
    d.move_down(false); // clamps into "xy"; goal stays the original col
    assert_eq!(d.goal_col, Some(4));
    assert_eq!(d.caret_pos(), (1, 2));

    // A horizontal motion drops the goal column...
    d.move_left(false);
    assert_eq!(d.goal_col, None);

    // ...so the next vertical motion picks up the *new* column (1), not
    // the stale one (4).
    d.move_down(false);
    assert_eq!(d.goal_col, Some(1));
    assert_eq!(d.caret_pos(), (2, 1));
}

#[test]
fn editing_clears_the_goal_column() {
    let mut d = doc_with("goalcol_edit", "abcdef\nxy\nghijkl\n");
    d.caret = 4;
    d.move_down(false);
    assert_eq!(d.goal_col, Some(4));
    d.insert("Z");
    assert_eq!(d.goal_col, None);
}

#[test]
fn vertical_motion_on_an_empty_document_is_a_no_op() {
    let mut d = doc_with("empty_vert", "");
    d.move_down(false);
    assert_eq!(d.caret, 0);
    d.move_up(false);
    assert_eq!(d.caret, 0);
}

// ── the document's edges ─────────────────────────────────────────────────

#[test]
fn vertical_motion_at_the_document_edges_runs_to_them_in_both_views() {
    // The reproduction, and the disagreement: Down on the last line ran to
    // the end of the document in the source view — by accident, an
    // out-of-range row clamping to the end of the string — and did nothing
    // whatever in the view leaf opens in. One rule now, in both.
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("edge_{tag}"), "abc");
        d.caret = 1;
        d.move_down(false);
        assert_eq!(d.caret, 3, "{tag}: Down on the last line runs to the end");
        d.move_up(false);
        assert_eq!(d.caret, 0, "{tag}: Up on the first line runs to the start");
    }
}

#[test]
fn vertical_motion_at_the_edges_carries_the_column_across_the_lines_between() {
    // Down off the bottom is a motion like any other, so it latches a goal
    // column — and Up comes back to the column the caret left, not to the
    // one the document's end happened to be in.
    for (view, tag) in VIEWS {
        let gap = if view == View::Source { "\n" } else { "\n\n" };
        let src = format!("abcdef{gap}ghijkl");
        let mut d = doc_in(view, &format!("edge_goal_{tag}"), &src);
        d.caret = 2; // row 0, col 2
        d.move_down(false);
        assert_eq!(d.caret_pos().1, 2, "{tag}: Down keeps the column");
        d.move_down(false);
        assert_eq!(
            d.caret,
            src.len(),
            "{tag}: Down off the bottom reaches the end"
        );
        d.move_up(false);
        assert_eq!(
            d.caret_pos().1,
            2,
            "{tag}: Up returns to the column Down left"
        );
    }
}

#[test]
fn vertical_motion_with_nowhere_to_go_latches_no_goal_column() {
    // `goal_col.get_or_insert` ran *before* the early return at row 0, so an
    // Up that did nothing still armed a goal column, and the next Down aimed
    // at a column the caret had never been in.
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("noop_goal_{tag}"), "abc\n\ndef");
        d.caret = 0;
        d.move_up(false);
        assert_eq!(d.caret, 0, "{tag}: already at the start");
        assert_eq!(d.goal_col, None, "{tag}: a no-op Up latched a goal column");

        d.caret = d.source.len();
        d.move_down(false);
        assert_eq!(d.caret, d.source.len(), "{tag}: already at the end");
        assert_eq!(
            d.goal_col, None,
            "{tag}: a no-op Down latched a goal column"
        );
    }
}

// ── soft wrap ────────────────────────────────────────────────────────────
// Every other test here builds the map at 80 columns, where no fixture is
// long enough to fold. A wrap is where one offset belongs to two rows at
// once, and it broke everything that asks the caret what row it is on.

/// The wrapped fixture these cases share, folded at 12 columns into
/// `one two ` / `three four ` / `five six ` / `seven eight`.
fn wrapped_doc(name: &str) -> Doc {
    let mut d = wysiwyg_doc(name, "one two three four five six seven eight");
    d.build_visual(12);
    d
}

#[test]
fn home_and_end_work_from_a_wrapped_row() {
    // The reproduction: offset 19 is the `f` of "five", the first character
    // of the third row — and also the offset the second row ends at. It
    // resolved to the *second* row, so End aimed at a place the caret was
    // already in and did nothing, while Home walked backwards onto a row the
    // caret had left.
    let mut d = wrapped_doc("wrap_home_end");
    d.caret = 19;
    assert_eq!(
        d.caret_pos(),
        (2, 0),
        "the wrap boundary opens the third row"
    );
    d.move_end(false);
    assert_eq!(d.caret, 27, "End stalled at the wrap boundary");
    d.move_home(false);
    assert_eq!(d.caret, 19, "Home left the row the caret was on");
}

#[test]
fn end_of_a_wrapped_row_stays_put_when_pressed_again() {
    // The row's end is the last offset that is only ever its own: the offset
    // past it opens the row below, and aiming there would send a second
    // press on to *that* row's end, and a third to the next — End walking
    // down the paragraph rather than sitting where it landed.
    let mut d = wrapped_doc("wrap_end_twice");
    d.caret = 12; // inside "three", on the second row
    d.move_end(false);
    assert_eq!(
        d.caret, 18,
        "the end of `three four`, before the space the wrap ate"
    );
    assert_eq!(d.caret_pos(), (1, 10), "drawn on the row it is the end of");
    d.move_end(false);
    assert_eq!(d.caret, 18, "a second End moved the caret");
    d.move_home(false);
    assert_eq!(d.caret, 8, "Home takes the row's own start");
}

#[test]
fn vertical_motion_crosses_a_soft_wrap() {
    // Down aimed at the row below's column 0, an offset that resolved *up*
    // to the row above's end — so it landed on the offset it already had and
    // the caret could never leave a paragraph's first row.
    let mut d = wrapped_doc("wrap_down");
    d.caret = 0;
    for (want, row) in [(8, 1), (19, 2), (28, 3), (39, 3)] {
        d.move_down(false);
        assert_eq!(d.caret, want, "Down stalled");
        assert_eq!(d.caret_pos().0, row, "Down landed on the wrong row");
    }
    d.move_down(false);
    assert_eq!(d.caret, 39, "the last row's Down runs to the end and stops");

    // ...and back up, one row per press. The goal column is the end of the
    // last row, past every other row's width, so each press clamps to the
    // row's own last offset rather than to the one that opens the next.
    let mut d = wrapped_doc("wrap_up");
    d.caret = 39;
    for (want, pos) in [(27, (2, 8)), (18, (1, 10)), (7, (0, 7)), (0, (0, 0))] {
        d.move_up(false);
        assert_eq!(d.caret, want, "Up stalled");
        assert_eq!(d.caret_pos(), pos, "Up landed on the wrong row");
    }
}

#[test]
fn a_kill_on_a_wrapped_row_stops_at_the_row() {
    // The kills take the same line Home and End do, so in WYSIWYG they take
    // the visual row — and a soft wrap has no newline in it to delete, so
    // nothing is joined by reaching the end of one.
    let mut d = wrapped_doc("wrap_kill");
    d.caret = 19; // the `f` of "five", opening the third row
    d.delete_to_line_end();
    // The space the wrap ate goes with the row it was drawn on: sparing it
    // would leave "four  seven", two spaces where the row had been.
    assert_eq!(d.source, "one two three four seven eight");

    // Backwards from the row's last caret position — which is *before* that
    // space, so this one survives, being on the far side of the caret.
    let mut d = wrapped_doc("wrap_kill_back");
    d.caret = 27;
    d.delete_to_line_start();
    assert_eq!(d.source, "one two three four  seven eight");
}

// ── document start / end ────────────────────────────────────────────────

#[test]
fn move_doc_start_and_end_jump_to_the_edges() {
    let g = |m, f: fn(&mut Doc)| golden("doc_edges", m, f);
    assert_eq!(
        g("hello\nwor|ld\n", |d| d.move_doc_start(false)),
        "|hello\nworld\n"
    );
    assert_eq!(
        g("hel|lo\nworld\n", |d| d.move_doc_end(false)),
        "hello\nworld\n|"
    );
    // Already at the edge: a no-op.
    assert_eq!(g("|hello\n", |d| d.move_doc_start(false)), "|hello\n");
    assert_eq!(g("hello|\n", |d| d.move_doc_end(false)), "hello\n|");
}

#[test]
fn move_doc_start_and_end_extend_the_selection() {
    assert_eq!(
        golden("doc_edges_ext_end", "hello wor|ld\n", |d| d
            .move_doc_end(true)),
        "hello wor[ld\n|]"
    );
    assert_eq!(
        golden("doc_edges_ext_start", "hello wor|ld\n", |d| d
            .move_doc_start(true)),
        "[|hello wor]ld\n"
    );
}

#[test]
fn move_doc_start_and_end_on_an_empty_document_are_a_no_op() {
    let mut d = doc_with("empty_edges", "");
    d.move_doc_end(false);
    assert_eq!(d.caret, 0);
    d.move_doc_start(false);
    assert_eq!(d.caret, 0);
}

// ── arrow collapses an active selection ─────────────────────────────────

#[test]
fn arrow_collapses_selection_to_its_near_edge() {
    let mut d = doc_with("collapse", "hello world\n");

    // Forward selection (anchor before caret): Right -> end, Left -> start.
    d.anchor = Some(2);
    d.caret = 7;
    d.move_right(false);
    assert_eq!((d.caret, d.anchor), (7, None));

    d.anchor = Some(2);
    d.caret = 7;
    d.move_left(false);
    assert_eq!((d.caret, d.anchor), (2, None));

    // Backward selection (anchor after caret): edges are the same
    // regardless of which end the caret started on.
    d.anchor = Some(7);
    d.caret = 2;
    d.move_right(false);
    assert_eq!((d.caret, d.anchor), (7, None));

    d.anchor = Some(7);
    d.caret = 2;
    d.move_left(false);
    assert_eq!((d.caret, d.anchor), (2, None));
}

#[test]
fn arrow_with_extend_keeps_growing_the_selection() {
    let mut d = doc_with("collapse_extend", "hello world\n");
    d.anchor = Some(2);
    d.caret = 7;
    d.move_right(true); // extend: no collapse, caret steps one further
    assert_eq!((d.caret, d.anchor), (8, Some(2)));
}

#[test]
fn arrow_without_a_selection_moves_one_character_as_before() {
    let mut d = doc_with("no_collapse", "hello\n");
    d.caret = 2;
    d.move_right(false);
    assert_eq!(d.caret, 3);
    d.move_left(false);
    assert_eq!(d.caret, 2);
}

#[test]
fn the_caret_crosses_a_soft_break() {
    // A newline inside a paragraph is a `soft_break`, which twig gives no
    // span of its own — the space it renders as used to borrow the offset of
    // the character before it, and a caret can't move without changing
    // offset. Right must walk clean off the end of the first line.
    let mut d = wysiwyg_doc("soft_break_walk", "one two\nthree four\n");
    d.caret = 0;
    let seen = walk_right(&mut d);
    assert_eq!(seen, (0..=18).collect::<Vec<_>>(), "walk stalled: {seen:?}");
}

#[test]
fn line_flow_preserve_resplits_the_map_and_defaults_to_fold() {
    // The paragraph holds one soft break. Folded (the default) it lays out as
    // a single reflowed row; Preserve re-lays it as a row per source line.
    // The setter must invalidate the cached map for the change to show, and
    // again on the way back — so a round trip returns to the folded layout.
    let mut d = wysiwyg_doc("line_flow", "one two\nthree four\n");
    assert_eq!(d.line_flow(), LineFlow::Fold, "fold is the default");
    d.build_visual(80);
    assert_eq!(d.vmap.num_rows(), 1, "fold: one flowing row");

    d.set_line_flow(LineFlow::Preserve);
    d.build_visual(80);
    assert_eq!(d.vmap.num_rows(), 2, "preserve: a row per source line");

    d.set_line_flow(LineFlow::Fold);
    d.build_visual(80);
    assert_eq!(d.vmap.num_rows(), 1, "fold again: back to one row");
}

#[test]
fn the_caret_still_crosses_a_preserved_soft_break() {
    // Preserve renders the soft break as a row boundary rather than a space,
    // but the caret must still reach every offset — the break's own offset is
    // the first row's end stop, so Right walks clean off the end of line one
    // onto line two, exactly as it does when the break is folded.
    let mut d = wysiwyg_doc("preserve_walk", "one two\nthree four\n");
    d.set_line_flow(LineFlow::Preserve);
    d.build_visual(80);
    d.caret = 0;
    let seen = walk_right(&mut d);
    assert_eq!(seen, (0..=18).collect::<Vec<_>>(), "walk stalled: {seen:?}");
}

#[test]
fn the_caret_walks_a_code_block() {
    // Every glyph of a code block used to map to the block's start, so the
    // whole block was a single offset and the caret couldn't move inside it.
    let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
    let mut d = wysiwyg_doc("code_walk", src);
    d.caret = 0;
    let seen = walk_right(&mut d);
    // The fences are markup: hidden, and no caret stop. The code between
    // them is reached a character at a time.
    let code = src.find("let").unwrap()..src.find("\n```").unwrap();
    for off in code.clone() {
        assert!(seen.contains(&off), "offset {off} unreachable: {seen:?}");
    }
    assert!(seen.contains(&code.end), "no stop after the last line");
}

#[test]
fn the_caret_walks_an_indented_code_block() {
    // An indented block's text has the four-space indent stripped, so it
    // isn't a verbatim slice and its lines have to be re-found. The caret
    // lands on the code, never in the indent.
    let src = "    indented\n    code\n";
    let mut d = wysiwyg_doc("indent_code_walk", src);
    d.caret = 0;
    let seen = walk_right(&mut d);
    assert!(seen.contains(&src.find("indented").unwrap()));
    assert!(seen.contains(&src.find("code").unwrap()));
    assert!(
        !seen.contains(&0) || seen[0] == 0,
        "the caret starts where it was put"
    );
    // Nothing in the stripped indent is a stop.
    for off in [1, 2, 3] {
        assert!(!seen.contains(&off), "landed in the indent at {off}");
    }
}

#[test]
fn the_caret_leaves_a_tight_heading() {
    // "# H" with text directly under it: the heading row's end and the
    // separator row's end are the same offset. Right used to find the
    // separator's copy, set the caret to where it already was, and stop.
    let mut d = wysiwyg_doc("tight_heading_walk", "# H\ntext\n");
    d.caret = 2; // the "H"
    let seen = walk_right(&mut d);
    assert!(
        seen.len() > 2,
        "Right stalled at the heading's end: {seen:?}"
    );
    assert!(
        seen.contains(&8),
        "never reached the end of \"text\": {seen:?}"
    );
}

#[test]
fn the_caret_skips_the_gap_between_two_paragraphs() {
    // The blank line between two paragraphs is the boundary itself. The
    // caret used to be able to sit on it, and typing there landed in the
    // previous paragraph — "A\n\nB" became "A\nx\nB", one paragraph with a
    // soft break, so the text visibly snapped back up.
    let mut d = wysiwyg_doc("gap_skip", "A\n\nB\n");
    d.caret = 1; // the end of "A"
    d.move_right(false);
    assert_eq!(d.caret, 3, "Right stopped in the gap");
    d.insert("x");
    assert_eq!(d.source, "A\n\nxB\n", "typing landed outside B");
}

#[test]
fn down_from_a_paragraph_lands_on_the_next_one() {
    let mut d = wysiwyg_doc("gap_down", "A\n\nB\n");
    d.caret = 0;
    d.move_down(false);
    assert_eq!(d.caret, 3, "Down stopped in the gap");
}

#[test]
fn clicking_the_gap_lands_on_real_text() {
    // A click can still *reach* the gap — it's drawn, so it's clickable.
    // It has to resolve to somewhere the caret can be.
    let mut d = wysiwyg_doc("gap_click", "A\n\nB\n");
    d.click(1, 0, false); // the gap row
    assert!(
        d.caret == 1 || d.caret == 3,
        "click left the caret in the gap at {}",
        d.caret
    );
    d.insert("x");
    // Either edge of the boundary is a fair place to land; inside it isn't.
    assert!(
        d.source == "Ax\n\nB\n" || d.source == "A\n\nxB\n",
        "click in the gap typed into the boundary: {:?}",
        d.source
    );
}

#[test]
fn enter_opens_an_empty_paragraph_the_caret_can_type_into() {
    // Enter inserts a paragraph break, which leaves a blank line spare on
    // either side of a new one. That middle line is a real empty paragraph:
    // the caret lands there, and typing makes a paragraph rather than
    // extending a neighbour.
    let mut d = wysiwyg_doc("gap_enter", "A\n\nB\n");
    d.caret = 1;
    d.newline();
    assert_eq!(d.source, "A\n\n\n\nB\n");
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(
        d.vmap.row_is_navigable(row),
        "the caret landed on a gap row"
    );
    d.insert("x");
    assert_eq!(
        d.source, "A\n\nx\n\nB\n",
        "the new paragraph merged into a neighbour"
    );
}

#[test]
fn enter_at_the_end_of_the_document_opens_a_paragraph_too() {
    let mut d = wysiwyg_doc("gap_eof", "A\n");
    d.caret = 1;
    d.newline();
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(
        d.vmap.row_is_navigable(row),
        "the caret landed on a gap row"
    );
    d.insert("x");
    assert!(
        d.source.starts_with("A\n\n") && d.source.contains('x'),
        "typing at the end merged into A: {:?}",
        d.source
    );
}
