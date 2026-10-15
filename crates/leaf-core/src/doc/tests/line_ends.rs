use super::*;

// ── Home / End ───────────────────────────────────────────────────────────

#[test]
fn home_toggles_between_the_line_s_text_and_its_margin() {
    // Source: the indentation is what the toggle is for. WYSIWYG resolves an
    // indent to the markup it spells everywhere it means one, so the fixture
    // with whitespace left to walk is a code block, which is verbatim.
    let g = |m, f: fn(&mut Doc)| golden("smart_home", m, f);
    assert_eq!(g("    inden|ted", |d| d.move_home(false)), "    |indented");
    assert_eq!(g("    |indented", |d| d.move_home(false)), "|    indented");
    assert_eq!(g("|    indented", |d| d.move_home(false)), "    |indented");
    // A line with no indentation has one place to go, so the toggle is a
    // no-op rather than a trip to nowhere.
    assert_eq!(g("hel|lo", |d| d.move_home(false)), "|hello");
    assert_eq!(g("|hello", |d| d.move_home(false)), "|hello");

    let mut d = wysiwyg_doc("smart_home_wys", "```\n    indented\n```\n");
    let indent = d.source.find("    indented").unwrap();
    d.caret = indent + 6; // inside "indented"
    d.move_home(false);
    assert_eq!(
        d.caret,
        indent + 4,
        "wysiwyg: Home aims at the code line's text"
    );
    d.move_home(false);
    assert_eq!(
        d.caret, indent,
        "wysiwyg: the second press takes the indent"
    );
    d.move_home(false);
    assert_eq!(d.caret, indent + 4, "wysiwyg: the toggle swaps back");
}

#[test]
fn end_takes_the_line_the_view_is_showing() {
    // The line differs by view for the same document, and that is the point:
    // a bare newline inside a paragraph is a soft break, which WYSIWYG draws
    // as a space on one row and the source view as two lines.
    let mut d = doc_with("end_src", "one two\nthree\n");
    d.caret = 1;
    d.move_end(false);
    assert_eq!(d.caret, 7, "source: the end of the source line");

    let mut d = wysiwyg_doc("end_wys", "one two\nthree\n");
    d.caret = 1;
    d.move_end(false);
    assert_eq!(
        d.caret, 13,
        "wysiwyg: the end of the row, soft break and all"
    );
}

#[test]
fn home_and_end_extend_the_selection_when_asked() {
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("home_end_ext_{tag}"), "hello world");
        d.caret = 6;
        d.move_end(true);
        assert_eq!(d.selection(), Some((6, 11)), "{tag}: End extends");
        let mut d = doc_in(view, &format!("home_ext_{tag}"), "hello world");
        d.caret = 6;
        d.move_home(true);
        assert_eq!(d.selection(), Some((0, 6)), "{tag}: Home extends");
    }
}

// ── kill to the line's start / end ───────────────────────────────────────

#[test]
fn kill_to_the_line_start_and_end_in_both_views() {
    for (view, tag) in VIEWS {
        // The gap that reads as a paragraph break in each view: the source
        // view's lines are the renderer's rows only where the source says so.
        let gap = if view == View::Source { "\n" } else { "\n\n" };
        let mut d = doc_in(
            view,
            &format!("kill_end_{tag}"),
            &format!("one two{gap}three\n"),
        );
        d.caret = 3;
        d.delete_to_line_end();
        assert_eq!(
            d.source,
            format!("one{gap}three\n"),
            "{tag}: ^K to the line's end"
        );
        assert_eq!(d.caret, 3, "{tag}: the caret stays where it kills from");

        let mut d = doc_in(
            view,
            &format!("kill_start_{tag}"),
            &format!("one two{gap}three\n"),
        );
        d.caret = 7; // the end of the first line
        d.delete_to_line_start();
        assert_eq!(
            d.source,
            format!("{gap}three\n"),
            "{tag}: ⌘⌫ to the line's start"
        );
        assert_eq!(d.caret, 0, "{tag}");
    }
}

