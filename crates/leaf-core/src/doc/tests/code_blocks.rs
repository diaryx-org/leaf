use super::*;

// ── toggle_code_block ────────────────────────────────────────────────────

#[test]
fn toggle_code_block_fences_the_paragraph_at_the_caret_and_reverses() {
    let g = |n, m| golden_in(View::Wysiwyg, n, m, |d| d.toggle_code_block());
    assert_eq!(g("cb_on", "hel|lo\n"), "```\nhel|lo\n```\n");
    assert_eq!(g("cb_off", "```\nhel|lo\n```\n"), "hel|lo\n");
    assert_eq!(g("cb_end", "hello|\n"), "```\nhello|\n```\n");
    assert_eq!(
        g("cb_wrap", "aaa\nb|bb\nccc\n"),
        "```\naaa\nb|bb\nccc\n```\n"
    );
    assert_eq!(
        g("cb_unwrap", "```\naaa\nb|bb\nccc\n```\n"),
        "aaa\nb|bb\nccc\n"
    );
    // An indented block dedents, and the lines stay one-to-one.
    assert_eq!(g("cb_indent", "    co|de\n"), "co|de\n");
    // A quote's marker is kept on every line, the fence's included.
    assert_eq!(g("cb_quote", "> hel|lo\n"), "> ```\n> hel|lo\n> ```\n");
}

#[test]
fn toggle_code_block_on_a_blank_line_opens_an_empty_fence() {
    let g = |n, m| golden_in(View::Wysiwyg, n, m, |d| d.toggle_code_block());
    assert_eq!(g("cb_blank", "A\n\n|"), "A\n\n```\n|\n```");
    assert_eq!(
        g("cb_blank_mid", "A\n\n|\n\nB\n"),
        "A\n\n```\n|\n```\n\nB\n"
    );
    // Tight against a neighbour, a blank line goes in on that side.
    assert_eq!(g("cb_blank_tight", "A\n|\nB\n"), "A\n\n```\n|\n```\n\nB\n");
    // Inside a quote, on a quoted blank line.
    assert_eq!(
        g("cb_blank_quote", "> A\n>\n> |\n"),
        "> A\n>\n> ```\n> |\n> ```\n"
    );
}

#[test]
fn toggle_code_block_from_a_blank_line_is_a_block_the_caret_can_type_into() {
    let mut d = wysiwyg_doc("cb_type", "A\n\n");
    d.click_past_end();
    d.toggle_code_block();
    assert!(
        d.caret_in_code_block(),
        "the caret is not in the block it opened"
    );
    d.insert("let x = 1;");
    d.newline();
    d.insert("x");
    assert_eq!(d.source, "A\n\n```\nlet x = 1;\nx\n```");
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(d.vmap.rows[row].code, "typed text is not on a code row");
    // And back out: the button reverses what it did, text kept.
    d.toggle_code_block();
    assert_eq!(d.source, "A\n\nlet x = 1;\nx\n");
    assert!(!d.caret_in_code_block());
}

#[test]
fn toggle_code_block_over_a_selection_fences_it_whole_and_keeps_it_selected() {
    let mut d = wysiwyg_doc("cb_sel", "one\n\ntwo\n\nthree\n");
    d.anchor = Some(1);
    d.caret = 7;
    d.toggle_code_block();
    assert_eq!(d.source, "```\none\n\ntwo\n```\n\nthree\n");
    assert!(d.selection().is_some(), "the block came back unselected");
    d.toggle_code_block();
    assert_eq!(
        d.source, "one\n\ntwo\n\nthree\n",
        "a second press did not reverse the first"
    );
}

#[test]
fn toggle_code_block_inside_a_list_item_fences_and_unfences_in_place() {
    for (name, before, fenced) in [
        ("cb_list", "- it|em\n", "- ```\n  it|em\n  ```\n"),
        ("cb_olist", "1. it|em\n", "1. ```\n   it|em\n   ```\n"),
        ("cb_task", "- [ ] it|em\n", "- [ ] ```\n  it|em\n  ```\n"),
        (
            "cb_nested",
            "- a\n  - it|em\n",
            "- a\n  - ```\n    it|em\n    ```\n",
        ),
    ] {
        let mut d = wysiwyg_doc(name, &before.replace('|', ""));
        d.caret = before.find('|').unwrap();
        d.toggle_code_block();
        assert_eq!(render_caret(&d), fenced, "{name}: fencing");
        assert!(
            d.caret_in_code_block(),
            "{name}: the caret is in the new block"
        );
        d.toggle_code_block();
        assert_eq!(
            render_caret(&d),
            before,
            "{name}: a second press reverses the first"
        );
    }
}

#[test]
fn toggle_code_block_in_a_list_item_is_one_undo_step() {
    let mut d = wysiwyg_doc("cb_list_undo", "- item\n");
    d.caret = 4;
    d.toggle_code_block();
    d.undo();
    assert_eq!(render_caret(&d), "- it|em\n");
}

