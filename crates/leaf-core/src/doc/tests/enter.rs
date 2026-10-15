use super::*;

/// How many list items the source actually parses into — the check that a
/// marker Leaf wrote is a marker the format agrees is one.
fn list_items(doc: &mut Doc) -> usize {
    doc.editor
        .nodes()
        .unwrap()
        .iter()
        .filter(|n| n.kind == Kind::ListItem || n.kind == Kind::TaskListItem)
        .count()
}

#[test]
fn enter_at_a_paragraph_s_start_opens_an_empty_paragraph_above_it() {
    // twig's split at a paragraph's start writes one newline ahead of it,
    // which Fold draws as one more gap, so the first Enter showed nothing.
    // A paragraph break opens a line above, the caret staying before the
    // text. Past a mark's opening delimiters — where a tap before the
    // first letter lands — the split used to cut the mark in two
    // (`**\n\nb**`); the break goes in front of them, as it goes in
    // front of a heading's `#`, where the heading's own Enter left an
    // empty heading over its text as a paragraph. A paragraph that
    // opens a div gets its line outside the div, where it draws.
    for (body, caret, want, want_caret) in [
        ("a\n\nb\n", 3, "a\n\n\n\nb\n", 5),
        ("# H\n\nb\n", 5, "# H\n\n\n\nb\n", 7),
        ("a\n\n# H\n", 5, "a\n\n\n\n# H\n", 7),
        ("a\n\n**b** c\n", 3, "a\n\n\n\n**b** c\n", 5),
        ("a\n\n**b** c\n", 5, "a\n\n\n\n**b** c\n", 7),
        (
            "a\n\n<span data-color=\"orange\">b</span>\n",
            29,
            "a\n\n\n\n<span data-color=\"orange\">b</span>\n",
            31,
        ),
        (
            "a\n\n<div class=\"center\">\n\nb\n\n</div>\n",
            25,
            "a\n\n\n\n<div class=\"center\">\n\nb\n\n</div>\n",
            27,
        ),
    ] {
        let mut d = wysiwyg_doc("enter_para_start", body);
        d.build_visual(80);
        d.caret = caret;
        d.newline();
        assert_eq!(d.source, want, "{body:?}");
        assert_eq!(d.caret, want_caret, "{body:?}");
        d.build_visual(80);
        let (row, _) = d.vmap.pos_of_offset(d.caret);
        assert!(
            d.vmap.rows[row - 2].glyphs.is_empty(),
            "{body:?}: an empty line above"
        );
        assert!(
            !d.vmap.rows[row - 2].decoration,
            "{body:?}: one the caret can stand on"
        );
    }
    // Past the first letter it is an ordinary split.
    let mut d = wysiwyg_doc("enter_para_mid", "a\n\n**b** c\n");
    d.caret = 8;
    d.newline();
    assert_eq!(d.source, "a\n\n**b**\n\nc\n");
}

#[test]
fn enter_again_at_a_paragraph_s_start_opens_one_more_line() {
    // The first Enter's paragraph break leaves a gap either side of the
    // empty line. A second break drew two lines more; a newline draws one.
    let mut d = wysiwyg_doc("enter_para_again", "a\n\nb\n");
    d.build_visual(80);
    d.caret = 3;
    for (want, lines) in [("a\n\n\n\nb\n", 1), ("a\n\n\n\n\nb\n", 2)] {
        d.newline();
        assert_eq!(d.source, want);
        assert_eq!(&d.source[d.caret..], "b\n");
        d.build_visual(80);
        let empty = d
            .vmap
            .rows
            .iter()
            .filter(|r| r.glyphs.is_empty() && !r.decoration)
            .count();
        assert_eq!(empty, lines, "{want:?}");
    }
}

