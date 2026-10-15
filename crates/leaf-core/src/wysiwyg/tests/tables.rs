use super::*;

#[test]
fn a_table_renders_as_an_aligned_grid() {
    let text = rendered(&map(TABLE));
    assert_eq!(
        text,
        "┌──────┬─────┐\n\
             │ Name │ Qty │\n\
             ├──────┼─────┤\n\
             │ Pear │   3 │\n\
             │ Fig  │  12 │\n\
             └──────┴─────┘",
        "got:\n{text}"
    );
}

#[test]
fn table_columns_honour_their_alignment() {
    // Centre and default(left) come straight from twig's cell.alignment —
    // the delimiter row it's spelled in is consumed and has no node.
    let text = rendered(&map("| A | Bee |\n| --- | :---: |\n| x | y |\n"));
    assert!(text.contains("│ x │  y  │"), "centred column: {text:?}");
}

#[test]
fn table_borders_are_decoration_the_caret_never_lands_on() {
    let m = map(TABLE);
    // The top and header rules are whole decoration rows.
    for r in [0, 2] {
        assert!(m.rows[r].decoration, "row {r} should be a decoration rule");
        assert!(
            !m.rows[r].glyphs.iter().any(|g| g.stop),
            "row {r} has a stop"
        );
    }
    // The bottom border is the exception: no glyph of it is a stop, but
    // its end is the table's trailing caret home — the one place the caret
    // can stand past the last cell.
    let bottom = &m.rows[5];
    assert!(
        !bottom.decoration,
        "the bottom border holds the trailing stop"
    );
    assert!(
        !bottom.glyphs.iter().any(|g| g.stop),
        "the bottom border's glyphs are not stops"
    );
    assert!(m.is_stop(bottom.end_src), "the trailing stop is a stop");
    assert!(m.table_end_stop(bottom.end_src));
    assert_eq!(
        bottom.end_src,
        TABLE.trim_end_matches('\n').len(),
        "the trailing stop is the table's own end, before its newline"
    );
    assert!(
        !m.table_end_stop(TABLE.rfind("12").unwrap() + 2),
        "a cell's end is not the trailing stop"
    );
    // A content row's `│` and padding are decoration; only the cell text
    // and each cell's one end-stop are stops.
    let header = &m.rows[1];
    assert!(!header.decoration);
    for g in &header.glyphs {
        if g.ch == '│' {
            assert!(!g.stop, "a border is not a caret stop");
        }
    }
    let stops: String = header
        .glyphs
        .iter()
        .filter(|g| g.stop)
        .map(|g| g.ch)
        .collect();
    assert_eq!(stops, "Name Qty ", "cell text plus one end-stop space each");
}

#[test]
fn a_cell_maps_to_its_own_source_text() {
    let m = map(TABLE);
    // "Pear" starts at byte 32 in TABLE; the caret there draws on the 'P'.
    let pear = TABLE.find("Pear").unwrap();
    let (r, c) = m.pos_of_offset(pear);
    assert_eq!(m.rows[r].glyphs[c].ch, 'P');
    assert_eq!(m.offset_of_pos(r, c), pear, "round trips");
}

#[test]
fn a_wide_table_is_cut_to_fit_and_its_cells_wrap() {
    // Columns wider than the surface used to run off the right edge, where
    // nothing could reach them. They're cut to the budget instead, and the
    // text wraps down inside the column — the header rule stays put, and
    // an alignment holds on every line of a wrapped cell, not just the first.
    let src = "| Ingredient | Notes |\n|---|---:|\n\
                   | flour milled coarse | sift it twice |\n| salt | a pinch |\n";
    let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
    let m = build_t(&ed.nodes().unwrap(), src, Some(30));
    let text = rendered(&m);
    assert_eq!(
        text,
        "┌──────────────┬─────────────┐\n\
             │ Ingredient   │       Notes │\n\
             ├──────────────┼─────────────┤\n\
             │ flour milled │     sift it │\n\
             │ coarse       │       twice │\n\
             │ salt         │     a pinch │\n\
             └──────────────┴─────────────┘",
        "got:\n{text}"
    );
    for (r, row) in m.rows.iter().enumerate() {
        assert!(
            row.glyphs.len() <= 30,
            "row {r} overflows: {}",
            row.glyphs.len()
        );
    }
}

