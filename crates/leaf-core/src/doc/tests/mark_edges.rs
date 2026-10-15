use super::*;

// ── the mark-edge rule (see `Doc::splice`) ───────────────────────────────

#[test]
fn a_space_typed_in_a_bold_run_never_leaves_the_delimiters_showing() {
    // The reported bug, keystroke for keystroke: ⌘b, "bold", space, "hey".
    // The space inside the run made `**bold **`, which is *not* bold — four
    // literal asterisks — so the rich view drew them, correctly and
    // uselessly, until the next character happened to close the run again.
    let mut d = wysiwyg_doc("edge_typing", "a \n");
    d.caret = 2;
    d.toggle(InlineKind::Strong);
    for c in "bold".chars() {
        d.insert(&c.to_string());
    }
    assert_eq!(d.source, "a **bold**\n");
    d.insert(" ");
    assert_eq!(
        d.source, "a **bold** \n",
        "the space belongs outside the run"
    );
    assert!(
        d.active_inline_marks().contains(InlineKind::Strong),
        "bold is still what's being typed, so the button stays lit"
    );
    // What the writer is looking at while all this happens: their words.
    d.build_visual(80);
    let drawn: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(drawn, "a bold ", "no delimiter ever surfaces: {}", d.source);
    for c in "hey".chars() {
        d.insert(&c.to_string());
    }
    assert_eq!(
        d.source, "a **bold hey**\n",
        "one bold phrase, not two runs"
    );
}

