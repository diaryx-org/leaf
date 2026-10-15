use super::*;

// ── hidden blocks ────────────────────────────────────────────────────────

#[test]
fn a_div_s_closing_tag_is_not_a_blank_row() {
    // The `</div>` sits on a line of its own under the div's last child and
    // draws nothing. Counting the separator from the child's end read that
    // line as a blank line between the div and the block below — a
    // navigable empty row the author never opened — and at the end of the
    // file, as an empty trailing paragraph.
    let m = map("above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n");
    assert_eq!(row_texts(&m), ["above", "", "hello", "", "below"]);
    assert!(!m.is_stop(36), "the `</div>` line is not a caret home");
    assert_eq!(
        m.stop_after(34),
        Some(44),
        "from `hello` the next stop is `below`"
    );

    let m = map("above\n\n<div class=\"center\">\n\nhello\n\n</div>\n");
    assert_eq!(row_texts(&m), ["above", "", "hello"], "no trailing rows");
}

#[test]
fn a_comment_between_two_blocks_is_stepped_over_not_drawn_as_a_gap() {
    // `<!-- exec -->` is a top-level block that draws no rows. The blocks
    // either side of it meet across the one boundary a paragraph and a code
    // block always meet across — not that boundary *plus* one blank row per
    // line of the comment, which is what counting the separator from the
    // paragraph's end used to spell.
    let m = map("para one\n\n<!-- exec -->\n```\ncode\n```\n\nafter\n");
    assert_eq!(row_texts(&m), ["para one", "", "code", "", "after"]);
    assert_eq!(
        boundaries(&m),
        vec![
            (BlockClass::Paragraph, BlockClass::Code),
            (BlockClass::Code, BlockClass::Paragraph),
        ],
        "the boundary names the drawn blocks either side, not the comment"
    );
    // The gap stands past the comment, so the caret's row lookup never
    // resolves inside it.
    assert_eq!(
        m.rows[1].end_src, 23,
        "the gap row ends at the comment's end"
    );
}

#[test]
fn a_comment_opening_the_document_draws_no_leading_gap() {
    let m = map("<!-- lead -->\n\npara\n");
    assert_eq!(row_texts(&m), ["para"]);
    assert_eq!(m.content_start, 0, "the comment is still the first block");
}

#[test]
fn a_comment_closing_the_document_is_not_trailing_blank_lines() {
    // Its lines are not blank lines the author opened with Enter, so no
    // gap-plus-empty-paragraph is fabricated under the last drawn block.
    let m = map("para\n\n<!-- trail -->\n");
    assert_eq!(row_texts(&m), ["para"]);
    // Enter at the end of the document still opens the empty paragraph the
    // caret rests on: the newlines *after* the comment count as they would
    // after any block.
    let m = map("para\n\n<!-- trail -->\n\n");
    assert_eq!(row_texts(&m), ["para", "", ""]);
}

#[test]
fn a_comment_in_a_list_item_leaves_the_bullet_to_what_follows_it() {
    // The first *drawn* child wears the item's marker; a hidden first child
    // would otherwise take it and leave the text without one.
    let m = map("- <!-- note -->\n\n  text\n- two\n");
    let texts = row_texts(&m);
    assert!(
        texts.iter().any(|t| t == "• text"),
        "the text wears the bullet: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t == "• "),
        "no empty bullet row for the comment: {texts:?}"
    );
}

#[test]
fn the_cached_build_does_not_spell_the_document_out_as_blank_rows_after_a_comment() {
    // The bug as seen: a 200-line document with one comment in it rendered
    // ~200 blank rows after the comment, one per source line, because the
    // comment's per-block builder handed back a `last_off` of 0. Parity with
    // `build` alone would not catch a *shared* wrong answer, so the count is
    // pinned outright.
    let body = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let src = format!("intro\n\n<!-- exec -->\n{body}\n");
    let mut ed = Editor::new_str(&src, Format::Markdown).unwrap();
    let mut cache = BlockCache::default();
    let (plain, cached) = render_both(&mut ed, &src, Some(80), &mut cache);
    assert_maps_eq(&plain, &cached, "comment then 200 paragraphs");
    // intro, then 200 × (gap, paragraph): 401 rows and not a row more.
    assert_eq!(cached.rows.len(), 401);
}

#[test]
fn a_link_reference_definition_is_stepped_over_like_a_comment() {
    // `[a]: /a` is a root beside `doc` with no rows of its own. Merged into
    // the walk it is a hidden block: the blocks either side meet across one
    // boundary, and its line is not a blank row.
    let m = map("see [a]\n\n[a]: /a\n\nafter\n");
    assert_eq!(row_texts(&m), ["see a", "", "after"]);
    assert_eq!(
        boundaries(&m),
        vec![(BlockClass::Paragraph, BlockClass::Paragraph)]
    );
}

