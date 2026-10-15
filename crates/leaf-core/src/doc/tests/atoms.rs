use super::*;

// ── typing against a block picture ────────────────────────────────────────

/// A rendered-view document with the caret parked on one of the picture's two
/// stops, and the map already built — the state a frontend is in between
/// drawing a frame and the next keystroke.
fn doc_at_picture(name: &str, src: &str, side: MediaStop) -> Doc {
    let mut d = doc_in(View::Wysiwyg, name, src);
    d.build_visual_unwrapped();
    let start = src.find("![").unwrap();
    d.caret = match side {
        MediaStop::Before => start,
        MediaStop::After => start + "![](p.png)".len(),
    };
    d
}

/// The block media the map publishes, after rebuilding it — "is this still a
/// picture, or has it become a line of text with an image in it?"
fn media_count(d: &mut Doc) -> usize {
    d.build_visual_unwrapped();
    d.vmap.media.len()
}

#[test]
fn typing_past_a_block_picture_opens_a_paragraph_under_it() {
    // The accident this prevents: tap the blank page under a photo (which
    // lands on the picture's trailing stop), type, and `![](p.png)xy` is a
    // paragraph with an *inline* image — the photo stops being drawn.
    let mut d = doc_at_picture("pic_after", "hi\n\n![](p.png)\n", MediaStop::After);
    d.insert("xy");
    assert_eq!(d.source, "hi\n\n![](p.png)\n\nxy\n");
    assert_eq!(media_count(&mut d), 1, "still a picture");
}

#[test]
fn typing_in_front_of_a_block_picture_opens_a_paragraph_above_it() {
    let mut d = doc_at_picture("pic_before", "hi\n\n![](p.png)\n", MediaStop::Before);
    d.insert("xy");
    assert_eq!(d.source, "hi\n\nxy\n\n![](p.png)\n");
    assert_eq!(media_count(&mut d), 1);
}

#[test]
fn a_picture_that_opens_the_document_still_takes_a_paragraph_above_it() {
    let mut d = doc_at_picture("pic_first", "![](p.png)\n", MediaStop::Before);
    d.insert("x");
    assert_eq!(d.source, "x\n\n![](p.png)\n");
    assert_eq!(media_count(&mut d), 1);
}

#[test]
fn one_undo_puts_the_picture_back_the_way_it_was_found() {
    // The opened paragraph is part of the keystroke, not an edit the writer
    // made — so it undoes with the character, not a step later.
    let mut d = doc_at_picture("pic_undo", "hi\n\n![](p.png)\n", MediaStop::After);
    d.insert("x");
    assert_eq!(d.source, "hi\n\n![](p.png)\n\nx\n");
    d.undo();
    assert_eq!(d.source, "hi\n\n![](p.png)\n");
}

#[test]
fn pasting_against_a_block_picture_opens_a_paragraph_too() {
    // ⌘V dissolves the picture exactly as a keystroke does.
    let mut d = doc_at_picture("pic_paste", "hi\n\n![](p.png)\n", MediaStop::After);
    d.paste("pasted");
    assert_eq!(d.source, "hi\n\n![](p.png)\n\npasted\n");
    assert_eq!(media_count(&mut d), 1);
}

#[test]
fn typing_beside_an_inline_image_is_ordinary_editing() {
    // An inline image has no placeholder row and no stops of its own. Opening
    // a paragraph mid-sentence would be the bug, not the fix.
    let mut d = doc_in(View::Wysiwyg, "pic_inline", "see ![](p.png) here\n");
    d.build_visual_unwrapped();
    d.caret = "see ![](p.png)".len();
    d.insert("!");
    assert_eq!(d.source, "see ![](p.png)! here\n");
}

#[test]
fn source_view_types_raw_markup_against_an_image_untouched() {
    // Source view is for writing the markup itself; a break inserted behind
    // the writer's back there would be the editor arguing with them.
    let mut d = doc_in(View::Source, "pic_src", "![](p.png)\n");
    d.caret = "![](p.png)".len();
    d.insert("x");
    assert_eq!(d.source, "![](p.png)x\n");
}

#[test]
fn typing_over_a_selection_that_starts_at_a_picture_stop_replaces_it() {
    // A selection is replaced, not joined into, so there is nothing to
    // protect: the range takes the picture with it.
    let mut d = doc_at_picture("pic_sel", "hi\n\n![](p.png)\n", MediaStop::Before);
    d.anchor = Some(d.caret);
    d.caret = d.source.find("![").unwrap() + "![](p.png)".len();
    d.insert("x");
    assert_eq!(d.source, "hi\n\nx\n");
}

