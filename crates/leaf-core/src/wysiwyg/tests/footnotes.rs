use super::*;

#[test]
fn a_footnote_reference_keeps_its_paragraph_visible() {
    // Regression: `footnote_reference` was in neither `is_inline_kind` nor
    // the inline walker, so a paragraph carrying one failed the "all children
    // inline" test, was walked as a container of blocks, and rendered as
    // empty rows with no caret stop anywhere — the whole line vanished.
    let src = "A claim[^1] and more.\n";
    let m = map(src);
    assert_eq!(rendered(&m).trim_end(), "A claim[1] and more.");
    // The `^` is spelling, not text: hidden the way a link's `](dest)` is.
    assert!(!rendered(&m).contains('^'));
}

#[test]
fn a_footnote_reference_is_raised_and_the_prose_around_it_is_not() {
    // What makes `[1]` read as a reference rather than as bracketed text.
    // The brackets ride with the label: the chip is one raised mark.
    let m = map("A claim[^1] and more.\n");
    assert_eq!(baselines_of(&m, '1'), vec![Baseline::Super]);
    assert_eq!(baselines_of(&m, '['), vec![Baseline::Super]);
    assert_eq!(baselines_of(&m, ']'), vec![Baseline::Super]);
    assert_eq!(baselines_of(&m, 'A'), vec![Baseline::Normal]);
}

#[test]
fn a_reference_drops_the_number_its_paragraph_opens_with() {
    // Labels are unique per document, so a chapter spells its notes `20a`,
    // `21a`; inside verse 20 the `20` is said already. Only the matching
    // number goes, and only when letters are left.
    let src = "{#v20}\n*20* And they did plead[^20a] and[^21b] wept[^20].\n\n[^20a]: note.\n\n[^21b]: note.\n\n[^20]: note.\n";
    let m = map_djot(src);
    let first = rendered(&m).lines().next().unwrap().to_string();
    assert_eq!(first, "20 And they did plead[a] and[21b] wept[20].");
    // The letter maps to its own byte, and the hidden `20` is no stop.
    let a = src.find("[^20a]").unwrap() + 4;
    let stops: Vec<usize> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| g.src)
        .collect();
    assert!(stops.contains(&a));
    assert!(!stops.contains(&(a - 2)) && !stops.contains(&(a - 1)));
    // The note keeps its whole label: nothing else says which verse it is for.
    assert!(rendered(&m).contains("[20a] note."), "{}", rendered(&m));
}

#[test]
fn a_block_attribute_between_paragraphs_is_not_a_second_gap() {
    // `{#v21}` is the next paragraph's, not a blank line: one boundary row.
    let m = map_djot("{#v20}\nTwenty.\n\n{#v21}\nTwenty-one.\n");
    let gaps = m.rows.iter().filter(|r| r.boundary.is_some()).count();
    assert_eq!(gaps, 1, "{:?}", rendered(&m));
    assert_eq!(m.rows.len(), 3);
}

#[test]
fn a_footnote_reference_keeps_the_link_role_it_had() {
    // The raised baseline is added to the role, not swapped for it: every
    // frontend already paints `Role::Link`, and a reference is one.
    let m = map("A claim[^1].\n");
    let label = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .find(|g| g.ch == '1')
        .unwrap();
    assert_eq!(label.style.role, Role::Link);
    assert_eq!(label.style.baseline, Baseline::Super);
}

#[test]
fn a_footnote_definition_renders_where_it_was_written() {
    // Regression: twig parses `[^1]: …` as a root *beside* `doc` — not a
    // child of it — so the walk from `doc` never reached one and every byte
    // of the note's body rendered as nothing at all.
    let src = "A claim[^1].\n\n[^1]: The note body.\n\nAfter.\n";
    let m = map(src);
    let text = rendered(&m);
    assert!(
        text.contains("The note body."),
        "the note body is invisible: {text:?}"
    );
    // In source order — between the paragraph that cites it and the one
    // after — not hoisted to the end, and marked to match its reference.
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines, ["A claim[1].", "[1] The note body.", "After."]);
}

#[test]
fn a_footnote_definitions_body_maps_to_its_own_source_bytes() {
    let src = "x[^a].\n\n[^a]: body\n";
    let m = map(src);
    // `body` sits at 14..18. Its glyphs must map there — a marker that ate
    // the offsets would put the caret in the wrong place on every click.
    let body: Vec<(char, usize)> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop && g.src >= 14)
        .map(|g| (g.ch, g.src))
        .collect();
    assert_eq!(body, [('b', 14), ('o', 15), ('d', 16), ('y', 17)]);
}

#[test]
fn an_empty_footnote_definition_still_shows_its_marker() {
    // The instant `[^1]: ` has been typed and nothing after it. `blocks`
    // renders no child, so without the explicit marker row the definition
    // wouldn't appear at all until something was typed into it.
    let src = "x[^1]\n\n[^1]:\n";
    let m = map(src);
    assert!(
        rendered(&m).contains("[1] "),
        "no marker row: {:?}",
        rendered(&m)
    );
}