#[test]
fn typing_past_a_space_can_still_leave_the_bold_behind() {
    // The other half: the marks stay armed across the space, so ⌘b turns
    // them off again there and the next word is plain — the run isn't
    // rejoined by a caret that was told not to.
    let mut d = wysiwyg_doc("edge_shed", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    for c in "bold ".chars() {
        d.insert(&c.to_string());
    }
    assert_eq!(d.source, "**bold** \n");
    d.toggle(InlineKind::Strong);
    assert!(!d.active_inline_marks().contains(InlineKind::Strong));
    d.insert("x");
    assert_eq!(d.source, "**bold** x\n");
}

#[test]
fn a_space_typed_first_of_all_still_leaves_the_mark_armed() {
    // ⌘b and then a space before any word: the space is not marked (nothing
    // is), and the word after it is.
    let mut d = wysiwyg_doc("edge_space_first", "a\n");
    d.caret = 1;
    d.toggle(InlineKind::Strong);
    d.insert(" ");
    assert_eq!(d.source, "a \n");
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
    d.insert("b");
    assert_eq!(d.source, "a **b**\n");
}

#[test]
fn a_space_typed_at_either_edge_of_an_existing_mark_steps_outside_it() {
    let mut d = wysiwyg_doc("edge_tail", "x **bold**\n");
    d.caret = 8; // the caret's home at the end of the run's text
    d.insert(" ");
    assert_eq!(
        d.source, "x **bold** \n",
        "the space lands past the delimiters"
    );
    assert_eq!(d.caret, 11, "and the caret stands past it, outside the run");

    let mut d = wysiwyg_doc("edge_head", "x **bold** y\n");
    d.caret = 4; // in front of the "b"
    d.insert(" ");
    assert_eq!(d.source, "x  **bold** y\n");
    assert_eq!(d.caret, 3, "in front of the run, where the space was typed");
}

#[test]
fn a_delete_that_backs_a_space_onto_a_delimiter_moves_the_delimiter() {
    // Backspace over the last letter of a bold phrase.
    let mut d = wysiwyg_doc("edge_bksp", "a **bold h**\n");
    d.caret = 10; // past the "h"
    d.backspace();
    assert_eq!(d.source, "a **bold** \n");
    assert_eq!(d.caret, 11, "the caret keeps the place on screen it had");
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
    d.insert("x");
    assert_eq!(d.source, "a **bold x**\n", "and typing rejoins the run");
}

#[test]
fn deleting_the_last_of_a_run_takes_its_delimiters_with_it() {
    // `**b**` with the `b` gone is `****`: two delimiters with nothing to
    // mark, which is only text. The marks live on in the caret instead.
    let mut d = wysiwyg_doc("edge_empty", "a **b** c\n");
    d.caret = 5;
    d.backspace();
    assert_eq!(d.source, "a  c\n");
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
    d.insert("x");
    assert_eq!(d.source, "a **x** c\n");
}

#[test]
fn typing_over_a_whole_bold_word_keeps_it_bold() {
    let mut d = wysiwyg_doc("edge_replace", "a **bold** c\n");
    d.anchor = Some(4);
    d.caret = 8; // the word, not its delimiters
    d.insert("x");
    assert_eq!(d.source, "a **x** c\n");
}

#[test]
fn a_code_span_keeps_the_space_it_is_given() {
    // Backticks are not whitespace-sensitive the way `**` is: `` `code ` ``
    // is still verbatim, so nothing is re-spelt. The repair asks the parser
    // rather than a table of kinds, and this is the answer it gets.
    let mut d = wysiwyg_doc("edge_code", "a `code` c\n");
    d.caret = 7;
    d.insert(" ");
    assert_eq!(d.source, "a `code ` c\n");
}

#[test]
fn a_delete_from_a_runs_outer_edge_reaches_into_the_run() {
    // A run's closing delimiter has a caret home on each side of it, one
    // column apart on screen — and a plain ← off the space after a bold word
    // lands on the outer one. The character drawn behind the caret there is
    // still the last letter of the phrase, so that is what Backspace takes;
    // the byte behind it is a `*` nobody can see.
    let mut d = wysiwyg_doc("edge_outer_close", "**bold** x\n");
    d.caret = 9;
    d.move_left(false);
    assert_eq!(d.caret, 8, "← rests past the delimiters, not inside them");
    d.backspace();
    assert_eq!(
        d.source, "**bol** x\n",
        "a letter of the phrase, not its `*`"
    );
    assert_eq!(d.caret, 5);

    // And the mirror in front of the opening delimiter, where Delete's
    // character is the first letter of the run.
    let mut d = wysiwyg_doc("edge_outer_open", "x**bold**\n");
    d.caret = 1;
    d.delete_forward();
    assert_eq!(d.source, "x**old**\n");
    assert_eq!(d.caret, 3, "inside the run, in front of what is left of it");
}

#[test]
fn a_delete_at_a_run_edge_never_eats_a_delimiter() {
    // The byte beside the caret at either edge of a bold word is a `*` the
    // rich view draws nothing for. Taking it is not the character delete the
    // key was pressed for — it unspells the run and puts a literal asterisk
    // on screen (`a *bold** c`). The visible character is the one that goes.
    let mut d = wysiwyg_doc("edge_open_bksp", "a **bold** c\n");
    d.caret = 4; // in front of the "b"
    d.backspace();
    assert_eq!(d.source, "a**bold** c\n", "the space goes, the run stands");

    let mut d = wysiwyg_doc("edge_close_del", "a **bold** c\n");
    d.caret = 8; // past the "d"
    d.delete_forward();
    assert_eq!(d.source, "a **bold**c\n");
    assert_eq!(d.caret, 8, "and the caret stays inside the run");
    d.insert("x");
    assert_eq!(d.source, "a **boldx**c\n");

    // A code span's backticks are hidden the same way, so they are covered
    // by the same rule and not by a list of kinds.
    let mut d = wysiwyg_doc("edge_open_code", "a `code` c\n");
    d.caret = 3;
    d.backspace();
    assert_eq!(d.source, "a`code` c\n");
}

#[test]
fn the_source_view_deletes_the_delimiter_byte_it_is_shown() {
    // The asterisks are on the screen there and the caret can stand between
    // them, so a delete takes exactly the byte it is aimed at.
    let mut d = doc_with("edge_open_src", "a **bold** c\n");
    d.caret = 4;
    d.backspace();
    assert_eq!(d.source, "a *bold** c\n");

    let mut d = doc_with("edge_close_src", "a **bold** c\n");
    d.caret = 8;
    d.delete_forward();
    assert_eq!(d.source, "a **bold* c\n");
}

#[test]
fn backspacing_the_space_out_of_a_bold_phrase_leaves_the_caret_in_it() {
    // The reported bug, keystroke for keystroke: ⌘b, "bold", space, Backspace.
    // The space had stepped outside the run (the mark-edge rule), taking the
    // caret with it, so the delete put it back down on the far side of the
    // closing `**` — one place on screen, and the wrong side of it. Typing
    // came out plain and the toolbar went dark, with nothing to see.
    let mut d = wysiwyg_doc("edge_bksp_space", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    for c in "bold".chars() {
        d.insert(&c.to_string());
    }
    d.insert(" ");
    assert_eq!(d.source, "**bold** \n");
    d.backspace();
    assert_eq!(
        d.source, "**bold**\n",
        "the space goes, the delimiters stay"
    );
    assert_eq!(d.caret, 6, "and the caret comes back inside the run");
    assert!(
        d.active_inline_marks().contains(InlineKind::Strong),
        "so the button is still lit"
    );
    d.insert("x");
    assert_eq!(
        d.source, "**boldx**\n",
        "and the next character is still bold"
    );
}

#[test]
fn a_second_backspace_there_deletes_a_letter_of_the_phrase() {
    // What the stranded caret did next: the byte behind it was the closing
    // `*`, so a second press took that instead of a letter — `**bold*`, the
    // styling gone and an asterisk on the screen where the word had been.
    let mut d = wysiwyg_doc("edge_bksp_twice", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    for c in "bold ".chars() {
        d.insert(&c.to_string());
    }
    assert_eq!(d.source, "**bold** \n");
    d.backspace();
    d.backspace();
    assert_eq!(d.source, "**bol**\n", "the delete lands inside the run");
    assert_eq!(d.caret, 5);
}

#[test]
fn a_delete_that_ends_at_a_nested_run_settles_inside_every_delimiter() {
    // `***both***` closes two runs with one stack of asterisks: the caret has
    // to walk in through all of them, or it lands between the emph and the
    // strong and types half-marked.
    let mut d = wysiwyg_doc("edge_bksp_nested", "***both*** \n");
    d.caret = 11;
    d.backspace();
    assert_eq!(d.source, "***both***\n");
    assert_eq!(d.caret, 7, "past the last letter, inside both runs");
    d.insert("x");
    assert_eq!(d.source, "***bothx***\n");
}

#[test]
fn a_delete_that_ends_mid_run_leaves_the_caret_where_it_fell() {
    // The settle only moves a caret a run actually closed over. Ordinary
    // deletes — inside a run, or in plain prose — are untouched.
    let mut d = wysiwyg_doc("edge_bksp_mid", "a **bold** c\n");
    d.caret = 8;
    d.backspace();
    assert_eq!(d.source, "a **bol** c\n");
    assert_eq!(d.caret, 7);

    let mut d = wysiwyg_doc("edge_bksp_plain", "plain\n");
    d.caret = 5;
    d.backspace();
    assert_eq!(d.source, "plai\n");
    assert_eq!(d.caret, 4);
}

#[test]
fn the_source_view_leaves_a_delete_where_it_landed() {
    // The delimiters are on the screen there, so the offset past them is a
    // place the caret can be seen to be — nothing to settle.
    let mut d = doc_with("edge_bksp_src", "**bold** \n");
    d.caret = 9;
    d.backspace();
    assert_eq!(d.source, "**bold**\n");
    assert_eq!(d.caret, 8);
}

#[test]
fn the_mark_edge_rule_clears_every_delimiter_of_a_nested_run() {
    // `***both***` closes two runs with one stack of asterisks; a space that
    // clears only the inner one lands against the outer's and breaks that
    // instead.
    let mut d = wysiwyg_doc("edge_nested", "a ***both***\n");
    d.caret = 9;
    d.insert(" ");
    assert_eq!(d.source, "a ***both*** \n");
    assert_eq!(d.caret, 13);
    d.insert("x");
    assert_eq!(d.source, "a ***both x***\n");
}

#[test]
fn the_mark_edge_repair_undoes_with_the_keystroke_that_caused_it() {
    // The delimiter shuffle is not an edit the writer made, so it is not a
    // step they have to undo past.
    let mut d = wysiwyg_doc("edge_undo", "a **bold**\n");
    d.caret = 8;
    d.insert(" ");
    assert_eq!(d.source, "a **bold** \n");
    d.undo();
    assert_eq!(d.source, "a **bold**\n");
}

#[test]
fn the_source_view_types_the_space_where_it_was_asked_to() {
    // The rule is a rich-view courtesy. In the source view the delimiters are
    // on the screen and the user is editing the bytes they can see.
    let mut d = doc_with("edge_src", "a **bold** c\n");
    d.caret = 8;
    d.insert(" ");
    assert_eq!(d.source, "a **bold ** c\n");
}

#[test]
fn toggling_a_mark_over_a_selection_leaves_its_edge_whitespace_out() {
    // Double-clicking a word takes the space after it; bolding that must not
    // spell `**word **`, which is not bold at all.
    let mut d = wysiwyg_doc("edge_sel", "a word b\n");
    d.anchor = Some(2);
    d.caret = 7; // "word "
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a **word** b\n");
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a word b\n");
    d.toggle(InlineKind::Strong);
    assert_eq!(
        d.source, "a **word** b\n",
        "reapplying the mark must not wrap stale delimiter offsets"
    );
    // And a selection of nothing but whitespace has no word to mark.
    let mut d = wysiwyg_doc("edge_sel_ws", "a word b\n");
    d.anchor = Some(6);
    d.caret = 7;
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a word b\n");
    assert!(d.status.is_some());
}

#[test]
fn set_block_turns_a_paragraph_into_a_heading_at_the_caret() {
    let mut d = doc_with("head_set", "hello\n");
    d.caret = 2; // caret inside the paragraph, no selection
    d.set_block(BlockKind::Heading(1));
    assert_eq!(d.source, "# hello\n");
}

#[test]
fn set_block_heading_works_in_wysiwyg_view() {
    // The app defaults to WYSIWYG; the caret is a source offset either way.
    let mut d = wysiwyg_doc("head_wys", "hello\n");
    d.caret = 2;
    d.set_block(BlockKind::Heading(1));
    assert_eq!(d.source, "# hello\n");
}

#[test]
fn toggle_heading_applies_switches_and_reverts() {
    let mut d = doc_with("head_toggle", "hello\n");
    d.caret = 2;
    d.toggle_heading(1);
    assert_eq!(d.source, "# hello\n"); // paragraph → H1
    d.toggle_heading(2);
    assert_eq!(d.source, "## hello\n"); // H1 → H2 (different level switches)
    d.toggle_heading(2);
    assert_eq!(d.source, "hello\n"); // same level reverts to paragraph
}

#[test]
fn preserve_enter_at_a_line_end_lands_the_caret_on_the_new_blank_line() {
    // Regression: Enter at the end of a soft-break line (mid-paragraph) opened
    // the blank line but the caret rendered on the *next* line, because the
    // separator was a non-navigable decoration row. In Preserve flow that
    // blank line is a real caret home — the caret must resolve onto it, and
    // typing there makes the soft break that continues the paragraph.
    let src = "line one:\nsecond line\n";
    let mut d = wysiwyg_doc("pre_enter_lineend", src);
    d.set_line_flow(LineFlow::Preserve);
    d.build_visual_unwrapped(); // the GUI path (pixel-wrapped)
    d.caret = 9; // the visual end of row 0, at the soft-break '\n'
    d.newline();
    d.build_visual_unwrapped();
    assert_eq!(d.source, "line one:\n\nsecond line\n");
    assert_eq!(
        d.caret, 10,
        "caret sits on the new blank line, not the next line"
    );
    // The blank line is row 1, and the caret resolves onto it — not row 2.
    assert_eq!(
        d.vmap.pos_of_offset(10),
        (1, 0),
        "caret renders on the blank row"
    );
    assert!(
        !d.vmap.rows[1].decoration,
        "the blank line is navigable in Preserve"
    );
    // Typing there makes a soft break: one paragraph, three lines.
    d.insert("new clause,");
    assert_eq!(d.source, "line one:\nnew clause,\nsecond line\n");
}

#[test]
fn preserve_enter_makes_a_soft_break_not_a_paragraph() {
    // Mid-paragraph: Enter splits the line with a single `\n`, a soft break
    // that keeps it one paragraph — where Fold would open a second paragraph.
    let mut d = wysiwyg_doc("pre_enter_mid", "abcdef\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = 3;
    d.newline();
    assert_eq!(d.source, "abc\ndef\n", "mid-line Enter is a soft break");

    // End-of-paragraph: Enter then typing continues the same paragraph on a
    // new line (a soft break), not a fresh paragraph.
    let mut d = wysiwyg_doc("pre_enter_end", "abc\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = 3;
    d.newline();
    d.insert("def");
    assert_eq!(
        d.source, "abc\ndef\n",
        "end-of-line Enter + typing is a soft break"
    );
}

#[test]
fn preserve_double_enter_still_makes_a_paragraph() {
    // Two Enters in a row promote to a real paragraph break: the second lands
    // on the blank line the first opened and takes the empty-line branch.
    let mut d = wysiwyg_doc("pre_enter_dbl", "abc\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = 3;
    d.newline();
    d.newline();
    d.insert("def");
    assert_eq!(
        d.source, "abc\n\ndef\n",
        "double Enter is a paragraph break"
    );
}

#[test]
fn preserve_backspace_joins_across_a_soft_break() {
    // Backspace is the symmetric undo of a Preserve Enter: over the `\n` of a
    // soft break it deletes the single newline and joins the two lines.
    let mut d = wysiwyg_doc("pre_bs", "abc\ndef\n");
    d.set_line_flow(LineFlow::Preserve);
    d.build_visual(80);
    d.caret = 4; // start of "def", just past the soft break
    d.backspace();
    assert_eq!(
        d.source, "abcdef\n",
        "Backspace joins across the soft break"
    );
    assert_eq!(d.caret, 3, "caret lands where the lines meet");
}

#[test]
fn fold_enter_still_starts_a_new_paragraph() {
    // The default flow is unchanged: a lone `\n` would render as an invisible
    // space, so Enter keeps opening the paragraph break that actually shows.
    let mut d = wysiwyg_doc("fold_enter", "abcdef\n");
    d.caret = 3;
    d.newline();
    assert_eq!(
        d.source, "abc\n\ndef\n",
        "Fold mid-line Enter is a paragraph break"
    );
}

#[test]
fn wysiwyg_one_enter_starts_a_new_paragraph() {
    // Regression: one Enter left the caret between the two newlines, so typing
    // made a soft break (one paragraph) and you needed a second Enter.
    let mut d = wysiwyg_doc("wys_enter", "abc\n");
    d.caret = 3;
    d.newline();
    d.insert("def");
    assert_eq!(d.source, "abc\n\ndef\n"); // two paragraphs, not "abc\ndef\n"
}

#[test]
fn enter_at_the_end_of_a_bold_run_keeps_its_closing_delimiter_attached() {
    // Regression: Enter at the caret's natural End-of-line resting place
    // after a bold run with nothing following it (on screen: right after
    // "bold", before the hidden closing "**") spliced the paragraph break
    // at that very byte offset — which sits *before* the closing "**" in
    // the source, since the delimiter is hidden and emits no glyph of its
    // own for `push_row`'s "end of row" fallback to count. That severed the
    // mark: "**bold**\n" became "**bold\n\n**\n", stranding the closing
    // "**" alone on the new line instead of leaving "**bold**" intact with
    // a fresh empty paragraph after it.
    let mut d = wysiwyg_doc("bold_eol_enter", "**bold**\n");
    d.move_end(false); // the WYSIWYG End key, from caret 0
    assert_eq!(
        d.caret, 6,
        "caret rests right after \"bold\", before the hidden \"**\""
    );
    d.newline();
    assert!(
        d.source.starts_with("**bold**"),
        "the closing ** must stay attached to \"bold\": got {:?}",
        d.source
    );
    assert_eq!(
        d.source, "**bold**\n\n\n",
        "a fresh empty paragraph follows the still-intact bold run"
    );
}

#[test]
fn source_view_enter_is_a_single_newline() {
    let mut d = doc_with("src_enter", "abc\n");
    d.caret = 3;
    d.newline();
    assert_eq!(d.source, "abc\n\n");
}

#[test]
fn heading_applies_at_the_end_of_a_paragraph() {
    // The caret at a line end sits at the doc level; set_block must still find
    // the block on that line.
    let mut d = doc_with("head_end", "abc\n");
    d.caret = 3; // end of "abc"
    d.toggle_heading(1);
    assert_eq!(d.source, "# abc\n");
}

#[test]
fn heading_on_an_empty_new_paragraph_creates_one() {
    let mut d = wysiwyg_doc("head_empty", "abc\n");
    d.caret = 3;
    d.newline(); // caret now on a fresh, empty paragraph
    d.toggle_heading(1);
    d.insert("Title");
    assert!(d.source.contains("# Title"), "got {:?}", d.source);
}

#[test]
fn a_heading_typed_on_a_blank_line_keeps_the_caret_on_its_own_row() {
    // The reported bug, end to end: click a blank line with another one under
    // it, press H1, type. The text landed in the heading and the caret's
    // offset was right (the source view drew it there), but the rich view
    // drew it two rows lower, on the trailing blank line — the empty `# `
    // heading had left every row below it short by the marker's two bytes,
    // and the blank line ended up claiming the heading's own end offset.
    let mut d = wysiwyg_doc("head_blank", "one\n\ntwo\n\n\n\n");
    d.build_visual_unwrapped();
    d.caret = d.vmap.offset_of_pos(4, 0); // the first of the two blank lines
    d.toggle_heading(1);
    for c in "title".chars() {
        d.insert(&c.to_string());
        d.build_visual_unwrapped(); // as a frontend does, one frame per key
    }
    assert_eq!(d.source, "one\n\ntwo\n\n# title\n\n");
    assert_eq!(
        d.caret_pos(),
        (4, 5),
        "the caret draws at the end of the heading"
    );
}

#[test]
fn clicking_an_empty_heading_types_after_its_marker() {
    // The same anchor from the other side: the empty heading's row is its own
    // caret home, so a click on it must land past the hidden `# `. Landing in
    // front of the hashes made the first keystroke un-heading the line.
    let mut d = wysiwyg_doc("head_click", "# \n");
    d.build_visual_unwrapped();
    d.caret = d.vmap.offset_of_pos(0, 0);
    d.insert("x");
    assert_eq!(d.source, "# x\n");
}

#[test]
fn wysiwyg_enter_after_a_heading_makes_a_paragraph() {
    let mut d = wysiwyg_doc("head_enter", "# Title\n");
    d.caret = 7; // end of the heading
    d.newline();
    d.insert("body");
    assert_eq!(d.source, "# Title\n\nbody\n");
}

#[test]
fn wysiwyg_enter_continues_a_bullet_list() {
    let mut d = wysiwyg_doc("wys_bullet", "- item\n");
    d.caret = 6; // end of "item"
    d.newline();
    d.insert("two");
    assert_eq!(d.source, "- item\n- two\n");
}

#[test]
fn wysiwyg_enter_increments_an_ordered_list() {
    let mut d = wysiwyg_doc("wys_ol", "1. one\n");
    d.caret = 6; // end of "one"
    d.newline();
    d.insert("two");
    assert_eq!(d.source, "1. one\n2. two\n");
}

#[test]
fn wysiwyg_backspace_after_leaving_a_list_collapses_the_gap_cleanly() {
    // Regression for the "extra newline" left between a list and the paragraph
    // below it. Enter, Enter leaves the list on a fresh empty paragraph
    // (`- item\n\n\n\nnext`, a navigable blank between the two blocks); one
    // Backspace should then take the caret cleanly back to the end of the list
    // item, `- item\n\nnext`, not delete a single newline and strand it on the
    // odd `- item\n\n\nnext` — a blank line the eye reads as one separator but
    // no caret can land on. The map is rebuilt between keystrokes exactly as a
    // frontend does, since Backspace reads the stop table to place the delete.
    let mut d = wysiwyg_doc("wys_exit_bksp", "- item\n\nnext\n");
    d.caret = 6; // end of "item"
    d.newline();
    d.build_visual(80);
    d.newline(); // leave the list onto a fresh empty paragraph
    d.build_visual(80);
    assert_eq!(
        d.source, "- item\n\n\n\nnext\n",
        "double-Enter opens the empty paragraph"
    );
    d.backspace();
    assert_eq!(
        d.source, "- item\n\nnext\n",
        "one Backspace collapses the whole gap"
    );
    assert_eq!(
        d.caret, 6,
        "and lands the caret back at the end of the list item"
    );
}

#[test]
fn wysiwyg_backspace_on_stacked_blank_lines_still_removes_just_one() {
    // The stop-wise delete must not over-reach when there is no block boundary
    // to cross: two blank lines in a row are one caret stop apart, so pressing
    // Enter on an empty line and then Backspace removes exactly the one newline
    // it added — the lone-Enter / lone-Backspace symmetry, preserved.
    let mut d = wysiwyg_doc("wys_stack", "abc\n\n\n");
    d.caret = 5; // the empty paragraph the first Enter already opened
    d.build_visual(80);
    d.newline();
    d.build_visual(80);
    assert_eq!(
        d.source, "abc\n\n\n\n",
        "Enter on the blank line adds one newline"
    );
    d.backspace();
    assert_eq!(
        d.source, "abc\n\n\n",
        "Backspace takes back exactly that one newline"
    );
}

#[test]
fn wysiwyg_enter_on_an_empty_list_item_exits_the_list() {
    let mut d = wysiwyg_doc("wys_exit", "- a\n- \n");
    d.caret = 6; // end of the empty "- " item
    d.newline();
    d.insert("p");
    assert_eq!(d.source, "- a\n\np\n");
}

#[test]
fn wysiwyg_enter_does_not_mistake_a_setext_underline_for_a_list() {
    // `text\n- \n` is a setext heading — the `- ` is its underline, not a
    // list item, though it reads as a `- ` marker byte-for-byte. Enter must
    // not take the list-exit path (which would splice the `- ` away as if
    // leaving an empty item); the AST guard sends it to a normal break and
    // leaves the underline intact.
    let mut d = wysiwyg_doc("wys_setext", "text\n- \n");
    assert!(
        d.nodes().iter().any(|n| n.kind == Kind::Heading),
        "precondition: twig parses this as a heading, not a list",
    );
    d.caret = 7; // on the `- ` underline line
    d.newline();
    assert!(
        d.source.contains("- "),
        "the setext underline survives, not spliced away as a list item: {:?}",
        d.source,
    );
}

#[test]
fn wysiwyg_enter_in_a_code_block_is_a_literal_newline() {
    let mut d = wysiwyg_doc("wys_code", "```\nabc\n```\n");
    d.caret = 7; // end of "abc" inside the fence
    d.newline();
    d.insert("def");
    assert_eq!(d.source, "```\nabc\ndef\n```\n");
}

#[test]
fn wysiwyg_enter_continues_a_block_quote() {
    // Enter opens a new *paragraph* inside the quote, not a second line of
    // the same one. `> quote\n> more` is a soft break, which under
    // `LineFlow::Fold` renders as a space — the keystroke would look like it
    // did nothing. The quoted blank line is what makes the break visible, and
    // it's the same thing Enter does in running prose.
    let mut d = wysiwyg_doc("wys_quote", "> quote\n");
    d.caret = 7; // end of "quote"
    d.newline();
    d.insert("more");
    assert_eq!(d.source, "> quote\n>\n> more\n");
    // Still one quote, now holding two paragraphs — not a quote and a stray
    // line that fell out of it.
    let quotes = d
        .nodes()
        .iter()
        .filter(|n| n.kind == Kind::BlockQuote)
        .count();
    assert_eq!(quotes, 1);
}

#[test]
fn set_block_makes_a_heading_at_the_caret() {
    let mut d = doc_with("head", "Title\n\nbody\n");
    d.caret = 0;
    d.set_block(BlockKind::Heading(2));
    assert_eq!(d.source, "## Title\n\nbody\n");
    d.set_block(BlockKind::Paragraph);
    assert_eq!(d.source, "Title\n\nbody\n");
}
