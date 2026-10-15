use super::*;

// ── thematic breaks ─────────────────────────────────────────────────────

#[test]
fn insert_thematic_break_parts_the_paragraph_around_the_caret() {
    // A rule is a block, so twig's `insert_thematic_break` alone lands it
    // after the whole paragraph. `split_block` parts the paragraph first and
    // the rule is aimed at the *first* half, which is what a rule button is
    // understood to do — and what leaf spelled by hand until twig grew both
    // halves of the gesture.
    let mut d = doc_with("hr_mid", "before after\n");
    d.caret = 7; // between "before " and "after"
    d.insert_thematic_break();
    assert_eq!(d.source, "before \n\n---\n\nafter\n");
    assert_eq!(d.selection(), None);
    assert_eq!(
        kind_at(&mut d, "before \n\n".len()),
        Some(Kind::ThematicBreak)
    );
}

#[test]
fn insert_thematic_break_at_a_paragraph_s_end_splits_nothing() {
    // At the end there is nothing to part, and a split there writes the
    // separator anyway — a blank line and the empty slot the next paragraph
    // would fill — which the rule then landed above: `para\n\n* * *\n\n\n`,
    // two blank lines nothing fills. Now the rule lands after the paragraph,
    // where the split-and-aim was sending it regardless. Both formats, and
    // both shapes of a last line — terminated, and still being typed —
    // because the two reach the split through different doors: Markdown's
    // paragraph span stops before its newline, so `para\n` at 4 never split
    // there, but `para` at 4 did.
    //
    // What stands under the rule is the line the caret goes on to, not a
    // slot: nothing above the rule but the one blank line, and the caret on
    // the line beneath it.
    for (fmt, rule) in [(Format::Markdown, "---"), (Format::Djot, "* * *")] {
        for src in ["para\n", "para"] {
            let mut d = Doc::from_source(src.into(), fmt).unwrap();
            d.caret = 4;
            d.insert_thematic_break();
            assert_eq!(d.source, format!("para\n\n{rule}\n\n"), "{fmt:?} {src:?}");
            assert_eq!(d.caret, d.source.len());
        }
        // Mid-document the slot sat between the rule and the next block;
        // the caret's line does now, a blank line either side of it.
        let mut d = Doc::from_source("para\n\nnext\n".into(), fmt).unwrap();
        d.caret = 4;
        d.insert_thematic_break();
        assert_eq!(d.source, format!("para\n\n{rule}\n\n\n\nnext\n"), "{fmt:?}");
        // Trailing whitespace is nothing to part either.
        let mut d = Doc::from_source("para  \n".into(), fmt).unwrap();
        d.caret = 4;
        d.insert_thematic_break();
        assert_eq!(d.source, format!("para  \n\n{rule}\n\n"), "{fmt:?}");
    }
}

#[test]
fn insert_thematic_break_at_a_paragraph_s_start_lands_before_it() {
    // The split at the start parts nothing, but it is kept on purpose:
    // `|para` becomes `\npara` with the caret on a blank line, and twig
    // (3.5.2) writes a rule aimed at a blank line ON that line — the only
    // way "before the paragraph" is reachable through a gesture that only
    // places after. Before 3.5.2 this came out as `\n\n---\n\npara`.
    for (fmt, rule) in [(Format::Markdown, "---"), (Format::Djot, "* * *")] {
        let mut d = Doc::from_source("para\n".into(), fmt).unwrap();
        d.caret = 0;
        d.insert_thematic_break();
        assert_eq!(d.source, format!("{rule}\n\npara\n"), "{fmt:?}");
        let mut d = Doc::from_source("prev\n\npara\n".into(), fmt).unwrap();
        d.caret = 6;
        d.insert_thematic_break();
        assert_eq!(d.source, format!("prev\n\n{rule}\n\npara\n"), "{fmt:?}");
    }
}

