use super::*;

// ── math ─────────────────────────────────────────────────────────────────

/// [`map_leaf`] on a chosen surface and reveal — the whole of what a math
/// rendering turns on.
fn map_math(src: &str, format: Format, surface: &Surface, reveal: Option<Reveal>) -> VisualMap {
    let mut ed = Editor::new_with_flags(
        src.as_bytes(),
        format.twig(),
        crate::doc::parse_extensions(format),
    )
    .unwrap();
    build(&ed.nodes().unwrap(), src, Some(80), false, surface, reveal)
}

fn pictures() -> Surface {
    Surface {
        inline_pictures: true,
        ..Default::default()
    }
}

#[test]
fn markdown_reads_math_and_a_dollar_before_whitespace_stays_prose() {
    // The `math` extension is on for every leaf document; twig's own rule
    // keeps a price out of it.
    let m = map_leaf("Say $E = mc^2$ for $5 and $6.\n", Format::Markdown);
    assert_eq!(row_texts(&m), vec!["Say E = mc^2 for $5 and $6."]);
    let e = m.rows[0].glyphs.iter().find(|g| g.ch == 'E').unwrap();
    assert_eq!(
        e.style.role,
        Role::Code,
        "the formula's TeX, in the code style"
    );
    assert_eq!(e.src, 5, "at its own byte, past the `$`");
    let five = m.rows[0].glyphs.iter().find(|g| g.ch == '5').unwrap();
    assert_eq!(five.style.role, Role::Body);
    assert!(m.math.is_empty(), "no picture stands in on a plain surface");
}

#[test]
fn a_plain_surface_draws_inline_math_as_code_with_the_delimiters_hidden() {
    // The verbatim treatment, in both languages — what `inline_math` always
    // rendered as — with the caret's home at the content's end.
    for (src, fmt) in [
        ("a $x+y$ b\n", Format::Markdown),
        ("a $`x+y` b\n", Format::Djot),
    ] {
        let m = map_leaf(src, fmt);
        assert_eq!(row_texts(&m), vec!["a x+y b"], "{src:?}");
        let x = m.rows[0].glyphs.iter().find(|g| g.ch == 'x').unwrap();
        assert_eq!(x.style.role, Role::Code);
        assert!(!m.mark_ends.is_empty(), "the content end is a caret home");
    }
}

#[test]
fn display_math_in_a_line_of_prose_puts_its_text_at_the_text_s_own_offset() {
    // `display_math` had no arm and fell to the default one, which pushed
    // the text at the *node's* start: three bytes short in djot, past `$$`
    // and the backtick.
    let m = map_leaf("Before $$`x`$$ after\n", Format::Djot);
    let x = m.rows[0].glyphs.iter().find(|g| g.ch == 'x').unwrap();
    assert_eq!(x.src, "Before $$`".len());
    assert_eq!(x.style.role, Role::Code);
    let m = map_leaf("Before $$x$$ after\n", Format::Markdown);
    let x = m.rows[0].glyphs.iter().find(|g| g.ch == 'x').unwrap();
    assert_eq!(x.src, "Before $$".len());
}

#[test]
fn a_surface_that_paints_in_a_line_gets_one_atom_per_inline_formula() {
    let src = "Say $E = mc^2$ and $a$.\n";
    let m = map_math(src, Format::Markdown, &pictures(), None);
    assert_eq!(row_texts(&m), vec!["Say ∑ and ∑."]);
    assert_eq!(m.math.len(), 2);
    let first = &m.math[0];
    assert_eq!(first.tex, "E = mc^2");
    assert!(!first.display);
    assert_eq!(first.row, 0);
    assert_eq!(first.rows_span, 0..1);
    assert_eq!(first.glyph, Some(4));
    assert_eq!(first.src, 4, "the formula's start, where a click lands");
    let atom = &m.rows[0].glyphs[4];
    assert_eq!(atom.ch, MATH_ATOM);
    assert_eq!(atom.style.role, Role::Math);
    assert!(atom.stop);
    assert_eq!(atom.src, 4);
    // The caret has a stop on the atom and the next glyph's past it, and
    // nothing inside the markup.
    assert_eq!(m.stop_after(4), Some("Say $E = mc^2$".len()));
    assert!(m.mark_ends.is_empty(), "no home inside the hidden markup");
    // The row carries the marks the side-table is derived from.
    assert_eq!(m.rows[0].math.len(), 2);
    assert_eq!(m.rows[0].math[1].glyph, Some(m.math[1].glyph.unwrap()));
    assert_eq!(m.math[1].tex, "a");
}