#[test]
fn backspace_past_a_block_picture_deletes_the_picture_not_its_last_byte() {
    // What this actually cost: a real vault's photo, to one stray Backspace.
    // The caret past `![](p.png)` was deleting the closing paren — invisible
    // in the rendered view — and the photo became the text `![](p.png`.
    let mut d = doc_at_picture("pic_bs", "hi\n\n![](p.png)\n", MediaStop::After);
    d.backspace();
    assert_eq!(d.source, "hi\n");
    assert_eq!(media_count(&mut d), 0, "the picture went, in one piece");
    d.undo();
    assert_eq!(
        d.source, "hi\n\n![](p.png)\n",
        "and comes back in one piece"
    );
}

#[test]
fn backspace_in_front_of_a_block_picture_steps_out_instead_of_merging_it() {
    // Deleting the break here would join the picture to the paragraph above,
    // where it is an *inline* image and stops being drawn. Step over the
    // boundary; the next press deletes in the paragraph the caret reached.
    let mut d = doc_at_picture("pic_bs_before", "hi\n\n![](p.png)\n", MediaStop::Before);
    d.backspace();
    assert_eq!(d.source, "hi\n\n![](p.png)\n", "nothing deleted");
    assert_eq!(d.caret, 2, "the caret stepped up to the end of `hi`");
    d.backspace();
    assert_eq!(d.source, "h\n\n![](p.png)\n", "and now it deletes there");
    assert_eq!(media_count(&mut d), 1, "the picture was never at risk");
}

#[test]
fn forward_delete_in_front_of_a_block_picture_deletes_the_picture() {
    // The mirror. A byte-step here eats the `!` and leaves a link.
    let mut d = doc_at_picture("pic_del", "hi\n\n![](p.png)\n\nbye\n", MediaStop::Before);
    d.delete_forward();
    assert_eq!(d.source, "hi\n\nbye\n");
    assert_eq!(media_count(&mut d), 0);
}

#[test]
fn forward_delete_past_a_block_picture_steps_over_the_boundary() {
    let mut d = doc_at_picture(
        "pic_del_after",
        "hi\n\n![](p.png)\n\nbye\n",
        MediaStop::After,
    );
    d.delete_forward();
    assert_eq!(d.source, "hi\n\n![](p.png)\n\nbye\n", "nothing deleted");
    assert_eq!(
        d.caret,
        d.source.find("bye").unwrap(),
        "the caret stepped down to `bye`"
    );
}

#[test]
fn a_picture_that_is_the_whole_document_still_deletes_cleanly() {
    let mut d = doc_at_picture("pic_only", "![](p.png)\n", MediaStop::After);
    d.backspace();
    assert_eq!(d.source, "\n");
    assert_eq!(media_count(&mut d), 0);
}

#[test]
fn a_word_delete_takes_the_picture_whole_or_steps_out_of_it() {
    // ⌥⌫ past a picture would otherwise eat a "word" of its markup.
    let mut d = doc_at_picture("pic_wordbs", "hi there\n\n![](p.png)\n", MediaStop::After);
    d.delete_word_back();
    assert_eq!(d.source, "hi there\n");

    // And in front of one it runs *through* the paragraph break into the
    // prose above, which merges the picture inline — so it steps out first,
    // and the second press deletes the word it was aimed at.
    let mut d = doc_at_picture("pic_wordbs2", "hi there\n\n![](p.png)\n", MediaStop::Before);
    d.delete_word_back();
    assert_eq!(d.source, "hi there\n\n![](p.png)\n");
    d.delete_word_back();
    assert_eq!(
        d.source, "hi \n\n![](p.png)\n",
        "the word above went, the picture stayed"
    );
    assert_eq!(media_count(&mut d), 1);
}

#[test]
fn source_view_deletes_raw_markup_against_an_image_untouched() {
    let mut d = doc_in(View::Source, "pic_src_del", "![](p.png)\n");
    d.caret = "![](p.png)".len();
    d.backspace();
    assert_eq!(d.source, "![](p.png\n", "raw editing, byte by byte");
}

// ── typing against a block leaf directive ─────────────────────────────────

