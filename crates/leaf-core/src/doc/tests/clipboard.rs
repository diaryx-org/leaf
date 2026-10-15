use super::*;

// ── the clipboard's rich flavor ──────────────────────────────────────────

#[test]
fn an_inline_selection_publishes_html_without_a_paragraph_wrapper() {
    let mut d = doc_with("sel_inline", "a **bold** c\n");
    d.anchor = Some(2);
    d.caret = 10; // `**bold**`, inside the paragraph
    assert_eq!(d.selection_html().as_deref(), Some("<strong>bold</strong>"));
}

#[test]
fn a_whole_block_selection_keeps_its_paragraph() {
    let mut d = doc_with("sel_block", "a **bold** c\n");
    d.anchor = Some(0);
    d.caret = 12; // the entire paragraph
    assert_eq!(
        d.selection_html().as_deref(),
        Some("<p>a <strong>bold</strong> c</p>")
    );
}

#[test]
fn a_multi_block_selection_keeps_its_structure() {
    let mut d = doc_with("sel_multi", "para\n\n- one\n- two\n");
    d.select_all();
    let html = d.selection_html().expect("renders");
    assert!(html.contains("<p>para</p>"), "{html:?}");
    assert!(html.contains("<li>one</li>"), "{html:?}");
}

#[test]
fn a_word_inside_a_heading_publishes_as_text_not_a_heading() {
    // The fragment `Head` is a paragraph standalone; the *document* says it
    // sits inside one block, so the wrapper is an artifact either way.
    let mut d = doc_with("sel_heading", "# Head line\n");
    d.anchor = Some(2);
    d.caret = 6;
    assert_eq!(d.selection_html().as_deref(), Some("Head"));
}

#[test]
fn no_selection_publishes_no_html() {
    let mut d = doc_with("sel_none", "a b\n");
    d.caret = 1;
    assert_eq!(d.selection_html(), None);
}

#[test]
fn pasting_html_converts_it_and_is_one_undo_step() {
    let mut d = doc_with("paste_html", "x\n");
    d.caret = 1;
    assert!(d.paste_html("<p>a <strong>b</strong> c</p>"));
    assert_eq!(d.source, "xa **b** c\n");
    d.undo();
    assert_eq!(d.source, "x\n", "the whole paste, in one step");
}

#[test]
fn pasting_html_replaces_the_selection() {
    let mut d = doc_with("paste_html_sel", "keep drop\n");
    d.anchor = Some(5);
    d.caret = 9;
    assert!(d.paste_html("<em>new</em>"));
    assert_eq!(d.source, "keep *new*\n");
}

#[test]
fn html_that_would_paste_garbage_declines_so_the_caller_falls_back() {
    let mut d = doc_with("paste_html_bad", "x\n");
    d.caret = 1;
    // twig builds no table from HTML; raw `<table>` in prose is worse than
    // the plain flavor the caller still holds.
    assert!(!d.paste_html("<table><tr><td>a</td></tr></table>"));
    assert_eq!(d.source, "x\n", "declined edits nothing");
}

#[test]
fn copy_then_paste_round_trips_through_the_html_flavor() {
    let mut d = doc_with("clip_round", "a **b** and [l](https://x.dev)\n");
    d.select_all();
    let html = d.selection_html().expect("renders");
    let mut into = doc_with("clip_round_dst", "\n");
    into.caret = 0;
    assert!(into.paste_html(&html));
    assert_eq!(into.source, "a **b** and [l](https://x.dev)\n");
}

#[test]
fn moving_the_caret_starts_a_new_undo_group() {
    let mut d = doc_with("break", "\n");
    d.caret = 0;
    d.insert("a");
    d.insert("b"); // "ab\n", caret at 2
    d.move_left(false); // breaks the run
    d.insert("X"); // "aXb\n"
    assert_eq!(d.source, "aXb\n");
    d.undo();
    assert_eq!(
        d.source, "ab\n",
        "first undo removes only the post-move insert"
    );
    d.undo();
    assert_eq!(d.source, "\n", "second undo removes the earlier run");
}

