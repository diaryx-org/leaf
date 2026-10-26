use super::*;

#[test]
fn undo_then_redo_round_trips_an_edit() {
    let mut d = doc_with("undo", "hello\n");
    d.caret = 5;
    d.insert("!");
    assert_eq!(d.source, "hello!\n");
    d.undo();
    assert_eq!(d.source, "hello\n");
    assert_eq!(d.caret, 5, "undo restores the caret");
    d.redo();
    assert_eq!(d.source, "hello!\n");
}

#[test]
fn a_run_of_typing_undoes_as_one_step() {
    let mut d = doc_with("coalesce", "\n");
    d.caret = 0;
    d.insert("a");
    d.insert("b");
    d.insert("c");
    assert_eq!(d.source, "abc\n");
    d.undo(); // the whole typed run, not just "c"
    assert_eq!(d.source, "\n");
    d.undo(); // nothing left — the run was one step
    assert_eq!(d.source, "\n");
    assert_eq!(d.status.as_deref(), Some("nothing to undo"));
}

// ── IME composition ──────────────────────────────────────────────────────

#[test]
fn a_composition_run_undoes_as_one_step() {
    let mut d = doc_with("compose", "\n");
    d.caret = 0;
    // What an IME does: each step replaces the last one's provisional bytes.
    d.edit_composing(0, 0, "k");
    d.edit_composing(0, 1, "か");
    d.edit_composing(0, 3, "かん");
    d.edit_composing(0, 6, "感"); // the commit
    d.end_composition();
    assert_eq!(d.source, "感\n");
    d.undo(); // the whole composition, not its last keystroke
    assert_eq!(d.source, "\n");
    assert_eq!(d.status.as_deref(), None, "the run was a single step");
}

/// A Replace All the way a host does one: every match, last to first so
/// the offsets still to go stay put, each a selection replaced by an
/// insert, inside one undo group. `select_range` rather than the host's
/// snapping `place_caret`, which would snap against a map these tests do
/// not rebuild between edits.
fn replace_all_in(d: &mut Doc, needle: &str, with: &str) {
    let hits: Vec<usize> = d.source.match_indices(needle).map(|(i, _)| i).collect();
    d.begin_undo_group();
    for at in hits.into_iter().rev() {
        d.select_range(at, at + needle.len());
        d.insert(with);
    }
    d.end_undo_group();
}

#[test]
fn a_replace_all_undoes_and_redoes_as_one_step() {
    let mut d = wysiwyg_doc("group", "a cat, a **cat**, a cat\n");
    d.caret = 0;
    d.insert("x");
    replace_all_in(&mut d, "cat", "dog");
    assert_eq!(d.source, "xa dog, a **dog**, a dog\n");
    assert!(d.undo(), "one undo");
    assert_eq!(
        d.source, "xa cat, a **cat**, a cat\n",
        "puts back every match"
    );
    assert!(d.redo(), "one redo");
    assert_eq!(
        d.source, "xa dog, a **dog**, a dog\n",
        "replaces them all again"
    );
    assert!(d.undo());
    assert!(d.undo(), "the typing before is its own step");
    assert_eq!(d.source, "a cat, a **cat**, a cat\n");
    assert!(!d.can_undo());
}

#[test]
fn typing_after_a_group_is_a_step_of_its_own() {
    // A one-character replacement is an insert like a keystroke, and a
    // keystroke after it would coalesce with it outside a group.
    let mut d = wysiwyg_doc("group", "a b a\n");
    replace_all_in(&mut d, "a", "c");
    d.caret = d.source.len() - 1;
    d.insert("d");
    assert_eq!(d.source, "c b cd\n");
    assert!(d.undo());
    assert_eq!(d.source, "c b c\n", "the typing alone");
    assert!(d.undo());
    assert_eq!(d.source, "a b a\n", "then the replacement, whole");
}

#[test]
fn a_group_of_more_edits_than_the_history_holds_is_still_one_step() {
    // twig keeps 200 steps; a group folds as it goes, so it never holds
    // more than one of them.
    let src = format!("start\n{}\n", "x ".repeat(300));
    let mut d = wysiwyg_doc("group", &src);
    d.caret = 5;
    d.insert("!");
    replace_all_in(&mut d, "x", "yy");
    assert_eq!(d.undo_steps, 2);
    assert!(d.undo());
    assert_eq!(d.source, src.replacen("start", "start!", 1));
    assert!(d.undo());
    assert_eq!(d.source, src);
    assert!(!d.can_undo());
}