#[test]
fn a_kill_at_the_line_s_edge_leaves_the_lines_joined() {
    // The decision: at the boundary both kills do nothing, rather than
    // eating the line break. "Line" is the view's own — in WYSIWYG it ends
    // at a soft wrap as often as at a newline, where there is nothing
    // written to delete — and a source newline is only half of the blank
    // line between two paragraphs, so taking it leaves a soft break rather
    // than the join it looks like. Backspace and Delete are the keys for it.
    for (view, tag) in VIEWS {
        let gap = if view == View::Source { "\n" } else { "\n\n" };
        let src = format!("one{gap}three\n");
        let mut d = doc_in(view, &format!("kill_edge_end_{tag}"), &src);
        d.caret = 3; // the end of "one"
        d.delete_to_line_end();
        assert_eq!(
            d.source, src,
            "{tag}: ^K at the line's end joined it to the next"
        );

        let mut d = doc_in(view, &format!("kill_edge_start_{tag}"), &src);
        d.caret = 3 + gap.len(); // the start of "three"
        d.delete_to_line_start();
        assert_eq!(
            d.source, src,
            "{tag}: ⌘⌫ at the line's start joined it to the last"
        );
    }
}

#[test]
fn a_kill_takes_the_selection_when_there_is_one() {
    // What every other delete here does with one, so these two as well.
    for (view, tag) in VIEWS {
        for (name, kill) in [
            (
                "end",
                (|d: &mut Doc| d.delete_to_line_end()) as fn(&mut Doc),
            ),
            ("start", |d: &mut Doc| d.delete_to_line_start()),
        ] {
            let mut d = doc_in(view, &format!("kill_sel_{name}_{tag}"), "one two three\n");
            d.anchor = Some(4);
            d.caret = 7; // "two"
            kill(&mut d);
            assert_eq!(
                d.source, "one  three\n",
                "{tag}: {name} ignored the selection"
            );
            assert_eq!(d.selection(), None, "{tag}: {name}");
        }
    }
}

#[test]
fn a_kill_takes_the_markup_it_empties_with_it() {
    // The same hazard a word-delete has: a WYSIWYG range covers what the
    // user can see, which for `**bold**` is the word and never the
    // delimiters, so a kill that stopped at the text would leave `a ****` —
    // markup wrapped around nothing.
    let mut d = wysiwyg_doc("kill_widen", "a **bold**\n");
    d.caret = d.source.find("bold").unwrap();
    d.delete_to_line_end();
    assert_eq!(d.source, "a \n");
}

#[test]
fn a_kill_is_undone_in_one_step() {
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("kill_undo_{tag}"), "one two three\n");
        d.caret = 3;
        d.delete_to_line_end();
        assert_eq!(d.source, "one\n", "{tag}");
        d.undo();
        assert_eq!(d.source, "one two three\n", "{tag}: a kill takes one undo");
    }
}

#[test]
fn select_block_grabs_the_whole_paragraph_from_any_wrapped_row() {
    // Regression: triple-click used move_home/move_end over visual rows, so
    // it only worked on a paragraph's first row (a wrap-boundary offset maps
    // to the earlier row). select_block_at reads the AST, so every offset in
    // the paragraph selects the whole thing.
    let body = "one two three four five six seven eight\n";
    let mut d = doc_with("sel_block", body);
    d.view = View::Wysiwyg;
    d.build_visual(12); // force the paragraph to wrap into several rows
    assert!(d.vmap.num_rows() > 1, "test needs a wrapped paragraph");
    let para = (0, "one two three four five six seven eight".len());
    for off in [0usize, 8, 19, 28, 38] {
        d.caret = 0;
        d.anchor = None;
        d.select_block_at(off);
        assert_eq!(
            d.selection(),
            Some(para),
            "offset {off} should select the paragraph"
        );
    }
}