#[test]
fn insert_thematic_break_on_a_blank_line_takes_that_line() {
    // The gap between two blocks is where a click lands the caret; the
    // rule goes on the blank, one blank each side, and the caret on the
    // line opened under it.
    let mut d = doc_with("hr_gap", "a\n\nb\n");
    d.caret = 2;
    d.insert_thematic_break();
    assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
    assert_eq!(d.caret, "a\n\n---\n\n".len());
}

#[test]
fn insert_thematic_break_leaves_the_caret_on_a_line_under_the_rule() {
    // The caret used to be left where twig's splice ended, the home past
    // the rule, which is drawn on the rule's own row: the writer saw the
    // caret beside the rule, and mid-document what they typed next ran
    // into the block below. Now it goes on to an empty row under the
    // rule's gap from every place a rule is asked for — the end of a
    // paragraph, the empty paragraph Enter opened, a list and the line
    // Enter leaves a list for, closing the document and before another
    // block — and what is typed there is a paragraph of its own.
    for (body, caret, typed) in [
        ("para\n", 4, "para\n\n---\n\nX"),
        ("para\n\n\n", 6, "para\n\n---\n\nX"),
        ("- one\n- two\n", 11, "- one\n- two\n\n---\n\nX"),
        ("- one\n- two\n\n\n", 13, "- one\n- two\n\n---\n\nX"),
        ("a\n\nb\n", 1, "a\n\n---\n\nX\n\nb\n"),
        ("a\n\n\n\nb\n", 3, "a\n\n---\n\nX\n\nb\n"),
    ] {
        let mut d = wysiwyg_doc("hr_caret", body);
        d.build_visual(80);
        d.caret = caret;
        d.insert_thematic_break();
        d.build_visual(80);
        let rule = d.vmap.pos_of_offset(d.source.find("---").unwrap()).0;
        let (row, col) = d.vmap.pos_of_offset(d.caret);
        assert_eq!((row, col), (rule + 2, 0), "{body:?} → {:?}", d.source);
        assert!(d.vmap.rows[rule + 1].decoration, "the gap under the rule");
        assert!(d.vmap.rows[row].glyphs.is_empty(), "an empty line");
        d.insert("X");
        assert_eq!(d.source, typed, "{body:?}");
        let rule_at = d.source.find("---").unwrap();
        assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
    }
    // djot's rule takes in its newline; the line under it is the same.
    let mut d = Doc::from_source("a\n\nb\n".into(), Format::Djot).unwrap();
    d.view = View::Wysiwyg;
    d.caret = 1;
    d.insert_thematic_break();
    d.build_visual(80);
    assert_eq!(d.source, "a\n\n* * *\n\n\n\nb\n");
    assert_eq!(d.vmap.pos_of_offset(d.caret), (4, 0));
}

#[test]
fn a_rule_parting_a_paragraph_leaves_the_caret_before_its_second_half() {
    // The line under the rule is the paragraph's second half, so that is
    // where the caret goes: the text it was standing in front of is still
    // in front of it.
    let mut d = wysiwyg_doc("hr_parted", "before after\n");
    d.caret = 7;
    d.insert_thematic_break();
    assert_eq!(d.source, "before \n\n---\n\nafter\n");
    assert_eq!(d.caret, d.source.find("after").unwrap());
    // At the start of a paragraph too, the rule landing above it.
    let mut d = wysiwyg_doc("hr_parted_start", "prev\n\npara\n");
    d.caret = 6;
    d.insert_thematic_break();
    assert_eq!(d.caret, d.source.find("para").unwrap());
}

#[test]
fn the_line_under_a_new_rule_is_one_undo_step_with_it() {
    let mut d = wysiwyg_doc("hr_undo", "a\n\nb\n");
    d.caret = 1;
    d.insert_thematic_break();
    assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
    d.undo();
    assert_eq!(d.source, "a\n\nb\n");
    assert!(!d.can_undo());
    d.redo();
    assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
}