#[test]
fn an_undo_closes_an_open_group() {
    let mut d = wysiwyg_doc("group", "one\n");
    d.caret = 3;
    d.begin_undo_group();
    d.begin_undo_group();
    d.insert("x");
    assert!(d.in_undo_group());
    assert!(d.undo());
    assert!(
        !d.in_undo_group(),
        "the history moved, so the group is over"
    );
    d.end_undo_group();
    d.end_undo_group();
    assert_eq!(d.source, "one\n");
    assert!(d.redo());
    assert_eq!(d.source, "onex\n");
}

#[test]
fn replacing_the_source_keeps_the_caret_on_its_characters() {
    let mut d = wysiwyg_doc("replace", "One.\n\nTwo.\n\nThree.\n");
    let on = d.source.find("Three").unwrap() + 2;
    d.caret = on;
    // Another device's edit, before the caret: the caret moves along.
    assert!(d.replace_source("One, and more.\n\nTwo.\n\nThree.\n"));
    assert_eq!(d.source, "One, and more.\n\nTwo.\n\nThree.\n");
    assert_eq!(&d.source[d.caret..], "ree.\n", "still inside Three");
    // After the caret: it stays.
    let at = d.caret;
    assert!(d.replace_source("One, and more.\n\nTwo.\n\nThree.\n\nFour.\n"));
    assert_eq!(d.caret, at);
    assert!(d.selection().is_none());
    // One undo step, the caret where it stood.
    assert!(d.undo());
    assert_eq!(d.source, "One, and more.\n\nTwo.\n\nThree.\n");
    assert_eq!(d.caret, at);
}

#[test]
fn replacing_the_source_writes_markup_as_markup() {
    let mut d = wysiwyg_doc("replace-markup", "plain\n");
    assert!(d.replace_source("**bold** and *it*\n"));
    assert_eq!(
        d.source, "**bold** and *it*\n",
        "source, not typing: nothing escaped"
    );
    assert!(d.replace_source("**bold** and *it*\n"), "already there");
}

#[test]
fn a_substitution_behind_the_caret_leaves_the_caret_and_is_its_own_step() {
    let mut d = wysiwyg_doc("substitute", "\n");
    d.caret = 0;
    for c in ["I", " ", "s", "a", "w", " ", "t", "e", "h", " "] {
        d.insert(c);
    }
    assert_eq!(d.source, "I saw teh \n");
    let at = d.source.find("teh").unwrap();
    assert!(d.substitute(at, at + 3, "the"));
    assert_eq!(d.source, "I saw the \n");
    assert_eq!(d.caret, 10, "still after the space");
    assert!(d.selection().is_none());
    d.insert("c");
    assert_eq!(d.source, "I saw the c\n");
    assert!(d.undo(), "the typing after it is a step of its own");
    assert_eq!(d.source, "I saw the \n");
    assert!(d.undo(), "the correction is one step");
    assert_eq!(d.source, "I saw teh \n", "what was typed");
    assert_eq!(d.caret, 10, "with the caret where it stood");
    assert!(d.redo());
    assert_eq!(d.source, "I saw the \n");
    assert!(d.undo());
    assert!(d.undo(), "and the typing before it is its own");
    assert_eq!(d.source, "\n");
    assert!(!d.can_undo());
}

#[test]
fn a_substitution_keeps_the_markup_it_stands_beside() {
    // A quote typed just past a bold run: only its own byte changes.
    let mut d = wysiwyg_doc("substitute", "A **bold** word\n");
    let at = d.source.find(" word").unwrap();
    d.caret = at;
    d.insert("\"");
    assert_eq!(d.source, "A **bold**\" word\n");
    assert!(d.substitute(at, at + 1, "\u{201d}"));
    assert_eq!(d.source, "A **bold**\u{201d} word\n");
    assert_eq!(d.caret, at + "\u{201d}".len());
    assert!(
        d.marks_at(d.source.find("bold").unwrap())
            .iter()
            .any(|(k, _)| *k == InlineKind::Strong)
    );
    // A range that is not one is refused, and changes nothing.
    let src = d.source.clone();
    assert!(!d.substitute(3, 2, "x"));
    assert!(!d.substitute(0, src.len() + 1, "x"));
    assert!(!d.substitute(d.source.find('\u{201d}').unwrap() + 1, d.source.len(), "x"));
    assert_eq!(d.source, src);
}