#[test]
fn select_block_uses_content_span_for_a_heading() {
    let mut d = doc_with("sel_head", "# Title\n\nbody\n");
    d.select_block_at(4); // inside "Title"
    // content_span excludes the "# " marker.
    assert_eq!(d.selected_text(), Some("Title"));
    d.select_block_at(10); // inside "body"
    assert_eq!(d.selected_text(), Some("body"));
}

#[test]
fn select_all_spans_the_document() {
    let mut d = doc_with("sel_all", "abc\n\ndef\n");
    d.select_all();
    assert_eq!(d.selection(), Some((0, d.source.len())));
}

#[test]
fn select_word_at_picks_the_surrounding_word() {
    let mut d = doc_with("sel_word", "hello world\n");
    d.select_word_at(8); // inside "world"
    assert_eq!(d.selection(), Some((6, 11)));
    // Double-clicking at end-of-word still grabs the word to its left.
    d.select_word_at(5); // the space between the words
    assert_eq!(d.selection(), Some((5, 6)));
}

#[test]
fn word_at_names_the_word_without_selecting_it() {
    let d = doc_with("word_at", "hello café, world\n");
    assert_eq!(d.word_at(1), Some(0..5));
    // The boundary after a word still means it: a point on the trailing
    // half of its last letter hit-tests there.
    assert_eq!(d.word_at(5), Some(0..5));
    // Multi-byte letters are word characters, and the range ends on a
    // character boundary; an offset inside `é` is the word it is in.
    assert_eq!(d.word_at(9), Some(6..11));
    assert_eq!(d.word_at(10), Some(6..11));
    // Between two non-word characters there is nothing to look up.
    assert_eq!(d.word_at(12), None);
    // Asking moves nothing.
    assert_eq!(d.selection(), None);
    assert_eq!(d.caret, 0);
}

#[test]
fn word_at_leaves_hidden_delimiters_out_of_the_word() {
    let body = "a _important_ un**believ**able x\n";
    let d = wysiwyg_doc("word_at_hidden", body);
    let at = |needle: &str| body.find(needle).unwrap();
    let text = |r: Range<usize>| body[r].replace(['*', '_'], "");
    // Inside the emphasis, on its hidden opening `_`, and on the boundary
    // after its hidden closing one: the word is the drawn `important`.
    let important = at("important")..at("important") + "important".len();
    assert_eq!(d.word_at(at("port")), Some(important.clone()));
    assert_eq!(d.word_at(at("_important")), Some(important.clone()));
    assert_eq!(d.word_at(at("_ un")), Some(important.clone()));
    // A word the markup splits is still one word on screen.
    let w = d.word_at(at("believ")).unwrap();
    assert_eq!((w.start, text(w)), (at("un**"), "unbelievable".to_string()));
    // The source view reads the source, where `_` is a word character.
    let s = doc_with("word_at_hidden_src", body);
    assert_eq!(s.word_at(at("port")), Some(at("_important")..at(" un")));
}

#[test]
fn word_helpers_respect_utf8_boundaries() {
    // "café" is 5 bytes ('é' is two); motion must land on char boundaries.
    assert_eq!(
        golden("utf8", "|café ok", |d| d.move_word_right(false)),
        "café| ok"
    );
    assert_eq!(golden("utf8b", "café |ok", |d| d.delete_word_back()), "|ok");
}

#[test]
fn typing_inserts_at_the_caret_and_advances_it() {
    let mut d = doc_with("type", "hello\n");
    d.insert("Hi ");
    assert_eq!(d.source, "Hi hello\n");
    assert_eq!(d.caret, 3);
    assert!(d.dirty);
}

#[test]
fn backspace_deletes_the_char_before_the_caret() {
    let mut d = doc_with("bs", "hello\n");
    d.caret = 3; // after "hel"
    d.backspace();
    assert_eq!(d.source, "helo\n");
    assert_eq!(d.caret, 2);
}