#[test]
fn undo_reverses_a_format_toggle() {
    let mut d = doc_with("fmt_undo", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a **word** b\n");
    d.undo();
    assert_eq!(d.source, "a word b\n");
}

#[test]
fn undo_back_to_the_saved_state_clears_dirty() {
    let mut d = doc_with("dirty_undo", "hello\n");
    assert!(!d.dirty);
    d.caret = 5;
    d.insert("!");
    assert!(d.dirty);
    d.undo();
    assert!(
        !d.dirty,
        "undoing to the saved source is not a modification"
    );
}

#[test]
fn marking_saved_as_what_was_written_keeps_later_typing_dirty() {
    let mut d = doc_with("saved_as", "Does this\n");
    d.caret = 9;
    d.insert(" ");
    // The host reads the source, and its write takes a while…
    let written = d.source.clone();
    // …during which the rest of the sentence is typed.
    d.insert("work?");
    d.mark_saved_as(&written);
    assert!(d.dirty, "what was typed during the write is not saved");
    d.undo();
    assert!(!d.dirty, "undoing to what was written is the saved state");

    d.redo();
    let written = d.source.clone();
    d.mark_saved_as(&written);
    assert!(!d.dirty, "nothing typed meanwhile: saved");
}

#[test]
fn a_new_edit_invalidates_redo() {
    let mut d = doc_with("redo_inv", "\n");
    d.caret = 0;
    d.insert("a");
    d.undo();
    d.insert("b"); // diverges — the redo of "a" is now gone
    d.redo();
    assert_eq!(d.source, "b\n");
}

#[test]
fn can_undo_and_can_redo_follow_the_history_a_menu_would_enable_by() {
    let mut d = doc_with("can_undo", "hello\n");
    assert!(
        !d.can_undo() && !d.can_redo(),
        "a fresh document has no history"
    );
    d.caret = 5;
    d.insert("!");
    assert!(
        d.can_undo() && !d.can_redo(),
        "an edit is a step to take back"
    );
    d.undo();
    assert!(!d.can_undo() && d.can_redo(), "undone: only redo remains");
    d.redo();
    assert!(d.can_undo() && !d.can_redo(), "redone: back to undoable");
    d.undo();
    d.insert("?");
    assert!(
        d.can_undo() && !d.can_redo(),
        "a fresh edit ends the redo chain"
    );
    // A coalesced run over-counts steps — the bound is what a menu needs,
    // and it reconciles the moment twig reports the history empty.
    d.insert("a");
    d.insert("b");
    while d.can_undo() {
        d.undo();
    }
    assert_eq!(d.source, "hello\n");
    assert!(!d.can_undo());
    // A reading surface has nothing to undo, whatever the history holds.
    d.redo();
    d.set_read_only(true);
    assert!(!d.can_undo() && !d.can_redo());
}

#[test]
fn undo_on_empty_history_is_a_no_op() {
    let mut d = doc_with("undo_empty", "hi\n");
    d.undo();
    assert_eq!(d.source, "hi\n");
    assert_eq!(d.status.as_deref(), Some("nothing to undo"));
}

#[test]
fn a_one_character_paste_is_its_own_undo_step() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "paste_step", "ab\n");
        d.caret = 0;
        d.insert("x");
        d.insert("y"); // a run of typing
        d.paste("z"); // one character, but pasted — not part of that run
        assert_eq!(d.source, "xyzab\n");
        d.undo();
        assert_eq!(d.source, "xyab\n", "the paste undoes on its own");
        assert_eq!(d.caret, 2, "and hands back the caret it found");
        d.undo();
        assert_eq!(d.source, "ab\n", "the typed run is still one step under it");
    }
}

#[test]
fn the_same_character_typed_still_joins_the_run() {
    // The other half of the pair: `z` is a keystroke here and a paste above,
    // and the two undo differently. Nothing about the *string* says which —
    // which is why provenance has to come from the door the caller uses.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "typed_run", "ab\n");
        d.caret = 0;
        d.insert("x");
        d.insert("y");
        d.insert("z");
        d.undo();
        assert_eq!(d.source, "ab\n", "one run, one step");
    }
}