/// A rendered-view `fmt` document with the caret on one of the stops of
/// the directive spelled `mark`, and the map built — [`doc_at_picture`]
/// for a directive, in either format.
fn doc_at_directive(src: &str, fmt: Format, mark: &str, side: MediaStop) -> Doc {
    let mut d = Doc::from_source(src.into(), fmt).unwrap();
    d.view = View::Wysiwyg;
    d.build_visual_unwrapped();
    let start = src.find(mark).unwrap();
    d.caret = match side {
        MediaStop::Before => start,
        MediaStop::After => start + mark.len(),
    };
    assert!(d.vmap.is_stop(d.caret), "{src:?} {side:?}");
    d
}

/// The leaf directives the map publishes, by name, after rebuilding it —
/// "is this still a directive, or has it become a paragraph of source?"
fn directive_names(d: &mut Doc) -> Vec<String> {
    d.build_visual_unwrapped();
    d.vmap.directives.iter().map(|i| i.name.clone()).collect()
}

/// Markdown's `::embed{…}` and djot's empty fence, each with the source
/// typing at its before and after stops should leave.
const DIRECTIVE_CASES: [(Format, &str, &str, &str, &str); 3] = [
    (
        Format::Markdown,
        "hi\n\n::embed{src=x}\n",
        "::embed{src=x}",
        "hi\n\nB\n\n::embed{src=x}\n",
        "hi\n\n::embed{src=x}\n\nB\n",
    ),
    (
        Format::Markdown,
        "hi\n\n::page-break\n\nbye\n",
        "::page-break",
        "hi\n\nB\n\n::page-break\n\nbye\n",
        "hi\n\n::page-break\n\nB\n\nbye\n",
    ),
    (
        Format::Djot,
        "hi\n\n::: page-break\n:::\n\nbye\n",
        "::: page-break\n:::",
        "hi\n\nB\n\n::: page-break\n:::\n\nbye\n",
        "hi\n\n::: page-break\n:::\n\nB\n\nbye\n",
    ),
];

#[test]
fn typing_past_a_leaf_directive_opens_a_paragraph_under_it() {
    // The task's accident: `::page[…]{…}TWO` is a paragraph of raw source.
    for (fmt, src, mark, _, after) in DIRECTIVE_CASES {
        let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
        let names = directive_names(&mut d);
        d.insert("B");
        assert_eq!(d.source, after, "{fmt:?}");
        assert_eq!(directive_names(&mut d), names, "{fmt:?}: still a directive");
    }
}

#[test]
fn typing_in_front_of_a_leaf_directive_opens_a_paragraph_above_it() {
    // Found in leaf-tui: `B` typed here made `B::embed{src=…}`.
    for (fmt, src, mark, before, _) in DIRECTIVE_CASES {
        let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
        let names = directive_names(&mut d);
        d.insert("B");
        assert_eq!(d.source, before, "{fmt:?}");
        assert_eq!(directive_names(&mut d), names, "{fmt:?}: still a directive");
    }
    // And one that opens the document takes a paragraph above it too.
    let mut d = doc_at_directive(
        "::embed{src=x}\n",
        Format::Markdown,
        "::",
        MediaStop::Before,
    );
    d.insert("B");
    assert_eq!(d.source, "B\n\n::embed{src=x}\n");
    assert_eq!(directive_names(&mut d), ["embed"]);
}

#[test]
fn two_directives_in_a_row_take_a_paragraph_between_them_from_either_side() {
    // The task's document: no paragraph between the marks to click into, so
    // the end of the first mark's row, or the front of the second's, is the
    // only way in.
    let md = "::page[Page 1 of 2]{card=\"a\"}\n\n::page[Page 2 of 2]{card=\"b\"}\n";
    let md_want = "::page[Page 1 of 2]{card=\"a\"}\n\nTWO\n\n::page[Page 2 of 2]{card=\"b\"}\n";
    let dj = "::: page-break\n:::\n\n::: x-card\n:::\n";
    let dj_want = "::: page-break\n:::\n\nTWO\n\n::: x-card\n:::\n";
    for (fmt, src, first, second, want) in [
        (
            Format::Markdown,
            md,
            "::page[Page 1 of 2]{card=\"a\"}",
            "::page[Page 2",
            md_want,
        ),
        (
            Format::Djot,
            dj,
            "::: page-break\n:::",
            "::: x-card",
            dj_want,
        ),
    ] {
        for (mark, side) in [(first, MediaStop::After), (second, MediaStop::Before)] {
            let mut d = doc_at_directive(src, fmt, mark, side);
            let names = directive_names(&mut d);
            assert_eq!(names.len(), 2);
            d.insert("T");
            d.insert("W");
            d.insert("O");
            assert_eq!(d.source, want, "{fmt:?} {side:?}");
            assert_eq!(directive_names(&mut d), names, "{fmt:?} {side:?}");
            let labels: Vec<_> = d.vmap.directives.iter().map(|i| i.label.clone()).collect();
            if fmt == Format::Markdown {
                assert_eq!(labels, ["Page 1 of 2", "Page 2 of 2"]);
                assert_eq!(d.vmap.directives[1].attr("card"), Some("b"));
            }
        }
    }
}