#[test]
fn a_column_too_narrow_for_a_word_breaks_it_rather_than_spilling() {
    // A paragraph lets an overlong word trail off the end of the line; a
    // table column can't — a glyph past the border lands on the border.
    let src = "| A | B |\n|---|---|\n| antidisestablishmentarianism | x |\n";
    let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
    let m = build_t(&ed.nodes().unwrap(), src, Some(20));
    for (r, row) in m.rows.iter().enumerate() {
        assert!(
            row.glyphs.len() <= 20,
            "row {r} overflows: {}",
            row.glyphs.len()
        );
    }
    // Broken across lines, but whole: every letter is still drawn, at its
    // own source byte, where the caret can reach it.
    let word = "antidisestablishmentarianism";
    let at = src.find(word).unwrap();
    for (i, ch) in word.char_indices() {
        assert!(
            m.rows
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .any(|g| g.stop && g.src == at + i && g.ch == ch),
            "{ch:?} at {} was lost to the break",
            at + i
        );
    }
}

// ── the structural view of a table ───────────────────────────────────────

#[test]
fn a_table_is_published_structurally_beside_its_picture() {
    let m = map(TABLE);
    let t = &m.tables[0];
    let cell =
        |r: usize, c: usize| -> String { t.grid[r].cells[c].glyphs.iter().map(|g| g.ch).collect() };
    assert_eq!(t.grid.len(), 3, "head + two body rows");
    assert_eq!(
        (cell(0, 0), cell(0, 1), cell(1, 0), cell(2, 1)),
        ("Name".into(), "Qty".into(), "Pear".into(), "12".into())
    );
    assert_eq!(
        t.grid.iter().map(|r| r.head).collect::<Vec<_>>(),
        [true, false, false]
    );
    // The alignment the delimiter row spelled, carried per cell — the only
    // place it survives, since the parser consumes that row.
    assert!(matches!(t.grid[1].cells[0].align, Alignment::Left));
    assert!(matches!(t.grid[1].cells[1].align, Alignment::Right));
}

#[test]
fn a_block_media_is_published_structurally_beside_its_placeholder() {
    let m = map("intro\n\n![a cat](img/cat.png)\n\nend\n");
    assert_eq!(m.media.len(), 1, "one block image");
    let img = &m.media[0];
    assert_eq!(img.destination, "img/cat.png");
    assert_eq!(img.alt, "a cat");
    // The placeholder row named by `rows_span` carries the label a plain
    // surface paints and a capable frontend replaces.
    let row_text = |r: usize| -> String { m.rows[r].glyphs.iter().map(|g| g.ch).collect() };
    assert_eq!(
        img.rows_span.end - img.rows_span.start,
        1,
        "one placeholder row"
    );
    assert_eq!(row_text(img.rows_span.start), "🖼 a cat");
    // The row carries the mark `media_spans` derives the side-table from.
    assert!(m.rows[img.rows_span.start].media.is_some());
}

#[test]
fn an_image_without_alt_labels_itself_with_its_filename() {
    let m = map("![](photos/beach.jpg)\n");
    let row = &m.rows[m.media[0].rows_span.start];
    assert_eq!(
        row.glyphs.iter().map(|g| g.ch).collect::<String>(),
        "🖼 beach.jpg"
    );
    assert_eq!(m.media[0].alt, "");
}

#[test]
fn an_empty_cells_home_is_read_from_either_shape_of_span() {
    // A whole-row span: the cell's pipes are the `col`-th and next.
    let row = "|  |  |";
    assert_eq!(empty_cell_offset(row, 10, 0), 12);
    assert_eq!(empty_cell_offset(row, 10, 1), 15);
    // A cell's own span, opening pipe to closing pipe exclusive: the same
    // homes, each read from its own span.
    assert_eq!(empty_cell_offset("|  ", 10, 0), 12);
    assert_eq!(empty_cell_offset("|  ", 13, 1), 15);
    // Nothing to stand in: just inside the pipe, never past the span.
    assert_eq!(empty_cell_offset("|", 10, 0), 11);
    assert_eq!(empty_cell_offset("", 10, 1), 10);
}