#[test]
fn undo_restores_the_caret_to_where_it_was_not_to_the_edit_site() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "undo_caret", "hello world\n");
        d.caret = 11; // standing at the end of "world", away from the edit
        d.edit(0, 5, "goodbye");
        assert_eq!(d.source, "goodbye world\n");
        d.undo();
        assert_eq!(d.source, "hello world\n");
        // The undone edit ends at offset 5; the user was at 11.
        assert_eq!(d.caret, 11, "the caret comes back with the bytes");
    }
}

#[test]
fn undo_restores_the_selection_the_edit_replaced() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "undo_sel", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6; // "word" selected
        d.insert("X");
        assert_eq!(d.source, "a X b\n");
        d.undo();
        assert_eq!(d.source, "a word b\n");
        assert_eq!(d.selection(), Some((2, 6)), "the selection comes back too");
    }
}

#[test]
fn redo_restores_the_caret_the_edit_left_behind() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "redo_caret", "hello world\n");
        d.caret = 11;
        d.edit(0, 5, "goodbye");
        assert_eq!(d.caret, 7, "the edit left the caret after its new text");
        d.undo();
        d.redo();
        assert_eq!(d.source, "goodbye world\n");
        assert_eq!(d.caret, 7, "redo puts it back where the edit had it");
    }
}

#[test]
fn undoing_a_typed_run_restores_the_caret_from_before_the_whole_run() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "run_caret", "hi\n");
        d.caret = 2;
        d.insert("a");
        d.insert("b");
        d.insert("c");
        assert_eq!(d.source, "hiabc\n");
        d.undo();
        assert_eq!(d.source, "hi\n");
        assert_eq!(d.caret, 2, "before the run, not before its last keystroke");
        d.redo();
        assert_eq!(d.caret, 5, "and redo restores the end of the whole run");
    }
}

#[test]
fn undo_restores_the_caret_across_a_format_toggle() {
    // A toggle reaches twig without going through `splice`, so it has to
    // record its own step — miss it and every stack depth below it is off by
    // one, and undo starts handing back another edit's caret.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "fmt_caret", "a word b\n");
        d.caret = 8;
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        d.undo();
        assert_eq!(d.source, "a word b\n");
        assert_eq!(
            d.selection(),
            Some((2, 6)),
            "the toggled selection comes back"
        );
    }
}

#[test]
fn an_edit_after_an_undo_truncates_the_caret_history_with_twigs() {
    // The drift that would never announce itself: twig drops its redo stack
    // on any fresh edit, so a leaf redo entry that outlives it would restore
    // a caret from the timeline that edit abandoned.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "redo_trunc", "hello world\n");
        d.caret = 11;
        d.edit(0, 5, "goodbye"); // step A, caret 11 → 7
        d.undo();
        assert_eq!(d.caret, 11);
        d.caret = 0;
        d.insert("X"); // diverges: A's redo is gone from twig
        assert_eq!(d.source, "Xhello world\n");

        d.redo();
        assert_eq!(d.source, "Xhello world\n", "nothing to redo onto");
        assert_eq!(d.status.as_deref(), Some("nothing to redo"));
        d.undo();
        assert_eq!(d.source, "hello world\n");
        assert_eq!(
            d.caret, 0,
            "the surviving step's caret, not the dropped one"
        );
    }
}

#[test]
fn indent_and_outdent_move_the_caret_line_with_its_text() {
    for view in [View::Source, View::Wysiwyg] {
        let g = |m, f: fn(&mut Doc)| golden_in(view, "indent_line", m, f);
        assert_eq!(g("he|llo\n", |d| d.indent()), "  he|llo\n");
        assert_eq!(g("  he|llo\n", |d| d.outdent()), "he|llo\n");
        // Indentation the caret is standing *in* collapses to the line start
        // rather than dragging the caret into the text.
        assert_eq!(g("| hello\n", |d| d.outdent()), "|hello\n");
        // A line with none to give back is left exactly as it was.
        assert_eq!(g("he|llo\n", |d| d.outdent()), "he|llo\n");
        // Less than a full level gives back what it has.
        assert_eq!(g(" he|llo\n", |d| d.outdent()), "he|llo\n");
        // A tab is one level however many spaces it isn't.
        assert_eq!(g("\the|llo\n", |d| d.outdent()), "he|llo\n");
    }
}