#[test]
fn a_djot_directive_s_attribute_line_stays_with_it() {
    // The `{…}` line above the fence is the directive's markup: a paragraph
    // opened between the two would take the attributes, and a delete that
    // left the line would hand them to the paragraph below.
    let src = "hi\n\n{.wide src=\"x\"}\n::: x-card\n:::\n\nbye\n";
    let mark = "::: x-card\n:::";
    let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
    d.insert("B");
    assert_eq!(
        d.source,
        "hi\n\nB\n\n{.wide src=\"x\"}\n::: x-card\n:::\n\nbye\n"
    );
    assert_eq!(directive_names(&mut d), ["x-card"]);
    assert_eq!(d.vmap.directives[0].attr("src"), Some("x"));

    let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::After);
    d.backspace();
    assert_eq!(
        d.source, "hi\n\nbye\n",
        "the directive went with its attributes"
    );
    let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
    d.delete_forward();
    assert_eq!(d.source, "hi\n\nbye\n");

    // A djot block picture's attribute line is the same markup, kept the
    // same way.
    let pic = "hi\n\n{.wide}\n![](p.png)\n";
    let mut d = doc_at_directive(pic, Format::Djot, "![](p.png)", MediaStop::Before);
    d.insert("B");
    assert_eq!(d.source, "hi\n\nB\n\n{.wide}\n![](p.png)\n");
    assert_eq!(media_count(&mut d), 1);
    let mut d = doc_at_directive(pic, Format::Djot, "![](p.png)", MediaStop::After);
    d.backspace();
    assert_eq!(d.source, "hi\n");

    // Where the attribute line opens the document, too.
    let src = "{.wide}\n::: x-card\n:::\n";
    let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
    d.insert("B");
    assert_eq!(d.source, "B\n\n{.wide}\n::: x-card\n:::\n");
    assert_eq!(directive_names(&mut d), ["x-card"]);
}

#[test]
fn a_directive_drawn_taller_still_takes_a_paragraph_under_it() {
    // Reserved filler rows are decoration: the after-stop is still the
    // directive's end, and a click on the drawing's lower part lands there.
    let src = "hi\n\n::embed{src=x}\n";
    let mut d = doc_at_directive(src, Format::Markdown, "::embed{src=x}", MediaStop::After);
    let key = d.vmap.directives[0].key();
    d.set_directive_rows(HashMap::from([(key, 4)]));
    d.build_visual_unwrapped();
    assert_eq!(d.vmap.directives[0].rows_span.len(), 4);
    d.insert("B");
    assert_eq!(d.source, "hi\n\n::embed{src=x}\n\nB\n");
}

#[test]
fn pasting_against_a_leaf_directive_opens_a_paragraph_too() {
    for (fmt, src, mark, before, after) in DIRECTIVE_CASES {
        for (side, want) in [(MediaStop::Before, before), (MediaStop::After, after)] {
            let mut d = doc_at_directive(src, fmt, mark, side);
            d.paste("pasted");
            assert_eq!(d.source, want.replace("B", "pasted"), "{fmt:?} {side:?}");
        }
    }
}

#[test]
fn one_undo_puts_a_directive_back_the_way_it_was_found() {
    for (fmt, src, mark, ..) in DIRECTIVE_CASES {
        for side in [MediaStop::Before, MediaStop::After] {
            let mut d = doc_at_directive(src, fmt, mark, side);
            d.insert("x");
            d.insert("y");
            assert_ne!(d.source, src);
            d.undo();
            assert_eq!(d.source, src, "{fmt:?} {side:?}");
        }
    }
}

