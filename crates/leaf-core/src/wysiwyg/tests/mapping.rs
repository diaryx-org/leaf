use super::*;

/// An arena and a string that disagree — spans reaching past the source they
/// are built against.
///
/// [`crate::Doc`] keeps the two in step, so this is a "cannot happen" that
/// nonetheless *did*: `examples/bench` timed a loop of `edit_range` and then
/// went on handing the grown editor's spans to a builder holding the string
/// from before it, and every run ended in a slice panic rather than a
/// number. `push_escaped_text` was already written to survive the mismatch —
/// it clamps the span's end and falls back to an empty slice — and this is
/// the half of that intent it did not carry through.
///
/// Rendering the wrong thing is the acceptable answer here; panicking in a
/// paint loop is not.
#[test]
fn a_source_shorter_than_the_arena_built_over_it_renders_rather_than_panicking() {
    // An escape puts the run on `push_escaped_text`'s slow path — the fast
    // path is a length comparison that a truncated source fails anyway.
    let src = "alpha \\*beta\\* gamma delta epsilon\n";
    let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
    let nodes = ed.nodes().unwrap();

    // Every truncation of it, so the cut lands before, inside and after the
    // escaped run rather than only where one hand-picked index put it.
    for cut in 0..=src.len() {
        if !src.is_char_boundary(cut) {
            continue;
        }
        let map = build_t(&nodes, &src[..cut], Some(80));
        for row in &map.rows {
            for g in &row.glyphs {
                assert!(
                    g.src <= src.len(),
                    "cut {cut}: glyph {:?} points past the source at {}",
                    g.ch,
                    g.src
                );
            }
        }
    }
}

/// The whole correctness claim of the block cache: `build_cached` produces a
/// byte-identical map to `build`, on a fresh cache *and* — the case that
/// actually exercises reuse-and-shift plus per-block subtree marshalling — on
/// a warm cache after the source has been edited underneath it.
/// **Every glyph must stand on the character it claims.** A row's source
/// extent is computed from its last glyph's offset, so a glyph carrying an
/// offset that is not its own character's start yields a row end inside a
/// multi-byte character — and every later slice of the source panics on it.
///
/// Reproduces a real crash from a journal entry: a bracketed elision inside
/// a blockquote (`[…]`) gave the closing bracket a `text` of `"]"` over a
/// source span covering `"…]"`, because the parse folded the ellipsis into a
/// neighbouring node. `push_escaped_text` walked that span assuming a
/// dropped backslash was the only way text and source could diverge, so the
/// `]` landed on the `…`'s first byte:
/// `byte index 1236 is not a char boundary; it is inside '…'`.
#[test]
fn a_glyph_never_lands_inside_the_character_before_it() {
    let src = "> engage with it rather than look away. […]\n>\n> The through-line\n";
    let vmap = map(src);
    for (r, row) in vmap.rows.iter().enumerate() {
        assert!(
            src.is_char_boundary(row.end_src.min(src.len())),
            "row {r} ends at {} — inside a character",
            row.end_src
        );
        for g in &row.glyphs {
            assert!(
                src.is_char_boundary(g.src.min(src.len())),
                "row {r} has {:?} at {}, which is inside a character",
                g.ch,
                g.src
            );
        }
    }
    // The elision survives, and its bracket sits on the real `]`.
    let text: String = vmap
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.ch))
        .collect();
    assert!(text.contains("[…]"), "the elision should render: {text:?}");
    let close = vmap
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .find(|g| g.ch == ']')
        .expect("a closing bracket");
    assert_eq!(
        src[close.src..].chars().next(),
        Some(']'),
        "the bracket glyph should stand on the source's own `]`"
    );
}