#[test]
fn one_indent_level_leaves_a_paragraph_a_paragraph() {
    // Why the level is two spaces and not the four both frontends type
    // today. Four is markdown's indented-code-block marker, so a Tab on a
    // paragraph would silently restyle it as code — a width that changes
    // what the document *means* isn't an indent. Pinned because the number
    // is the kind of thing a later list-aware pass would reach for.
    let mut d = doc_with("indent_kind", "hello\n");
    d.caret = 2;
    d.indent();
    assert_eq!(d.source, "  hello\n");
    assert!(
        d.nodes().iter().any(|n| n.kind == Kind::Para),
        "still prose after a Tab"
    );
    assert!(!d.nodes().iter().any(|n| n.kind == Kind::CodeBlock));

    // The four-space level this replaces, for contrast: same text, and twig
    // reparses the paragraph into a code block.
    let mut wide = doc_with("indent_kind_4", "    hello\n");
    wide.build_visual(80);
    assert!(
        wide.nodes().iter().any(|n| n.kind == Kind::CodeBlock),
        "four spaces is a code block, not an indented paragraph"
    );
}

#[test]
fn indent_nests_a_list_item_under_its_parent() {
    // Tab indents a list item by its own marker width, landing its marker at
    // the parent's content column so twig reparses it as a nested list.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "indent_nest", "- a\n- b\n");
        d.caret = 6; // on the second item
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n");
        let lists = d
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::BulletList)
            .count();
        assert_eq!(lists, 2, "the indented item is a nested list");
    }
}

#[test]
fn indent_nests_an_ordered_item_at_its_marker_width() {
    // An ordered marker `1. ` is three columns wide, so a two-space step
    // (which nests a bullet) leaves it flat. Regression: Tab must use the
    // marker width, three, so the item actually nests — and the source
    // renumbers so the sub-list restarts at 1 and the outer list resumes.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "indent_ord", "1. a\n2. b\n3. c\n");
        d.caret = d.source.find('b').unwrap();
        d.indent();
        assert_eq!(d.source, "1. a\n   1. b\n2. c\n");
        let lists = d
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::OrderedList)
            .count();
        assert_eq!(lists, 2, "the indented item is a nested ordered list");
    }
}

#[test]
fn indent_leaves_a_lists_first_item_put() {
    // The first item of a list has no sibling above it to nest under, so Tab
    // is a no-op there — the marker stays at column zero rather than being
    // shoved into indentation twig can't read as a sub-list.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "indent_first", "- a\n- b\n");
        d.caret = 1; // on the FIRST item
        d.indent();
        assert_eq!(d.source, "- a\n- b\n", "the first item doesn't nest");
        // The sibling below still nests, proving the guard is per-item.
        d.caret = d.source.find('b').unwrap();
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n");
    }
}

#[test]
fn hidden_mode_keeps_typed_markup_literal() {
    // The Diaryx default: typing `*hi*` gives the characters, not emphasis —
    // twig escapes what would open markup, so the source is `\*hi\*` and the
    // AST is a plain string. Formatting is the commands' job in this mode.
    let mut d = doc_in(View::Wysiwyg, "hidden_literal", "");
    d.insert("*hi*");
    assert_eq!(d.source, "\\*hi\\*");
    assert!(
        d.nodes()
            .iter()
            .all(|n| n.kind != Kind::Emph && n.kind != Kind::Strong)
    );
}

#[test]
fn hidden_mode_escapes_a_line_start_block_marker() {
    // A `#`/`-`/`>` at a line start would open a block, so Hidden mode keeps
    // it literal too — a Diaryx user's "# 1 idea" stays prose, not a heading.
    let mut d = doc_in(View::Wysiwyg, "hidden_block", "");
    d.insert("# hi");
    assert_eq!(d.source, "\\# hi");
    assert!(d.nodes().iter().all(|n| n.kind != Kind::Heading));
}