#[test]
fn caret_in_code_block_reads_fenced_and_indented_blocks() {
    let mut d = wysiwyg_doc("cb_in", "para\n\n```\ncode\n```\n\n    more\n");
    d.caret = 2;
    assert!(!d.caret_in_code_block());
    d.caret = 11;
    assert!(d.caret_in_code_block());
    d.caret = 25;
    assert!(d.caret_in_code_block(), "an indented block is a code block");
}

#[test]
fn caret_in_blockquote_reads_every_line_of_a_quote_and_nothing_after() {
    let src = "para\n\n> # Head\n> body|\n\nafter\n";
    let mut d = wysiwyg_doc("bq_in", &src.replace('|', ""));
    let quote_start = src.find('>').unwrap();
    let body_end = src.find('|').unwrap();
    let after = src.find("after").unwrap() - 1;
    for (at, inside) in [
        (2, false),
        (quote_start + 4, true),
        (body_end - 2, true),
        (body_end, true),
        (after, false),
        (after + 2, false),
    ] {
        d.caret = at;
        assert_eq!(d.caret_in_blockquote(), inside, "caret at {at}");
    }
    // A heading in a quote is both: the quote is a layer over the style.
    d.caret = quote_start + 4;
    assert_eq!(d.current_heading_level(), Some(1));
}

#[test]
fn caret_in_blockquote_follows_the_toggle() {
    let mut d = wysiwyg_doc("bq_toggle", "hello\n");
    d.caret = 2;
    assert!(!d.caret_in_blockquote());
    d.toggle_blockquote();
    assert!(d.caret_in_blockquote());
    d.toggle_blockquote();
    assert!(!d.caret_in_blockquote());
}

#[test]
fn toggle_code_block_is_one_undo_step() {
    let mut d = wysiwyg_doc("cb_undo", "hello\n");
    d.caret = 2;
    d.toggle_code_block();
    d.undo();
    assert_eq!(render_caret(&d), "he|llo\n");
}

#[test]
fn triple_click_selects_a_paragraph_across_its_soft_breaks() {
    // A paragraph broken over two source lines is one paragraph. Selecting
    // it must not stop at the newline inside it — that newline is markup the
    // rich-text view exists to hide.
    let src = "one two\nthree four\n\nnext\n";
    let mut d = wysiwyg_doc("triple_para", src);
    d.select_block_at(2);
    assert_eq!(
        d.selected_text(),
        Some("one two\nthree four"),
        "stopped at the soft break"
    );
}

#[test]
fn the_wheel_can_scroll_away_from_a_caret_that_stays_put() {
    // The reader scrolls down past the caret's row. Nothing moved the
    // caret, so the view must stay where it was put — the old code revealed
    // the caret every frame, which dragged the view straight back and made
    // the document unscrollable past the caret.
    let mut d = wysiwyg_doc("scroll_free", "a\n\nb\n\nc\n\nd\n\ne\n");
    d.caret = 0;
    d.follow_caret(0, 3, 9); // first frame: the caret is at the top
    d.scroll = 4; // the wheel
    d.follow_caret(0, 3, 9);
    assert_eq!(
        d.scroll, 4,
        "the wheel was overruled by a caret that never moved"
    );
}

#[test]
fn moving_the_caret_brings_the_view_back_to_it() {
    let mut d = wysiwyg_doc("scroll_follow", "a\n\nb\n\nc\n\nd\n\ne\n");
    d.caret = 0;
    d.follow_caret(0, 3, 9);
    d.scroll = 6; // scrolled away
    d.move_right(false); // ...and now the caret moves
    let (row, _) = d.caret_pos();
    d.follow_caret(row, 3, 9);
    assert!(
        d.scroll <= row && row < d.scroll + 3,
        "caret row {row} off screen at scroll {}",
        d.scroll
    );
}

#[test]
fn scrolling_stops_at_the_last_row() {
    let mut d = wysiwyg_doc("scroll_clamp", "a\n\nb\n");
    d.caret = 0;
    d.follow_caret(0, 3, 3); // a first frame, so the caret isn't "new"
    d.scroll = 999; // the wheel, spun hard
    d.follow_caret(0, 3, 3);
    assert_eq!(d.scroll, 2, "scrolled into the void past the document");
}

#[test]
fn every_cell_of_a_wide_table_is_reachable() {
    // A table whose cells are far wider than the surface: the columns are
    // cut to fit and the text wraps inside them, so no cell hangs off the
    // right edge where the caret can never go.
    let src = "| Ingredient | Notes |\n|---|---|\n\
                   | flour milled coarse | sift it twice before folding it in |\n";
    let mut d = wysiwyg_doc("wide_table_walk", src);
    d.build_visual(30);
    d.caret = 0;
    let seen = walk_right(&mut d);
    for word in ["Ingredient", "Notes", "coarse", "folding"] {
        let at = src.find(word).unwrap();
        assert!(seen.contains(&at), "{word:?} at {at} unreachable: {seen:?}");
    }
}