#[test]
fn enter_at_the_first_block_s_start_pushes_it_down() {
    // No row was drawn for a blank line above the first block, so Enter
    // there wrote a line and the text stayed where it was. Each Enter now
    // draws one empty line above it, the caret staying with the text;
    // past frontmatter, the line conventionally under it still draws
    // nothing until Enter adds to it.
    for (body, caret, wants) in [
        ("b\n", 0, ["\n\nb\n", "\n\n\nb\n"]),
        ("**b** c\n", 2, ["\n\n**b** c\n", "\n\n\n**b** c\n"]),
        ("# H\n", 2, ["\n\n# H\n", "\n\n\n# H\n"]),
        (
            "---\nt: x\n---\n\nb\n",
            14,
            ["---\nt: x\n---\n\n\nb\n", "---\nt: x\n---\n\n\n\nb\n"],
        ),
    ] {
        let mut d = wysiwyg_doc("enter_first_block", body);
        d.build_visual(80);
        let empty = |d: &Doc| {
            d.vmap
                .rows
                .iter()
                .filter(|r| r.glyphs.is_empty() && !r.decoration)
                .count()
        };
        assert_eq!(empty(&d), 0, "{body:?}: nothing above the text yet");
        d.caret = caret;
        for (lines, want) in wants.into_iter().enumerate() {
            d.newline();
            assert_eq!(d.source, want, "{body:?}");
            assert!(d.source[d.caret..].starts_with(&body[caret..]), "{body:?}");
            d.build_visual(80);
            assert_eq!(empty(&d), lines + 1, "{want:?}");
        }
        // The caret can go up onto the lines it opened.
        d.move_up(false);
        let (row, _) = d.vmap.pos_of_offset(d.caret);
        assert!(d.vmap.rows[row].glyphs.is_empty(), "{body:?}");
        assert!(!d.vmap.rows[row].decoration, "{body:?}");
    }
}

/// What a reader sees of a map: each row's text, whether it is a gap,
/// and the presentation it is drawn with. Offsets are left out, since an
/// edit shifts them without anything on screen changing.
fn drawn(d: &Doc) -> Vec<String> {
    d.vmap
        .rows
        .iter()
        .map(|r| {
            let text: String = r.glyphs.iter().map(|g| g.ch).collect();
            format!(
                "{}{text:?} h={:?} lh={:?} a={:?}",
                if r.decoration { "~" } else { "" },
                r.heading,
                r.line_height,
                r.align
            )
        })
        .collect()
}