#[test]
fn a_substitution_that_half_lands_takes_back_only_its_own_deletion() {
    // The deletion lands and twig refuses the literal text: the deleted
    // bytes go back, and nothing else moves — not an earlier substitution
    // of the same keystroke, not the group it is in, and no redo is left
    // to delete them again.
    let mut d = wysiwyg_doc("substitute", "\n");
    d.set_markup_mode(MarkupMode::None);
    d.caret = 0;
    for c in ["a", "\"", "b", " ", "\""] {
        d.insert(c);
    }
    assert_eq!(d.source, "a\"b \"\n");

    d.refuse_next_literal = true;
    assert!(!d.substitute(1, 2, "\u{201c}"));
    assert_eq!(d.source, "a\"b \"\n");
    assert_eq!(d.caret, 5, "the caret where it stood");
    assert!(!d.can_redo(), "no redo step left behind");

    d.begin_undo_group();
    assert!(d.substitute(4, 5, "\u{201d}"));
    d.refuse_next_literal = true;
    assert!(!d.substitute(1, 2, "\u{201c}"));
    assert!(d.in_undo_group(), "the group is still open");
    assert_eq!(d.source, "a\"b \u{201d}\n", "the first substitution stands");
    d.end_undo_group();
    assert!(!d.can_redo());
    assert!(d.undo());
    assert_eq!(d.source, "a\"b \"\n", "one step takes the group back");
    assert!(d.undo());
    assert_eq!(d.source, "\n", "and the typing is still one step");
    assert!(!d.can_undo());

    // A group whose first edit is the one that half lands has no step.
    let mut d = wysiwyg_doc("substitute", "\n");
    d.set_markup_mode(MarkupMode::None);
    d.caret = 0;
    d.insert("\"");
    d.begin_undo_group();
    d.refuse_next_literal = true;
    assert!(!d.substitute(0, 1, "\u{201c}"));
    assert!(d.substitute(0, 1, "\u{201c}"));
    d.end_undo_group();
    assert!(d.undo());
    assert_eq!(
        d.source, "\"\n",
        "the substitution that landed is the group's step"
    );
    assert!(d.undo());
    assert_eq!(d.source, "\n");
}

#[test]
fn a_substitution_is_written_the_way_typing_writes() {
    // Where typing is literal, so is a replacement's text.
    let mut d = wysiwyg_doc("substitute", "say hi\n");
    d.set_markup_mode(MarkupMode::None);
    assert!(d.substitute(4, 6, "*hi*"));
    assert_eq!(d.source, "say \\*hi\\*\n");
    assert!(d.undo());
    assert_eq!(d.source, "say hi\n", "one step, escapes and all");
}

#[test]
fn can_undo_is_false_once_a_coalesced_run_is_undone() {
    // The task's repro: five characters typed are one twig step, and the
    // counter used to say five.
    let mut d = wysiwyg_doc("undo_truth", "\n");
    d.caret = 0;
    for c in ["h", "e", "l", "l", "o"] {
        d.insert(c);
    }
    assert!(d.can_undo());
    assert!(d.undo(), "the run undoes");
    assert_eq!(d.source, "\n");
    assert!(!d.can_undo(), "and nothing is left behind it");
    let rev = d.revision();
    assert!(!d.undo(), "an undo that does not move says so");
    assert_eq!(d.revision(), rev);
    assert!(d.can_redo());
    assert!(d.redo());
    assert_eq!(d.source, "hello\n");
    assert!(!d.can_redo());
    assert!(!d.redo());
}

