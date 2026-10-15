use super::*;

fn md(src: &str) -> Doc {
    Doc::from_source(src.into(), Format::Markdown).unwrap()
}

#[test]
fn verse_is_a_capability_of_markdown_djot_and_asciidoc_not_html() {
    assert!(Capabilities::of(Format::Markdown).verse);
    assert!(Capabilities::of(Format::Djot).verse);
    assert!(Capabilities::of(Format::Asciidoc).verse);
    assert!(!Capabilities::of(Format::Html).verse);
}

#[test]
fn toggle_verse_wraps_the_caret_s_paragraph_and_keeps_the_caret_on_its_line() {
    let mut d = md("Roses are red,\nviolets blue.\n");
    d.caret = "Roses are red,\nviol".len();
    d.toggle_verse();
    assert_eq!(
        d.source,
        "<div class=\"verse\">\n\nRoses are red,\\\nviolets blue.\n\n</div>\n"
    );
    assert_eq!(&d.source[d.caret..d.caret + 3], "ets");
    // And off again, with the caret still on `violets`.
    d.toggle_verse();
    assert_eq!(d.source, "Roses are red,\nviolets blue.\n");
    assert_eq!(&d.source[d.caret..d.caret + 3], "ets");
}

#[test]
fn enter_in_a_verse_makes_a_line_and_typing_on_it_keeps_the_stanza() {
    let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
    d.caret = d.source.find("one").unwrap() + 3;
    d.newline();
    // The `\` break became a blank line with the caret on it…
    assert_eq!(d.source, "<div class=\"verse\">\n\none\n\ntwo\n\n</div>\n");
    assert_eq!(d.caret, d.source.find("one").unwrap() + 4);
    // …which the first character makes a line of the same stanza.
    d.insert("x");
    assert_eq!(d.source, "<div class=\"verse\">\n\none\nx\ntwo\n\n</div>\n");
}

#[test]
fn enter_mid_line_in_a_verse_splits_the_line() {
    let mut d = md("<div class=\"verse\">\n\nonetwo\n\n</div>\n");
    d.caret = d.source.find("two").unwrap();
    d.newline();
    assert_eq!(d.source, "<div class=\"verse\">\n\none\ntwo\n\n</div>\n");
}

#[test]
fn backspace_at_a_verse_line_s_start_joins_it_and_takes_the_break_with_it() {
    let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
    d.caret = d.source.find("two").unwrap();
    d.backspace();
    assert_eq!(d.source, "<div class=\"verse\">\n\nonetwo\n\n</div>\n");
    assert_eq!(d.caret, d.source.find("two").unwrap());
}

#[test]
fn backspace_at_a_verse_s_first_line_turns_it_back_into_prose() {
    let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
    d.caret = d.source.find("one").unwrap();
    d.backspace();
    assert_eq!(d.source, "one\ntwo\n");
}

#[test]
fn tab_in_a_verse_indents_by_an_em_space_and_shift_tab_takes_it_back() {
    let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
    d.caret = d.source.find("two").unwrap() + 1;
    d.indent();
    assert_eq!(
        d.source,
        "<div class=\"verse\">\n\none\\\n\u{2003}two\n\n</div>\n"
    );
    assert_eq!(&d.source[d.caret..d.caret + 2], "wo");
    d.outdent();
    assert_eq!(d.source, "<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
    assert_eq!(&d.source[d.caret..d.caret + 2], "wo");
}
