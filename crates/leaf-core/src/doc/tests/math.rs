use super::*;

// ── math ─────────────────────────────────────────────────────────────────

#[test]
fn a_formula_reveals_on_the_caret_line_in_the_hidden_modes() {
    // The rule the proposal states: a formula's content is its TeX, not
    // its picture, so it reveals on the caret's line in *every* mode —
    // and nothing else on that line does outside `Full`.
    let mut d = doc_in(
        View::Wysiwyg,
        "math_reveal",
        "*one* $x+y$ here\n\ntwo there\n",
    );
    d.set_inline_pictures(true);
    assert_eq!(d.markup_mode(), MarkupMode::None);

    // Away from the formula's line: the atom, and no reveal at all.
    caret_at(&mut d, "two");
    assert!(
        drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
        "{:?}",
        drawn_rows(&d)
    );
    assert_eq!(d.vmap.math.len(), 1);
    assert_eq!(d.reveal_line(), None, "a line with no math keys nothing");

    // On it: the formula is its source, the emphasis is still resolved.
    caret_at(&mut d, "here");
    assert!(
        drawn_rows(&d).iter().any(|r| r == "one $x+y$ here"),
        "{:?}",
        drawn_rows(&d)
    );
    assert!(d.vmap.math.is_empty());
    assert_eq!(d.reveal_line(), Some(Reveal::math(0..16)));

    // Off again, and the picture is back.
    caret_at(&mut d, "two");
    assert!(drawn_rows(&d).iter().any(|r| r == "one ∑ here"));

    // The same in Shortcuts; and Full reveals the emphasis too.
    d.set_markup_mode(MarkupMode::Shortcuts);
    caret_at(&mut d, "here");
    assert!(drawn_rows(&d).iter().any(|r| r == "one $x+y$ here"));
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "here");
    assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
}

#[test]
fn an_unrevealed_map_shows_the_carets_line_as_a_page_does() {
    let mut d = doc_in(
        View::Wysiwyg,
        "math_unrevealed",
        "*one* $x+y$ here\n\ntwo there\n",
    );
    d.set_inline_pictures(true);
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "here");
    assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
    let (screen, caret) = (d.visual_key(), d.caret);

    // On paper: the delimiters hidden and the formula a picture, though
    // the caret has not moved off the line.
    d.set_unrevealed(true);
    d.build_visual(80);
    assert!(
        drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
        "{:?}",
        drawn_rows(&d)
    );
    assert_eq!(d.vmap.math.len(), 1);
    assert_eq!(d.caret, caret);

    // Off again: the screen's map is back as it was, and the next build
    // reuses it rather than building it over.
    d.set_unrevealed(false);
    assert_eq!(d.visual_key(), screen);
    d.build_visual(80);
    assert_eq!(d.visual_key(), screen);
    assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
    assert_eq!(d.caret, caret);
}

#[test]
fn a_formula_closed_by_typing_reveals_at_once() {
    // The reveal line is decided from the last build's layout, which
    // across an edit is stale: the keystroke that closes a `$…$` asks a
    // layout that knew no math. `build_map` asks again once the new
    // layout is in, so the formula does not snap to its picture under
    // the caret.
    let mut d = doc_in(View::Wysiwyg, "math_typed", "say \n");
    d.set_inline_pictures(true);
    d.set_markup_mode(MarkupMode::Shortcuts);
    d.caret = 4;
    for ch in ["$", "x", "$"] {
        d.insert(ch);
        d.build_visual(80);
    }
    assert_eq!(d.source, "say $x$\n");
    assert_eq!(
        drawn_rows(&d)[0],
        "say $x$",
        "source, not a picture, under the caret"
    );
    assert!(d.vmap.math.is_empty());
    // Leaving the line folds it — there is only one line, so add one.
    d.newline();
    d.insert("more");
    d.build_visual(80);
    assert_eq!(drawn_rows(&d)[0], "say ∑");
    assert_eq!(d.vmap.math.len(), 1);
    // And deleting the formula while revealed drops the reveal with it.
    d.caret = 7;
    d.build_visual(80);
    assert_eq!(drawn_rows(&d)[0], "say $x$");
    for _ in 0..3 {
        d.backspace();
    }
    d.build_visual(80);
    assert_eq!(d.source, "say \n\nmore\n");
    assert_eq!(d.reveal_line(), None);
}