#[test]
fn can_undo_is_exact_across_the_gestures_that_coalesce() {
    // Each gesture below folds, or may fold, one twig step into another.
    // After it, the mirror must name exactly the number of undos twig
    // takes — and the same number of redos back.
    type Gesture = fn(&mut Doc);
    let cases: &[(&str, &str, Gesture)] = &[
        ("typing", "\n", |d| {
            d.caret = 0;
            d.insert("ab");
            d.insert("c");
            d.move_left(false);
            d.insert("x");
        }),
        ("backspaces", "abcdef\n", |d| {
            d.caret = 6;
            d.backspace();
            d.backspace();
            d.insert("z");
            d.backspace();
        }),
        ("ordered list item", "1. one\n2. two\n", |d| {
            d.caret = 6;
            d.newline();
            d.insert("x");
        }),
        ("list indent", "- one\n- two\n", |d| {
            d.caret = d.source.find("two").unwrap();
            d.indent();
            d.outdent();
        }),
        ("bold then type", "one two\n", |d| {
            d.select_range(0, 3);
            d.toggle(InlineKind::Strong);
            d.caret = d.source.len() - 1;
            d.insert("!");
        }),
        ("highlight", "one two\n", |d| {
            d.select_range(0, 3);
            d.highlight(None);
            d.highlight(Some(MarkColor::Green));
        }),
        ("heading and quote", "one\n", |d| {
            d.caret = 1;
            d.toggle_heading(2);
            d.toggle_blockquote();
        }),
        ("footnote", "one\n", |d| {
            d.caret = 3;
            d.insert_footnote();
            d.insert("note");
        }),
        ("blocks", "one\n\ntwo\n\nthree\n", |d| {
            d.caret = 1;
            d.move_block_down();
            d.toggle_list(true);
            d.set_alignment(Some(Align::Center));
            d.insert_page_break();
            d.insert_table(2, 2);
        }),
        ("paste and compose", "\n", |d| {
            d.caret = 0;
            d.paste("one two");
            d.delete_word_back();
            d.edit_composing(d.caret, d.caret, "か");
            d.edit_composing(d.caret - 3, d.caret, "蚊");
            d.end_composition();
            d.insert("z");
        }),
        ("table", "| a | b |\n| - | - |\n| c | d |\n", |d| {
            d.place_caret(d.source.find("d |").unwrap(), false);
            d.cell_tab(true);
            d.insert("e");
            d.cell_return();
        }),
        ("replace all", "cat cat cat\n", |d| {
            d.caret = 0;
            d.insert("a ");
            replace_all_in(d, "cat", "dog");
            d.insert("!");
        }),
        ("nested group", "one\n\ntwo\n", |d| {
            d.begin_undo_group();
            d.caret = 3;
            d.insert("x");
            d.begin_undo_group();
            d.toggle_heading(1);
            d.end_undo_group();
            d.insert("y");
            d.end_undo_group();
            d.backspace();
        }),
        ("group around a list", "1. one\n2. two\n", |d| {
            d.begin_undo_group();
            d.caret = 6;
            d.newline();
            d.insert("x");
            d.end_undo_group();
        }),
        ("substitution", "\n", |d| {
            d.caret = 0;
            d.insert("t");
            d.insert("e");
            d.insert("h");
            d.insert(" ");
            d.substitute(0, 3, "the");
            d.insert("c");
        }),
        ("empty group", "one\n", |d| {
            d.caret = 3;
            d.insert("x");
            d.begin_undo_group();
            d.end_undo_group();
            d.insert("y");
        }),
        ("undo inside a group", "one\n", |d| {
            d.caret = 3;
            d.begin_undo_group();
            d.insert("x");
            d.undo();
            d.insert("y");
            d.insert("z");
            d.end_undo_group();
        }),
    ];
    for (name, src, gesture) in cases {
        let mut d = wysiwyg_doc("undo_exact", src);
        gesture(&mut d);
        let (steps, after) = (d.undo_steps, d.source.clone());
        let mut undone = 0;
        while d.can_undo() {
            assert!(d.undo(), "{name}: can_undo said yes, undo moved nothing");
            undone += 1;
        }
        assert!(!d.undo(), "{name}: can_undo said no, undo moved");
        assert_eq!(undone, steps, "{name}");
        assert_eq!(d.source, *src, "{name}: back to the start");
        let mut redone = 0;
        while d.can_redo() {
            assert!(d.redo(), "{name}: can_redo said yes, redo moved nothing");
            redone += 1;
        }
        assert!(!d.redo(), "{name}: can_redo said no, redo moved");
        assert_eq!(redone, steps, "{name}");
        assert_eq!(d.source, after, "{name}: forward to the end");
    }
}

#[test]
fn two_compositions_are_two_undo_steps() {
    let mut d = doc_with("compose_two", "\n");
    d.caret = 0;
    d.edit_composing(0, 0, "か");
    d.edit_composing(0, 3, "蚊");
    d.end_composition();
    d.edit_composing(3, 3, "き");
    d.edit_composing(3, 6, "木");
    d.end_composition();
    assert_eq!(d.source, "蚊木\n");
    d.undo();
    assert_eq!(d.source, "蚊\n", "only the second composition");
    d.undo();
    assert_eq!(d.source, "\n");
}

#[test]
fn a_composition_does_not_fold_into_the_typing_around_it() {
    let mut d = doc_with("compose_typing", "\n");
    d.caret = 0;
    d.insert("a");
    d.insert("b");
    d.edit_composing(2, 2, "か");
    d.edit_composing(2, 5, "蚊");
    d.end_composition();
    d.insert("c");
    assert_eq!(d.source, "ab蚊c\n");
    d.undo();
    assert_eq!(d.source, "ab蚊\n");
    d.undo();
    assert_eq!(d.source, "ab\n");
    d.undo();
    assert_eq!(d.source, "\n");
}

#[test]
fn ending_a_composition_that_never_began_leaves_a_typing_run_alone() {
    let mut d = doc_with("compose_spurious", "\n");
    d.caret = 0;
    d.insert("a");
    d.end_composition(); // an IME unmarking unprompted
    d.insert("b");
    assert_eq!(d.source, "ab\n");
    d.undo();
    assert_eq!(d.source, "\n", "still one typed run");
}