#[test]
fn authoring_modes_keep_typed_markup_live() {
    // Both authoring rungs of the ladder: typing `*hi*` really is emphasis
    // (no escape), the same as source view — escaping is `None`'s alone, and
    // it's the axis, not the reveal, that decides.
    for (view, mode) in [
        (View::Wysiwyg, MarkupMode::Shortcuts),
        (View::Wysiwyg, MarkupMode::Full),
        (View::Source, MarkupMode::None),
    ] {
        let mut d = doc_in(view, "live_markup", "");
        d.set_markup_mode(mode);
        d.insert("*hi*");
        assert_eq!(d.source, "*hi*", "{mode:?} in {view:?} types raw markup");
    }
}

#[test]
fn hidden_mode_overwrite_undoes_in_one_step() {
    // Typing over a selection escapes the replacement *and* stays a single
    // undo — the selection-delete and the literal insert fold together, so
    // one undo brings the whole selection back, like a plain overwrite.
    let mut d = doc_in(View::Wysiwyg, "hidden_overwrite", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6; // "word"
    d.insert("*");
    assert_eq!(d.source, "a \\* b\n", "the replacement is escaped");
    d.undo();
    assert_eq!(d.source, "a word b\n");
    assert_eq!(d.selection(), Some((2, 6)), "one undo, selection restored");
}

#[test]
fn backspace_over_an_escaped_char_takes_the_hidden_backslash_too() {
    // Type `*` in Hidden mode → `\*` (drawn as one `*`); one Backspace clears
    // the whole visual character, never stranding the hidden `\`.
    let mut d = doc_in(View::Wysiwyg, "bsp_escape", "");
    d.insert("*");
    assert_eq!(d.source, "\\*");
    d.backspace();
    assert_eq!(d.source, "", "the escape backslash went with the *");
    // A *literal* backslash (source view, no escape) is an ordinary char.
    let mut s = doc_in(View::Source, "bsp_lit", "a\\b\n");
    s.caret = 3; // after `b`
    s.backspace();
    assert_eq!(s.source, "a\\\n", "only the b is deleted, the \\ stays");
}

#[test]
fn hidden_mode_leaves_structural_markup_alone() {
    // Enter continues a bullet list by writing a real `- ` marker (an
    // `insert_raw`, not the typing path), so Hidden mode's escaping never
    // touches it — the list keeps working.
    let mut d = doc_in(View::Wysiwyg, "hidden_struct", "- item\n");
    d.caret = 6;
    d.newline();
    d.insert("two");
    assert_eq!(d.source, "- item\n- two\n");
}

#[test]
fn markup_mode_defaults_to_none_and_round_trips() {
    // Diaryx's default is the clean `None` surface; a markup-fluent
    // frontend can climb the ladder, and the choice sticks.
    let mut d = doc_in(View::Wysiwyg, "markup_mode", "hi\n");
    assert_eq!(d.markup_mode(), MarkupMode::None, "None by default");
    for mode in [MarkupMode::Shortcuts, MarkupMode::Full, MarkupMode::None] {
        d.set_markup_mode(mode);
        assert_eq!(d.markup_mode(), mode);
    }
}

#[test]
fn full_mode_reveals_only_the_caret_line() {
    // The mode's whole claim: the caret's line shows its raw delimiters and
    // every other line stays resolved. Two paragraphs with identical markup
    // so the only difference between the rows is where the caret is.
    let mut d = doc_in(
        View::Wysiwyg,
        "reveal_caret_line",
        "*one* here\n\n*two* there\n",
    );
    d.set_markup_mode(MarkupMode::Full);

    caret_at(&mut d, "one");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "*one* here"),
        "caret's line raw: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r == "two there"),
        "other line resolved: {rows:?}"
    );

    // Move to the other paragraph: the reveal follows, and the line just
    // left goes back to being resolved.
    caret_at(&mut d, "two");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "*two* there"),
        "caret's line raw: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r == "one here"),
        "left line resolved: {rows:?}"
    );
}