#[test]
fn a_hidden_marks_content_end_is_a_caret_home_but_not_a_glyph_stop() {
    // `a **bold** b`: the `d` is at 7, the content ends at 8, the closing
    // `**` draws nothing, and the space after it is at 10. Two homes at one
    // spot on screen: 8 (inside the bold) and 10 (past it).
    let m = map("a **bold** b\n");
    assert!(
        !m.stops.contains(&8),
        "8 has no glyph, so it is no glyph stop"
    );
    assert_eq!(m.mark_ends, vec![8]);
    assert!(m.is_stop(8), "but the caret may rest there");
    assert_eq!(m.snap_to_stop(8), 8, "and is left there when placed there");
    // Left/Right take both homes; the character-pairing walk takes one.
    assert_eq!(m.caret_stop_after(7), Some(8));
    assert_eq!(m.caret_stop_after(8), Some(10));
    assert_eq!(m.caret_stop_before(10), Some(8));
    assert_eq!(m.caret_stop_before(8), Some(7));
    assert_eq!(m.stop_after(7), Some(10));
    assert_eq!(m.stop_before(10), Some(7));
    // Drawn where the next glyph is: after the `d`, not on it.
    assert_eq!(m.pos_of_offset(8), m.pos_of_offset(10));
}

#[test]
fn every_hidden_inline_mark_gives_its_content_end_a_home() {
    // One end per mark, whatever it is spelled with; nested marks closing
    // together share the outer's end and the inner's alike.
    assert_eq!(
        map("*em* `code` [link](u) ~~del~~\n").mark_ends,
        vec![3, 10, 17, 27]
    );
    assert_eq!(map("***both***\n").mark_ends, vec![7]);
    // A mark that closes at its row's end coincides with the row's own end
    // stop — one offset, in both tables.
    let m = map("**bold**\n");
    assert_eq!(m.mark_ends, vec![6]);
    assert!(m.stops.contains(&6));
    // Revealed, the delimiter is glyphs of its own and the end is an
    // ordinary glyph stop: nothing to add.
    let mut ed = Editor::new_str("a **bold** b\n", Format::Markdown).unwrap();
    let src = "a **bold** b\n";
    let revealed = build(
        &ed.nodes().unwrap(),
        src,
        Some(80),
        false,
        &Surface::default(),
        Some(Reveal::full(0..src.len())),
    );
    assert!(revealed.mark_ends.is_empty());
    assert!(revealed.stops.contains(&8));
}

#[test]
fn a_marks_content_end_is_a_home_inside_a_table_cell() {
    let src = "| A | B |\n| --- | --- |\n| **bold** | other |\n";
    let m = map(src);
    let end = src.find("bold").unwrap() + 4; // 32, before the closing `**`
    assert_eq!(m.mark_ends, vec![end]);
    assert_eq!(m.snap_to_stop(end), end);
    // Drawn after the `d`, in this cell — where the cell's own end stop is.
    assert_eq!(m.pos_of_offset(end), m.pos_of_offset(end + 2));
}

#[test]
fn a_block_media_gives_the_caret_a_home_before_and_after_it() {
    // `![x](y)` on its own line: the caret can rest in front of the image
    // (its start) and just past it (the row end), and nowhere inside the
    // markup — the same coarse mapping a thematic break uses.
    let src = "![x](y.png)\n";
    let m = map(src);
    let img = &m.rows[m.media[0].rows_span.start];
    let start = 0; // the image opens the document
    let end = "![x](y.png)".len();
    // Every placeholder glyph maps to the image start and is a stop there.
    assert!(img.glyphs.iter().all(|g| g.src == start && g.stop));
    assert_eq!(img.end_src, end, "the row ends past the image");
    assert_eq!(m.stops.first(), Some(&start));
    assert!(m.stops.contains(&end), "a stop sits after the image");
    // Nothing inside the markup is a stop.
    assert!(!m.stops.iter().any(|&s| s > start && s < end));
}