#[test]
fn a_display_formula_written_inline_is_an_atom_in_display_style() {
    let m = map_math("Before $$x$$ after\n", Format::Markdown, &pictures(), None);
    assert_eq!(row_texts(&m), vec!["Before ∑ after"]);
    assert_eq!(m.math.len(), 1);
    assert!(m.math[0].display);
    assert_eq!(m.math[0].tex, "x");
}

#[test]
fn an_atom_follows_its_glyph_across_a_wrap() {
    // Fifteen words, then a formula that wraps onto the second row: the
    // mark is drained onto the row the glyph landed on, at its index there.
    let src = format!("{}$x$ end\n", "word ".repeat(15));
    let mut ed = Editor::new_with_flags(
        src.as_bytes(),
        Format::Markdown.twig(),
        crate::doc::parse_extensions(Format::Markdown),
    )
    .unwrap();
    let m = build(
        &ed.nodes().unwrap(),
        &src,
        Some(40),
        false,
        &pictures(),
        None,
    );
    assert!(m.rows.len() >= 2);
    assert_eq!(m.math.len(), 1);
    let info = &m.math[0];
    let g = &m.rows[info.row].glyphs[info.glyph.unwrap()];
    assert_eq!(g.ch, MATH_ATOM);
    assert_eq!(g.src, src.find("$x$").unwrap());
    assert!(m.rows[..info.row].iter().all(|r| r.math.is_empty()));
}

#[test]
fn an_atom_in_a_table_cell_rides_the_cell_s_row() {
    let m = map_math(
        "| a | b |\n|---|---|\n| $x$ | c |\n",
        Format::Markdown,
        &pictures(),
        None,
    );
    assert_eq!(m.math.len(), 1);
    let info = &m.math[0];
    assert_eq!(m.rows[info.row].glyphs[info.glyph.unwrap()].ch, MATH_ATOM);
}

#[test]
fn a_display_formula_on_its_own_lines_is_a_block_placeholder_on_every_surface() {
    for (src, fmt) in [
        (
            "intro\n\n$$\n\\int_0^1 x\\,dx\n$$\n\nend\n",
            Format::Markdown,
        ),
        ("intro\n\n$$`\\int_0^1 x\\,dx`\n\nend\n", Format::Djot),
    ] {
        for surface in [Surface::default(), pictures()] {
            let m = map_math(src, fmt, &surface, None);
            assert_eq!(
                row_texts(&m),
                vec!["intro", "", "∑ \\int_0^1 x\\,dx", "", "end"],
                "{src:?}"
            );
            assert_eq!(m.math.len(), 1);
            let info = &m.math[0];
            assert!(info.display);
            assert_eq!(info.glyph, None, "a block, not an atom");
            assert_eq!(info.rows_span, 2..3);
            assert_eq!(info.tex.trim(), "\\int_0^1 x\\,dx");
            let start = src.find("$$").unwrap();
            let end = start + src[start..].find("\n\nend").unwrap();
            assert_eq!(info.src, start);
            // Every label glyph at the formula's start, a stop there and
            // one past the block, nothing inside — a picture's two homes.
            let row = &m.rows[2];
            assert!(
                row.glyphs
                    .iter()
                    .all(|g| g.src == start && g.style.role == Role::Math)
            );
            assert_eq!(row.end_src, end);
            assert_eq!(m.stop_after(start), Some(end));
            assert_eq!(m.stop_before(end), Some(start));
            // The gaps either side are labelled as a formula's.
            assert_eq!(m.rows[1].boundary.map(|b| b.below), Some(BlockClass::Math));
            assert_eq!(m.rows[3].boundary.map(|b| b.above), Some(BlockClass::Math));
        }
    }
}