#[test]
fn a_display_block_is_edited_where_it_stands() {
    let mut d = doc_in(
        View::Wysiwyg,
        "math_block",
        "intro\n\n$$\n\\int_0^1 x\n$$\n\nend\n",
    );
    caret_at(&mut d, "end");
    assert_eq!(
        drawn_rows(&d),
        vec!["intro", "", "∑ \\int_0^1 x", "", "end"]
    );
    // Up from `end` lands on the placeholder, whose glyphs all carry the
    // block's start — which is on its `$$` line, so the block reveals.
    d.move_up(false);
    d.build_visual(80);
    assert_eq!(d.caret, 7);
    assert_eq!(
        drawn_rows(&d),
        vec!["intro", "", "$$", "\\int_0^1 x", "$$", "", "end"]
    );
    // Down walks the source lines, still revealed; typing edits the TeX.
    d.move_down(false);
    d.build_visual(80);
    assert_eq!(d.caret, 10);
    d.move_end(false);
    d.insert("^2");
    d.build_visual(80);
    assert_eq!(d.source, "intro\n\n$$\n\\int_0^1 x^2\n$$\n\nend\n");
    assert_eq!(drawn_rows(&d)[3], "\\int_0^1 x^2");
    // Out below, and it folds to the placeholder with the new TeX.
    d.move_down(false);
    d.move_down(false);
    d.move_down(false);
    d.build_visual(80);
    assert_eq!(drawn_rows(&d)[2], "∑ \\int_0^1 x^2");
    assert_eq!(d.vmap.math[0].tex, "\n\\int_0^1 x^2\n");
}

#[test]
fn a_frontend_that_edits_math_keeps_the_picture_on_the_caret_line() {
    let src = "*one* $x+y$ here\n\n$$\n\\int_0^1 x\n$$\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "math_edits", src);
    d.set_inline_pictures(true);
    d.set_owned_regions(&["math"]);
    // On the formula's line, and on the block's: still pictures.
    caret_at(&mut d, "here");
    assert!(
        drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
        "{:?}",
        drawn_rows(&d)
    );
    assert_eq!(d.reveal_line(), None);
    d.move_down(false);
    d.move_down(false);
    d.build_visual(80);
    assert_eq!(d.vmap.math.len(), 2, "{:?}", drawn_rows(&d));
    // Each formula is a region that says where it is and where its TeX
    // is, exactly.
    assert_eq!(d.vmap.regions.len(), 2);
    for (r, m) in d.vmap.regions.iter().zip(&d.vmap.math) {
        assert_eq!(r.kind, "math");
        assert_eq!(r.src, m.src);
        assert_eq!(r.inline, m.glyph.is_some());
        assert_eq!(&d.source[r.content.clone()], m.tex);
        assert!(d.source[r.src..r.end].starts_with('$'));
        assert!(d.source[r.src..r.end].ends_with('$'));
    }
    assert_eq!(&d.source[d.vmap.regions[0].content.clone()], "x+y");
    // An edit above shifts the reused block; its spans move with it.
    d.place_caret(0, false);
    d.insert("Z");
    d.build_visual(80);
    for (r, m) in d.vmap.regions.iter().zip(&d.vmap.math) {
        assert_eq!(&d.source[r.content.clone()], m.tex);
    }
    // Full still reveals everything.
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "here");
    assert!(drawn_rows(&d).iter().any(|r| r.contains("$x+y$")));
}

#[test]
fn a_formula_nobody_owns_is_no_region() {
    let mut d = doc_in(View::Wysiwyg, "math_unowned", "say $x$ here\n\nnext\n");
    d.set_inline_pictures(true);
    d.place_caret(d.source.len(), false);
    d.build_visual(80);
    assert_eq!(d.vmap.math.len(), 1);
    assert!(d.vmap.regions.is_empty());
    d.set_owned_regions(&["fence:mermaid"]);
    d.build_visual(80);
    assert!(
        d.vmap.regions.is_empty(),
        "owning fences owned formulas too"
    );
}

