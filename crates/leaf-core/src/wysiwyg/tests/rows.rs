use super::*;

#[test]
fn unwrapped_mode_emits_one_row_per_paragraph() {
    // A long paragraph that would wrap under a column budget stays a single
    // row when wrap is None (the GUI wraps it at pixel width instead).
    let long = "one two three four five six seven eight nine ten eleven twelve\n";
    let mut ed = Editor::new_str(long, Format::Markdown.twig()).unwrap();
    let wrapped = build_t(&ed.nodes().unwrap(), long, Some(12));
    let unwrapped = build_t(&ed.nodes().unwrap(), long, None);
    assert!(wrapped.num_rows() > 1, "narrow column should wrap");
    assert_eq!(unwrapped.num_rows(), 1, "no budget should keep it one row");
    // Every glyph's source byte is preserved in the single row.
    let text: String = unwrapped.rows[0].glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(text.trim_end(), long.trim_end());
}

fn line_texts(m: &VisualMap) -> Vec<String> {
    m.rows
        .iter()
        .map(|r| {
            // Trim the trailing whitespace a row may carry — the zero-width
            // '\n' that closes a preserved line, and any space glyph left at
            // a wrap boundary (both real caret stops, neither visible text).
            r.glyphs
                .iter()
                .map(|g| g.ch)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn preserve_lays_each_soft_break_on_its_own_row() {
    // A soft break (a bare newline inside a paragraph) folds into a space by
    // default — the whole paragraph is one reflowed row...
    let src = "one two\nthree four\n";
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    let folded = build_t(&ed.nodes().unwrap(), src, None);
    assert_eq!(folded.num_rows(), 1, "fold: one reflowed row");
    assert_eq!(
        line_texts(&folded),
        vec!["one two three four"],
        "break folded to a space"
    );

    // ...and under Preserve it renders where it was written, a row per line.
    let kept = map_preserve(src, None);
    assert_eq!(
        line_texts(&kept),
        vec!["one two", "three four"],
        "preserve: a row per line"
    );
}

#[test]
fn a_preserved_break_keeps_the_newline_offset_as_a_caret_stop() {
    // The break must leave a caret stop at the newline byte, or the caret
    // could not rest at the end of the first line. The '\n' glyph is dropped
    // from the row (so nothing stray renders); its offset (7 here) becomes the
    // row's end stop instead — the same offset the folded space would carry.
    let src = "one two\nthree four\n";
    let m = map_preserve(src, None);
    assert!(
        !m.rows[0].glyphs.iter().any(|g| g.ch == '\n'),
        "the break glyph is dropped"
    );
    assert_eq!(
        m.rows[0].end_src, 7,
        "the first row ends at the newline byte"
    );
    assert!(m.is_stop(7), "the newline offset is a caret stop");
    // Row end offsets stay strictly ascending — no two rows pin one offset.
    let offs: Vec<usize> = m.rows.iter().map(|r| r.end_src).collect();
    assert!(
        offs.windows(2).all(|w| w[0] < w[1]),
        "offsets not unique: {offs:?}"
    );
}

#[test]
fn preserved_lines_wrap_independently() {
    // Each preserved line wraps to the column on its own; the break between
    // them is hard, so a word never crosses it — "gamma" and "delta" could
    // share a row on width alone but the soft break keeps them apart.
    let src = "alpha beta gamma\ndelta epsilon\n";
    let m = map_preserve(src, Some(12));
    assert_eq!(
        line_texts(&m),
        vec!["alpha beta", "gamma", "delta", "epsilon"],
        "each source line wraps on its own"
    );
}

#[test]
fn an_empty_paragraph_between_blocks_renders_its_own_rows() {
    // "A", then two blank lines (an empty paragraph opened with Enter), then
    // "B": the empty paragraph must be navigable rows, not collapsed onto B.
    // Rows: "A", spacer, empty-paragraph, spacer, "B" — each blank row a
    // distinct source offset.
    let m = map("A\n\n\n\nB\n");
    let text: Vec<String> = m
        .rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert_eq!(text, vec!["A", "", "", "", "B"], "got {text:?}");
    let offs: Vec<usize> = m.rows.iter().map(|r| r.end_src).collect();
    // Strictly ascending — no two rows share an offset (else the caret pins).
    assert!(
        offs.windows(2).all(|w| w[0] < w[1]),
        "offsets not unique: {offs:?}"
    );
}

#[test]
fn a_tight_block_boundary_still_gets_one_separator() {
    // A heading directly above text (no blank line between) keeps the single
    // conventional separator row, as before.
    let m = map("# H\ntext\n");
    let text: Vec<String> = m
        .rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert_eq!(text, vec!["H", "", "text"], "got {text:?}");
}

#[test]
fn an_escaped_delimiter_renders_without_its_backslash_and_maps_true_offsets() {
    // `a\*b` renders the three visible chars `a * b` — the escape backslash
    // is hidden — and every glyph points at its real source byte, so a caret
    // past the escape lands right (the `*` at source 2, `b` at source 3, not
    // the drifted 1/2 the naive text-offset mapping gave).
    let m = map("a\\*b\n");
    let row: Vec<(char, usize)> = m.rows[0].glyphs.iter().map(|g| (g.ch, g.src)).collect();
    assert_eq!(row, vec![('a', 0), ('*', 2), ('b', 3)], "got {row:?}");
}

#[test]
fn an_escaped_hash_stays_a_paragraph_and_shows_the_hash() {
    // `\# hi` is a paragraph beginning with a literal `#`, not a heading —
    // the backslash is hidden, the `#` shown at its true offset.
    let m = map("\\# hi\n");
    let text: String = m.rows[0].glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(text, "# hi");
    assert_eq!(
        m.rows[0].glyphs[0].src, 1,
        "the # is at source byte 1, past the \\"
    );
}

#[test]
fn a_tight_nested_list_hangs_its_sublist_directly_under_the_item() {
    // A list item's own text and the sub-list nested under it are written on
    // adjacent source lines, so the rich view butts them together — no
    // fabricated blank row. Regression: the synthetic "breathe" separator
    // used to open a gap between `• a` and its `  ◦ b`.
    assert_eq!(rendered(&map("- a\n  - b\n")), "• a\n  ◦ b");
}

#[test]
fn a_loose_nested_list_keeps_its_real_blank_line() {
    // A genuine blank source line (a loose list) still parts the item from
    // its sub-list — only the *fabricated* separator is suppressed, never a
    // real one the author typed. The gap row wears the item's continuation
    // prefix (the two-space indent), so it renders as "  ", not empty.
    assert_eq!(rendered(&map("- a\n\n  - b\n")), "• a\n  \n  ◦ b");
}

#[test]
fn a_code_block_in_a_list_item_wears_one_marker() {
    // The item's marker goes on the block's first line and its indent on
    // the rest, as a wrapped paragraph's rows do. Every line wore the
    // marker once, so a two-line block in an item read as two items.
    assert_eq!(
        rendered(&map("- ```\n  one\n  two\n  ```\n")),
        "• one\n  two"
    );
    assert_eq!(
        rendered(&map("1. ```\n   one\n   two\n   ```\n")),
        "1. one\n   two"
    );
    assert_eq!(
        rendered(&map("- [ ] ```\n  one\n  two\n  ```\n")),
        "☐ one\n  two"
    );
}

#[test]
fn an_items_later_rows_wear_its_marker_as_a_blank_indent() {
    // Spelled with the marker's characters so a proportional face gives
    // the indent exactly the marker's width; drawn blank everywhere.
    for (src, marker) in [
        ("- a\n\n  b\n", "• "),
        ("1. a\n\n   b\n", "1. "),
        ("- [ ] a\n\n  b\n", "☐ "),
    ] {
        let m = map(src);
        let last = m.rows.last().unwrap();
        let indent: String = last
            .glyphs
            .iter()
            .take_while(|g| g.style.role == Role::ListIndent)
            .map(|g| g.ch)
            .collect();
        assert_eq!(indent, marker, "{src:?}");
        assert!(rendered(&m).ends_with(&format!("{}b", " ".repeat(marker.chars().count()))));
    }
}

#[test]
fn a_code_block_in_a_quote_draws_no_row_for_its_closing_fence() {
    // The gutter is the same on every row. The closing fence is markup,
    // and drew an empty quoted line under the code as if the writer had
    // typed one, at the end of the document or before more.
    assert_eq!(
        rendered(&map("> ```\n> one\n> two\n> ```\n")),
        "│ one\n│ two"
    );
    assert_eq!(
        rendered(&map("> ```\n> one\n> ```\n\nafter\n")),
        "│ one\n\nafter"
    );
    // A quoted line the writer added under the fence is still one.
    assert_eq!(rendered(&map("> ```\n> one\n> ```\n>\n")), "│ one\n│ ");
}

#[test]
fn frontmatter_is_hidden_and_the_document_opens_into_its_content() {
    // Leading YAML frontmatter renders nothing — no phantom blank rows for
    // its lines, no leading gap — and `content_start` points at the first
    // real block so the caret floor can keep out of the hidden metadata.
    let fm = "---\nconfig: prov.yaml\ncontents:\n- '[Sample](sample.md)'\n---\n";
    let src = format!("{fm}# leaf\n\nA line.\n");
    let m = map(&src);
    let text = rendered(&m);
    assert!(
        !text.contains("config"),
        "frontmatter body leaked: {text:?}"
    );
    assert!(!text.contains("prov"), "frontmatter body leaked: {text:?}");
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "leaf"
    );
    assert_eq!(
        m.content_start,
        fm.len(),
        "floor should be the first real block"
    );
}

#[test]
fn a_frontmatter_only_document_puts_the_floor_after_the_frontmatter() {
    // Nothing to render, so the caret floor is the end of the hidden
    // frontmatter — not 0, which is *before* the opening `---` and made the
    // first keystroke in a fresh metadata-only note land ahead of it. And
    // the frontmatter's own newlines are not trailing blank lines: they used
    // to open phantom rows at offsets 1..4, inside the metadata.
    let src = "---\ntitle: 2026-08-29\nid: f8s32cd\n---\n";
    let m = map(src);
    assert_eq!(m.content_start, src.len(), "floor must clear the metadata");
    assert!(
        m.rows.is_empty(),
        "frontmatter must render no rows: {:?}",
        rendered(&m)
    );
    assert!(
        m.stops.is_empty(),
        "no stop may sit inside the metadata: {:?}",
        m.stops
    );
}

#[test]
fn a_frontmatter_only_document_still_counts_its_real_blank_lines() {
    // Two blank lines after the frontmatter are the author's empty paragraph
    // and still render, counted from the metadata's end rather than from 0.
    let fm = "---\ntitle: n\n---\n";
    let m = map(&format!("{fm}\n\n"));
    assert_eq!(m.content_start, fm.len());
    assert_eq!(m.rows.len(), 2, "the two trailing newlines each open a row");
    assert!(
        m.rows.iter().all(|r| r.end_src > fm.len()),
        "rows must sit past the frontmatter"
    );
}

#[test]
fn a_document_without_frontmatter_has_a_zero_floor() {
    let m = map("# leaf\n\nbody\n");
    assert_eq!(m.content_start, 0);
}

#[test]
fn trailing_spaces_become_caret_stops_so_the_caret_can_be_drawn_past_them() {
    // Markdown/Djot drop the trailing space in `hello ` from the `str` node,
    // so without help the row would end at `hello` and the caret couldn't be
    // drawn past column 5 — typing a space at a line's end wouldn't move it
    // on screen until the next visible character reparsed the space into an
    // interior node. The builder recovers it from the block's span/content_span
    // gap and emits it as a real, caret-stoppable glyph.
    let m = map("hello \n");
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "hello "
    );
    assert_eq!(
        m.rows[0].end_src, 6,
        "the row now ends past the trailing space"
    );
    // The caret can rest both on and past the space.
    assert_eq!(m.pos_of_offset(5), (0, 5), "between 'o' and the space");
    assert_eq!(m.pos_of_offset(6), (0, 6), "past the space");
    // Two trailing spaces, both stops.
    let m = map("hello  \n");
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "hello  "
    );
    assert_eq!(m.pos_of_offset(7), (0, 7));
}

