use super::*;

// ── active inline marks ───────────────────────────────────────────────────

/// The marks at a `|`-marked fixture's caret, in `InlineMarks::iter` order.
fn marks(view: View, name: &str, marked: &str) -> Vec<InlineKind> {
    let (src, caret) = parse_caret(marked);
    let mut d = doc_in(view, name, &src);
    d.caret = caret;
    d.active_inline_marks().iter().collect()
}

/// The marks over the selection `[start, end)`.
fn marks_over(view: View, name: &str, src: &str, start: usize, end: usize) -> Vec<InlineKind> {
    let mut d = doc_in(view, name, src);
    d.anchor = Some(start);
    d.caret = end;
    d.active_inline_marks().iter().collect()
}

#[test]
fn a_caret_in_a_mark_reports_it() {
    for (view, tag) in VIEWS {
        let m = |marked| marks(view, &format!("marks_in_{tag}"), marked);
        assert_eq!(m("a **bo|ld** b"), [InlineKind::Strong], "{tag}");
        assert_eq!(m("a *it|alic* b"), [InlineKind::Emph], "{tag}");
        assert_eq!(m("a `co|de` b"), [InlineKind::Verbatim], "{tag}");
        // Plain text under no mark lights nothing — the toolbar's resting state.
        assert_eq!(m("a| **bold** b"), [], "{tag}");
        assert!(m("plain t|ext").is_empty(), "{tag}");
    }
}

#[test]
fn nested_marks_all_report() {
    // Bold *and* italic: a toolbar lights both buttons, so the set has both —
    // the ancestor chain is a chain, and every mark on it is in force.
    for (view, tag) in VIEWS {
        assert_eq!(
            marks(
                view,
                &format!("marks_nested_{tag}"),
                "**bold and *bo|th*** end"
            ),
            [InlineKind::Strong, InlineKind::Emph],
            "{tag}"
        );
    }
}

#[test]
fn the_caret_at_a_marks_edge_reports_it_where_typing_would_extend_it() {
    // The offsets a WYSIWYG caret actually reaches at a bold run's edges are
    // the first byte of its text and the byte after its last — both inside
    // the mark's span, both places typing lands inside the bold. The offset
    // past the closing delimiter is the next text, and reports nothing.
    let src = "a **bold** b";
    let inner_start = src.find("bold").unwrap(); // 4
    let inner_end = inner_start + "bold".len(); // 8, on the closing `**`
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("marks_edge_{tag}"), src);
        for off in [2, 3, inner_start, inner_end, 9] {
            d.caret = off;
            assert!(
                d.active_inline_marks().contains(InlineKind::Strong),
                "{tag}: offset {off} is inside the strong span"
            );
        }
        for off in [0, 1, 10, 11, 12] {
            d.caret = off;
            assert!(
                !d.active_inline_marks().contains(InlineKind::Strong),
                "{tag}: offset {off} is outside the strong run"
            );
        }
    }
}

#[test]
fn a_mark_ends_the_same_way_at_the_end_of_the_buffer_as_in_the_middle() {
    // Regression: twig resolves an offset that is one node's end and the
    // next one's start to the node that *starts* there, so `**bold**|\n`
    // isn't bold. With nothing following there's no tie to break and the
    // chain still ended at the mark, which made a trailing `\n` — not the
    // text — decide whether the caret after a bold word reported bold. It's
    // the offset past the mark either way, and typing there is plain either
    // way. A blank document typed into is exactly this shape.
    for (view, tag) in VIEWS {
        let m = |name: String, marked| marks(view, &name, marked);
        assert_eq!(
            m(format!("marks_eob_{tag}"), "**bold**|"),
            [],
            "{tag}: no trailing newline"
        );
        assert_eq!(
            m(format!("marks_eol_{tag}"), "**bold**|\n"),
            [],
            "{tag}: with one"
        );
        // And the last offset that *is* in the mark still is.
        assert_eq!(
            m(format!("marks_eob_in_{tag}"), "**bold*|*"),
            [InlineKind::Strong],
            "{tag}"
        );
    }
}

#[test]
fn a_selection_reports_a_mark_only_when_it_covers_the_whole_thing() {
    let src = "a **bold** b";
    let (b, d_) = (src.find("bold").unwrap(), src.find("bold").unwrap() + 4);
    for (view, tag) in VIEWS {
        let m = |s, e| marks_over(view, &format!("marks_sel_{tag}"), src, s, e);
        // The whole bold word, and a slice of it.
        assert_eq!(m(b, d_), [InlineKind::Strong], "{tag}: the whole word");
        assert_eq!(m(b + 1, d_ - 1), [InlineKind::Strong], "{tag}: a slice");
        // Ending exactly at the closing delimiter's start is still all-bold:
        // an exclusive end sits *past* the last selected character, so the
        // question is asked of the character, not the boundary.
        assert_eq!(
            m(b, d_ + 2),
            [InlineKind::Strong],
            "{tag}: through the close"
        );
        // Half in, half out: Bold lit here would claim a press turns it off.
        assert_eq!(m(0, d_), [], "{tag}: leading plain text");
        assert_eq!(m(b, src.len()), [], "{tag}: trailing plain text");
    }
}

#[test]
fn a_selection_across_two_runs_of_the_same_mark_reports_nothing() {
    // Both ends are bold, but the space between them isn't — two runs are two
    // nodes, which is exactly what the node id catches and a kind-only
    // comparison would not.
    let src = "**one** **two**";
    for (view, tag) in VIEWS {
        let m = marks_over(view, &format!("marks_runs_{tag}"), src, 2, 13);
        assert_eq!(m, [], "{tag}: `one** **two` is not all bold");
    }
}

#[test]
fn marks_read_the_document_as_it_is_edited() {
    // The point of asking twig every frame instead of caching: the answer has
    // to follow the toggle that changed it.
    let mut d = wysiwyg_doc("marks_live", "one two\n");
    d.anchor = Some(0);
    d.caret = 3;
    assert!(d.active_inline_marks().is_empty(), "plain to start");
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "**one** two\n");
    // `toggle` leaves the bolded text selected, so the button it lit stays lit.
    assert!(d.active_inline_marks().contains(InlineKind::Strong));
    d.toggle(InlineKind::Strong);
    assert!(d.active_inline_marks().is_empty(), "and off again");
}

#[test]
fn a_link_is_not_an_inline_mark() {
    // `link`/`str` are inline nodes, but nothing on the inline toolbar
    // toggles them — a set with a "link mark" in it would have no button.
    for (view, tag) in VIEWS {
        assert_eq!(
            marks(view, &format!("marks_link_{tag}"), "a [te|xt](u) b"),
            [],
            "{tag}"
        );
    }
}