#[test]
fn an_inline_image_amid_text_is_not_a_block_media() {
    // An image sharing its line with prose isn't block-level: it stays in the
    // inline path (rendered as its alt text), and publishes no MediaInfo.
    let m = map("see ![a cat](cat.png) here\n");
    assert!(m.media.is_empty(), "not a block image");
    assert!(
        rendered(&m).contains("a cat"),
        "alt text still renders inline"
    );
}

/// The block images `Doc` publishes for `src`, driven through the real
/// production build (`build_visual` → `build_cached`) with `html_elements`
/// on — the path a `<picture>` actually travels. Not the raw `build` the
/// other tests use: the editor's flat whole-arena snapshot tangles the links
/// of inline-promoted HTML (phantom roots, dangling `parent`s), which only
/// the per-block subtree walk `build_cached` does untangles.
fn doc_media(src: &str) -> Vec<MediaInfo> {
    let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
    doc.build_visual(80);
    doc.vmap.media.clone()
}

#[test]
fn a_video_block_is_media_with_its_src_poster_and_kind() {
    // The load-bearing assumption of video support: twig has no `video` node
    // kind, so `html_elements` promotion must land a `<video>` as a generic
    // `element` whose tag name and attributes survive onto `FlatNode` — the
    // same treatment `<picture>` gets. If that ever stops holding, this is
    // the test that says so.
    let m = doc_media("<video src=\"clip.mp4\" poster=\"still.png\" controls>\n</video>\n");
    assert_eq!(m.len(), 1, "the video is one block media");
    assert_eq!(m[0].kind, MediaKind::Video);
    assert_eq!(m[0].destination, "clip.mp4");
    assert_eq!(m[0].poster, "still.png");
}

#[test]
fn a_single_line_video_is_a_block_too() {
    // The spelling everyone actually writes. It used to parse as a paragraph
    // of raw inline HTML — CommonMark opens a block on a complete tag only
    // when the line ends there, and its fixed tag list predates `<video>` —
    // so the tags never reached core as an element at all. twig 2.5.1 widened
    // that list under `html_elements`; this is the test that would catch the
    // pin sliding back.
    let m = doc_media("<video src=\"clip.mp4\" controls></video>\n");
    assert_eq!(m.len(), 1, "single-line <video> is a block");
    assert_eq!(m[0].kind, MediaKind::Video);
    assert_eq!(m[0].destination, "clip.mp4");
}

#[test]
fn a_single_line_picture_is_a_block_with_its_alternatives() {
    // `<picture>` had the identical gap and it went unnoticed because the
    // conventional spelling breaks the lines. Same twig fix covers it.
    let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\">\
                   <img src=\"l.svg\" alt=\"banner\"></picture>\n";
    let m = doc_media(src);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].kind, MediaKind::Image);
    assert_eq!(m[0].destination, "l.svg");
    assert_eq!(m[0].resolve(ColorScheme::Dark), "d.svg");
}

#[test]
fn an_audio_block_is_media_with_no_poster() {
    let m = doc_media("<audio src=\"take.mp3\" controls>\n</audio>\n");
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].kind, MediaKind::Audio);
    assert_eq!(m[0].destination, "take.mp3");
    assert!(m[0].poster.is_empty(), "audio has no poster frame");
}

#[test]
fn a_videos_source_children_are_its_candidates_typed_by_mime() {
    // A `<video>` with no `src` of its own — the common shape, since it's how
    // you offer more than one codec. The candidates come from `<source src>`
    // (not `srcset`, which is `<picture>`'s spelling) and carry their MIME.
    let src = "<video controls>\n\
                   <source src=\"a.webm\" type=\"video/webm\">\n\
                   <source src=\"a.mp4\" type=\"video/mp4\">\n\
                   fallback\n\
                   </video>\n";
    let m = doc_media(src);
    assert_eq!(m.len(), 1);
    assert!(
        m[0].destination.is_empty(),
        "no src attribute on the element"
    );
    assert_eq!(m[0].sources.len(), 2);
    assert_eq!(m[0].sources[0].srcset, "a.webm");
    assert_eq!(m[0].sources[0].mime, "video/webm");
    assert_eq!(m[0].sources[1].srcset, "a.mp4");
    // With an empty destination, `resolve` falls through to the first
    // candidate rather than handing the frontend nothing to load.
    assert_eq!(m[0].resolve(ColorScheme::Light), "a.webm");
}

