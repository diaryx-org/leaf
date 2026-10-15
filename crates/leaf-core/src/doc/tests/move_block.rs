use super::*;

// ── move_block ─────────────────────────────────────────────────────────────

/// A move, then its undo: one step takes the whole thing back.
fn moved(name: &str, body: &str, from: usize, to: usize) -> (String, Doc) {
    let mut d = doc_with(name, body);
    d.caret = from;
    let steps = d.undo_steps;
    d.move_block(from, to);
    let after = d.source.clone();
    assert_eq!(d.undo_steps, steps + 1, "one undo step");
    assert!(d.dirty);
    d.undo();
    assert_eq!(d.source, body, "one undo restores the original");
    (after, d)
}

#[test]
fn move_block_carries_a_paragraph_between_two_paragraphs_and_to_the_end() {
    let body = "a\n\nb\n\nc\n";
    assert_eq!(moved("mv_p1", body, 6, 3).0, "a\n\nc\n\nb\n");
    assert_eq!(moved("mv_p2", body, 0, body.len()).0, "b\n\nc\n\na\n");
    assert_eq!(moved("mv_p3", body, 3, 0).0, "b\n\na\n\nc\n");
}

#[test]
fn move_block_carries_an_image_block() {
    let body = "a\n\n![p](x.png)\n\nc\n";
    assert_eq!(moved("mv_img1", body, 4, 0).0, "![p](x.png)\n\na\n\nc\n");
    assert_eq!(
        moved("mv_img2", body, 4, body.len()).0,
        "a\n\nc\n\n![p](x.png)\n"
    );
}

#[test]
fn move_block_carries_a_table() {
    let body = "a\n\n| h |\n|---|\n| c |\n\nc\n";
    assert_eq!(
        moved("mv_tbl1", body, 5, 0).0,
        "| h |\n|---|\n| c |\n\na\n\nc\n"
    );
    assert_eq!(
        moved("mv_tbl2", body, 5, body.len()).0,
        "a\n\nc\n\n| h |\n|---|\n| c |\n"
    );
}

#[test]
fn move_block_carries_a_code_block() {
    let body = "a\n\n```rs\nx\n```\n\nc\n";
    assert_eq!(moved("mv_code1", body, 8, 0).0, "```rs\nx\n```\n\na\n\nc\n");
    assert_eq!(
        moved("mv_code2", body, 8, body.len()).0,
        "a\n\nc\n\n```rs\nx\n```\n"
    );
}

#[test]
fn move_block_onto_its_own_boundary_is_a_quiet_no_op() {
    let mut d = doc_with("mv_noop", "a\n\nb\n");
    let steps = d.undo_steps;
    d.move_block(0, 0);
    d.move_block(0, 3);
    assert_eq!(d.source, "a\n\nb\n");
    assert_eq!(d.undo_steps, steps);
    assert_eq!(d.status, None);
    assert!(!d.dirty);
}

#[test]
fn move_block_into_a_fence_is_refused_with_a_status() {
    let mut d = doc_with("mv_fence", "a\n\n```\nx\ny\n```\n");
    d.move_block(0, 7);
    assert_eq!(d.source, "a\n\n```\nx\ny\n```\n");
    assert!(
        d.status
            .as_deref()
            .is_some_and(|s| s.starts_with("move block"))
    );
}

#[test]
fn move_block_rides_the_caret_with_the_block() {
    // Down: "second" is line 0 of its block, caret 3 bytes from its end.
    let mut d = doc_with("mv_caret1", "first\n\nsecond\n\nthird\n");
    d.caret = 10; // "sec|ond"
    d.move_block(10, d.source.len());
    assert_eq!(d.source, "first\n\nthird\n\nsecond\n");
    assert_eq!(&d.source[d.caret..], "ond\n");
    // Up, across a wrapped paragraph's second line.
    let mut d = doc_with("mv_caret2", "first\n\nsecond\nline two\n");
    d.caret = 16; // "li|ne two"
    d.move_block(16, 0);
    assert_eq!(d.source, "second\nline two\n\nfirst\n");
    assert_eq!(&d.source[d.caret..], "ne two\n\nfirst\n");
    assert_eq!(d.selection(), None);
}

#[test]
fn move_block_keeps_the_caret_on_its_line_through_a_quotes_prefix() {
    let mut d = doc_with("mv_quote_caret", "para\n\n> a\n");
    d.caret = 2; // "pa|ra"
    d.move_block(2, 9); // after a, inside the quote
    assert_eq!(d.source, "> a\n>\n> para\n");
    assert_eq!(&d.source[d.caret..], "ra\n");
}

#[test]
fn move_block_up_and_down_step_over_siblings() {
    let mut d = doc_with("mv_updown", "a\n\nb\n\nc\n");
    d.caret = 3;
    d.move_block_down();
    assert_eq!(d.source, "a\n\nc\n\nb\n");
    assert_eq!(&d.source[d.caret..], "b\n");
    d.move_block_down();
    assert_eq!(d.source, "a\n\nc\n\nb\n", "nothing below the last block");
    assert_eq!(d.status.as_deref(), Some("move block: nothing below"));
    d.move_block_up();
    d.move_block_up();
    assert_eq!(d.source, "b\n\na\n\nc\n");
    assert_eq!(&d.source[d.caret..], "b\n\na\n\nc\n");
    d.move_block_up();
    assert_eq!(d.status.as_deref(), Some("move block: nothing above"));
}