#[test]
fn a_footnote_definition_wearing_a_long_label_indents_its_wrapped_body() {
    let src = "x[^src]\n\n[^src]: one two three four five six seven\n";
    let m = map_at(src, Some(24));
    let text = rendered(&m);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // Continuation lines hang under the marker, as a list item's do — the
    // indent is the marker's own width, not a fixed one.
    assert_eq!(lines[1].trim_end(), "[src] one two three four");
    assert!(
        lines[2].starts_with("      "),
        "body doesn't hang: {:?}",
        lines[2]
    );
    assert_eq!(lines[2].trim(), "five six seven");
}

/// [`map`] on a surface that raises a superscript — a pixel frontend's.
fn map_raised(src: &str, wrap: Option<usize>) -> VisualMap {
    let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
    let surface = Surface {
        raised_superscripts: true,
        ..Default::default()
    };
    build(&ed.nodes().unwrap(), src, wrap, false, &surface, None)
}

#[test]
fn a_surface_that_raises_draws_a_reference_as_its_bare_label() {
    // The raise is what sets it apart, so the brackets have nothing left
    // to do; the label is still raised, still a link, still its own bytes.
    let src = "A claim[^12] and more.\n";
    let m = map_raised(src, Some(80));
    assert_eq!(rendered(&m).trim_end(), "A claim12 and more.");
    assert_eq!(baselines_of(&m, '1'), vec![Baseline::Super]);
    assert_eq!(baselines_of(&m, '2'), vec![Baseline::Super]);
    assert_eq!(role_of(&m, '1'), Role::Link);
    let label = src.find("12").unwrap();
    let stops: Vec<usize> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| g.src)
        .collect();
    assert!(stops.contains(&label) && stops.contains(&(label + 1)));
    // The `[^` and `]` are stepped over as hidden markup, as ever: the
    // stop after the label is the prose that follows the reference.
    for off in [label - 2, label - 1, label + 2] {
        assert!(!stops.contains(&off), "markup byte {off} is a stop");
    }
    assert!(stops.contains(&(label + 3)), "the space after it");
}

#[test]
fn a_plain_surface_keeps_the_brackets_a_terminal_needs() {
    // The default is today's: on one baseline, `claim1` reads as a typo.
    let m = map("A claim[^1].\n\n[^1]: note\n");
    assert!(rendered(&m).contains("claim[1]."), "{}", rendered(&m));
    assert!(rendered(&m).contains("[1] note"), "{}", rendered(&m));
}

#[test]
fn a_surface_that_raises_keeps_dropping_a_verses_number() {
    let src = "{#v20}\n*20* And they did plead[^20a] wept[^20].\n\n[^20a]: note.\n\n[^20]: note.\n";
    let mut ed = Editor::new_str(src, Format::Djot).unwrap();
    let surface = Surface {
        raised_superscripts: true,
        ..Default::default()
    };
    let m = build(&ed.nodes().unwrap(), src, Some(80), false, &surface, None);
    let first = rendered(&m).lines().next().unwrap().to_string();
    assert_eq!(first, "20 And they did pleada wept20.");
}

#[test]
fn a_surface_that_raises_marks_a_note_with_its_raised_label() {
    // The marker pairs with its reference by eye: the same label, raised
    // the same way, and a space before the body — no brackets on either.
    let src = "x[^src]\n\n[^src]: one two three four five six seven\n";
    let m = map_raised(src, Some(24));
    let text = rendered(&m);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines[0], "xsrc");
    assert_eq!(lines[1].trim_end(), "src one two three four");
    let note = &m
        .rows
        .iter()
        .find(|r| {
            r.glyphs
                .first()
                .is_some_and(|g| g.ch == 's' && g.style.role == Role::ListMarker)
        })
        .expect("the marker row")
        .glyphs;
    assert!(
        note[..3]
            .iter()
            .all(|g| g.style.baseline == Baseline::Super && !g.stop)
    );
    assert_eq!(
        (note[3].ch, note[3].style.baseline),
        (' ', Baseline::Normal)
    );
    // The wrapped body hangs under the marker, and the indent is spelled
    // and raised exactly as the marker is, so a frontend that sets a raised
    // glyph smaller measures the two the same.
    let cont = m
        .rows
        .iter()
        .find(|r| r.glyphs.iter().any(|g| g.ch == 'v' && g.stop))
        .expect("a continuation row");
    let indent: Vec<(char, Role, Baseline)> = cont.glyphs[..4]
        .iter()
        .map(|g| (g.ch, g.style.role, g.style.baseline))
        .collect();
    assert_eq!(
        indent,
        [
            ('s', Role::ListIndent, Baseline::Super),
            ('r', Role::ListIndent, Baseline::Super),
            ('c', Role::ListIndent, Baseline::Super),
            (' ', Role::ListIndent, Baseline::Normal),
        ]
    );
    assert_eq!(lines[2].trim(), "five six seven");
}

#[test]
fn a_surface_that_raises_still_marks_an_empty_note() {
    let m = map_raised("x[^1]\n\n[^1]:\n", Some(80));
    assert!(
        rendered(&m).lines().any(|l| l == "1 "),
        "{:?}",
        rendered(&m)
    );
}
