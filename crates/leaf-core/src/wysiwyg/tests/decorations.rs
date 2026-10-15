use super::*;

#[test]
fn a_markdown_highlight_is_a_mark_and_a_coloured_one_names_its_colour() {
    // twig 3.3's `highlight`/`highlight_colors`, which `parse_extensions`
    // turns on for every leaf document: `==text==` is a `mark` in Markdown
    // and not the literal `==` it used to be, and `==🔴 text==` is one
    // carrying a colour.
    //
    // `doc_built` rather than `map`, deliberately — the extensions are
    // leaf's choice, not twig's default, so a test that parsed bare
    // Markdown here would be testing a document leaf never builds.
    let doc = doc_built("Plain ==yes== and ==🔴 red== ok\n");
    assert_eq!(role_of(&doc.vmap, 'y'), Role::Mark(None));
    assert_eq!(
        role_of(&doc.vmap, 'r'),
        Role::Mark(Some(MarkColor::Red)),
        "the `data-color` twig stripped the emoji into"
    );
    assert_eq!(role_of(&doc.vmap, 'P'), Role::Body);
}

#[test]
fn the_emoji_that_named_a_highlight_is_markup_and_never_drawn() {
    // The colour is *spelling*: twig strips the emoji out of the mark's
    // content, so the reader sees the words and the wash, never the circle.
    // Drawing it would put a character in the rendered text that the author
    // wrote as syntax — the same mistake as drawing an emphasis's `*`.
    let doc = doc_built("Plain ==yes== and ==🔴 red== ok\n");
    let drawn: String = doc.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(drawn, "Plain yes and red ok");
}

#[test]
fn a_superscript_and_a_subscript_sit_off_the_baseline() {
    // Regression: both rendered flat, so the toolbar's superscript button
    // produced markup that looked exactly like the text around it.
    let m = map_djot("H~2~O and x^2^\n");
    assert_eq!(baselines_of(&m, '2'), vec![Baseline::Sub, Baseline::Super]);
    assert_eq!(baselines_of(&m, 'H'), vec![Baseline::Normal]);
    assert_eq!(baselines_of(&m, 'O'), vec![Baseline::Normal]);
}

#[test]
fn a_raised_glyph_keeps_the_style_it_was_raised_out_of() {
    // Why this is a `Baseline` and not a `Role`: raising a glyph says where
    // it sits, and must not cost it what it already was.
    let m = map_djot("# Heading x^2^\n");
    let two = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .find(|g| g.ch == '2')
        .unwrap();
    assert_eq!(two.style.baseline, Baseline::Super);
    assert_eq!(two.style.role, Role::Heading(1), "still heading text");
}

#[test]
fn a_footnote_references_brackets_are_decoration_and_only_its_label_is_a_stop() {
    let src = "see[^note] here\n";
    let m = map(src);
    // `[^note]` spans 3..10, its label `note` 5..9. The caret walks the
    // label; the brackets are drawn but never stood on, as a table's are,
    // and the `[^`/`]` bytes are stepped over like any hidden delimiter.
    let stops: Vec<usize> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| g.src)
        .collect();
    for off in 5..9 {
        assert!(
            stops.contains(&off),
            "label byte {off} isn't a caret stop: {stops:?}"
        );
    }
    for off in [3usize, 4, 9] {
        assert!(
            !stops.contains(&off),
            "delimiter byte {off} is a caret stop: {stops:?}"
        );
    }
}

#[test]
fn a_task_item_draws_its_box_where_the_bullet_would_be() {
    // Regression: the `[ ] ` is markup twig consumes — the item's paragraph
    // content starts past it — so a task item used to render as `• todo`,
    // identical to a plain bullet and with no way to see it was ticked.
    let m = map("- [ ] todo\n- [x] done\n- plain\n");
    assert_eq!(rendered(&m), "☐ todo\n☑ done\n• plain");

    // The tick rides the item's first row, for a GUI that paints its own box.
    let ticks: Vec<Option<bool>> = m.rows.iter().map(|r| r.task).collect();
    assert_eq!(ticks, [Some(false), Some(true), None]);
}

#[test]
fn a_task_items_box_survives_a_wrap_and_marks_only_the_first_row() {
    let m = map_at(
        "- [x] a much longer task that has to wrap somewhere\n",
        Some(20),
    );
    assert!(m.rows.len() > 1, "the item should wrap: {:?}", rendered(&m));
    assert_eq!(m.rows[0].task, Some(true));
    assert!(
        m.rows[1..].iter().all(|r| r.task.is_none()),
        "only the first row"
    );
    // The continuation lines hang under the box, not under column zero.
    assert!(
        rendered(&m)
            .lines()
            .nth(1)
            .is_some_and(|l| l.starts_with("  "))
    );
}

#[test]
fn a_bracket_in_an_items_prose_is_not_a_checkbox() {
    // `task_checked` finds the box past the list marker; a plain item whose
    // text merely contains a bracket has none, and must keep its bullet.
    let m = map("- see [1] below\n");
    assert_eq!(rendered(&m), "• see [1] below");
    assert_eq!(m.rows[0].task, None);
}