#[test]
fn a_rule_in_preserve_flow_leaves_the_caret_on_the_first_line_under_it() {
    // Preserve draws every blank line as somewhere to type, the first under
    // the rule included, so that is the caret's line and the one written.
    let mut d = wysiwyg_doc("hr_preserve", "para\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = 4;
    d.insert_thematic_break();
    assert_eq!(d.source, "para\n\n---\n\n");
    assert_eq!(d.caret, "para\n\n---\n".len());
    d.build_visual(80);
    let rule = d.vmap.pos_of_offset(d.source.find("---").unwrap()).0;
    assert_eq!(d.vmap.pos_of_offset(d.caret), (rule + 1, 0));
    // Before another block, one blank line more keeps it off that block.
    let mut d = wysiwyg_doc("hr_preserve_mid", "a\n\nb\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = 1;
    d.insert_thematic_break();
    assert_eq!(d.source, "a\n\n---\n\n\nb\n");
    assert_eq!(d.caret, "a\n\n---\n".len());
}

#[test]
fn insert_page_break_leaves_the_caret_on_a_line_under_it() {
    // The rule button's placement, for the other block the toolbar writes:
    // the caret was left at the end of twig's splice, beside the break at
    // the end of the document and on the next block's first letter before
    // one. djot spells the break as a fence of two lines; the line under
    // it is under the closing `:::`.
    for (fmt, body, want, typed) in [
        (
            Format::Markdown,
            "a\n\nb\n",
            "a\n\n::page-break\n\n\n\nb\n",
            "a\n\n::page-break\n\nX\n\nb\n",
        ),
        (
            Format::Markdown,
            "a\n",
            "a\n\n::page-break\n\n",
            "a\n\n::page-break\n\nX",
        ),
        (
            Format::Djot,
            "a\n\nb\n",
            "a\n\n::: page-break\n:::\n\n\n\nb\n",
            "a\n\n::: page-break\n:::\n\nX\n\nb\n",
        ),
    ] {
        let mut d = Doc::from_source(body.into(), fmt).unwrap();
        d.view = View::Wysiwyg;
        d.caret = 1;
        d.insert_page_break();
        assert_eq!(d.source, want, "{fmt:?}");
        d.build_visual(80);
        let (row, col) = d.vmap.pos_of_offset(d.caret);
        assert_eq!(col, 0);
        assert!(d.vmap.rows[row].glyphs.is_empty(), "{fmt:?}: an empty line");
        assert!(
            d.vmap.rows[row - 2].leaf_directive.is_some(),
            "{fmt:?}: under the break"
        );
        d.insert("X");
        assert_eq!(d.source, typed, "{fmt:?}");
        d.undo();
        d.undo();
        assert_eq!(
            d.source, body,
            "{fmt:?}: the break and its line are one step"
        );
    }
}

#[test]
fn a_block_parting_a_paragraph_is_one_undo_step() {
    // The split went to twig without leaf counting it, so the first Undo
    // took back the block, left the paragraph parted, and reported nothing
    // more to undo.
    let mut rule = wysiwyg_doc("part_undo_rule", "before after\n");
    rule.caret = 7;
    rule.insert_thematic_break();
    let mut table = wysiwyg_doc("part_undo_table", "before after\n");
    table.caret = 7;
    table.insert_table(1, 1);
    let mut page = wysiwyg_doc("part_undo_page", "before after\n");
    page.caret = 7;
    page.insert_page_break();
    for (name, d) in [
        ("rule", &mut rule),
        ("table", &mut table),
        ("page break", &mut page),
    ] {
        assert_ne!(d.source, "before after\n", "{name}");
        d.undo();
        assert_eq!(d.source, "before after\n", "{name}: one Undo");
        assert_eq!(d.caret, 7, "{name}: the caret back where it was");
        assert!(!d.can_undo(), "{name}");
        assert!(d.can_redo(), "{name}");
        d.redo();
        assert!(!d.source.starts_with("before after"), "{name}: one Redo");
        assert!(!d.can_redo(), "{name}");
    }
    // A table twig would refuse parts nothing: the refusal comes first.
    let mut d = wysiwyg_doc("part_undo_zero", "before after\n");
    d.caret = 7;
    d.insert_table(0, 1);
    assert_eq!(d.source, "before after\n");
    assert!(!d.can_undo());
}