#[test]
fn delete_keys_take_a_leaf_directive_whole_or_step_out_of_it() {
    // The picture's rule: the key aimed at the directive takes it whole, the
    // key aimed away steps over the boundary — never a byte of its markup.
    for (fmt, src, mark, ..) in DIRECTIVE_CASES {
        let gone = src
            .replace(&format!("{mark}\n\n"), "")
            .replace(&format!("\n\n{mark}"), "");
        let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
        d.backspace();
        assert_eq!(d.source, gone, "{fmt:?}: Backspace past it");
        d.undo();
        assert_eq!(d.source, src, "{fmt:?}: and back in one piece");

        let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
        d.delete_forward();
        assert_eq!(d.source, gone, "{fmt:?}: Delete in front of it");

        let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
        d.backspace();
        assert_eq!(d.source, src, "{fmt:?}: Backspace in front deletes nothing");
        assert_eq!(d.caret, 2, "{fmt:?}: it steps up to the end of `hi`");

        let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
        let at = d.caret;
        d.delete_forward();
        assert_eq!(d.source, src, "{fmt:?}: Delete past it deletes nothing");
        assert!(
            d.caret > at || src.ends_with(&format!("{mark}\n")),
            "{fmt:?}"
        );
    }
}

#[test]
fn source_view_types_raw_markup_against_a_directive_untouched() {
    let mut d = doc_in(View::Source, "dir_src", "::embed{src=x}\n");
    d.caret = "::embed{src=x}".len();
    d.insert("x");
    assert_eq!(d.source, "::embed{src=x}x\n");
}

#[test]
fn image_destination_at_caret_reads_the_image_under_the_caret() {
    let mut d = doc_with("img_read", "![a cat](cat.png)\n");
    d.caret = 3; // inside the image markup
    assert_eq!(d.image_destination_at_caret(), Some("cat.png".to_string()));
    // Past the image, the caret is in no image.
    d.caret = "![a cat](cat.png)".len();
    assert_eq!(d.image_destination_at_caret(), None);
}

#[test]
fn set_media_rows_reserves_blank_filler_rows_the_frontend_paints_over() {
    // The image is one placeholder row by default, and `set_media_rows` grows
    // it to the height the frontend measured: the label row plus blank
    // `decoration` fillers that hold the vertical space a raster is drawn into.
    let mut d = wysiwyg_doc("img_rows", "intro\n\n![a cat](cat.png)\n\nend\n");
    assert_eq!(d.vmap.media.len(), 1);
    let img_row = d.vmap.media[0].rows_span.start;
    assert_eq!(
        d.vmap.media[0].rows_span,
        img_row..img_row + 1,
        "default is one row"
    );

    d.set_media_rows(HashMap::from([("cat.png".to_string(), 4)]));
    d.build_visual(80);
    assert_eq!(d.vmap.media.len(), 1, "still one image, now taller");
    let span = d.vmap.media[0].rows_span.clone();
    assert_eq!(span.end - span.start, 4, "reserves the four rows asked for");
    // The label row carries the mark and its glyphs; the three below are blank
    // decoration — drawn, but no caret and no text.
    assert!(
        d.vmap.rows[span.start].media.is_some(),
        "mark rides the first row"
    );
    for r in (span.start + 1)..span.end {
        assert!(d.vmap.rows[r].decoration, "filler row {r} is decoration");
        assert!(d.vmap.rows[r].glyphs.is_empty(), "filler row {r} is blank");
        assert!(
            d.vmap.rows[r].media.is_none(),
            "only the first row is marked"
        );
    }
}

#[test]
fn a_taller_image_adds_no_caret_stops_and_motion_steps_over_its_fillers() {
    // The extra rows are pure spacers: the caret's only homes stay the stop in
    // front of the image and the one just past it, so walking the document top
    // to bottom visits the same offsets whether the image is 1 row or 5.
    let body = "ab\n\n![x](p.png)\n\ncd\n";
    let stops_at = |rows: usize| -> Vec<usize> {
        let mut d = wysiwyg_doc("img_stops", body);
        if rows > 1 {
            d.set_media_rows(HashMap::from([("p.png".to_string(), rows)]));
            d.build_visual(80);
        }
        d.caret = 0;
        let mut seen = vec![d.caret];
        loop {
            d.move_right(false);
            if *seen.last().unwrap() == d.caret {
                break;
            }
            seen.push(d.caret);
        }
        seen
    };
    assert_eq!(
        stops_at(1),
        stops_at(5),
        "reserving rows must not add stops"
    );
}
