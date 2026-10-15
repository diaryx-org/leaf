use super::*;

/// The alignment and line spacing of every row that draws text, in order —
/// how a test reads a block property off the map.
fn line_facts(m: &VisualMap) -> Vec<(Option<Align>, Option<LineHeight>)> {
    m.rows
        .iter()
        .filter(|r| r.glyphs.iter().any(|g| !g.ch.is_whitespace()))
        .map(|r| (r.align, r.line_height))
        .collect()
}

/// The style of the glyph spelling `ch`, first occurrence — how a test reads
/// a run property off the map.
fn style_of(m: &VisualMap, ch: char) -> Style {
    m.rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .find(|g| g.ch == ch)
        .unwrap_or_else(|| panic!("no glyph {ch:?} in the map"))
        .style
}

// ── the presentation vocabulary ─────────────────────────────────────────

/// A block's own attributes, in the three formats that spell one on the
/// block itself: HTML's tag, djot's `{…}` line, and — the odd one — a
/// Markdown `<div>` around it, which is where twig has to put a Markdown
/// block's attributes because the format has nowhere else.
#[test]
fn a_block_carries_its_alignment_on_every_row_it_draws() {
    // HTML, on the paragraph. `lead` is somebody else's class and is
    // neither read nor in the way.
    let html = map_leaf("<p class=\"lead center\">hi</p>\n", Format::Html);
    assert_eq!(line_facts(&html), vec![(Some(Align::Center), None)]);

    // djot's attribute line, on the block.
    let dj = map_leaf("{.right}\nhi\n", Format::Djot);
    assert_eq!(line_facts(&dj), vec![(Some(Align::Right), None)]);

    // A heading carries it too, and on every row a wrapped one draws.
    let h = map_leaf("{.center}\n# a heading\n", Format::Djot);
    assert_eq!(line_facts(&h), vec![(Some(Align::Center), None)]);
    assert_eq!(h.rows[0].heading, Some(1));

    // Both keys at once, and the line spacing is read the same way.
    let both = map_leaf("{.justify data-line-height=\"1.5\"}\nhi\n", Format::Djot);
    assert_eq!(
        line_facts(&both),
        vec![(
            Some(Align::Justify),
            Some(LineHeight::Step(LineSpacing::OneHalf))
        )]
    );

    // An unknown token is somebody else's and the block draws at the
    // theme's alignment; a ratio outside the menu's three is the author's
    // own and draws at exactly what they wrote.
    let other = map_leaf("{.lead data-line-height=\"1.3\"}\nhi\n", Format::Djot);
    assert_eq!(line_facts(&other), vec![(None, LineHeight::ratio(1.3))]);

    // A value the grammar does not cover is neither: carried by the
    // document, drawn at the theme's spacing, and read as nothing at all.
    let em = map_leaf("{data-line-height=\"1.3em\"}\nhi\n", Format::Djot);
    assert_eq!(line_facts(&em), vec![(None, None)]);
}

/// `<div class="center">` around three paragraphs centres all three, which
/// is what the author of that HTML meant — and around one is the sole-child
/// shape twig's `set_block_attrs` writes in Markdown.
#[test]
fn a_div_lends_its_alignment_to_every_block_inside_it() {
    let m = map_leaf(
        "<div class=\"center\" data-line-height=\"2\">\n\none\n\ntwo\n\n</div>\n",
        Format::Markdown,
    );
    assert_eq!(
        line_facts(&m),
        vec![
            (
                Some(Align::Center),
                Some(LineHeight::Step(LineSpacing::Double))
            ),
            (
                Some(Align::Center),
                Some(LineHeight::Step(LineSpacing::Double))
            ),
        ]
    );

    // The nearer node wins, and the block after the div is untouched — the
    // context is restored, not left running.
    let nested = map_leaf(
        "<div class=\"center\">\n\n<div class=\"right\">\n\ninner\n\n</div>\n\nouter\n\n</div>\n\nafter\n",
        Format::Markdown,
    );
    assert_eq!(
        line_facts(&nested),
        vec![
            (Some(Align::Right), None),
            (Some(Align::Center), None),
            (None, None),
        ]
    );
}