#[test]
fn link_reference_definitions_closing_the_document_are_not_trailing_blank_lines() {
    // The README shape: prose, then a `[links]` block nobody reads. Its
    // lines used to be counted as blank ones, an empty paragraph per
    // definition under the last real block.
    let m = map("see [a] and [b]\n\n<!-- links -->\n[a]: /a\n[b]: /b \"bee\"\n");
    assert_eq!(row_texts(&m), ["see a and b"]);
}

#[test]
fn a_definition_glued_under_a_paragraph_stays_inside_it() {
    // `[a]: /a` at the front of a paragraph's lines is stripped from the
    // paragraph's text, but the paragraph's span still starts on its line.
    // Both blocks start at the same offset; the definition, sorted first,
    // is stepped over, and the paragraph draws as it always did — one gap
    // above it, none inside.
    let m = map("intro\n\n[a]: /a\ntext [a]\n");
    assert_eq!(row_texts(&m), ["intro", "", "text a"]);
}

#[test]
fn a_definition_with_no_span_is_left_out_of_the_walk() {
    // twig before 3.3.3 reported `0..0` for every link reference
    // definition. One of those has nowhere to be merged: sorted first by
    // its zero start it would open the document with a phantom block, and
    // the walk would step back to offset 0. It is simply not a block. A
    // footnote definition is always placed; it has a body to draw.
    assert!(!is_placed_definition(&Kind::Reference, &(0..0)));
    assert!(is_placed_definition(&Kind::Reference, &(7..14)));
    assert!(is_placed_definition(&Kind::Footnote, &(0..0)));
    assert!(!is_placed_definition(&Kind::Str, &(7..14)));
}

#[test]
fn the_trailing_gap_closes_the_last_block() {
    // Two Enters at the end of a document: a drawn gap, then the navigable
    // empty paragraph. Only the gap is labelled, so a frontend that shrinks
    // boundaries shrinks the spacer and leaves the row being typed on alone.
    let m = map("# Head\n\n\n");
    assert_eq!(
        boundaries(&m),
        vec![(BlockClass::Heading, BlockClass::Paragraph)]
    );
}

#[test]
fn only_the_drawn_gap_rows_carry_a_boundary() {
    let m = map("one\n\ntwo\n");
    for row in &m.rows {
        assert_eq!(
            row.boundary.is_some(),
            row.decoration,
            "a boundary is exactly a drawn gap row: {:?}",
            row.glyphs.iter().map(|g| g.ch).collect::<String>()
        );
    }
}

#[test]
fn preserve_flow_labels_no_boundary() {
    // Every blank line is a caret home there — somewhere text can go, not a
    // gap between blocks — so nothing is drawn-only and nothing is labelled.
    // A frontend keying its spacing off `boundary` can't shrink a row the
    // author is about to type on.
    let m = map_preserve("one\n\ntwo\n\n# Head\n", Some(80));
    assert!(boundaries(&m).is_empty());
}

#[test]
fn a_list_draws_no_boundary_between_its_items() {
    // Tight or loose, core puts no gap row between two items of one list —
    // so an item↔item boundary is a shape no frontend will ever be handed,
    // and spacing one is spacing something that isn't there.
    for src in ["- one\n- two\n", "- one\n\n- two\n"] {
        let m = map(src);
        assert!(
            boundaries(&m).is_empty(),
            "no gap row inside the list of {src:?}"
        );
    }
    // Leaving the list is an ordinary boundary, and the list is named as
    // what sits above it.
    let m = map("- one\n- two\n\npara\n");
    assert_eq!(
        boundaries(&m),
        vec![(BlockClass::List, BlockClass::Paragraph)]
    );
}

#[test]
fn a_nested_boundary_names_the_blocks_inside_the_container() {
    // Two paragraphs inside a blockquote are divided by a Paragraph↔Paragraph
    // boundary — the quote is the container they're both in, not what the gap
    // separates.
    let m = map("> one\n>\n> two\n");
    assert_eq!(
        boundaries(&m),
        vec![(BlockClass::Paragraph, BlockClass::Paragraph)]
    );
}