#[test]
fn an_owned_fence_is_one_placeholder_row_and_a_region() {
    let src = "intro\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n```rust\nfn x() {}\n```\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "fence_owned", src);
    d.set_owned_regions(&["fence:mermaid"]);
    caret_at(&mut d, "end");
    let rows = drawn_rows(&d);
    assert!(rows.iter().any(|r| r == "▤ mermaid"), "{rows:?}");
    assert!(!rows.iter().any(|r| r.contains("A-->B")), "{rows:?}");
    // The other language is still code.
    assert!(rows.iter().any(|r| r == "fn x() {}"), "{rows:?}");
    assert_eq!(d.vmap.regions.len(), 1);
    let r = d.vmap.regions[0].clone();
    assert_eq!(r.kind, "fence:mermaid");
    assert!(!r.inline);
    assert_eq!(&d.source[r.content.clone()], "graph TD\n  A-->B\n");
    assert!(d.source[r.src..r.end].starts_with("```mermaid"));
    assert!(d.source[r.src..r.end].trim_end().ends_with("```"));
    // The caret rests either side of it, and on its line it stays a
    // placeholder.
    d.place_caret(r.src, false);
    d.build_visual(80);
    assert!(drawn_rows(&d).iter().any(|row| row == "▤ mermaid"));
    assert_eq!(d.caret, r.src);
    d.move_right(false);
    assert_eq!(d.caret, r.end, "one step crosses the block");
    // An edit through its content moves the end and keeps the region.
    d.edit(r.content.start, r.content.start, "%% note\n");
    d.build_visual(80);
    let r2 = d.vmap.regions[0].clone();
    assert_eq!(
        &d.source[r2.content.clone()],
        "%% note\ngraph TD\n  A-->B\n"
    );
    assert_eq!(r2.end, r.end + 8);
    // An edit above shifts the reused block, and the region with it.
    d.place_caret(0, false);
    d.insert("Z");
    d.build_visual(80);
    let r3 = d.vmap.regions[0].clone();
    assert_eq!(r3.src, r2.src + 1);
    assert_eq!(
        &d.source[r3.content.clone()],
        "%% note\ngraph TD\n  A-->B\n"
    );
    // Full reveals the fence on the caret's line as its source.
    d.set_markup_mode(MarkupMode::Full);
    d.place_caret(r3.src, false);
    d.build_visual(80);
    assert!(drawn_rows(&d).iter().any(|row| row.contains("A-->B")));
    assert!(d.vmap.regions.is_empty());
}

#[test]
fn an_empty_owned_fence_has_empty_content_at_its_closing_fence() {
    let src = "```mermaid\n```\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "fence_empty", src);
    d.set_owned_regions(&["fence:mermaid"]);
    caret_at(&mut d, "end");
    let r = d.vmap.regions[0].clone();
    assert_eq!(r.content, 11..11);
    d.edit(r.content.start, r.content.end, "graph LR\n");
    assert_eq!(d.source, "```mermaid\ngraph LR\n```\n\nend\n");
}