/// Size, face and colour are the run's, and the block's when the whole
/// block is meant — read at both levels with the nearer winning.
#[test]
fn a_span_s_size_beats_its_block_s_and_its_face_falls_through() {
    // `<div data-font>` over `<p data-size>` over `<span data-size>`: the
    // span wins on size, the block is still what says the face.
    // The span is not first on its line: a `<span …>` opening one is an
    // HTML *block* to CommonMark, which is a fact about Markdown and not
    // about this.
    let m = map_leaf(
        "<div data-font=\"serif\">\n\nc <span data-size=\"small\">a</span> b\n\n</div>\n",
        Format::Markdown,
    );
    let a = style_of(&m, 'a');
    assert_eq!(a.size, Some(FontSize::Step(SizeStep::Small)));
    assert_eq!(a.font, Some(FaceRef::Generic(FontFamily::Serif)));
    // The text outside the span keeps the div's face and no size at all.
    let b = style_of(&m, 'b');
    assert_eq!(b.size, None);
    assert_eq!(b.font, Some(FaceRef::Generic(FontFamily::Serif)));

    // djot spells the same span anonymously and it reads identically.
    let dj = map_leaf(
        "{data-size=\"large\"}\nx [y]{data-size=\"xx-large\" data-color=\"blue\"} z\n",
        Format::Djot,
    );
    assert_eq!(
        style_of(&dj, 'x').size,
        Some(FontSize::Step(SizeStep::Large))
    );
    assert_eq!(
        style_of(&dj, 'y').size,
        Some(FontSize::Step(SizeStep::XxLarge))
    );
    assert_eq!(
        style_of(&dj, 'y').color,
        Some(TextColor::Named(MarkColor::Blue))
    );
    // The block's size is still the block's outside the span.
    assert_eq!(
        style_of(&dj, 'z').size,
        Some(FontSize::Step(SizeStep::Large))
    );
    assert_eq!(style_of(&dj, 'z').color, None);
}

/// The exact half of the vocabulary reaches a glyph and a row by the same
/// doors the names do — the fold has one rule, not one per form. A named
/// family is the one that cannot ride the glyph as itself: the walker
/// interns it and the glyph carries the id.
#[test]
fn an_exact_size_face_and_colour_reach_the_glyph_and_the_row() {
    let m = map_leaf(
        "<div data-line-height=\"1.3\">\n\nc <span data-size=\"14pt\" \
             data-color=\"#c03030\" data-font=\"Garamond\">a</span> b\n\n</div>\n",
        Format::Markdown,
    );
    let a = style_of(&m, 'a');
    assert_eq!(a.size, FontSize::points(14.0));
    assert_eq!(
        a.color,
        Some(TextColor::Rgb {
            r: 0xc0,
            g: 0x30,
            b: 0x30
        })
    );
    assert_eq!(a.font, Some(FaceRef::Named(FaceId::of("Garamond"))));
    assert_eq!(m.face_name(FaceId::of("Garamond")), Some("Garamond"));
    // The div's ratio is the row's, on every row the block draws.
    assert_eq!(line_facts(&m), vec![(None, LineHeight::ratio(1.3))]);
    // And the text outside the span has none of the span's three.
    let b = style_of(&m, 'b');
    assert_eq!((b.size, b.font, b.color), (None, None, None));

    // One name, one entry, however many spans wear it — the table is what
    // keeps a `Style` `Copy` and it should not grow per run.
    let twice = map_leaf(
        "x <span data-font=\"Garamond\">a</span> y <span data-font=\"Garamond\">b</span>\n",
        Format::Markdown,
    );
    assert_eq!(twice.faces().len(), 1);
    assert_eq!(
        style_of(&twice, 'a').font,
        style_of(&twice, 'b').font,
        "one family, one id"
    );

    // A generic needs no entry at all: it names itself.
    let generic = map_leaf(
        "<div data-font=\"serif\">\n\nhi\n\n</div>\n",
        Format::Markdown,
    );
    assert_eq!(
        style_of(&generic, 'h').font,
        Some(FaceRef::Generic(FontFamily::Serif))
    );
    assert!(generic.faces().is_empty());
}