#[test]
fn move_block_up_and_down_reorder_list_items_with_their_children() {
    let mut d = doc_with("mv_items", "- a\n  - x\n- b\n- c\n");
    d.caret = 12; // in "b"
    d.move_block_up();
    assert_eq!(d.source, "- b\n- a\n  - x\n- c\n");
    assert_eq!(&d.source[d.caret..], "b\n- a\n  - x\n- c\n");
    d.caret = 6; // in "a"
    d.move_block_down();
    assert_eq!(d.source, "- b\n- c\n- a\n  - x\n");
    assert_eq!(&d.source[d.caret..], "a\n  - x\n");
    // A nested item leaves its list upward, as an item of the outer one.
    d.caret = 16; // in "x"
    d.move_block_up();
    assert_eq!(d.source, "- b\n- c\n- x\n- a\n");
}

#[test]
fn move_block_on_a_lone_item_that_stays_a_bullet_is_no_step() {
    let mut d = doc_with("mv_lone_item", "x\n\n- b\n\ny\n");
    d.caret = 5;
    let steps = d.undo_steps;
    d.move_block_up();
    assert_eq!(d.source, "x\n\n- b\n\ny\n");
    assert_eq!(
        d.undo_steps, steps,
        "a move that rewrote nothing is no undo step"
    );
    assert_eq!(d.status.as_deref(), Some("move block: nothing above"));
}

#[test]
fn move_block_up_leaves_a_container_at_its_first_block_and_down_at_its_last() {
    let mut d = doc_with("mv_leave", "x\n\n> a\n>\n> b\n\ny\n");
    d.caret = 5; // "a"
    d.move_block_up();
    assert_eq!(d.source, "x\n\na\n\n> b\n\ny\n");
    d.caret = 8; // "b"
    d.move_block_down();
    assert_eq!(d.source, "x\n\na\n\nb\n\ny\n");
    // A tail block leaves its item into the next item's tail, then the list.
    let mut d = doc_with("mv_tail", "- a\n\n  t\n- b\n");
    d.caret = 7;
    d.move_block_down();
    assert_eq!(d.source, "- a\n- b\n\n  t\n");
    d.move_block_down();
    assert_eq!(d.source, "- a\n- b\n\nt\n");
    // And a paragraph after a quote steps over the whole quote, not into it.
    let mut d = doc_with("mv_over", "x\n\n> a\n>\n> b\n\ny\n");
    d.caret = 14;
    d.move_block_up();
    assert_eq!(d.source, "x\n\ny\n\n> a\n>\n> b\n");
    assert_eq!(d.caret, 3);
    d.move_block_down();
    assert_eq!(d.status, None);
    assert_eq!(d.source, "x\n\n> a\n>\n> b\n\ny\n");
    // Above a quote that opens the document is offset 0 — before the
    // quote, not inside it.
    let mut d = doc_with("mv_over_top", "> a\n\ny\n");
    d.caret = 5;
    d.move_block_up();
    assert_eq!(d.source, "y\n\n> a\n");
    assert_eq!(d.caret, 0);
}

#[test]
fn move_block_up_from_the_first_block_of_a_quote_that_opens_the_document_leaves_it() {
    let mut d = doc_with("mv_top_quote", "> a\n>\n> b\n");
    d.caret = 2;
    d.move_block_up();
    assert_eq!(d.source, "a\n\n> b\n");
    assert_eq!(d.caret, 0);
    assert_eq!(d.status, None);
}

#[test]
fn move_block_on_a_blank_line_or_read_only_does_nothing() {
    let mut d = doc_with("mv_blank", "a\n\nb\n");
    d.caret = 2;
    d.move_block_down();
    assert_eq!(d.source, "a\n\nb\n");
    assert_eq!(d.status.as_deref(), Some("move block: no block here"));
    d.read_only = true;
    d.caret = 0;
    d.move_block_down();
    d.move_block(0, 5);
    assert_eq!(d.source, "a\n\nb\n");
}

#[test]
fn move_block_never_lands_above_hidden_frontmatter() {
    let mut d = wysiwyg_doc("mv_fm", "---\nt: x\n---\n\na\n\nb\n");
    let b = d.source.find('b').unwrap();
    d.move_block(b, 0);
    assert_eq!(d.source, "---\nt: x\n---\n\nb\n\na\n");
}

#[test]
fn block_range_at_is_the_block_a_move_picks_up() {
    let mut d = doc_with("mv_range", "a\n\n- b\n  - c\n\nd\n");
    assert_eq!(d.block_range_at(0), Some(0..1));
    assert_eq!(d.block_range_at(5), Some(3..12), "the item with its child");
    assert_eq!(d.block_range_at(10), Some(7..12), "the nested item alone");
    assert_eq!(d.block_range_at(2), None, "a blank line");
}