#[test]
fn build_cached_matches_build() {
    let docs = [
        "# Title\n\nThe quick brown fox.\n\nAnother paragraph here.\n",
        "## H\n\n- one\n- two\n- three\n\n> a quote\n> continued\n",
        "para one\n\n```\ncode\nlines\n```\n\nafter code\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n\ntext after a table\n",
        "line\n- \nsetext?\n\nreal para\n\n\n\ntrailing blanks\n",
        "> quote with **bold** and a [link](https://x.dev)\n>\n> - item\n> - item2\n\ntail\n",
        "intro\n\n![a cat](img/cat.png)\n\nbetween\n\n![](https://x.dev/logo.svg)\n\nend\n",
        "- text item\n- ![alt](pic.png)\n- more text\n",
        // Footnotes: twig parses each definition as a root beside `doc`, so
        // these are the docs where the reference build and the incremental
        // one could disagree about what the top-level blocks even are.
        "A claim[^1] and another[^src].\n\n[^1]: First note.\n\n[^src]: Second.\n\ntail\n",
        "note[^a]\n\n[^a]: body **bold**\n    wrapped on\n    three lines\n\nafter\n",
        // No trailing newline. twig closes the document's last block on the
        // virtual newline it supplies at EOF, so that block's `span.end` is
        // `source.len() + 1` — a range that slices no bytes at all. Keying
        // the block cache off such a slice made every last block hash alike;
        // see [`block_bytes`].
        "# Title\n\nThe quick brown fox.\n\nA tail with no newline",
        "A claim[^1] and another[^src].\n\n[^1]: First note.\n[^src]: Second, ending the file.",
        // Comments draw nothing. The per-block builder the cached path
        // renders one with starts at offset 0 and, drawing nothing, never
        // moved — so the walk went on from 0 and spelled every line of the
        // document as a blank row. One at the start, one between blocks,
        // one at the end, so each position is covered.
        "<!-- lead -->\n\npara\n\n<!-- exec -->\n```\ncode\n```\n\nafter\n\n<!-- trail -->\n",
        // A Markdown `<div>` ends with a hidden `</div>` line the walk
        // steps over — between blocks and closing the file, so both the
        // separator after it and the trailing count are covered.
        "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n",
        "above\n\n<div class=\"center\">\n\nhello\n\nworld\n\n</div>\n",
        // Link reference definitions: roots beside `doc` like footnotes,
        // but drawing nothing. Alone between blocks, glued under a
        // paragraph, and closing the file under a comment — the README
        // shape.
        "see [a] and [b]\n\n[a]: /a\n\nmid\n[b]: /b \"bee\"\n\nend [c]\n\n<!-- links -->\n[c]: /c\n",
        // A rule's row ends past the newline under it while the walk
        // stands at the rule, and the rule's block is never cached for
        // it: an empty line under one mid-document and closing it.
        "para\n\n---\n\n\n\nbetween rules\n\n---\n\n",
        // Blank lines above the first block draw rows: bare, past
        // frontmatter, and past a comment that draws nothing.
        "\n\n\nfirst\n\nsecond\n",
        "---\ntitle: x\n---\n\n\nafter frontmatter\n",
        "<!-- lead -->\n\n\n\nafter a comment\n",
        // An empty paragraph at the end of a div draws inside it.
        "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n\n\n</div>\n\nafter\n",
        // Text glued under `</div>` opens an HTML block that runs to the
        // next blank line: twig leaves the div's opening tag an empty
        // container and the text a bare `str` at the top level.
        "<div data-line-height=\"1.5\">\n\nknees\n\n</div>\nafter\n",
    ];
    for wrap in [None, Some(80usize), Some(20)] {
        for src in docs {
            let ctx = format!("wrap={wrap:?} src={src:?}");
            let mut ed = Editor::new_str(src, Format::Markdown.twig()).unwrap();
            let mut cache = BlockCache::default();

            // 1) Fresh cache equals the cache-free build.
            let (plain, cached) = render_both(&mut ed, src, wrap, &mut cache);
            assert_maps_eq(&plain, &cached, &format!("fresh {ctx}"));

            // 2) Type a char mid-document, reparse, rebuild with the now-warm
            //    cache: the edited block is re-marshalled and re-rendered,
            //    every block below it is reused shifted, and the result must
            //    still match a from-scratch build.
            let at = (src.len() / 2..=src.len())
                .find(|&i| src.is_char_boundary(i))
                .unwrap();
            ed.edit_range(at, at, "Z").unwrap();
            let src2 = ed.source_str().unwrap();
            let (plain2, cached2) = render_both(&mut ed, &src2, wrap, &mut cache);
            assert_maps_eq(&plain2, &cached2, &format!("after insert {ctx}"));

            // 3) Delete it again: offsets shift back the other way, and the
            //    warm cache must not hand back stale shifted rows.
            ed.edit_range(at, at + 1, "").unwrap();
            let src3 = ed.source_str().unwrap();
            let (plain3, cached3) = render_both(&mut ed, &src3, wrap, &mut cache);
            assert_maps_eq(&plain3, &cached3, &format!("after delete {ctx}"));
        }
    }
}