#[test]
fn revealing_a_coloured_highlight_shows_the_emoji_that_spelled_it() {
    // The emoji is a delimiter, not content — so `MarkupMode::Full` owes it
    // the same treatment as an emphasis's `*`: hidden while the caret is
    // elsewhere, shown in full where the caret lands. That falls out of
    // `delims` reading the bytes between the mark's span and its content
    // span, which is exactly `==🔴 ` and `==`, rather than from a table
    // of spellings — so the no-space form `==🟢green==` reveals right too.
    let mut d = doc_in(
        View::Wysiwyg,
        "reveal_coloured_mark",
        "a ==🔴 red== one\n\nb ==plain== two\n",
    );
    d.set_markup_mode(MarkupMode::Full);

    caret_at(&mut d, "red");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "a ==🔴 red== one"),
        "the caret's line shows the colour it was written with: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r == "b plain two"),
        "and every other line stays resolved: {rows:?}"
    );

    // Away from it, the emoji goes back to being markup — the reader sees
    // the words and the wash.
    caret_at(&mut d, "two");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "a red one"),
        "resolved again: {rows:?}"
    );
}

#[test]
fn hidden_modes_never_reveal_wherever_the_caret_is() {
    // The two rungs below `Full` share a rendering: delimiters stay hidden
    // even under the caret. `Shortcuts` differing from `None` only in what
    // typing does is exactly the point of splitting the axes.
    for mode in [MarkupMode::None, MarkupMode::Shortcuts] {
        let mut d = doc_in(View::Wysiwyg, "reveal_hidden", "*one* here\n");
        d.set_markup_mode(mode);
        caret_at(&mut d, "one");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "one here"),
            "{mode:?} hides: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains('*')),
            "{mode:?} shows no `*`: {rows:?}"
        );
    }
}

#[test]
fn revealed_delimiters_are_the_authors_own_spelling() {
    // Delimiters are re-read from the source rather than synthesized per
    // kind, so a line comes back spelled the way it was written: `_em_` does
    // not turn into `*em*`, and a two-backtick fence keeps both backticks.
    let body = "_em_ and __st__ and ``lit ` tick`` and [lk](http://x) and ~~del~~\n";
    let mut d = doc_in(View::Wysiwyg, "reveal_spelling", body);
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "em");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == body.trim_end()),
        "the revealed line is its own source: {rows:?}"
    );
}

#[test]
fn revealed_heading_shows_its_hashes() {
    // The `# ` marker is a block-level prefix, not an inline delimiter, so
    // it takes its own path — but it reveals on the same rule.
    let mut d = doc_in(View::Wysiwyg, "reveal_heading", "# Title\n\nbody\n");
    d.set_markup_mode(MarkupMode::Full);

    caret_at(&mut d, "Title");
    assert!(
        drawn_rows(&d).iter().any(|r| r == "# Title"),
        "{:?}",
        drawn_rows(&d)
    );

    caret_at(&mut d, "body");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "Title"),
        "hashes hidden again: {rows:?}"
    );
}

#[test]
fn revealed_delimiters_are_caret_stops() {
    // A delimiter that is drawn but can't be reached is worse than one
    // that's hidden: the mode exists so the markup can be *edited*. Every
    // revealed byte must be somewhere the caret can stand.
    let mut d = doc_in(View::Wysiwyg, "reveal_stops", "*em* x\n");
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "em");
    let opener = d.source.find('*').unwrap();
    assert!(d.vmap.is_stop(opener), "the opening `*` is a caret stop");
    assert!(
        d.vmap.is_stop(opener + 3),
        "the closing `*` is a caret stop"
    );
}

#[test]
fn setext_heading_reveals_nothing_across_its_newline() {
    // A setext heading's underline is on another line, so it is not the
    // caret line's to reveal — and emitting it would inject a `\n` glyph
    // that splits the row where the author wrote no break.
    let mut d = doc_in(View::Wysiwyg, "reveal_setext", "Title\n=====\n\nbody\n");
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "Title");
    let rows = drawn_rows(&d);
    assert!(
        rows.iter().any(|r| r == "Title"),
        "title renders alone: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains('=')),
        "no underline leaks in: {rows:?}"
    );
}

#[test]
fn markup_mode_axes_split_the_ladder() {
    // The two behaviours the ladder spells: `Shortcuts` is the middle rung
    // that authors markup but still hides it, and it's the only rung where
    // the two axes disagree.
    assert!(!MarkupMode::None.authors());
    assert!(!MarkupMode::None.reveals_caret_line());
    assert!(MarkupMode::Shortcuts.authors());
    assert!(!MarkupMode::Shortcuts.reveals_caret_line());
    assert!(MarkupMode::Full.authors());
    assert!(MarkupMode::Full.reveals_caret_line());
}