#[test]
fn a_video_placeholder_row_carries_its_own_sigil_and_mark() {
    // The placeholder contract images already hold, now for a video: the row
    // renders as a labelled stand-in a plain surface can paint as-is, and
    // carries the mark a capable frontend replaces it from.
    let src = "<video src=\"clip.mp4\" controls>\n</video>\n";
    let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
    doc.build_visual(80);
    let row = &doc.vmap.rows[doc.vmap.media[0].rows_span.start];
    let text: String = row.glyphs.iter().map(|g| g.ch).collect();
    assert!(
        text.starts_with('🎬'),
        "video sigil, not the image one: {text:?}"
    );
    assert!(row.media.is_some(), "the mark rides the placeholder row");
}

#[test]
fn a_picture_block_carries_its_source_alternatives() {
    // A `<picture>` with a dark-mode `<source>`: one block image, whose
    // fallback destination is the `<img>` and whose `sources` carry the
    // `<source>`'s media + srcset for a theme-aware frontend to pick.
    let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.svg\"><img src=\"light.svg\" alt=\"banner\"></picture>\n";
    let images = doc_media(src);
    assert_eq!(images.len(), 1, "the picture is one block image");
    let img = &images[0];
    assert_eq!(img.destination, "light.svg", "fallback is the <img>");
    assert_eq!(img.alt, "banner");
    assert_eq!(
        img.sources,
        vec![MediaSource {
            media: "(prefers-color-scheme: dark)".into(),
            srcset: "dark.svg".into(),
            mime: String::new(),
        }],
    );
}

#[test]
fn a_picture_inside_a_heading_is_still_a_block_media_with_sources() {
    // fig.md's shape: the banner is an `<h1>` wrapping the `<picture>`.
    let src = "<h1><picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\"><img src=\"l.svg\" alt=\"fig\"></picture></h1>\n";
    let images = doc_media(src);
    assert_eq!(images.len(), 1, "heading-wrapped picture is a block image");
    assert_eq!(images[0].destination, "l.svg");
    assert_eq!(images[0].sources.len(), 1);
    assert_eq!(images[0].sources[0].srcset, "d.svg");
}

#[test]
fn a_plain_image_has_no_media_sources() {
    // A bare Markdown image carries an empty `sources` — nothing to pick from.
    let images = doc_media("![alt](p.png)\n");
    assert_eq!(images.len(), 1);
    assert!(
        images[0].sources.is_empty(),
        "no <picture>, no alternatives"
    );
}

#[test]
fn resolve_picks_the_source_matching_the_scheme() {
    let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.svg\"><img src=\"light.svg\" alt=\"b\"></picture>\n";
    let images = doc_media(src);
    let img = &images[0];
    // Dark theme takes the dark source; light falls through to the <img>.
    assert_eq!(img.resolve(ColorScheme::Dark), "dark.svg");
    assert_eq!(img.resolve(ColorScheme::Light), "light.svg");
}

#[test]
fn resolve_falls_back_for_a_plain_image_and_unknown_media() {
    // A plain image ignores the scheme.
    let plain = doc_media("![a](p.png)\n");
    assert_eq!(plain[0].resolve(ColorScheme::Dark), "p.png");

    // A <source> with an unrecognized media query is skipped; a light source
    // is taken under a light theme.
    let m = doc_media(
        "<picture><source media=\"print\" srcset=\"p.svg\"><source media=\"(prefers-color-scheme: light)\" srcset=\"l.svg\"><img src=\"f.svg\" alt=\"x\"></picture>\n",
    );
    assert_eq!(m[0].resolve(ColorScheme::Light), "l.svg");
    assert_eq!(
        m[0].resolve(ColorScheme::Dark),
        "f.svg",
        "no dark source → <img>"
    );
}