#[test]
fn a_display_block_reserves_the_rows_the_frontend_measured() {
    let src = "$$\nx\n$$\n\nend\n";
    let surface = Surface {
        math_rows: HashMap::from([("\nx\n".to_string(), 4)]),
        ..Default::default()
    };
    let m = map_math(src, Format::Markdown, &surface, None);
    assert_eq!(m.math[0].rows_span, 0..4);
    assert_eq!(row_texts(&m)[..5], ["∑ x", "", "", "", ""]);
    // The fillers are decoration: drawn, no caret, anchored past the block.
    for r in &m.rows[1..4] {
        assert!(r.decoration);
        assert_eq!(r.end_src, 7);
    }
    assert_eq!(m.stop_after(0), Some(7));
    // A height keyed by TeX that does not match reserves nothing.
    let surface = Surface {
        math_rows: HashMap::from([("x".to_string(), 4)]),
        ..Default::default()
    };
    let m = map_math(src, Format::Markdown, &surface, None);
    assert_eq!(m.math[0].rows_span, 0..1);
}

#[test]
fn a_paragraph_with_prose_beside_a_display_formula_is_not_a_block() {
    let m = map_math("see\n$$\nx\n$$\n", Format::Markdown, &pictures(), None);
    assert!(
        m.math.iter().all(|i| i.glyph.is_some()),
        "an atom, not a placeholder"
    );
    let m = map_math(
        "$$\nx\n$$\n$$\ny\n$$\n",
        Format::Markdown,
        &pictures(),
        None,
    );
    assert_eq!(m.math.len(), 2);
    assert!(
        m.math.iter().all(|i| i.glyph.is_some()),
        "two formulas is prose with math in it"
    );
}

#[test]
fn a_formula_on_the_reveal_line_is_its_tex_in_every_mode() {
    let src = "Say $E = mc^2$ here.\n";
    // The hidden modes' reveal: only the formula shows its markup.
    let m = map_math(
        src,
        Format::Markdown,
        &pictures(),
        Some(Reveal::math(0..src.len() - 1)),
    );
    assert_eq!(row_texts(&m), vec!["Say $E = mc^2$ here."]);
    assert!(
        m.math.is_empty(),
        "nothing stands in for a revealed formula"
    );
    let dollar = m.rows[0].glyphs.iter().find(|g| g.ch == '$').unwrap();
    assert_eq!(dollar.style.role, Role::Delimiter);
    let e = m.rows[0].glyphs.iter().find(|g| g.ch == 'E').unwrap();
    assert_eq!(e.style.role, Role::Code);
    // Full's reveal draws the same, and an emphasis beside it reveals too
    // where the hidden modes' does not.
    let src = "*a* $x$\n";
    let hidden = map_math(
        src,
        Format::Markdown,
        &pictures(),
        Some(Reveal::math(0..src.len() - 1)),
    );
    assert_eq!(row_texts(&hidden), vec!["a $x$"]);
    let full = map_math(
        src,
        Format::Markdown,
        &pictures(),
        Some(Reveal::full(0..src.len() - 1)),
    );
    assert_eq!(row_texts(&full), vec!["*a* $x$"]);
    // A reveal line that does not meet the formula leaves the atom.
    let other = map_math(
        "$x$\n\ntext\n",
        Format::Markdown,
        &pictures(),
        Some(Reveal::math(5..9)),
    );
    assert_eq!(row_texts(&other)[0], "∑");
}