/// A document that does not end in a newline is the one place twig hands
/// leaf a top-level span that addresses no source: the last block is closed
/// on the virtual newline the parser supplies at EOF, so its `span.end` is
/// `source.len() + 1`. The block cache keys on the bytes under that span, and
/// reading the out-of-range slice as *no bytes* broke it two ways at once —
/// [`block_bytes`] has the full account. Both ways are checked here, because
/// they fail independently.
#[test]
fn a_block_running_past_the_last_byte_still_keys_the_cache_by_its_own_bytes() {
    // One: two overrunning blocks collide. A footnote definition is a root
    // beside `doc` that [`top_blocks`] merges into the top level, while the
    // `section` above it spans the definition's bytes too — so when the
    // definition ends the file, both blocks end past it. The second was
    // served the first's rows, and the definition rendered as a copy of the
    // heading.
    let src = "A claim[^1] worth checking.\n\n# A heading with a reference[^1] in it\n\n[^1]: The first note.\n[^note]: A note with a word for a label.";
    let mut ed = Editor::new_str(src, Format::Djot.twig()).unwrap();
    let (plain, cached) = render_both(&mut ed, src, Some(80), &mut BlockCache::default());
    assert_maps_eq(&plain, &cached, "a definition ending the file");
    let text = rendered(&cached);
    assert!(
        text.ends_with("[note] A note with a word for a label."),
        "the last definition should render itself: {text:?}"
    );
    assert_eq!(
        text.matches("A heading with a reference").count(),
        1,
        "the heading should render exactly once: {text:?}"
    );

    // Two: one overrunning block goes stale. Its bytes are its cache key, so
    // a block that keeps hashing the same however it is edited is served the
    // rows built before the edit — the whole last line frozen as the user
    // types in it.
    let mut cache = BlockCache::default();
    let first = "first para\n\n# A heading\n\nlast para with no newline";
    let mut ed = Editor::new_str(first, Format::Djot.twig()).unwrap();
    let (_, warm) = render_both(&mut ed, first, Some(80), &mut cache);
    assert!(rendered(&warm).ends_with("last para with no newline"));

    let second = "first para\n\n# A heading\n\nDIFFERENT text without a newline";
    let mut ed = Editor::new_str(second, Format::Djot.twig()).unwrap();
    let (plain, cached) = render_both(&mut ed, second, Some(80), &mut cache);
    assert_maps_eq(&plain, &cached, "edited last block, warm cache");
    let text = rendered(&cached);
    assert!(
        text.ends_with("DIFFERENT text without a newline"),
        "the warm cache served the pre-edit rows: {text:?}"
    );
}

#[test]
fn resolves_markup_to_plain_text() {
    let text = rendered(&map("# Title\n\na **bold** word\n"));
    assert!(!text.contains('#'), "heading marker shown: {text:?}");
    assert!(!text.contains("**"), "strong delimiters shown: {text:?}");
    assert!(text.contains("Title") && text.contains("bold word"));
}

#[test]
fn every_glyph_points_at_its_source_byte() {
    let src = "a **bold** c\n";
    let m = map(src);
    for row in &m.rows {
        for g in &row.glyphs {
            // A real (non-synthetic) glyph's source byte is the glyph's char.
            if g.src < src.len()
                && src.is_char_boundary(g.src)
                && let Some(sc) = src[g.src..].chars().next()
                && sc == g.ch
            {
                continue;
            }
            // Synthetic prefixes (none here) would be the only exceptions.
            panic!("glyph {:?} at src {} doesn't match source", g.ch, g.src);
        }
    }
}

#[test]
fn offset_and_position_round_trip_on_visible_text() {
    let m = map("hello world\n");
    let (r, c) = m.pos_of_offset(6); // the 'w'
    assert_eq!(m.offset_of_pos(r, c), 6);
}