#[test]
fn enter_and_backspace_anywhere_show_what_they_write_and_undo_in_one_step() {
    // Enter that writes what the view does not draw looks like a key that
    // did nothing, so it gets pressed again, and the lines pile up unseen
    // until something redraws them all at once. The rule, the page break,
    // a paragraph's start, the first block's start and the blank page
    // were each that bug once. So: at every place the caret can stand, in
    // both flows, Enter either changes what is drawn or writes nothing,
    // and one Undo takes back whatever it wrote. Enter and Backspace both
    // leave the view a fresh open of the text would draw, so nothing
    // moves when the view is next rebuilt from scratch.
    let cases: &[(Format, &str)] = &[
        (
            Format::Markdown,
            "I talk to myself\n\nI laugh\n\nI cry\n\nknees to nose\n\nlet me go\n",
        ),
        (Format::Markdown, "I talk to myself\nI laugh\nI cry\n"),
        (
            Format::Markdown,
            "<div data-line-height=\"1.5\">\n\nI laugh\n\nI cry\n\nknees to nose\n\n</div>\n",
        ),
        (
            Format::Markdown,
            "<div data-line-height=\"1.15\">\n\nI cry\n\n</div>\n\n<div data-line-height=\"1.15\">\n\nknees\n\n</div>\n",
        ),
        (
            Format::Markdown,
            "<div class=\"center\">\n\nmiddle\n\n</div>\n\nafter\n",
        ),
        (
            Format::Markdown,
            "---\ntitle: x\n---\n\n# Title\n\nbody **bold** and <span data-color=\"red\">red</span>\n",
        ),
        (Format::Markdown, "\n\n\nafter blank lines\n\n\n\nmid\n\n\n"),
        (
            Format::Markdown,
            "- one\n- two\n\n1. a\n2. b\n\n- [ ] task\n",
        ),
        (Format::Markdown, "> quoted\n> more\n\n> - in a quote\n"),
        (Format::Markdown, "a\n\n---\n\nb\n\n::page-break\n\nc\n"),
        (Format::Markdown, "Title\n=====\n\nSub\n---\n\ntext\n"),
        (
            Format::Markdown,
            "```\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        ),
        (Format::Djot, "I laugh\n\nI cry\n\nknees to nose\n"),
        (
            Format::Djot,
            "{data-line-height=\"2\"}\nI cry\n\n{data-line-height=\"2\"}\nknees to nose\n",
        ),
        (Format::Djot, "# Title\n\na\n\n* * *\n\nb\n"),
        // HTML is not here: Enter writes blank lines there, which the
        // view draws and HTML does not keep. See
        // docs/tasks/enter-in-html-writes-whitespace.md.
    ];
    let mut failures = Vec::new();
    for &(format, body) in cases {
        for flow in [LineFlow::Fold, LineFlow::Preserve] {
            let open = || {
                let mut d = Doc::from_source(body.into(), format).unwrap();
                d.view = View::Wysiwyg;
                d.set_line_flow(flow);
                d.build_visual_unwrapped();
                d
            };
            // The map a fresh open of the same text would draw. The
            // edited map differing from it is a view that changes on the
            // next full rebuild, seconds after the key, with no key.
            let fresh = |source: &str| {
                let mut ed =
                    twig::Editor::new_ext(source.as_bytes(), format, parse_extensions()).unwrap();
                let nodes = ed.nodes().unwrap();
                crate::wysiwyg::build(
                    &nodes,
                    source,
                    None,
                    flow == LineFlow::Preserve,
                    &wysiwyg::Surface::default(),
                    None,
                )
            };
            let probe = open();
            let stops: Vec<usize> = (0..=body.len())
                .filter(|&o| probe.vmap.is_stop(o))
                .collect();
            for at in stops {
                for enter in [true, false] {
                    let mut d = open();
                    d.caret = at;
                    d.anchor = None;
                    let before = drawn(&d);
                    if enter {
                        d.newline();
                    } else {
                        d.backspace();
                    }
                    d.build_visual_unwrapped();
                    let key = if enter { "enter" } else { "backspace" };
                    let what = format!(
                        "{key} {format:?} {flow:?} caret {at} in {body:?} -> {:?}",
                        d.source
                    );
                    if maps_differ(&d.vmap, &fresh(&d.source)) {
                        failures.push(format!("redraws differently: {what}"));
                    }
                    if d.source == body {
                        continue;
                    }
                    if enter && drawn(&d) == before {
                        failures.push(format!("drew nothing: {what}"));
                    }
                    d.undo();
                    if d.source != body {
                        failures.push(format!("undo left {:?}: {what}", d.source));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn enter_opens_a_line_that_draws_inside_a_div_a_quote_and_preserve_flow() {
    // Each of these wrote a line the view did not draw, so Enter looked
    // dead and every press left one more.
    let enter = |format, flow, body: &str, at: usize| {
        let mut d = Doc::from_source(body.into(), format).unwrap();
        d.view = View::Wysiwyg;
        d.set_line_flow(flow);
        d.build_visual(80);
        d.caret = at;
        d.newline();
        d.build_visual(80);
        d
    };
    let div = "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n</div>\n";
    let end = div.find("knees").unwrap() + "knees".len();

    // At the end of a div's last paragraph, the empty paragraph opens
    // inside the div, drawn with its line height, and what is typed
    // there stays in it.
    let mut d = enter(Format::Markdown, LineFlow::Fold, div, end);
    let (row, _) = d.vmap.pos_of_offset(d.caret);
    let r = &d.vmap.rows[row];
    assert!(r.glyphs.is_empty() && !r.decoration, "{:?}", drawn(&d));
    assert!(
        r.line_height.is_some(),
        "the new line keeps the div's spacing"
    );
    d.insert("x");
    assert_eq!(
        d.source,
        "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\nx\n\n</div>\n"
    );
    // Preserve flow's soft line there draws too.
    let d = enter(Format::Markdown, LineFlow::Preserve, div, end);
    assert_eq!(
        d.source,
        "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n\n</div>\n"
    );
    assert_eq!(d.vmap.rows.len(), 4, "{:?}", drawn(&d));
    // Between two of the div's paragraphs, the empty line takes its
    // spacing as well.
    let d = enter(
        Format::Markdown,
        LineFlow::Fold,
        div,
        div.find("knees").unwrap(),
    );
    let (row, _) = d.vmap.pos_of_offset(d.caret);
    assert!(
        d.vmap.rows[row - 2].line_height.is_some(),
        "{:?}",
        drawn(&d)
    );

    // Under `</div>` the blank line is the one Markdown needs to end the
    // div. Preserve flow drew it as a line to type on, and Backspace
    // there glued the next paragraph to the tag (`</div>after`).
    let below = "<div class=\"center\">\n\nmiddle\n\n</div>\n\nafter\n";
    let mut d = Doc::from_source(below.into(), Format::Markdown).unwrap();
    d.view = View::Wysiwyg;
    d.set_line_flow(LineFlow::Preserve);
    d.build_visual(80);
    assert_eq!(d.vmap.rows.len(), 2, "{:?}", drawn(&d));
    d.caret = below.find("after").unwrap();
    d.backspace();
    assert!(!d.source.contains("</div>after"), "{:?}", d.source);

    // At a quote's first line, the line opens above the quote; twig's
    // split wrote a quoted blank line above the text, which drew nothing.
    let d = enter(
        Format::Markdown,
        LineFlow::Fold,
        "a\n\n> quoted\n> more\n",
        5,
    );
    assert_eq!(d.source, "a\n\n\n\n> quoted\n> more\n");
    assert_eq!(&d.source[d.caret..], "quoted\n> more\n");

    // Preserve flow at a paragraph's start: past a mark's delimiters its
    // newline cut the mark (`**\nb**`), and in djot a heading ran on
    // over it unseen.
    let d = enter(Format::Markdown, LineFlow::Preserve, "a\n\n**b** c\n", 5);
    assert_eq!(d.source, "a\n\n\n**b** c\n");
    let d = enter(Format::Djot, LineFlow::Preserve, "a\n\n# Title\n", 5);
    assert_eq!(d.source, "a\n\n\n# Title\n");
    // Inside a heading it parts the heading, as in Fold flow.
    let d = enter(Format::Djot, LineFlow::Preserve, "a\n\n# Title\n", 7);
    assert_eq!(d.source, "a\n\n# Ti\n\ntle\n");
}

#[test]
fn enter_on_a_blank_page_writes_nothing() {
    // With no block to push down, Fold flow draws no line for Enter to
    // open, and the newline it wrote was an edit and an undo step that
    // showed nothing. Preserve flow draws every line, so there it writes.
    for body in ["", "\n", "\n\n  \n", "---\nt: x\n---\n"] {
        let mut d = wysiwyg_doc("enter_blank_page", body);
        d.build_visual(80);
        d.caret = d.source.len();
        d.newline();
        assert_eq!(d.source, body);
        assert!(!d.can_undo(), "{body:?}");
        assert!(!d.dirty, "{body:?}");
    }
    let mut d = wysiwyg_doc("enter_blank_page_preserve", "");
    d.set_line_flow(LineFlow::Preserve);
    d.build_visual(80);
    d.newline();
    assert_eq!(d.source, "\n");
}

#[test]
fn enter_beside_a_rule_opens_a_line_under_it() {
    // Beside a rule the caret stands at the home past it, which in source is
    // the start of the blank line under the rule, and Enter took it for an
    // empty paragraph: a lone newline, drawn as one more gap, and the caret
    // put on the next block's first line — where typing joined that block.
    // It goes on under the rule instead, as the rule button leaves it.
    let mut d = wysiwyg_doc("enter_rule", "a\n\n---\n\nb\n");
    d.build_visual(80);
    d.caret = "a\n\n---\n".len();
    d.newline();
    assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
    assert_eq!(d.caret, "a\n\n---\n\n".len());
    d.insert("X");
    assert_eq!(d.source, "a\n\n---\n\nX\n\nb\n");

    // Closing the document.
    let mut d = wysiwyg_doc("enter_rule_end", "a\n\n---\n");
    d.caret = d.source.len();
    d.newline();
    assert_eq!(d.source, "a\n\n---\n\n");
    assert_eq!(d.caret, d.source.len());

    // In a quote, where the lines it writes keep the quote's prefix.
    let mut d = wysiwyg_doc("enter_rule_quote", "> a\n>\n> ---\n>\n> b\n");
    d.caret = "> a\n>\n> ---\n".len();
    d.newline();
    assert_eq!(d.source, "> a\n>\n> ---\n>\n> \n>\n> b\n");
    d.insert("X");
    assert_eq!(d.source, "> a\n>\n> ---\n>\n> X\n>\n> b\n");

    // Preserve draws the blank line under a rule as a line, and the caret
    // on it: Enter there is a blank line's, one line down.
    let mut d = wysiwyg_doc("enter_rule_preserve", "a\n\n---\n\nb\n");
    d.set_line_flow(LineFlow::Preserve);
    d.caret = "a\n\n---\n".len();
    d.newline();
    assert_eq!(d.source, "a\n\n---\n\n\nb\n");
    assert_eq!(d.caret, "a\n\n---\n\n".len());
}

#[test]
fn insert_table_at_a_paragraph_s_end_splits_nothing() {
    // The same door as the rule's, through the placement they share.
    let mut d = Doc::from_source("para\n".into(), Format::Djot).unwrap();
    d.caret = 4;
    d.insert_table(1, 1);
    assert_eq!(d.source, "para\n\n|  |\n|---|\n|  |\n");
    let mut d = doc_with("table_end_typed", "para");
    d.caret = 4;
    d.insert_table(1, 1);
    assert_eq!(d.source, "para\n\n|  |\n| --- |\n|  |\n");
    assert!(d.caret_in_table());
}

#[test]
fn insert_thematic_break_spells_the_rule_the_format_s_own_way() {
    // The whole point of delegating: `---` is Markdown's, `* * *` is djot's,
    // and leaf wrote the first into both until twig started spelling it.
    let mut md = doc_with("hr_md", "para\n");
    md.caret = 2;
    md.insert_thematic_break();
    assert_eq!(md.source, "pa\n\n---\n\nra\n");

    let mut dj = Doc::from_source("para\n".into(), Format::Djot).unwrap();
    dj.caret = 2;
    dj.insert_thematic_break();
    assert_eq!(dj.source, "pa\n\n* * *\n\nra\n");
}

#[test]
fn insert_table_parts_the_paragraph_and_lands_in_the_first_header_cell() {
    // The table goes *at* the caret the way the rule does: the paragraph is
    // parted first, and twig writes the grid after its first half. The
    // caret then sits in the first header cell — selected, as Tab would
    // leave it — so the next keystroke is the heading.
    let mut d = doc_with("table_mid", "before after\n");
    d.caret = 7;
    d.insert_table(2, 3);
    assert_eq!(
        d.source,
        "before \n\n|  |  |  |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n\nafter\n"
    );
    assert!(d.caret_in_table());
    let first_bar = d.source.find('|').unwrap();
    assert!(
        d.caret > first_bar && d.caret < d.source.find("| ---").unwrap(),
        "caret {} is not in the header row",
        d.caret
    );
    d.insert("Name");
    assert!(d.source.starts_with("before \n\n| Name |  |  |\n"));
    // And the grid the table was written into is one the table keys walk
    // (over the map a frontend rebuilds after every edit).
    d.build_visual(80);
    assert!(d.cell_tab(true));
    d.insert("Qty");
    assert!(d.source.starts_with("before \n\n| Name | Qty |  |\n"));
}

#[test]
fn insert_table_spells_the_grid_the_format_s_own_way() {
    // Djot's delimiter row is unpadded, and leaf never has to know that.
    let mut dj = Doc::from_source("para\n".into(), Format::Djot).unwrap();
    dj.caret = 2;
    dj.insert_table(1, 2);
    assert_eq!(dj.source, "pa\n\n|  |  |\n|---|---|\n|  |  |\n\nra\n");
    assert!(dj.caret_in_table());
}

#[test]
fn insert_table_refuses_where_the_format_spells_no_table() {
    let mut d = Doc::from_source("<p>ab</p>\n".into(), Format::Html).unwrap();
    d.caret = 4;
    d.insert_table(1, 1);
    assert_eq!(d.source, "<p>ab</p>\n");
    assert!(d.status.as_deref().unwrap_or("").contains("not supported"));
    assert!(!d.capabilities().table);
}

#[test]
fn insert_table_reports_a_zero_shape_and_writes_nothing() {
    let mut d = doc_with("table_zero", "para\n");
    d.caret = 2;
    d.insert_table(0, 2);
    assert_eq!(d.source, "para\n");
    assert!(d.status.as_deref().unwrap_or("").starts_with("table:"));
}

#[test]
fn clicking_below_a_final_thematic_break_can_type_after_it() {
    let mut d = wysiwyg_doc("hr_final_click", "---\n");
    d.build_visual(80);
    d.click(d.vmap.num_rows() + 2, 0, false);
    assert_eq!(d.caret, d.source.len(), "the caret belongs after the rule");
    d.insert("after");
    assert_eq!(d.source, "---\nafter");
}

#[test]
fn enter_in_a_nested_list_item_keeps_the_new_item_nested() {
    // The same bytes are two documents. In Markdown `  - b` is a nested item
    // and the next one belongs beside it, at its indent. In Djot a list
    // marker can't interrupt a paragraph, so those bytes are literal text in
    // item `a` and there is only one item — writing `  - ` under it would add
    // no item at all, just more text, and the new sibling has to go to
    // column zero. Both spellings come out of the *enclosing item's* line.
    let mut md = wysiwyg_doc("enter_nested_md", "- a\n  - b\n");
    md.caret = "- a\n  - b".len();
    md.newline();
    assert_eq!(md.source, "- a\n  - b\n  - \n");
    assert_eq!(list_items(&mut md), 3);

    let mut dj = Doc::from_source("- a\n  - b\n".into(), Format::Djot).unwrap();
    dj.view = View::Wysiwyg;
    dj.build_visual(80);
    dj.caret = "- a\n  - b".len();
    dj.newline();
    assert_eq!(dj.source, "- a\n  - b\n- \n");
    assert_eq!(list_items(&mut dj), 2);

    // Where Djot's nesting is real — opened by a blank line — the indent is
    // reproduced there too, and the two formats agree again.
    let mut dj = Doc::from_source("- a\n\n  - b\n".into(), Format::Djot).unwrap();
    dj.view = View::Wysiwyg;
    dj.build_visual(80);
    dj.caret = "- a\n\n  - b".len();
    dj.newline();
    assert_eq!(dj.source, "- a\n\n  - b\n  - \n");
    assert_eq!(list_items(&mut dj), 3);
}

#[test]
fn tab_nests_an_item_at_the_column_its_own_marker_asks_for() {
    // Tab replaces the line's whole prefix with the one twig spells, so the
    // quote markers, the parent's indent and an ordered marker's extra
    // column are all its answer rather than leaf's arithmetic.
    for (name, body, caret, want) in [
        ("bullet", "- a\n- b\n", 6, "- a\n  - b\n"),
        ("ordered", "1. a\n2. b\n", 8, "1. a\n   1. b\n"),
        ("quoted", "> - a\n> - b\n", 10, "> - a\n>   - b\n"),
        // A checkbox is markup the item's own text wraps past, but a nested
        // list may only open at the *list* marker's column — four in from
        // there is a paragraph continuation, and `- [ ] a\n      - [ ] b`
        // parses as one item, not two.
        ("task", "- [ ] a\n- [ ] b\n", 14, "- [ ] a\n  - [ ] b\n"),
        (
            "quoted task",
            "> - [ ] a\n> - [ ] b\n",
            18,
            "> - [ ] a\n>   - [ ] b\n",
        ),
    ] {
        let mut doc = wysiwyg_doc(name, body);
        doc.caret = caret;
        doc.indent();
        assert_eq!(doc.source, want, "{name}");
        // The nesting is real, not just indented text.
        assert_eq!(list_items(&mut doc), 2, "{name}");
    }
}

#[test]
fn backspace_only_outdents_where_the_format_says_there_is_an_item() {
    // The same bytes, the two formats disagreeing, and a gesture that used
    // to read the bytes. `  - b` is a nested item in Markdown, so Backspace
    // at its marker outdents. In Djot a marker can't interrupt a paragraph,
    // so those bytes are literal text inside item `a` — there is nothing to
    // outdent, and treating them as a marker turned one item into two, a
    // structural edit from a keystroke that should delete one character.
    //
    // twig's `line_prefix` is what tells them apart: it reports the marker
    // on the Markdown line and nothing on the Djot one, which is a
    // continuation. No byte scan can reach that answer.
    let src = "- a\n  - b\n";
    let at = "- a\n  - ".len();

    let mut md = Doc::from_source(src.into(), Format::Markdown).unwrap();
    md.view = View::Wysiwyg;
    md.build_visual(80);
    md.caret = at;
    md.backspace();
    assert_eq!(md.source, "- a\n- b\n");
    assert_eq!(list_items(&mut md), 2);

    let mut dj = Doc::from_source(src.into(), Format::Djot).unwrap();
    dj.view = View::Wysiwyg;
    dj.build_visual(80);
    dj.caret = at;
    dj.backspace();
    assert_eq!(dj.source, "- a\n  -b\n"); // an ordinary character delete
    assert_eq!(list_items(&mut dj), 1); // and the structure is untouched
}

#[test]
fn enter_in_a_checklist_item_starts_another_unchecked_one() {
    // Leaf used to spell the next item from the marker bytes it scanned, and
    // its scanner stopped at the bullet — so Enter in a checklist wrote `- `
    // and dropped out of the checklist. twig reproduces the whole
    // continuation, and a fresh item is always unticked however the one above
    // it stands.
    for (name, body, want) in [
        ("unchecked", "- [ ] a\n", "- [ ] a\n- [ ] \n"),
        ("checked", "- [x] a\n", "- [x] a\n- [ ] \n"),
    ] {
        let mut doc = wysiwyg_doc(name, body);
        doc.caret = body.trim_end_matches('\n').len();
        doc.newline();
        assert_eq!(doc.source, want, "{name}");
        // Both items are checklist items — the new one is a box, not the
        // plain bullet the old marker scan left behind — and it is unticked
        // whichever way the one above it faces.
        let boxes: Vec<Option<bool>> = doc
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::TaskListItem)
            .map(|n| n.checked)
            .collect();
        assert_eq!(boxes.len(), 2, "{name}");
        assert_eq!(boxes[1], Some(false), "{name}");
    }
}

#[test]
fn a_split_takes_the_space_the_caret_was_in_front_of() {
    // Splicing a break at the caret strands the space the words were parted
    // at on the head of the second block, where it reads as an indent nobody
    // typed. twig's split consumes it.
    for (name, body, caret, want) in [
        ("para", "one two\n", 3, "one\n\ntwo\n"),
        ("item", "- one two\n", 5, "- one\n- two\n"),
        ("quote", "> one two\n", 5, "> one\n>\n> two\n"),
        // A heading takes leaf's own path, which has to match.
        ("heading", "# one two\n", 5, "# one\n\ntwo\n"),
    ] {
        let mut doc = wysiwyg_doc(name, body);
        doc.caret = caret;
        doc.newline();
        assert_eq!(doc.source, want, "{name}");
    }
}

#[test]
fn enter_at_the_end_of_a_heading_opens_a_paragraph() {
    // The one place leaf keeps its own break: `split_block` repeats the `#`,
    // and Enter after a title is how the body under it is asked for.
    let mut doc = wysiwyg_doc("head_enter", "# Title\n");
    doc.caret = "# Title".len();
    doc.newline();
    doc.insert("body");
    assert_eq!(doc.source, "# Title\n\nbody\n");
    assert_eq!(
        doc.nodes()
            .iter()
            .filter(|n| n.kind == Kind::Heading)
            .count(),
        1
    );
}

#[test]
fn enter_in_a_quoted_list_item_starts_the_next_quoted_item() {
    // A quoted item's marker doesn't open its line, so a scan that starts at
    // column zero finds a `>` where it wanted a bullet, calls the line "not a
    // list" and hands Enter to the plain-quote branch — which writes `> ` and
    // drops the list. The next item has to carry the whole prefix.
    for (name, body, want) in [
        ("flat", "> - a\n", "> - a\n> - \n"),
        ("sibling", "> - a\n> - b\n", "> - a\n> - b\n> - \n"),
        ("nested", "> - a\n>   - b\n", "> - a\n>   - b\n>   - \n"),
        ("ordered", "> 1. a\n> 2. b\n", "> 1. a\n> 2. b\n> 3. \n"),
        ("twice quoted", "> > - a\n", "> > - a\n> > - \n"),
    ] {
        let mut doc = wysiwyg_doc(name, body);
        doc.caret = body.trim_end_matches('\n').len();
        doc.newline();
        assert_eq!(doc.source, want, "{name}");
        // The marker isn't just spelled right, it parses as an item.
        assert_eq!(list_items(&mut doc), body.lines().count() + 1, "{name}");
    }
}

#[test]
fn an_empty_quoted_item_leaves_the_list_and_stays_in_the_quote() {
    // Double-Enter exits the list. Unquoted that means a blank line, but a
    // *bare* blank line would end the quote too and drop the caret out of it,
    // so the separator keeps its `>` and the caret's line keeps its `> `.
    let mut doc = wysiwyg_doc("quoted_exit", "> - a\n> - \n");
    doc.caret = "> - a\n> - ".len();
    doc.newline();
    assert_eq!(doc.source, "> - a\n>\n> \n");
    assert_eq!(list_items(&mut doc), 1);
    // What "still in the quote" means for the next keystroke: the caret sits
    // behind the prefix, and what's typed there lands inside the quote as a
    // paragraph of its own — not as more of item `a`.
    doc.insert("x");
    assert_eq!(doc.source, "> - a\n>\n> x\n");
    assert!(
        doc.editor
            .ancestors_at(doc.caret - 1)
            .is_ok_and(|c| c.into_iter().any(|m| m.kind == Kind::BlockQuote))
    );
}

#[test]
fn backspace_at_a_quoted_marker_takes_the_marker_and_leaves_the_quote() {
    // The marker is hidden block markup, so Backspace over it is structural —
    // but only the marker is the list's. Splicing from the line start would
    // take the `>` with it and silently unquote the line.
    let mut doc = wysiwyg_doc("quoted_bksp", "> - a\n");
    doc.caret = "> - ".len();
    doc.backspace();
    assert_eq!(doc.source, "> a\n");
    assert_eq!(list_items(&mut doc), 0);

    // A nested one outdents instead, moving the bullet within the quote
    // rather than moving the quote.
    let mut doc = wysiwyg_doc("quoted_outdent", "> - a\n>   - b\n");
    doc.caret = "> - a\n>   - ".len();
    doc.backspace();
    assert_eq!(doc.source, "> - a\n> - b\n");
    assert_eq!(list_items(&mut doc), 2);
}

#[test]
fn only_a_bare_paragraph_is_parted_around_the_caret() {
    // The split is deliberately narrow. Parting a fenced block would leave
    // two fences with a rule between them, and parting a list item would
    // mint an item nobody asked for on the way to a rule that lands after
    // the list either way — so both keep the whole block intact and take the
    // rule after it. A caret in a quote is likewise left alone.
    for (name, body, caret, want) in [
        (
            "code",
            "```\nfn x() {}\n```\n",
            8,
            "```\nfn x() {}\n```\n\n---\n\n",
        ),
        ("list", "- one two\n", 6, "- one two\n\n---\n\n"),
        ("quote", "> one two\n", 6, "> one two\n>\n> ---\n>\n> \n"),
    ] {
        let mut d = doc_with(&format!("hr_narrow_{name}"), body);
        d.caret = caret;
        d.insert_thematic_break();
        assert_eq!(d.source, want, "{name}: the block should stay whole");
    }
}

#[test]
fn insert_thematic_break_replaces_the_selection() {
    // Now that the rule lands *at* the caret again, replacing the selection
    // is coherent once more: the text goes, and the rule takes its place.
    // The space the deletion left leading the second half is consumed by the
    // split rather than opening the new paragraph with it.
    let mut d = doc_with("hr_sel", "one two three\n");
    d.anchor = Some(4);
    d.caret = 7; // "two"
    d.insert_thematic_break();
    assert_eq!(d.source, "one \n\n---\n\nthree\n");
    assert_eq!(d.selection(), None);
}

#[test]
fn insert_thematic_break_clears_a_code_block_and_a_table_rather_than_refusing() {
    // Both are blocks the rule lands *after*. Leaf used to refuse a fence,
    // because writing `---` into one is code, not a rule — twig now walks out
    // to the block that owns the caret's line, so there is nothing to refuse.
    let mut code = doc_with("hr_code", "```\nfn x() {}\n```\n");
    code.caret = 5; // inside the fenced code
    code.insert_thematic_break();
    assert_eq!(code.source, "```\nfn x() {}\n```\n\n---\n\n");
    assert_eq!(code.status, None, "no refusal to report any more");

    let mut table = doc_with("hr_table", "| a | b |\n|---|---|\n| 1 | 2 |\n");
    table.caret = 3; // in the header row
    table.insert_thematic_break();
    assert_eq!(table.source, "| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n");
}

#[test]
fn insert_thematic_break_in_a_list_item_ends_the_list() {
    // The un-indented rule cannot continue the list, so it closes the list
    // and lands at the top level rather than nested inside it.
    let mut d = doc_with("hr_list", "- one\n- two\n");
    d.caret = "- one\n- tw".len(); // mid "two"
    d.insert_thematic_break();
    d.build_visual(80);
    let rule_at = d.source.find("---").unwrap();
    assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
    assert!(
        !d.nodes()
            .iter()
            .any(|n| n.kind == Kind::BulletList && n.span.start <= rule_at && rule_at < n.span.end),
        "the rule must not be nested inside the list"
    );
}

#[test]
fn insert_thematic_break_in_a_blockquote_stays_in_the_quote() {
    // Leaf used to end the quote. twig gives the rule the quote's own prefix,
    // which is the document the gesture was actually asked for — and the
    // line the caret goes on to under it wears the prefix too, as the line
    // Enter opens in a quote does.
    let mut d = doc_with("hr_quote", "> hello\n");
    d.caret = 4; // inside the quoted text
    d.insert_thematic_break();
    assert_eq!(d.source, "> hello\n>\n> ---\n>\n> \n");
    assert_eq!(d.caret, "> hello\n>\n> ---\n>\n> ".len());
    d.build_visual(80);
    let rule_at = d.source.find("---").unwrap();
    assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
    assert!(
        d.nodes()
            .iter()
            .any(|n| n.kind == Kind::BlockQuote && n.span.start <= rule_at && rule_at < n.span.end),
        "the rule belongs to the quote it was asked for"
    );
}