#[test]
fn a_display_block_on_the_reveal_line_is_its_source_lines_in_the_code_style() {
    let src = "intro\n\n$$\n\\int_0^1 x\n$$\n\nend\n";
    // Any line of the block reveals the whole of it: here the middle one.
    let mid = src.find("\\int").unwrap();
    let line = mid..mid + "\\int_0^1 x".len();
    let m = map_math(src, Format::Markdown, &pictures(), Some(Reveal::math(line)));
    assert_eq!(
        row_texts(&m),
        vec!["intro", "", "$$", "\\int_0^1 x", "$$", "", "end"]
    );
    assert!(m.math.is_empty());
    let fence = m.rows[2].glyphs.iter().find(|g| g.ch == '$').unwrap();
    assert_eq!(fence.style.role, Role::Delimiter);
    assert_eq!(fence.src, 7);
    let x = m.rows[3].glyphs.iter().find(|g| g.ch == 'x').unwrap();
    assert_eq!(x.style.role, Role::Code);
    assert_eq!(x.src, src.find("x\n$$").unwrap());
    // Every byte of the source is reachable: a stop on each fence and each
    // character between.
    assert!(m.is_stop(7));
    assert!(m.is_stop(mid));
    // The closing fence's line too, and the same for djot.
    let close = src.rfind("$$").unwrap();
    let m = map_math(
        src,
        Format::Markdown,
        &pictures(),
        Some(Reveal::math(close..close + 2)),
    );
    assert_eq!(row_texts(&m)[2], "$$");
    let src = "$$`\n\\int\n`\n";
    let m = map_math(src, Format::Djot, &pictures(), Some(Reveal::math(4..8)));
    assert_eq!(row_texts(&m), vec!["$$`", "\\int", "`"]);
}

#[test]
fn a_block_that_holds_a_formula_says_so_to_the_cache() {
    let src = "plain\n\nwith $x$ in it\n\n$$\ny\n$$\n";
    let mut ed = Editor::new_with_flags(
        src.as_bytes(),
        Format::Markdown.twig(),
        crate::doc::parse_extensions(Format::Markdown),
    )
    .unwrap();
    let mut cache = BlockCache::default();
    let top = top_blocks(&mut ed);
    let _ = build_cached(
        &top,
        src,
        None,
        false,
        &pictures(),
        None,
        &mut cache,
        |id| ed.subtree(NodeId(id)).unwrap_or_default(),
    );
    assert!(!cache.math_meets(&(0..5)), "the plain paragraph");
    assert!(cache.math_meets(&(7..21)), "the one with an atom");
    assert!(
        cache.math_meets(&(26..27)),
        "the middle line of the display block"
    );
    // A hit carries the answer without a walk: build again from the cache.
    let _ = build_cached(
        &top,
        src,
        None,
        false,
        &pictures(),
        None,
        &mut cache,
        |_| Vec::new(),
    );
    assert!(cache.math_meets(&(7..21)));
    assert!(!cache.math_meets(&(0..5)));
}

#[test]
fn incremental_builds_agree_with_the_reference_on_math() {
    for src in [
        "a $x$ b\n\n$$\ny\n$$\n\nc\n",
        "one\n\ntwo $\\frac{a}{b}$ three\n",
        "$$\n\\int\n$$\n",
    ] {
        for surface in [Surface::default(), pictures()] {
            let mut ed = Editor::new_with_flags(
                src.as_bytes(),
                Format::Markdown.twig(),
                crate::doc::parse_extensions(Format::Markdown),
            )
            .unwrap();
            let all = ed.nodes().unwrap();
            let plain = build(&all, src, Some(80), false, &surface, None);
            let mut cache = BlockCache::default();
            let top = top_blocks(&mut ed);
            let cached = build_cached(
                &top,
                src,
                Some(80),
                false,
                &surface,
                None,
                &mut cache,
                |id| ed.subtree(NodeId(id)).unwrap_or_default(),
            );
            assert_eq!(row_texts(&plain), row_texts(&cached), "{src:?}");
            assert_eq!(plain.math, cached.math, "{src:?}");
            assert_eq!(plain.stops, cached.stops, "{src:?}");
        }
    }
}