#[test]
fn visible_utf16_indices_count_the_text_the_system_sees() {
    // Hidden delimiters, a two-unit emoji, and a block gap — every way the
    // visible text's UTF-16 length parts company with a source byte count.
    let src = "a **b\u{1F600}** c\n\nd\n";
    let m = map(src);
    let end = m.snap_to_stop(src.len());
    let text = m.visible_text(0, end);
    assert_eq!(text, "a b\u{1F600} c\nd");

    // Forward: the index of each offset is where that character sits in
    // the visible string, in UTF-16 units.
    for (i, (src_off, _)) in m.visible_items(0, end).iter().enumerate() {
        let expect: usize = text.chars().take(i).map(char::len_utf16).sum();
        assert_eq!(
            m.visible_utf16_len(0, *src_off),
            expect,
            "utf16 index of source offset {src_off}"
        );
        // And back: the index resolves to the offset it came from.
        assert_eq!(m.offset_at_visible_utf16(end, expect), Some(*src_off));
    }
    // Inside the emoji's surrogate pair resolves to the emoji.
    let emoji_src = src.find('\u{1F600}').unwrap();
    let emoji_idx = m.visible_utf16_len(0, emoji_src);
    assert_eq!(
        m.offset_at_visible_utf16(end, emoji_idx + 1),
        Some(emoji_src)
    );
    // At or past the end is nobody's character.
    let total = m.visible_utf16_len(0, end);
    assert_eq!(total, text.encode_utf16().count());
    assert_eq!(m.offset_at_visible_utf16(end, total), None);
}

/// The documents the lookups are checked against their reference scans
/// on: every shape that puts rows out of source order or a stop out of
/// step with a glyph. A wrapped table, whose cells' second lines sit
/// below the next column's first; a wide grapheme and an emoji outside
/// the BMP; hidden delimiters and a link's hidden destination; a list
/// with synthetic markers; a code block; an image row whose label glyphs
/// all share one offset; an empty paragraph, a heading, and a wrapped
/// paragraph — at a narrow width so the table and the prose both wrap,
/// and unwrapped, which is how a GUI builds it.
fn lookup_maps() -> Vec<(String, VisualMap)> {
    let table = "| left cell that wraps | right |\n|---|---|\n| a longer cell than the column can hold | b |\n| `k` | 你好 |\n\n";
    let srcs = [
        "hello world\n",
        "a **b\u{1F600}** c\n\nd\n",
        "- one *two*\n- three\n\n\n# Title\n\nend [link](https://e.org/x) tail\n",
        &format!("{table}```\nx\ny\n```\n\n![alt](a.png)\n\ntail 你好 **bold** here\n"),
        "one two three four five six seven eight nine ten eleven twelve thirteen\n\n| a | b |\n|-|-|\n| c d e f g h | i |\n",
    ];
    let mut out = Vec::new();
    for src in srcs {
        for wrap in [Some(14), None] {
            out.push((format!("{src:?} at {wrap:?}"), map_at(src, wrap)));
        }
    }
    out
}

/// `pos_of_offset` as it was written before the walk learnt to dismiss a
/// row from its ends: every row read through, the nearest stop kept.
fn pos_of_offset_by_scan(m: &VisualMap, off: usize) -> (usize, usize) {
    let mut best: Option<(usize, usize, usize)> = None;
    for (r, row) in m.rows.iter().enumerate() {
        if row.decoration {
            continue;
        }
        let cand = row
            .glyphs
            .iter()
            .enumerate()
            .find(|(_, g)| g.stop && g.src >= off)
            .map(|(i, g)| (g.src, r, row.col_of_glyph(i)))
            .or_else(|| (row.end_src >= off).then_some((row.end_src, r, row.width())));
        if let Some(c) = cand
            && best.is_none_or(|b| c.0 <= b.0)
        {
            best = Some(c);
        }
    }
    match best {
        Some((_, r, c)) => (r, c),
        None => {
            let r = m.last_stop_row();
            (r, m.row_width(r))
        }
    }
}

/// `visible_items` as it was written before the spelling was tabulated:
/// every stop glyph in the range gathered, sorted, and paired up.
fn visible_items_by_scan(m: &VisualMap, from: usize, to: usize) -> Vec<(usize, Option<char>)> {
    let (lo, hi) = m.visible_span(from, to);
    let from = m.snap_to_glyph_stop(from);
    let mut glyphs: Vec<(usize, char)> = m
        .rows
        .iter()
        .filter(|r| !r.decoration)
        .flat_map(|r| r.glyphs.iter())
        .filter(|g| g.stop && g.src >= from && g.src < to)
        .map(|g| (g.src, g.ch))
        .collect();
    glyphs.sort_by_key(|&(src, _)| src);
    glyphs.dedup_by_key(|&mut (src, _)| src);
    let cell_ends: Vec<usize> = m
        .tables
        .iter()
        .flat_map(|t| t.grid.iter())
        .flat_map(|r| r.cells.iter())
        .map(|c| c.end)
        .collect();
    m.stops[lo..hi]
        .iter()
        .map(|&s| {
            let ch = glyphs
                .iter()
                .find(|&&(src, _)| src == s)
                .filter(|_| !cell_ends.contains(&s))
                .map(|&(_, ch)| ch);
            (s, ch)
        })
        .collect()
}