#[test]
fn resolve_reads_the_first_srcset_url_ignoring_descriptors() {
    // A comma/descriptor srcset resolves to its first URL.
    assert_eq!(first_srcset_url("a.png 1x, b.png 2x"), Some("a.png"));
    assert_eq!(first_srcset_url("  solo.svg  "), Some("solo.svg"));
    assert_eq!(first_srcset_url(""), None);
    // An empty (unconditional) media always matches.
    assert!(media_matches("", ColorScheme::Light));
    assert!(media_matches(
        "(prefers-color-scheme:dark)",
        ColorScheme::Dark
    ));
    assert!(!media_matches(
        "(prefers-color-scheme: dark)",
        ColorScheme::Light
    ));
}

#[test]
fn a_block_media_carries_its_list_prefix() {
    // An image that is a list item's body opens past the bullet, like every
    // other block does.
    let m = map("- ![alt](p.png)\n");
    let row = &m.rows[m.media[0].rows_span.start];
    let text: String = row.glyphs.iter().map(|g| g.ch).collect();
    assert!(
        text.starts_with("• "),
        "the list marker prefixes the image row: {text:?}"
    );
    assert!(text.contains("🖼 alt"));
}

#[test]
fn the_structural_table_spans_exactly_its_drawn_rows() {
    // A frontend drawing its own grid skips `rows_span` and renders from
    // `grid`. If the span were short the leftover border rows would be
    // painted as text under the real table; if long it would eat a
    // neighbouring paragraph. Both are silent, so pin it to the picture.
    let m = map(&format!("before\n\n{TABLE}\nafter\n"));
    let t = &m.tables[0];
    let row_text = |r: usize| -> String { m.rows[r].glyphs.iter().map(|g| g.ch).collect() };
    assert!(
        row_text(t.rows_span.start).starts_with('┌'),
        "opens on the top border"
    );
    assert!(
        row_text(t.rows_span.end - 1).starts_with('└'),
        "closes on the bottom border"
    );
    assert!(
        !row_text(t.rows_span.start - 1).contains('┌'),
        "the row before the span is not the table's"
    );
    assert_eq!(
        row_text(t.rows_span.end),
        "",
        "the span ends before the gap row"
    );
}

#[test]
fn a_nested_tables_structure_carries_the_block_prefix() {
    // The picture puts the quote's gutter on every row of the grid. A
    // frontend drawing its own table has to draw that too and start past it,
    // so the prefix has to travel with the structure — without it a quoted
    // table renders flush at the margin and leaves the quote it's in.
    let m = map("> | a | b |\n> |---|---|\n> | c | d |\n");
    let t = &m.tables[0];
    let prefix: String = t.prefix.iter().map(|g| g.ch).collect();
    assert_eq!(prefix, "│ ", "the quote's gutter should ride the structure");
    // And it matches what the picture actually drew.
    let drawn: String = m.rows[t.rows_span.start]
        .glyphs
        .iter()
        .map(|g| g.ch)
        .collect();
    assert!(
        drawn.starts_with(&prefix),
        "picture and structure disagree: {drawn:?}"
    );
}

#[test]
fn a_top_level_table_carries_no_prefix() {
    assert!(map(TABLE).tables[0].prefix.is_empty());
}

#[test]
fn structural_cells_are_unwrapped_even_when_the_picture_wraps_them() {
    // The picture wraps a cell to its column; a frontend laying the grid out
    // in pixels needs the text as the document spells it, before that
    // decision. Narrow enough that the drawn cell must break.
    let src = "| Name |\n|------|\n| alpha beta gamma |\n";
    let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
    let m = build_t(&ed.nodes().unwrap(), src, Some(12));
    let drawn = rendered(&m);
    let cell: String = m.tables[0].grid[1].cells[0]
        .glyphs
        .iter()
        .map(|g| g.ch)
        .collect();
    assert_eq!(
        cell, "alpha beta gamma",
        "structure must not carry the wrap"
    );
    assert!(
        drawn.lines().count() > 5,
        "the picture should have wrapped, else this proves nothing:\n{drawn}"
    );
}