#[test]
fn a_directive_container_draws_one_boundary_like_every_other_block() {
    // A container's rows stop at its last *child*, so without anchoring
    // `last_off` past the closing `:::` the separator logic counted the fence
    // line as a blank row of its own and drew the gap twice — one authored
    // blank line, two boundaries, and a frontend spacing each of them put
    // double margin under every fenced div. The code-block arm anchors past
    // its ``` for exactly this reason; compare the two here.
    let fenced = map_directives(":::note\nin\n:::\n\ntwo\n");
    assert_eq!(
        boundaries(&fenced),
        vec![(BlockClass::Directive, BlockClass::Paragraph)],
        "one authored gap, one boundary row"
    );
    let code = map("```\nc\n```\n\ntwo\n");
    assert_eq!(
        boundaries(&code).len(),
        boundaries(&fenced).len(),
        "a fenced div spaces like a fenced code block"
    );
    // Nesting closes several fences at once; still one gap.
    let nested = map_directives(":::a\n:::b\nin\n:::\n:::\n\ntwo\n");
    assert_eq!(
        boundaries(&nested),
        vec![(BlockClass::Directive, BlockClass::Paragraph)]
    );
}

#[test]
fn a_block_media_names_itself_in_the_boundaries_either_side() {
    use BlockClass::*;
    // A block image is never a node of its own — `media_only` promotes the
    // *paragraph* wrapping it — so classifying the node the walk stands on
    // called the picture `Paragraph` and left `BlockClass::Media` unreachable:
    // a frontend could not give a photo more air than a line of prose.
    // `label_media_boundaries` reads it back off the finished rows instead.
    let m = map("one\n\n![alt](p.png)\n\ntwo\n");
    assert_eq!(boundaries(&m), vec![(Paragraph, Media), (Media, Paragraph)]);
    // At the edges of the document too: the leading gap has no boundary of
    // its own, and the trailing one is `emit_trailing_blank_lines`'.
    let edges = map("![a](p.png)\n\nmid\n\n![b](q.png)\n");
    assert_eq!(
        boundaries(&edges),
        vec![(Media, Paragraph), (Paragraph, Media)]
    );
    // One gap spelled with several rows — the row closing the block above and
    // the row opening the one below, with the author's spare blank line
    // navigable between them. That line is an empty paragraph, so only the
    // row opening the picture borders it; the other borders the empty line,
    // and is spaced as it will be once the line has text on it.
    let roomy = map("one\n\n\n\n![alt](p.png)\n");
    assert_eq!(
        boundaries(&roomy),
        vec![(Paragraph, Paragraph), (Paragraph, Media)]
    );
}

#[test]
fn a_block_video_is_media_at_its_boundaries_not_a_directive_panel() {
    // Worse than the image case before `label_media_boundaries`: a `<video>`
    // arrives as twig's generic `container`, which classifies `Directive` —
    // the one class a frontend reads as "draw a tinted panel here". A movie
    // got the chrome of a fenced div.
    let mut doc = crate::Doc::from_source(
        "one\n\n<video src=\"v.mp4\"></video>\n\ntwo\n".to_string(),
        Format::Markdown,
    )
    .unwrap();
    doc.build_visual(80);
    assert_eq!(
        boundaries(&doc.vmap),
        vec![
            (BlockClass::Paragraph, BlockClass::Media),
            (BlockClass::Media, BlockClass::Paragraph),
        ]
    );
}

#[test]
fn the_incremental_walk_labels_boundaries_like_the_full_one() {
    // `assert_maps_eq` compares boundaries too, so this pins the two doors
    // into `BlockClass::from_node_kind` — a `FlatNode`'s kind on the full
    // build, a query match's on the cached one — against a document with one
    // of every boundary in it.
    let src = "one\n\n# Head\n\ntwo\n\n- a\n- b\n\n> q\n\n```\nc\n```\n\npara\n";
    let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
    let mut cache = BlockCache::default();
    let (full, cached) = render_both(&mut ed, src, Some(80), &mut cache);
    assert_maps_eq(&full, &cached, "boundary labelling");
    assert!(
        !boundaries(&full).is_empty(),
        "the fixture has boundaries to compare"
    );
}

#[test]
fn every_caret_stop_opens_a_cluster_of_its_row() {
    // The two ways of finding a cluster have to agree. `push_text` marks the
    // stops by segmenting one run of text; the column mapping segments the
    // whole row, decoration and all. A stop that came out as the *middle* of
    // some row-level cluster would be a caret with no column of its own —
    // drawn at the column of whatever swallowed it.
    let src = "# 標題\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` 你好\n\n\
                   - 項目 one\n- e\u{0301}dge\n\n> 引用 text\n\n\
                   | A | 值 |\n|---|---|\n| 你好 | 👩‍🚀 |\n";
    let m = map(src);
    for (r, row) in m.rows.iter().enumerate() {
        let openers: Vec<usize> = clusters(&row.glyphs).iter().map(|c| c.glyph).collect();
        for (i, g) in row.glyphs.iter().enumerate() {
            assert!(
                !g.stop || openers.contains(&i),
                "row {r}: the stop at glyph {i} ({:?}) is inside a cluster, \
                     so it is drawn at another glyph's column",
                g.ch
            );
        }
    }
}