#[test]
fn pos_of_offset_agrees_with_reading_every_row_through() {
    for (name, m) in lookup_maps() {
        let len = m.rows.iter().map(|r| r.end_src).max().unwrap_or(0) + 2;
        for off in 0..=len {
            assert_eq!(
                m.pos_of_offset(off),
                pos_of_offset_by_scan(&m, off),
                "offset {off} of {name}"
            );
        }
    }
}

#[test]
fn the_tabulated_spelling_agrees_with_gathering_the_glyphs() {
    for (name, m) in lookup_maps() {
        let end = m.stops.last().copied().unwrap_or(0);
        // Whole text, and every window a frontend might ask for.
        let mut spans = vec![(0, end)];
        for &a in m.stops.iter().step_by(3) {
            spans.push((a, end));
            spans.push((0, a));
            spans.push((a, (a + 5).min(end)));
        }
        for (from, to) in spans {
            let items = visible_items_by_scan(&m, from, to);
            assert_eq!(
                m.visible_items(from, to),
                items,
                "items {from}..{to} of {name}"
            );
            let utf16: usize = items
                .iter()
                .map(|(_, ch)| ch.map_or(1, char::len_utf16))
                .sum();
            assert_eq!(
                m.visible_utf16_len(from, to),
                utf16,
                "utf16 {from}..{to} of {name}"
            );
        }
        // And back: every UTF-16 index in the text resolves to the stop
        // that spells it, as the scan would have found it.
        let items = visible_items_by_scan(&m, 0, end);
        let mut seen = 0;
        for (src, ch) in &items {
            for u in seen..seen + ch.map_or(1, char::len_utf16) {
                assert_eq!(
                    m.offset_at_visible_utf16(end, u),
                    Some(*src),
                    "index {u} of {name}"
                );
            }
            seen += ch.map_or(1, char::len_utf16);
        }
        assert_eq!(
            m.offset_at_visible_utf16(end, seen),
            None,
            "past the end of {name}"
        );
    }
}

#[test]
fn visible_text_spends_exactly_one_character_on_every_stop() {
    // A list (whose items' ends no gap row follows), a table (whose cells'
    // ends draw a gutter space), and a code block (one row per line):
    // every place the text used to part company with the stop count, in
    // both directions. `UITextInput`'s tokenizer indexes this text by
    // that count, so the two must agree exactly between any two stops.
    let src = "- one\n- two\n\n| a | b |\n| - | - |\n| c | d |\n\n```\nx\ny\n```\n\nend\n";
    let m = map(src);
    let end = m.snap_to_stop(src.len());
    // The table's trailing stop draws no glyph, so it is spelled as a line
    // end too: to the system the table ends on a blank line, which is
    // where the caret past it stands.
    assert_eq!(m.visible_text(0, end), "one\ntwo\na\nb\nc\nd\n\nx\ny\nend");
    // Between any two stops, one character per hop.
    let first = m.snap_to_glyph_stop(0);
    let stops: Vec<usize> = std::iter::successors(Some(first), |&o| m.stop_after(o)).collect();
    for (i, &a) in stops.iter().enumerate() {
        for (j, &b) in stops.iter().enumerate().skip(i) {
            assert_eq!(
                m.visible_text(a, b).chars().count(),
                j - i,
                "text between stops {a} and {b}"
            );
        }
    }
    // A cell's end is spelled as a line end, not the space it draws, so a
    // tap landing past `a`'s last letter has nothing to step over into `b`.
    let a_end = src.find("a |").unwrap() + 1;
    assert_eq!(m.visible_text(a_end, a_end + 1), "\n");
}

#[test]
fn caret_steps_over_hidden_delimiters() {
    // "a **bold** c": bytes 8,9 are the closing ** — no glyph. Moving right
    // from 'd' (src 7) lands on the space before 'c' (src 10), not inside **.
    let m = map("a **bold** c\n");
    let (r, c) = m.pos_of_offset(7);
    assert_eq!(m.offset_of_pos(r, c + 1), 10);
}