#[test]
fn indenting_an_empty_dash_item_under_text_dodges_the_setext_collapse() {
    // Tabbing an empty `- ` under a text line would spell `- hello\n  - `,
    // which twig (correctly, per CommonMark — pandoc agrees) reparses as a
    // setext H2. leaf swaps the dash for a `*` so the item stays an empty
    // nested bullet and `hello` stays prose: the file round-trips instead of
    // hiding a heading the user never asked for.
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "setext_guard", "- hello\n- \n");
        d.caret = d.source.find("- \n").unwrap() + 2; // after the empty marker
        d.indent();
        assert_eq!(d.source, "- hello\n  * \n");
        assert!(
            d.nodes().iter().all(|n| n.kind != Kind::Heading),
            "no heading"
        );
        // And it's genuinely a nested list, not a flat one.
        assert_eq!(
            d.nodes()
                .iter()
                .filter(|n| n.kind == Kind::BulletList)
                .count(),
            2
        );
    }
}

#[test]
fn indenting_a_dash_item_with_content_keeps_its_dash() {
    // With content, `- x` can't be a setext underline, so there's nothing to
    // dodge: the marker stays a dash and nests as an ordinary sub-bullet.
    let mut d = doc_in(View::Wysiwyg, "setext_ok", "- hello\n- x\n");
    d.caret = d.source.find('x').unwrap();
    d.indent();
    assert_eq!(d.source, "- hello\n  - x\n");
}

#[test]
fn the_setext_swap_undoes_as_one_step_with_the_indent() {
    // The dash→`*` repair coalesces into the Tab, so a single undo restores
    // the whole pre-Tab state rather than stranding a half-collapsed doc.
    let mut d = doc_in(View::Wysiwyg, "setext_undo", "- hello\n- \n");
    d.caret = d.source.find("- \n").unwrap() + 2;
    d.indent();
    assert_eq!(d.source, "- hello\n  * \n");
    d.undo();
    assert_eq!(d.source, "- hello\n- \n", "one undo, not two");
}

#[test]
fn indent_leaves_a_nested_lists_first_item_put_too() {
    // The guard is about siblings, not depth: the first item of an *inner*
    // list (already nested under `a`) still has nothing before it at its own
    // level, so Tab can't take it deeper.
    let mut d = doc_in(View::Wysiwyg, "indent_first_nested", "- a\n  - b\n  - c\n");
    d.caret = d.source.find('b').unwrap();
    d.indent();
    assert_eq!(d.source, "- a\n  - b\n  - c\n", "inner first item holds");
    // But `c` (a sibling of `b`) nests under `b`.
    d.caret = d.source.find('c').unwrap();
    d.indent();
    assert_eq!(d.source, "- a\n  - b\n    - c\n");
}

#[test]
fn backspace_at_a_nested_item_start_outdents_it() {
    // Backspace with the caret right after a nested item's marker gives back
    // one level of nesting, the mirror of Tab — and renumbers the flattened
    // ordered list back to a clean run.
    let mut d = doc_in(View::Wysiwyg, "bsp_outdent", "1. a\n   1. b\n2. c\n");
    d.caret = d.source.find('b').unwrap(); // start of the nested item's content
    d.backspace();
    assert_eq!(d.source, "1. a\n2. b\n3. c\n");
}

#[test]
fn backspace_at_a_top_level_item_start_strips_the_marker() {
    // At the outermost level there's no nesting left to give back, so the same
    // keystroke drops the bullet and leaves a plain paragraph.
    let mut d = doc_in(View::Wysiwyg, "bsp_strip", "- a\n- b\n");
    d.caret = d.source.find('b').unwrap(); // right after `- `
    d.backspace();
    assert_eq!(d.source, "- a\nb\n", "the marker is gone, the text stays");
}

#[test]
fn backspace_mid_item_still_deletes_a_character() {
    // The list behaviour is armed only at the item's content start; anywhere
    // else Backspace is the ordinary character delete.
    let mut d = doc_in(View::Wysiwyg, "bsp_mid", "- ab\n");
    d.caret = d.source.find('b').unwrap(); // between `a` and `b`
    d.backspace();
    assert_eq!(d.source, "- b\n");
}