/// The one key two nodes share. `data-color` on a `mark` is the highlight's
/// *background* and reaches a glyph through [`Role::Mark`]; the same key on
/// an attributed span is the text's foreground. Same vocabulary, same enum,
/// no collision — and a mark inside a coloured span wears both.
#[test]
fn a_mark_keeps_its_highlight_colour_and_a_span_colours_the_text() {
    let m = map_leaf("a ==\u{1f534} red== b\n", Format::Markdown);
    let r = style_of(&m, 'r');
    assert_eq!(r.role, Role::Mark(Some(MarkColor::Red)));
    assert_eq!(r.color, None, "a highlight is not a text colour");

    let both = map_leaf(
        "<span data-color=\"blue\">a ==\u{1f534} red== b</span>\n",
        Format::Markdown,
    );
    let r = style_of(&both, 'r');
    assert_eq!(r.role, Role::Mark(Some(MarkColor::Red)), "the highlight");
    assert_eq!(
        r.color,
        Some(TextColor::Named(MarkColor::Blue)),
        "the letters"
    );
}

/// A page break is the `::page-break` leaf directive, and djot spells the
/// same document as an empty `::: page-break` fence whose name comes back
/// as a class, HTML as a `<page-break>` element and AsciiDoc as `<<<`. All
/// four draw the placeholder row every leaf directive gets and carry the
/// same [`DirectiveMark`], because a frontend that opens a page at one
/// must not be able to tell which format the file is in.
#[test]
fn a_page_break_reads_the_same_in_every_format() {
    for (fmt, src) in [
        (Format::Markdown, "a\n\n::page-break\n\nb\n"),
        (Format::Djot, "a\n\n::: page-break\n:::\n\nb\n"),
        (
            Format::Html,
            "<p>a</p>\n\n<page-break></page-break>\n\n<p>b</p>\n",
        ),
        (Format::Asciidoc, "a\n\n<<<\n\nb\n"),
    ] {
        let m = map_leaf(src, fmt);
        let marks: Vec<&DirectiveMark> = m
            .rows
            .iter()
            .filter_map(|r| r.leaf_directive.as_ref())
            .collect();
        assert_eq!(marks.len(), 1, "{fmt:?} draws one placeholder");
        assert_eq!(marks[0].name, "page-break", "{fmt:?}");
        assert!(marks[0].attrs.is_empty(), "{fmt:?}: {:?}", marks[0].attrs);
        assert!(
            m.rows
                .iter()
                .any(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>()
                    == "\u{29c9} page-break"),
            "{fmt:?} draws the label, got {:?}",
            m.rows
                .iter()
                .map(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>())
                .collect::<Vec<_>>()
        );
    }

    // A Markdown `:::note` with nothing in it is *not* this: its name is
    // its own, and nothing about it says "a block with no body" the way
    // djot's spelling of a leaf directive does.
    let empty_fence = map_leaf("::: note\n:::\n", Format::Markdown);
    assert!(
        empty_fence.rows.iter().all(|r| r.leaf_directive.is_none()),
        "a named empty fence keeps the reading it has"
    );
}

/// A djot fence carrying more than its name keeps the rest as an attribute
/// rather than folding it into the name: with a bare `:::` fence, which
/// names nothing itself, the *first* class token is the name.
#[test]
fn a_djot_fence_s_first_class_is_the_directive_s_name_and_the_rest_is_attributes() {
    let m = map_leaf("{.page-break .wide}\n:::\n:::\n", Format::Djot);
    let mark = m
        .rows
        .iter()
        .find_map(|r| r.leaf_directive.as_ref())
        .expect("a placeholder");
    assert_eq!(mark.name, "page-break");
    assert_eq!(
        mark.attrs,
        vec![("class".to_string(), Some("wide".to_string()))]
    );
}

/// With the name on the fence line, djot appends it *after* the classes of
/// the attribute line above, so the name is the fence's word wherever it
/// lands in the class — here, and inside a quote.
#[test]
fn a_djot_fence_s_word_is_the_directive_s_name_behind_an_attribute_line() {
    for src in [
        "{.wide src=\"u\"}\n::: x-card\n:::\n",
        "> {.wide src=\"u\"}\n> ::: x-card\n> :::\n",
    ] {
        let m = map_leaf(src, Format::Djot);
        let mark = m
            .rows
            .iter()
            .find_map(|r| r.leaf_directive.as_ref())
            .unwrap_or_else(|| panic!("a placeholder for {src:?}"));
        assert_eq!(mark.name, "x-card", "{src:?}");
        assert_eq!(
            mark.attrs,
            vec![
                ("class".to_string(), Some("wide".to_string())),
                ("src".to_string(), Some("u".to_string())),
            ],
            "{src:?}"
        );
    }
}
