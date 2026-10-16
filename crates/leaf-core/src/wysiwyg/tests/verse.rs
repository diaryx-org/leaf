use super::*;
use crate::Format;

fn map(src: &str, format: Format, wrap: Option<usize>) -> VisualMap {
    let mut ed = Editor::new_with_flags(
        src.as_bytes(),
        format.twig(),
        crate::doc::parse_extensions(format),
    )
    .unwrap();
    build(
        &ed.nodes().unwrap(),
        src,
        wrap,
        false,
        &Surface::default(),
        None,
    )
}

/// The text each row draws, the blank prefix (a list's indent, a verse's
/// inset and turnover) left out.
fn texts(m: &VisualMap) -> Vec<String> {
    m.rows
        .iter()
        .map(|r| {
            r.glyphs
                .iter()
                .filter(|g| g.style.role != Role::ListIndent)
                .map(|g| g.ch)
                .collect()
        })
        .collect()
}

/// How many blank prefix glyphs — ems — a row starts with.
fn lead(r: &VRow) -> usize {
    r.glyphs
        .iter()
        .take_while(|g| g.style.role == Role::ListIndent)
        .count()
}

#[test]
fn a_hard_break_in_prose_ends_the_row() {
    // `\` and two trailing spaces are line breaks, in Markdown and djot,
    // wrapped or not. They drew as a space, folding a poem into prose.
    for wrap in [Some(80), None] {
        let m = map("one\\\ntwo  \nthree\n", Format::Markdown, wrap);
        assert_eq!(texts(&m), ["one", "two", "three"], "wrap {wrap:?}");
        let m = map("one\\\ntwo\n", Format::Djot, wrap);
        assert_eq!(texts(&m), ["one", "two"], "djot, wrap {wrap:?}");
    }
    // Each row ends where its break begins, so End on `one` lands before
    // the `\`, and the next row starts past the newline.
    let m = map("one\\\ntwo\n", Format::Markdown, None);
    assert_eq!(m.rows[0].end_src, 3);
    assert_eq!(m.rows[1].glyphs[0].src, 5);
}

#[test]
fn a_verse_draws_a_row_per_line_and_a_navigable_row_per_stanza_break() {
    let src =
        "<div class=\"verse\">\n\nRoses are red,\\\nviolets blue.\n\nSugar is sweet.\n\n</div>\n";
    let m = map(src, Format::Markdown, Some(80));
    assert_eq!(
        texts(&m),
        ["Roses are red,", "violets blue.", "", "Sugar is sweet."]
    );
    // The stanza break is somewhere text can go, not a drawn gap.
    assert!(!m.rows[2].decoration);
    assert_eq!(m.rows[2].boundary, None);
    // The blank line Markdown needs above `</div>` draws nothing, and
    // neither does the tag.
    assert_eq!(m.rows.len(), 4);
    // Every line row says how far its turnover hangs.
    assert!(
        m.rows
            .iter()
            .filter(|r| !r.glyphs.is_empty())
            .all(|r| r.hang == Some(VERSE_HANG as u8))
    );
}

#[test]
fn a_verse_line_s_indent_is_its_em_spaces_drawn_as_caret_stops() {
    // Markdown, whose entities decode: djot has none, and reads `&emsp;`
    // as the text it spells.
    let src = "<div class=\"verse\">\n\nzero\n\u{2003}one\n&emsp;&emsp;two\n\n</div>\n";
    let m = map(src, Format::Markdown, None);
    assert_eq!(texts(&m), ["zero", "\u{2003}one", "\u{2003}\u{2003}two"]);
    // Each em space is a real glyph at its own spelling's offset.
    let at = src.find("&emsp;").unwrap();
    // Past the inset, which is prefix and draws blank.
    let row: Vec<_> = m.rows[2]
        .glyphs
        .iter()
        .filter(|g| g.style.role != Role::ListIndent)
        .collect();
    assert_eq!(row[0].src, at);
    assert_eq!(row[1].src, at + "&emsp;".len());
    assert!(row[0].stop && row[1].stop);
    assert_eq!(m.rows[1].hang, Some(1 + VERSE_HANG as u8));
    assert_eq!(m.rows[2].hang, Some(2 + VERSE_HANG as u8));
}

#[test]
fn a_verse_line_too_long_for_the_measure_hangs_its_turnover() {
    let src = "::: verse\n\u{2003}one two three four five six\n:::\n";
    let m = map(src, Format::Djot, Some(16));
    assert!(m.rows.len() >= 2, "{:?}", texts(&m));
    // The first row stands in by the inset alone (its indent is its own
    // em spaces); the turnover by the inset, the indent and the hang.
    assert_eq!(lead(&m.rows[0]), VERSE_INSET);
    assert_eq!(lead(&m.rows[1]), VERSE_INSET + 1 + VERSE_HANG);
}

#[test]
fn a_verse_stands_in_from_the_prose_and_its_gaps_say_verse() {
    let src = "Prose above.\n\n<div class=\"verse\">\n\none\n\ntwo\n\n</div>\n\nProse below.\n";
    let m = map(src, Format::Markdown, Some(80));
    assert_eq!(
        texts(&m),
        ["Prose above.", "", "one", "", "two", "", "Prose below."]
    );
    // Every row the verse draws — lines and the stanza break — stands in
    // by the inset and carries a hang; the prose does not.
    assert!(
        m.rows[2..5]
            .iter()
            .all(|r| lead(r) == VERSE_INSET && r.hang.is_some())
    );
    assert!(lead(&m.rows[0]) == 0 && m.rows[0].hang.is_none());
    assert!(lead(&m.rows[6]) == 0 && m.rows[6].hang.is_none());
    // The gaps either side name the verse, so a frontend can widen them.
    assert_eq!(m.rows[1].boundary.map(|b| b.below), Some(BlockClass::Verse));
    assert_eq!(m.rows[5].boundary.map(|b| b.above), Some(BlockClass::Verse));
}

#[test]
fn lines_opened_under_a_verse_s_last_line_draw_before_the_closing_tag() {
    // Enter at the end of the poem leaves an extra blank line above
    // `</div>`; it is a line the author is about to type on, so it draws.
    let src = "<div class=\"verse\">\n\nlast\n\n\n</div>\n";
    let m = map(src, Format::Markdown, Some(80));
    assert_eq!(texts(&m), ["last", ""]);
    assert!(!m.rows[1].decoration);
    assert_eq!(m.rows[1].end_src, src.find("\n\n\n").unwrap() + 1);
}

#[test]
fn a_justified_verse_stays_ragged_and_a_centred_one_centres() {
    let m = map(
        "<div class=\"verse justify\">\n\na\nb\n\n</div>\n",
        Format::Markdown,
        Some(80),
    );
    assert!(m.rows.iter().all(|r| r.align.is_none()));
    let m = map(
        "<div class=\"verse center\">\n\na\nb\n\n</div>\n",
        Format::Markdown,
        Some(80),
    );
    assert!(m.rows.iter().all(|r| r.align == Some(Align::Center)));
}
