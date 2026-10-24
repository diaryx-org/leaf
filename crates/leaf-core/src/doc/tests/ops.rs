//! The ops a host takes: every change to the text as twig's retain, insert
//! and delete ops, and `replace_source` and a reload made of them.

use super::*;

/// Take the ops and check they bring `base` to the source.
fn take_onto(d: &mut Doc, base: &str) -> Vec<Op> {
    let ops = d.take_ops();
    assert_eq!(
        Op::apply(base, &ops).unwrap(),
        d.source,
        "ops {ops:?} onto {base:?}"
    );
    ops
}

#[test]
fn typing_is_one_list_of_ops_against_the_text_last_taken() {
    let mut d = doc_with("ops-typing", "hello\n");
    assert!(!d.has_ops());
    assert_eq!(d.take_ops(), vec![]);
    d.caret = 5;
    for c in [" ", "w", "o", "r", "l", "d"] {
        d.insert(c);
    }
    assert!(d.has_ops());
    let ops = take_onto(&mut d, "hello\n");
    assert_eq!(ops, vec![Op::Retain(5), Op::Insert(" world".into())]);
    assert!(!d.has_ops());

    d.backspace();
    let ops = take_onto(&mut d, "hello world\n");
    assert_eq!(ops, vec![Op::Retain(10), Op::Delete(1)]);
}

#[test]
fn a_gesture_is_the_fewest_ops_that_make_it() {
    let mut d = doc_with("ops-bold", "a word here\n");
    d.select_range(2, 6);
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "a **word** here\n");
    // The word is kept, not deleted and written again — what lets a merge
    // keep an edit someone else made inside it.
    assert_eq!(
        take_onto(&mut d, "a word here\n"),
        vec![
            Op::Retain(2),
            Op::Insert("**".into()),
            Op::Retain(4),
            Op::Insert("**".into())
        ]
    );
}

#[test]
fn an_undo_and_a_redo_are_changes_like_any_other() {
    let mut d = doc_with("ops-undo", "one\n");
    d.caret = 3;
    d.insert("!");
    let base = d.source.clone();
    d.take_ops();
    assert!(d.undo());
    assert_eq!(take_onto(&mut d, &base), vec![Op::Retain(3), Op::Delete(1)]);
    assert!(d.redo());
    assert_eq!(
        take_onto(&mut d, "one\n"),
        vec![Op::Retain(3), Op::Insert("!".into())]
    );
}

#[test]
fn edits_that_cancel_out_leave_no_ops() {
    let mut d = doc_with("ops-cancel", "abc\n");
    d.caret = 3;
    d.insert("x");
    d.backspace();
    assert_eq!(d.source, "abc\n");
    assert!(!d.has_ops());
}

#[test]
fn every_kind_of_change_composes_onto_the_text_last_taken() {
    let mut d = doc_with("ops-mixed", "# Title\n\nsome text here\n\n- one\n- two\n");
    let base = d.source.clone();
    d.caret = d.source.find("text").unwrap();
    d.insert("more ");
    d.select_range(0, 7);
    d.toggle(InlineKind::Emph);
    d.caret = d.source.len();
    d.insert("- three");
    d.newline();
    d.undo();
    d.redo();
    assert!(d.replace_source(&d.source.replace("some", "any")));
    take_onto(&mut d, &base);
}

#[test]
fn replace_source_keeps_a_caret_between_two_changes_on_its_text() {
    let mut d = doc_with("ops-replace", "one\n\ntwo\n\nthree\n");
    d.caret = d.source.find("two").unwrap() + 1;
    d.select_range(d.caret, d.caret + 1);
    let (caret, anchor) = (d.caret, d.anchor);
    assert!(d.replace_source("1st:\n\ntwo\n\nthree!\n"));
    // Both changes are around the selection: one before it moves it along,
    // one after it does not. One span from the first to the last would have
    // swallowed it and sent the caret to the end of the file.
    assert_eq!(d.caret, caret + 1);
    assert_eq!(d.anchor, anchor.map(|a| a + 1));
    assert_eq!(&d.source[d.caret - 1..d.caret], "w");
    assert_eq!(
        take_onto(&mut d, "one\n\ntwo\n\nthree\n"),
        vec![
            Op::Insert("1st:".into()),
            Op::Delete(3),
            Op::Retain(12),
            Op::Insert("!".into())
        ]
    );
    // One undo step.
    assert!(d.undo());
    assert_eq!(d.source, "one\n\ntwo\n\nthree\n");
}

#[test]
fn a_reload_is_the_ops_of_what_changed_and_not_the_whole_file() {
    let mut d = doc_with("ops-reload", "alpha\n\nbeta\n");
    std::fs::write(&d.path, "alpha\n\nbeta, again\n").unwrap();
    d.reload();
    assert_eq!(d.source, "alpha\n\nbeta, again\n");
    assert!(!d.dirty);
    assert_eq!(
        take_onto(&mut d, "alpha\n\nbeta\n"),
        vec![Op::Retain(11), Op::Insert(", again".into())]
    );
}

#[test]
fn carry_moves_a_place_past_changes_before_it_only() {
    use crate::doc::ops::{carry, splices};
    let ops = [
        Op::Retain(2),
        Op::Insert("xx".into()),
        Op::Delete(1),
        Op::Retain(3),
        Op::Insert("y".into()),
    ];
    let s = splices(&ops);
    assert_eq!(s, vec![(2, 3, 2), (6, 6, 1)]);
    assert_eq!(carry(1, &s), 1); // before both
    assert_eq!(carry(2, &s), 2); // at the first's start: stays
    assert_eq!(carry(3, &s), 4); // at its end: moves with it
    assert_eq!(carry(5, &s), 6); // between the two
    assert_eq!(carry(6, &s), 8); // at an insert: after it, as a splice moves it
    assert_eq!(carry(9, &s), 11); // past both
}