#[test]
fn an_owned_inline_directive_is_an_atom_and_an_inline_region() {
    let src = "Due :date[2026-10-12] and :date[2026-11-01]{tz=UTC} then :tag[x] done\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "inline_directive_owned", src);
    d.set_inline_pictures(true);
    caret_at(&mut d, "end");
    let plain = drawn_rows(&d);
    assert!(plain[0].contains("2026-10-12"), "{plain:?}");
    assert!(d.vmap.regions.is_empty());
    d.set_owned_regions(&["directive:date"]);
    caret_at(&mut d, "end");
    let rows = drawn_rows(&d);
    assert_eq!(rows[0], "Due ▤ and ▤ then x done", "{rows:?}");
    let [a, b] = d
        .vmap
        .regions
        .clone()
        .try_into()
        .unwrap_or_else(|v| panic!("two regions: {v:?}"));
    for r in [&a, &b] {
        assert_eq!(r.kind, "directive:date");
        assert!(r.inline && !r.display);
        assert_eq!(r.rows_span, 0..1);
        assert_eq!(
            r.content,
            r.src..r.end,
            "an inline directive's content is its whole source"
        );
    }
    assert_eq!(&d.source[a.src..a.end], ":date[2026-10-12]");
    assert_eq!(&d.source[b.src..b.end], ":date[2026-11-01]{tz=UTC}");
    let mark = a.directive.as_ref().expect("its directive");
    assert_eq!(
        (mark.name.as_str(), mark.label.as_str()),
        ("date", "2026-10-12")
    );
    let mark = b.directive.as_ref().expect("its directive");
    assert_eq!(
        mark.attrs,
        vec![("tz".to_string(), Some("UTC".to_string()))]
    );
    // The atom is one stop: the caret steps over the whole directive.
    d.place_caret(a.src, false);
    d.move_right(false);
    assert_eq!(d.caret, a.end);
    d.move_left(false);
    assert_eq!(d.caret, a.src);
    // An edit through its content is an edit of the directive.
    let at = a.src + ":date[2026-10-".len();
    d.edit(at, at + 2, "31");
    d.build_visual(80);
    assert_eq!(
        d.vmap.regions[0].directive.as_ref().unwrap().label,
        "2026-10-31"
    );
    // An edit above shifts the reused block, and the regions with it.
    d.place_caret(0, false);
    d.insert("Z");
    d.build_visual(80);
    assert_eq!(
        &d.source[d.vmap.regions[1].src..d.vmap.regions[1].end],
        ":date[2026-11-01]{tz=UTC}"
    );
    // Full shows the caret's line as an unowned directive is drawn: its
    // label, to be typed into.
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "Due");
    let line = &drawn_rows(&d)[0];
    assert!(
        line.contains("2026-10-31") && !line.contains('▤'),
        "{line:?}"
    );
    assert!(d.vmap.regions.is_empty());
}

#[test]
fn an_inline_directive_draws_as_before_where_it_is_not_owned_or_cannot_be_a_picture() {
    let src = "Due :date[2026-10-12] and :vis{.family} done\n\nend\n";
    // Not owned, on a picture surface (the web, Apple).
    let mut d = doc_in(View::Wysiwyg, "inline_directive_unowned", src);
    d.set_inline_pictures(true);
    caret_at(&mut d, "end");
    let before = drawn_rows(&d);
    d.set_owned_regions(&["directive:other", "math"]);
    caret_at(&mut d, "end");
    assert_eq!(drawn_rows(&d), before);
    assert!(d.vmap.regions.is_empty());
    // Owned, on a surface that paints nothing in a line (the terminal,
    // Android): drawn as it always was.
    let mut t = doc_in(View::Wysiwyg, "inline_directive_terminal", src);
    caret_at(&mut t, "end");
    let terminal = drawn_rows(&t);
    t.set_owned_regions(&["directive:date", "directive:vis"]);
    caret_at(&mut t, "end");
    assert_eq!(drawn_rows(&t), terminal);
    assert!(t.vmap.regions.is_empty());
    assert!(
        terminal[0].contains("2026-10-12") && terminal[0].contains("⧉ vis"),
        "{terminal:?}"
    );
}

#[test]
fn an_owned_inline_directive_wraps_with_its_atom() {
    let words = "word ".repeat(30);
    let src = format!("{words}:date[2026-10-12] {words}\n");
    let mut d = doc_in(View::Wysiwyg, "inline_directive_wrapped", &src);
    d.set_inline_pictures(true);
    d.set_owned_regions(&["directive:date"]);
    d.place_caret(0, false);
    d.build_visual(40);
    let [r] = d
        .vmap
        .regions
        .clone()
        .try_into()
        .unwrap_or_else(|v| panic!("one region: {v:?}"));
    let row = &d.vmap.rows[r.rows_span.start];
    assert!(
        row.glyphs.iter().any(|g| g.ch == '▤' && g.src == r.src),
        "the region's row lacks its atom"
    );
    assert_eq!(&d.source[r.src..r.end], ":date[2026-10-12]");
}