#[test]
fn backspace_at_a_heading_start_strips_the_marker() {
    // The `# ` is markup the rich view hides, so Backspace over it takes the
    // whole marker and leaves a paragraph. Deleting a byte of it instead left
    // `#Title` — no longer a heading, with the hash now literal text the user
    // never typed and has to delete again.
    let mut d = doc_in(View::Wysiwyg, "bsp_head", "## Title\n");
    d.caret = d.source.find('T').unwrap(); // right after `## `
    d.backspace();
    assert_eq!(d.source, "Title\n");
    assert_eq!(
        d.caret, 0,
        "the caret stays with the text it was in front of"
    );
}

#[test]
fn backspace_at_a_heading_start_keeps_the_block_around_it() {
    // Only the heading's own marker goes — the quote (or list) it sits in is
    // untouched, exactly as un-heading it should be.
    let mut d = doc_in(View::Wysiwyg, "bsp_head_quote", "> # Title\n");
    d.caret = d.source.find('T').unwrap();
    d.backspace();
    assert_eq!(d.source, "> Title\n");
}

#[test]
fn backspace_at_a_heading_start_takes_its_closing_sequence_too() {
    // `# Title #`'s trailing hashes are hidden at the other end; leaving them
    // behind would surface the same stray hash the marker delete just avoided.
    let mut d = doc_in(View::Wysiwyg, "bsp_head_closed", "# Title #\n");
    d.caret = d.source.find('T').unwrap();
    d.backspace();
    assert_eq!(d.source, "Title\n");
    // And it's one edit: a single undo puts the whole heading back.
    d.undo();
    assert_eq!(d.source, "# Title #\n");
}

#[test]
fn backspace_mid_heading_still_deletes_a_character() {
    // The heading behaviour is armed only at the content's start; anywhere
    // else Backspace is the ordinary character delete.
    let mut d = doc_in(View::Wysiwyg, "bsp_head_mid", "# ab\n");
    d.caret = d.source.find('b').unwrap();
    d.backspace();
    assert_eq!(d.source, "# b\n");
}

#[test]
fn source_view_backspace_still_edits_the_heading_marker_literally() {
    // In source view the `# ` is text on the screen the user is deleting a
    // byte of, so it keeps its literal meaning — the same split the list
    // ladder and Enter draw between the two views.
    let mut d = doc_with("bsp_head_src", "# Title\n");
    d.caret = d.source.find('T').unwrap();
    d.backspace();
    assert_eq!(d.source, "#Title\n");
}

#[test]
fn outdent_unnests_an_ordered_item_in_one_press() {
    // Shift+Tab gives back exactly the marker width the indent added, so a
    // nested ordered item unnests in a single press, and the flattened list
    // renumbers back to a clean 1, 2, 3.
    let mut d = doc_with("outdent_ord", "1. a\n   2. b\n3. c\n");
    d.caret = d.source.find('b').unwrap();
    d.outdent();
    assert_eq!(d.source, "1. a\n2. b\n3. c\n");
    let lists = d
        .nodes()
        .iter()
        .filter(|n| n.kind == Kind::OrderedList)
        .count();
    assert_eq!(lists, 1, "back to one flat list");
}

#[test]
fn table_insert_row_adds_a_row_below_the_caret() {
    let mut d = doc_with("tbl_ins_row", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
    d.caret = d.source.find('1').unwrap(); // in the body row
    d.table_insert_row(true);
    assert_eq!(d.source, "| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n");
}

#[test]
fn table_insert_and_delete_column_at_the_caret() {
    let mut d = doc_with("tbl_col", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
    d.caret = d.source.find('a').unwrap(); // column 0
    d.table_insert_column(true); // add a column to the right of `a`
    assert_eq!(
        d.source,
        "| a |  | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n"
    );
    d.caret = d.source.find('b').unwrap(); // now the third column
    d.table_delete_column();
    assert_eq!(d.source, "| a |  |\n| --- | --- |\n| 1 |  |\n");
}