#[test]
fn a_headings_trailing_space_is_a_caret_stop_too() {
    // The hidden `# ` marker means `# hi ` renders as `hi ` in three columns;
    // the caret past the trailing space lands on the third.
    let m = map("# hi \n");
    assert_eq!(
        m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "hi "
    );
    assert_eq!(m.pos_of_offset(5), (0, 3));
}

#[test]
fn a_table_cells_trailing_padding_is_not_mistaken_for_block_trailing_space() {
    // A cell's own `span` is the whole row, so the trailing-whitespace
    // recovery must not run for cells or it would swallow the `│` delimiters
    // and neighbours between the cell text and the row's end. The grid stays
    // exactly as before.
    let text = rendered(&map(TABLE));
    assert!(
        text.contains("│ Pear │   3 │"),
        "cell padding disturbed:\n{text}"
    );
}

#[test]
fn a_click_below_the_last_row_lands_on_the_last_stop_not_offset_zero() {
    // A drag into the empty space under a short document used to resolve to
    // offset 0 — the wrong direction, and not even a caret stop when the
    // document opens on hidden frontmatter (its `content_start` floor is not
    // a stop), which crashed the caret invariant. It now lands on the last
    // stop: the end of the document, where dragging downward should reach.
    let fm = "---\ntitle: n\n---\n";
    let m = map(&format!("{fm}# Hi\n\nbody\n"));
    let below = m.num_rows() + 5;
    let off = m.offset_of_pos(below, 0);
    assert!(
        m.is_stop(off),
        "offset {off} from a below-content click is not a stop"
    );
    assert_eq!(
        off,
        m.stops.last().copied().unwrap(),
        "should be the document's last stop"
    );
    assert!(
        off > fm.len(),
        "must not fall onto the hidden frontmatter floor"
    );
}

#[test]
fn offset_of_pos_is_a_stop_for_every_row_including_past_the_end() {
    // The invariant the caret motion asserts: whatever cell a click names,
    // the offset it resolves to is one the caret can actually rest at.
    for src in [
        "hello \n",
        "# A heading here \n\nbody text goes on \n",
        "---\nk: v\n---\n# Title\n\nprose here that wraps a bit \n",
    ] {
        let m = map(src);
        for row in 0..m.num_rows() + 3 {
            for col in 0..30 {
                let off = m.offset_of_pos(row, col);
                assert!(
                    m.is_stop(off),
                    "row {row} col {col} → {off} is not a stop in {src:?}"
                );
            }
        }
    }
}