#[test]
fn drop_target_at_splits_a_block_at_its_middle_row_and_ends_below_everything() {
    let long = "two ".repeat(40).trim_end().to_string(); // wraps to three rows at 80
    let mut d = wysiwyg_doc("drop", &format!("one\n\n{long} four\n\nfive\n"));
    let rows = d.vmap.rows.len();
    assert_eq!(rows, 7, "one, gap, three wrapped rows, gap, five");
    assert_eq!(d.drop_target_at(0), Some(DropTarget { offset: 0, row: 0 }));
    let two = d.source.find("two").unwrap();
    assert_eq!(
        d.drop_target_at(1),
        Some(DropTarget {
            offset: two,
            row: 2
        }),
        "the gap resolves to the block under it"
    );
    assert_eq!(
        d.drop_target_at(2),
        Some(DropTarget {
            offset: two,
            row: 2
        })
    );
    assert_eq!(
        d.drop_target_at(3),
        Some(DropTarget {
            offset: two,
            row: 2
        })
    );
    let four_end = d.source.find("four").unwrap() + 4;
    assert_eq!(
        d.drop_target_at(4),
        Some(DropTarget {
            offset: four_end,
            row: 5
        })
    );
    assert_eq!(
        d.drop_target_at(rows + 3),
        Some(DropTarget {
            offset: d.source.len(),
            row: rows
        })
    );
    // And the offsets are ones move_block takes.
    let five = d.source.find("five").unwrap();
    let t = d.drop_target_at(2).unwrap();
    d.move_block(five, t.offset);
    assert_eq!(d.source, format!("one\n\nfive\n\n{long} four\n"));
}

#[test]
fn append_media_lands_at_the_end_as_its_own_block() {
    let mut d = doc_with("append_mid", "first word and more\n");
    d.caret = 0; // an editor nobody has tapped: the caret is at the start
    d.append_media(MediaKind::Image, "cat.png", "");
    assert_eq!(d.source, "first word and more\n\n![](cat.png)");
    assert_eq!(d.selection(), None);
    assert_eq!(d.caret, "first word and more\n\n![](cat.png)".len());
}

#[test]
fn append_media_needs_no_separator_after_a_blank_line_or_in_an_empty_document() {
    let mut d = doc_with("append_blank", "para\n\n");
    d.append_media(MediaKind::Image, "a.png", "");
    assert_eq!(d.source, "para\n\n![](a.png)");

    let mut e = doc_with("append_empty", "");
    e.append_media(MediaKind::Image, "b.png", "");
    assert_eq!(e.source, "![](b.png)");

    let mut f = doc_with("append_noeol", "no newline at end");
    f.anchor = Some(0);
    f.caret = 2; // a selection, which the verb ignores
    f.append_media(MediaKind::Video, "clip.mp4", "");
    assert_eq!(
        f.source,
        "no newline at end\n\n<video src=\"clip.mp4\" controls></video>"
    );
}

#[test]
fn insert_media_spells_a_video_as_html_and_reads_it_back_as_a_block() {
    // The round trip is the point: it's no use writing markup the reader
    // can't pick up again. This is the pair that only holds from twig 2.5.1
    // on — before it, the one-line form went in fine and came back as a
    // paragraph of raw tags, publishing no media at all.
    let mut d = doc_with("vid_rt", "\n");
    d.caret = 0;
    d.insert_media(MediaKind::Video, "clip.mp4", "a clip");
    assert_eq!(
        d.source,
        "<video src=\"clip.mp4\" controls>a clip</video>\n"
    );

    d.build_visual(80);
    assert_eq!(d.vmap.media.len(), 1, "reads back as one block media");
    assert_eq!(d.vmap.media[0].kind, MediaKind::Video);
    assert_eq!(d.vmap.media[0].destination, "clip.mp4");
    assert_eq!(d.vmap.media[0].alt, "a clip");
}

#[test]
fn insert_media_spells_audio_with_its_own_tag() {
    let mut d = doc_with("aud_rt", "\n");
    d.caret = 0;
    d.insert_media(MediaKind::Audio, "take.mp3", "");
    assert_eq!(d.source, "<audio src=\"take.mp3\" controls></audio>\n");
    d.build_visual(80);
    assert_eq!(d.vmap.media[0].kind, MediaKind::Audio);
}

#[test]
fn insert_media_uses_the_selection_as_fallback_text() {
    // The same courtesy `insert_image` does with alt: select a caption,
    // insert, and the caption labels the thing rather than being replaced.
    let mut d = doc_with("vid_sel", "the talk here\n");
    d.anchor = Some(0);
    d.caret = 8; // "the talk"
    d.insert_media(MediaKind::Video, "talk.mp4", "ignored fallback");
    assert_eq!(
        d.source,
        "<video src=\"talk.mp4\" controls>the talk</video> here\n"
    );
}

#[test]
fn insert_media_with_an_image_kind_is_just_insert_image() {
    let mut d = doc_with("img_via_media", "\n");
    d.caret = 0;
    d.insert_media(MediaKind::Image, "logo.svg", "x");
    assert_eq!(d.source, "![x](logo.svg)\n");
}
