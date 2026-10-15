use super::*;

#[test]
fn click_maps_a_row_col_to_a_byte_offset() {
    let mut d = doc_with("click", "ab\ncd\n");
    d.click(1, 1, false); // row 1 ("cd"), col 1 -> the 'd'
    assert_eq!(d.caret, 4);
}

// A pixel-hit-test placement (the GUI's `place_caret`) must land on a caret
// stop just as the `(row, col)` click path does, so the caret can never come
// to rest in the blank gap between two paragraphs — where it would draw in one
// place and type in another.
#[test]
fn place_caret_snaps_out_of_the_blank_gap_between_paragraphs() {
    // "A\n\nB": offset 2 is the gap the paragraph break is drawn with, not a
    // caret stop (stops are 0,1,3,4).
    let mut d = wysiwyg_doc("place_gap", "A\n\nB");
    assert!(!d.vmap.is_stop(2), "offset 2 should be an unreachable gap");
    d.place_caret(2, false);
    assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
    assert_eq!(d.caret, 1, "should snap to the end of the paragraph above");
}

#[test]
fn place_caret_dragging_through_the_gap_keeps_selection_on_stops() {
    let mut d = wysiwyg_doc("place_gap_drag", "A\n\nB");
    d.place_caret(0, false); // anchor at the start of "A"
    d.place_caret(2, true); // drag into the gap
    assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
    let (s, e) = d.selection().expect("a selection");
    assert!(
        d.vmap.is_stop(s) && d.vmap.is_stop(e),
        "selection {s}..{e} off a stop"
    );
}

#[test]
fn place_caret_on_a_real_stop_is_left_untouched() {
    let mut d = wysiwyg_doc("place_stop", "A\n\nB");
    d.place_caret(3, false); // the start of "B" — a genuine stop
    assert_eq!(d.caret, 3);
}

// An *empty paragraph* (two blank lines, an intentional blank line the user
// opened) is a real caret stop, unlike the gap — a click into it must stay.
#[test]
fn place_caret_rests_in_an_empty_paragraph() {
    let mut d = wysiwyg_doc("place_empty_para", "A\n\n\n\nB");
    let empty = 3; // the navigable empty row's offset (stops: 0,1,3,5,6)
    assert!(d.vmap.is_stop(empty));
    d.place_caret(empty, false);
    assert_eq!(d.caret, empty);
}

// The content end of a hidden mark is a home too (`VisualMap::mark_ends`):
// a drag over the word `bold` ends there, and a caret placed there stays.
#[test]
fn place_caret_rests_at_the_end_of_a_hidden_marks_content() {
    let src = "| A | B |\n| --- | --- |\n| **bold** | other |\n";
    let mut d = wysiwyg_doc("place_mark_end", src);
    let start = src.find("bold").unwrap();
    d.place_caret(start, false);
    d.place_caret(start + 4, true);
    assert_eq!(d.selection(), Some((start, start + 4)), "the whole word");
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, src.replace("**bold**", "bold"));
}

#[test]
fn right_steps_onto_the_end_of_a_mark_and_then_past_its_delimiter() {
    let mut d = wysiwyg_doc("right_mark_end", "a **bold** b");
    d.caret = 7; // before the `d`
    d.move_right(false);
    assert_eq!(d.caret, 8, "onto the end of the bold");
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
    d.move_right(false);
    assert_eq!(d.caret, 10, "past the closing `**`");
    assert!(!d.active_inline_marks().contains(InlineKind::Strong));
    d.move_left(false);
    assert_eq!(d.caret, 8);
    d.move_left(false);
    assert_eq!(d.caret, 7);
    // Typing at the inner home extends the bold.
    d.caret = 8;
    d.insert("!");
    assert_eq!(d.source, "a **bold!** b");
}

#[test]
fn a_marks_end_home_follows_an_edit_through_the_incremental_map() {
    // The splice path shifts the home with the block it is in, and the
    // re-rendered block finds its own again.
    let mut d = wysiwyg_doc("mark_end_splice", "x\n\na **bold** b\n\ny\n");
    d.build_visual_unwrapped();
    d.edit(0, 0, "zz");
    d.build_visual_unwrapped();
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after a shift");
    assert!(d.vmap.is_stop(d.source.find("bold").unwrap() + 4));
    let at = d.source.find("bold").unwrap();
    d.edit(at, at, "very ");
    d.build_visual_unwrapped();
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after a re-render");
    assert!(d.vmap.is_stop(d.source.find("bold").unwrap() + 4));
}