#[test]
fn typing_replaces_the_selection() {
    let mut d = doc_with("replace", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6; // "word" selected
    d.insert("X");
    assert_eq!(d.source, "a X b\n");
    assert_eq!(d.caret, 3);
    assert_eq!(d.anchor, None);
}

#[test]
fn toggle_bold_wraps_then_unwraps_the_selection() {
    let mut d = doc_with("bold", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a **word** b\n");
    // The toggled region stays selected, so a second toggle reverses it.
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a word b\n");
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a **word** b\n");
}

#[test]
fn toggle_code_wraps_then_unwraps_the_selection() {
    let mut d = doc_with("code_rt", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Verbatim);
    assert_eq!(d.source, "a `word` b\n");
    d.toggle(InlineKind::Verbatim);
    assert_eq!(d.source, "a word b\n");
}

#[test]
fn sticky_bold_with_no_selection_wraps_the_next_typed_text() {
    // ⌘b at a bare caret, then type: the text comes out bold with no
    // selection ever made — the word-processor "start bold here" gesture.
    let mut d = doc_with("sticky_wrap", "xy\n");
    d.caret = 1; // between x and y
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "xy\n", "arming a mark must not edit the document");
    d.insert("A");
    assert_eq!(d.source, "x**A**y\n");
}

#[test]
fn sticky_bold_lights_the_toolbar_before_any_typing() {
    // The button must light the instant ⌘b is pressed, or the mode is
    // invisible until the first character lands.
    let mut d = doc_with("sticky_light", "xy\n");
    d.caret = 1;
    assert!(!d.active_inline_marks().contains(InlineKind::Strong));
    d.toggle(InlineKind::Strong);
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
}

#[test]
fn sticky_bold_toggled_off_types_normally_again() {
    // ⌘b, type, ⌘b, type: the first run is bold, the second is not — all
    // in the flow of typing, the exact sequence the user described.
    let mut d = doc_with("sticky_off", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    d.insert("a");
    d.insert("b"); // continues inside the run, no re-arming
    assert_eq!(d.source, "**ab**\n");
    d.toggle(InlineKind::Strong); // ⌘b again — shed bold
    d.insert("c");
    assert_eq!(d.source, "**ab**c\n");
}

#[test]
fn continued_typing_after_a_sticky_run_stays_in_the_run() {
    // Once a mark is realised the caret sits inside the run, so plain typing
    // extends it rather than starting a second, adjacent bold span.
    let mut d = doc_with("sticky_cont", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Emph);
    d.insert("h");
    d.insert("i");
    assert_eq!(d.source, "*hi*\n");
}

#[test]
fn moving_the_caret_disarms_a_sticky_mark() {
    // Arming a mark and then moving away must not style text elsewhere.
    let mut d = doc_with("sticky_disarm", "xy\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    d.move_right(false); // caret 0 → 1, disarms
    assert!(!d.active_inline_marks().contains(InlineKind::Strong));
    d.insert("A");
    assert_eq!(d.source, "xAy\n", "the mark must not follow the caret");
}

#[test]
fn stacked_sticky_marks_apply_together() {
    // ⌘b then ⌘i before typing: the text comes out both bold and italic.
    let mut d = doc_with("sticky_stack", "\n");
    d.caret = 0;
    d.toggle(InlineKind::Strong);
    d.toggle(InlineKind::Emph);
    d.insert("x");
    // Land the caret on the styled character and confirm both marks are live.
    d.anchor = Some(d.source.find('x').unwrap());
    d.caret = d.anchor.unwrap() + 1;
    let marks = d.active_inline_marks();
    assert!(marks.contains(InlineKind::Strong), "bold: {}", d.source);
    assert!(marks.contains(InlineKind::Emph), "italic: {}", d.source);
}