#[test]
fn an_owned_directive_is_a_region_of_its_whole_source() {
    let src = "intro\n\n::embed[Card]{src=\"a.md\" wide}\n\n::toc\n\n::page-break\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "directive_owned", src);
    caret_at(&mut d, "end");
    let before = drawn_rows(&d);
    assert!(d.vmap.regions.is_empty());
    d.set_owned_regions(&["directive:embed", "directive:page-break"]);
    caret_at(&mut d, "end");
    assert_eq!(
        drawn_rows(&d),
        before,
        "owning a directive changed its rows"
    );
    let [r] = d
        .vmap
        .regions
        .clone()
        .try_into()
        .unwrap_or_else(|v| panic!("one region: {v:?}"));
    assert_eq!(r.kind, "directive:embed");
    assert!(!r.inline && !r.display);
    assert_eq!(&d.source[r.src..r.end], "::embed[Card]{src=\"a.md\" wide}");
    assert_eq!(r.content, r.src..r.end);
    let mark = r.directive.as_ref().expect("its directive");
    assert_eq!(mark.name, "embed");
    assert_eq!(mark.label, "Card");
    assert_eq!(
        mark.attrs,
        vec![
            ("src".to_string(), Some("a.md".to_string())),
            ("wide".to_string(), Some(String::new()))
        ]
    );
    assert_eq!(r.rows_span, d.vmap.directives[0].rows_span);
    // The caret rests either side of it, as before.
    d.place_caret(r.src, false);
    d.move_right(false);
    assert_eq!(d.caret, r.end);
    // An edit through its content is an edit of the directive.
    let at = r.src + "::embed[".len();
    d.edit(at, at + 4, "Album");
    d.build_visual(80);
    let r2 = d.vmap.regions[0].clone();
    assert_eq!(r2.directive.expect("its directive").label, "Album");
    assert_eq!(&d.source[r2.content], "::embed[Album]{src=\"a.md\" wide}");
}

#[test]
fn a_fence_inside_a_quote_is_not_owned() {
    let src = "> ```mermaid\n> graph TD\n> ```\n\nend\n";
    let mut d = doc_in(View::Wysiwyg, "fence_quoted", src);
    d.set_owned_regions(&["fence:mermaid"]);
    caret_at(&mut d, "end");
    assert!(d.vmap.regions.is_empty(), "{:?}", drawn_rows(&d));
    assert!(drawn_rows(&d).iter().any(|r| r.contains("graph TD")));
}

#[test]
fn set_math_rows_reserves_filler_rows_by_tex() {
    let mut d = doc_in(View::Wysiwyg, "math_rows", "$$\nx\n$$\n\nend\n");
    caret_at(&mut d, "end");
    assert_eq!(d.vmap.math[0].rows_span, 0..1);
    d.set_math_rows(HashMap::from([(d.vmap.math[0].tex.clone(), 3)]));
    d.build_visual(80);
    assert_eq!(d.vmap.math[0].rows_span, 0..3);
    assert_eq!(drawn_rows(&d)[..3], ["∑ x", "", ""]);
    // Cheap when nothing changed.
    let key = d.visual_key();
    d.set_math_rows(HashMap::from([(d.vmap.math[0].tex.clone(), 3)]));
    d.build_visual(80);
    assert_eq!(d.visual_key(), key);
}

#[test]
fn a_dollar_typed_in_shortcuts_authors_math() {
    let mut d = doc_in(View::Wysiwyg, "math_dollar_sc", "\n");
    d.set_markup_mode(MarkupMode::Shortcuts);
    d.caret = 0;
    d.insert("$x$");
    assert_eq!(d.source, "$x$\n");
    d.caret = 1;
    assert_eq!(d.breadcrumb(), "doc › para › inline_math");

    // `None` keeps typed syntax literal, and under the math extension a
    // `$` is syntax: twig escapes it, and the paragraph stays text.
    let mut d = doc_in(View::Wysiwyg, "math_dollar_none", "\n");
    assert_eq!(d.markup_mode(), MarkupMode::None);
    d.caret = 0;
    d.insert("$x$");
    assert_eq!(d.source, "\\$x\\$\n");
    d.caret = 2;
    assert_eq!(d.breadcrumb(), "doc › para › str");
}

#[test]
fn counts_see_a_formula_as_a_picture_however_it_is_written() {
    let c = counts_of(
        "counts_math",
        "the sum $\\sum_i x_i$ and\n\n$$\ny = mx + c\n$$\n",
    );
    // `the sum … and` is three words; neither formula counts, and the
    // display block is not a paragraph of text.
    assert_eq!(c.words, 3);
    assert_eq!(c.paragraphs, 1);
}