// ── click_past_end ───────────────────────────────────────────────────────

/// [`Doc::click_past_end`] on `body`, and the source it left, with the caret
/// rendered as `|`.
fn past_end(name: &str, body: &str) -> (Doc, String) {
    let mut d = wysiwyg_doc(name, body);
    d.click_past_end();
    let out = render_caret(&d);
    (d, out)
}

#[test]
fn click_past_end_opens_an_empty_paragraph_under_the_last_block() {
    let (mut d, out) = past_end("pe_para", "A\n");
    assert_eq!(out, "A\n\n|");
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(
        d.vmap.row_is_navigable(row),
        "the caret landed on a gap row"
    );
    d.insert("x");
    assert_eq!(d.source, "A\n\nx", "typing merged into A");
}

#[test]
fn click_past_end_writes_both_newlines_when_the_file_has_none() {
    assert_eq!(past_end("pe_bare", "A").1, "A\n\n|");
}

#[test]
fn click_past_end_is_only_a_caret_move_when_the_paragraph_is_already_there() {
    let (d, out) = past_end("pe_there", "A\n\n");
    assert_eq!(out, "A\n\n|");
    assert!(!d.dirty, "a click wrote to a document it did not need to");
    assert!(
        !d.can_undo(),
        "a click that changed nothing left an undo step"
    );
}

#[test]
fn click_past_end_leaves_a_closed_fence() {
    let (mut d, out) = past_end("pe_fence", "```\ncode\n```\n");
    assert_eq!(out, "```\ncode\n```\n\n|");
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(!d.vmap.rows[row].code, "the caret is still on a code row");
    d.insert("x");
    assert_eq!(d.source, "```\ncode\n```\n\nx");
}

#[test]
fn click_past_end_closes_an_unclosed_fence_first() {
    assert_eq!(past_end("pe_open", "```\ncode\n").1, "```\ncode\n```\n\n|");
    assert_eq!(
        past_end("pe_open2", "````\ncode").1,
        "````\ncode\n````\n\n|"
    );
    assert_eq!(past_end("pe_tilde", "~~~\ncode\n").1, "~~~\ncode\n~~~\n\n|");
    assert_eq!(
        past_end("pe_quoted", "> ```\n> code\n").1,
        "> ```\n> code\n> ```\n\n|"
    );
}

#[test]
fn click_past_end_does_not_close_an_indented_block() {
    assert_eq!(past_end("pe_indent", "    code\n").1, "    code\n\n|");
}

#[test]
fn click_past_end_under_a_list_and_a_table_leaves_them() {
    let (mut d, out) = past_end("pe_list", "- a\n- b\n");
    assert_eq!(out, "- a\n- b\n\n|");
    d.insert("x");
    assert_eq!(d.source, "- a\n- b\n\nx", "typed into the list");
    assert_eq!(
        past_end("pe_table", "| a |\n|---|\n| b |\n").1,
        "| a |\n|---|\n| b |\n\n|"
    );
}

#[test]
fn click_past_end_on_an_empty_document_writes_nothing() {
    let (d, out) = past_end("pe_empty", "");
    assert_eq!(out, "|");
    assert!(!d.dirty);
}

#[test]
fn click_past_end_is_one_undo_step() {
    let (mut d, _) = past_end("pe_undo", "A\n");
    d.undo();
    assert_eq!(d.source, "A\n");
}

#[test]
fn click_past_end_in_the_source_view_only_moves_the_caret() {
    let mut d = doc_with("pe_source", "A\n");
    d.click_past_end();
    assert_eq!(render_caret(&d), "A\n|");
    assert!(!d.dirty);
}

#[test]
fn click_past_end_on_a_read_only_document_only_moves_the_caret() {
    let mut d = wysiwyg_doc("pe_ro", "A\n");
    d.set_read_only(true);
    d.click_past_end();
    assert_eq!(render_caret(&d), "A\n|");
}
