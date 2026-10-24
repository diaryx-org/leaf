use super::*;

// ── the presentation vocabulary ─────────────────────────────────────────

/// A document in `format`, for the gesture tests that want more than the
/// Markdown `doc_with` writes.
fn fmt_doc(body: &str, format: Format) -> Doc {
    Doc::from_source(body.to_string(), format).unwrap()
}

/// Alignment is a block property, so the gesture is `set_block_attrs` on
/// the caret's block whatever is selected — written as CSS's `text-align`
/// in `style`, which each format spells its own way: djot's `{…}` line above the block, a `<div>` around it in
/// Markdown (the format has nowhere else to put it), the tag in HTML.
#[test]
fn set_alignment_spells_the_class_the_format_s_own_way() {
    let mut dj = fmt_doc("hello\n", Format::Djot);
    dj.caret = 1;
    dj.set_alignment(Some(Align::Center));
    assert_eq!(dj.source, "{style=\"text-align: center\"}\nhello\n");
    assert!(dj.dirty);
    assert_eq!(dj.status, None);

    let mut md = fmt_doc("hello\n", Format::Markdown);
    md.caret = 1;
    md.set_alignment(Some(Align::Right));
    assert_eq!(
        md.source,
        "<div style=\"text-align: right\">\n\nhello\n\n</div>\n"
    );

    let mut html = fmt_doc("<p>hello</p>\n", Format::Html);
    html.caret = html.source.find("hello").unwrap();
    html.set_alignment(Some(Align::Justify));
    assert_eq!(html.source, "<p style=\"text-align: justify\">hello</p>\n");
}

/// Each gesture edits **one key and keeps the rest** — twig's contract is
/// replace-not-merge, so leaf reads the node's attributes, edits its own
/// key out of them, and passes the list back whole. A document from
/// elsewhere passes through the editor unharmed.
#[test]
fn a_presentation_gesture_keeps_every_attribute_it_did_not_write() {
    let mut d = fmt_doc(
        "{.lead .center #intro data-line-height=\"1.5\"}\nhello\n",
        Format::Djot,
    );
    d.caret = d.source.find("hello").unwrap();
    d.set_alignment(Some(Align::Right));
    // leaf 0.2's `center` goes and the alignment moves into `style`; `lead`
    // stays, and neither the id nor the spacing is touched.
    // The serializer picks the order; what matters is which keys survive.
    assert!(d.source.contains(".lead"), "{:?}", d.source);
    assert!(
        d.source.contains("style=\"text-align: right\""),
        "{:?}",
        d.source
    );
    assert!(!d.source.contains(".center"), "{:?}", d.source);
    assert!(!d.source.contains(".right"), "{:?}", d.source);
    assert!(d.source.contains("#intro"), "{:?}", d.source);
    assert!(
        d.source.contains("data-line-height=\"1.5\""),
        "{:?}",
        d.source
    );
    assert_eq!(d.alignment_at_caret(), Some(Align::Right));
    assert_eq!(
        d.line_spacing_at_caret(),
        Some(LineHeight::Step(LineSpacing::OneHalf))
    );

    // And the other way round: the spacing gesture leaves the classes and
    // the alignment be, and moves its own key into `style` beside it.
    d.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
    assert!(d.source.contains(".lead"), "{:?}", d.source);
    assert!(
        d.source
            .contains("style=\"text-align: right; line-height: 2\""),
        "{:?}",
        d.source
    );
    assert!(!d.source.contains("data-line-height"), "{:?}", d.source);
    assert_eq!(
        d.line_spacing_at_caret(),
        Some(LineHeight::Step(LineSpacing::Double))
    );
}

/// Clearing is the same gesture with `None`: the key goes, the tokens leaf
/// owns go out of `class`, and a block left with nothing at all is spelled
/// bare again — in Markdown by unwrapping the div twig wrapped it in.
#[test]
fn none_clears_a_key_and_an_empty_set_unwraps_the_block() {
    let mut dj = fmt_doc("{.lead .center}\nhello\n", Format::Djot);
    dj.caret = dj.source.find("hello").unwrap();
    dj.set_alignment(None);
    assert_eq!(dj.source, "{.lead}\nhello\n", "the foreign class stays");
    assert_eq!(dj.alignment_at_caret(), None);

    let mut bare = fmt_doc("{.center}\nhello\n", Format::Djot);
    bare.caret = bare.source.find("hello").unwrap();
    bare.set_alignment(None);
    assert_eq!(
        bare.source, "hello\n",
        "the last key takes the line with it"
    );

    let mut md = fmt_doc("hello\n", Format::Markdown);
    md.caret = 1;
    md.set_alignment(Some(Align::Center));
    assert_eq!(
        md.source,
        "<div style=\"text-align: center\">\n\nhello\n\n</div>\n"
    );
    md.caret = md.source.find("hello").unwrap();
    md.set_line_spacing(Some(LineHeight::Step(LineSpacing::OneFifteen)));
    assert_eq!(
        md.source, "<div style=\"text-align: center; line-height: 1.15\">\n\nhello\n\n</div>\n",
        "the second key rewrites the div rather than nesting a second"
    );
    md.caret = md.source.find("hello").unwrap();
    md.set_alignment(None);
    md.caret = md.source.find("hello").unwrap();
    md.set_line_spacing(None);
    assert_eq!(md.source, "hello\n", "an empty set unwraps the div");
}

/// Size, face and colour are the run's over a selection and the block's
/// with none — so "make this paragraph larger" is a click with the caret in
/// it rather than a select-all first.
#[test]
fn a_run_gesture_wraps_a_selection_and_sets_the_block_without_one() {
    // With a selection: a span, in each format's own spelling.
    let mut dj = fmt_doc("a big b\n", Format::Djot);
    dj.anchor = Some(2);
    dj.caret = 5;
    dj.set_font_size(Some(FontSize::Step(SizeStep::Large)));
    assert_eq!(dj.source, "a [big]{style=\"font-size: large\"} b\n");
    assert_eq!(
        dj.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Large))
    );

    let mut md = fmt_doc("a big b\n", Format::Markdown);
    md.anchor = Some(2);
    md.caret = 5;
    md.set_text_color(Some(TextColor::Named(MarkColor::Blue)));
    // A name is written as its ink, which every renderer draws, and read
    // back as the name, whose two inks the theme owns.
    assert_eq!(md.source, "a <span style=\"color: #3d7fd6\">big</span> b\n");
    assert_eq!(
        md.text_color_at_caret(),
        Some(TextColor::Named(MarkColor::Blue))
    );

    // Without one: the caret's block, through the block gesture.
    let mut block = fmt_doc("a big b\n", Format::Djot);
    block.caret = 3;
    block.set_font_family(Some(FontFace::Generic(FontFamily::Monospace)));
    assert_eq!(
        block.source,
        "{style=\"font-family: monospace\"}\na big b\n"
    );
    assert_eq!(
        block.font_family_at_caret(),
        Some(FontFace::Generic(FontFamily::Monospace))
    );
}

/// The *Other…* row of each of the four menus: a value goes into the
/// document in its canonical spelling and comes back out of the query as
/// the same value. One round trip per property, because the four go out
/// through different doors — two block gestures, and the run three through
/// the span that `wrap_range_attrs` mints.
#[test]
fn an_exact_value_round_trips_through_the_gesture_and_the_query() {
    // Size: the run three, over a selection.
    let mut d = fmt_doc("a big b\n", Format::Djot);
    d.anchor = Some(2);
    d.caret = 5;
    d.set_font_size(FontSize::points(14.0));
    assert_eq!(d.source, "a [big]{style=\"font-size: 14pt\"} b\n");
    assert_eq!(d.font_size_at_caret(), FontSize::points(14.0));

    // Colour, onto the same span — the gesture keeps the size it finds.
    d.set_text_color(Some(TextColor::Rgb {
        r: 0xc0,
        g: 0x30,
        b: 0x30,
    }));
    assert_eq!(
        d.source,
        "a [big]{style=\"font-size: 14pt; color: #c03030\"} b\n"
    );
    assert_eq!(
        d.text_color_at_caret(),
        Some(TextColor::Rgb {
            r: 0xc0,
            g: 0x30,
            b: 0x30
        })
    );

    // Face: a family name, as given, quoted as CSS quotes a family.
    d.set_font_family(Some(FontFace::Named("Garamond".into())));
    assert!(
        d.source.contains("font-family: 'Garamond'"),
        "{:?}",
        d.source
    );
    assert_eq!(
        d.font_family_at_caret(),
        Some(FontFace::Named("Garamond".into()))
    );

    // Line spacing: a block gesture, and an exact ratio.
    let mut block = fmt_doc("hello\n", Format::Djot);
    block.caret = 1;
    block.set_line_spacing(LineHeight::ratio(1.3));
    assert_eq!(block.source, "{style=\"line-height: 1.3\"}\nhello\n");
    assert_eq!(block.line_spacing_at_caret(), LineHeight::ratio(1.3));

    // And a value spelled long is written back short, so the same press
    // twice writes the same bytes: `14.0pt` in, `14pt` out — here from leaf
    // 0.2's `data-size`, which the press moves into `style`.
    let mut long = fmt_doc("{data-size=\"14.0pt\"}\nhello\n", Format::Djot);
    long.caret = long.source.find("hello").unwrap();
    assert_eq!(long.font_size_at_caret(), FontSize::points(14.0));
    let in_force = long.font_size_at_caret();
    long.set_font_size(in_force);
    assert_eq!(long.source, "{style=\"font-size: 14pt\"}\nhello\n");
}

/// A value the grammar does not cover is what it was before the vocabulary
/// opened: carried untouched by the document, answered `None` by the query
/// so the menu ticks *Default*, and rewritten only by a gesture on its own
/// key. leaf is not going to grow a CSS parser to guess at `1.3em`.
#[test]
fn a_value_outside_the_grammar_is_carried_and_the_menu_ticks_the_default() {
    let src = "{data-size=\"huge\" data-color=\"rgb(1,2,3)\" data-line-height=\"1.3em\"}\nhello\n";
    let mut d = fmt_doc(src, Format::Djot);
    d.caret = d.source.find("hello").unwrap();
    assert_eq!(d.font_size_at_caret(), None);
    assert_eq!(d.text_color_at_caret(), None);
    assert_eq!(d.line_spacing_at_caret(), None);

    // The keys are still there, untouched, after a gesture on a *different*
    // key — "edit one key and keep the rest" holds for a value it cannot
    // read as readily as for one it can.
    d.set_alignment(Some(Align::Center));
    assert!(d.source.contains("data-size=\"huge\""), "{:?}", d.source);
    assert!(
        d.source.contains("data-color=\"rgb(1,2,3)\""),
        "{:?}",
        d.source
    );
    assert!(
        d.source.contains("data-line-height=\"1.3em\""),
        "{:?}",
        d.source
    );
    // And the gesture on its *own* key replaces it, which is the one way a
    // carried value ever changes.
    d.caret = d.source.find("hello").unwrap();
    d.set_font_size(FontSize::points(12.0));
    assert!(d.source.contains("font-size: 12pt"), "{:?}", d.source);
    assert!(!d.source.contains("huge"), "{:?}", d.source);
}

/// The nearest node wins whichever *form* either node wrote: a value inside
/// a name, a name inside a value. The fold has one rule and does not learn
/// a second one for exact values.
#[test]
fn the_nearest_node_wins_whether_it_named_a_size_or_measured_one() {
    // A value inside a name: the block says `small`, the span says `14pt`.
    let mut d = fmt_doc(
        "{data-size=\"small\"}\nx [y]{data-size=\"14pt\"} z\n",
        Format::Djot,
    );
    d.caret = d.source.find('y').unwrap();
    assert_eq!(d.font_size_at_caret(), FontSize::points(14.0));
    d.caret = d.source.find('x').unwrap();
    assert_eq!(
        d.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Small))
    );

    // And a name inside a value, which is the same rule read the other way.
    let mut e = fmt_doc(
        "{data-size=\"14pt\" data-color=\"#c03030\"}\nx [y]{data-size=\"small\"} z\n",
        Format::Djot,
    );
    e.caret = e.source.find('y').unwrap();
    assert_eq!(
        e.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Small))
    );
    assert_eq!(
        e.text_color_at_caret(),
        Some(TextColor::Rgb {
            r: 0xc0,
            g: 0x30,
            b: 0x30
        }),
        "the block's colour still reaches the span"
    );
    e.caret = e.source.find('x').unwrap();
    assert_eq!(e.font_size_at_caret(), FontSize::points(14.0));
}

/// twig re-styles the span a range already lies in rather than nesting a
/// second, and an empty set unwraps it — so a second press of the menu
/// fixes the size instead of building `[[big]{.a}]{.b}`, and the entry that
/// means "the theme's own" takes the span away.
#[test]
fn a_second_run_gesture_re_styles_the_span_and_none_unwraps_it() {
    let mut d = fmt_doc("a big b\n", Format::Djot);
    d.anchor = Some(2);
    d.caret = 5;
    d.set_font_size(Some(FontSize::Step(SizeStep::Large)));
    assert_eq!(d.source, "a [big]{style=\"font-size: large\"} b\n");

    // The selection `wrap_range_attrs` left behind covers the whole span;
    // colouring it now keeps the size, because the gesture reads the span's
    // attributes before it edits its own key.
    d.set_text_color(Some(TextColor::Named(MarkColor::Red)));
    assert_eq!(
        d.source, "a [big]{style=\"font-size: large; color: #d24b3f\"} b\n",
        "one span, both properties"
    );
    assert_eq!(
        d.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Large))
    );
    assert_eq!(
        d.text_color_at_caret(),
        Some(TextColor::Named(MarkColor::Red))
    );

    d.set_text_color(None);
    assert_eq!(d.source, "a [big]{style=\"font-size: large\"} b\n");
    d.set_font_size(None);
    assert_eq!(d.source, "a big b\n", "the last key unwraps the span");
    assert_eq!(d.font_size_at_caret(), None);
}

/// The queries read the nearest node that names the property: the span the
/// caret is in, then its block, then the `div`s around it.
#[test]
fn a_presentation_query_reads_the_nearest_node_that_names_it() {
    let mut d = fmt_doc(
        "{.center data-size=\"small\" data-font=\"serif\"}\nx [y]{data-size=\"xx-large\"} z\n",
        Format::Djot,
    );
    // In the span: its own size, the block's face and alignment.
    d.caret = d.source.find('y').unwrap();
    assert_eq!(
        d.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::XxLarge))
    );
    assert_eq!(
        d.font_family_at_caret(),
        Some(FontFace::Generic(FontFamily::Serif))
    );
    assert_eq!(d.alignment_at_caret(), Some(Align::Center));
    assert_eq!(d.line_spacing_at_caret(), None);
    assert_eq!(d.text_color_at_caret(), None);

    // Outside it: the block's size.
    d.caret = d.source.find('x').unwrap();
    assert_eq!(
        d.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Small))
    );

    // And through a Markdown div, which is where a Markdown block's
    // attributes live.
    let mut md = fmt_doc(
        "<div class=\"center\" data-size=\"large\">\n\nhello\n\n</div>\n",
        Format::Markdown,
    );
    md.caret = md.source.find("hello").unwrap();
    assert_eq!(md.alignment_at_caret(), Some(Align::Center));
    assert_eq!(
        md.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Large))
    );

    // A document that names none of it answers `None` everywhere, which is
    // "the theme's own" and what every toolbar draws unlit.
    let mut plain = doc_with("plain_presentation", "hello\n");
    plain.caret = 1;
    assert_eq!(plain.alignment_at_caret(), None);
    assert_eq!(plain.line_spacing_at_caret(), None);
    assert_eq!(plain.font_size_at_caret(), None);
    assert_eq!(plain.font_family_at_caret(), None);
    assert_eq!(plain.text_color_at_caret(), None);
}

/// A djot fenced div is anonymous the way an attributed span is, and is a
/// block all the same — the *form* is the whole of what tells them apart.
/// Read as a span it poisoned both halves: the run gesture copied the div's
/// entire attribute set onto the span it minted, duplicating the `id`, and
/// the run and block queries answered off a node the walker draws nothing
/// for.
#[test]
fn a_djot_fenced_div_is_not_an_attributed_span() {
    let src = "{.center data-size=\"small\" #box}\n:::\nhello world\n:::\n";
    let mut d = fmt_doc(src, Format::Djot);
    let at = d.source.find("world").unwrap();
    d.anchor = Some(at);
    d.caret = at + "world".len();
    d.set_text_color(Some(TextColor::Named(MarkColor::Red)));
    assert_eq!(
        d.source,
        "{.center data-size=\"small\" #box}\n:::\nhello [world]{style=\"color: #d24b3f\"}\n:::\n",
        "the span carries its own key and nothing of the div's"
    );

    // And the queries stop at the block: a djot div is not a `<div>`, the
    // walker lends its keys to nothing inside it, and a query that said
    // otherwise would tick a menu entry no glyph on screen obeys.
    assert_eq!(
        d.text_color_at_caret(),
        Some(TextColor::Named(MarkColor::Red))
    );
    assert_eq!(d.font_size_at_caret(), None);
    assert_eq!(d.alignment_at_caret(), None);
}

/// Clearing a property the block does not name and a `div` around it does
/// would write nothing and change nothing — twig's `set_block_attrs`
/// reaches one node, and the div is not it. The gesture says so instead of
/// leaving the author pressing an entry that never ticks.
#[test]
fn clearing_a_property_an_enclosing_div_names_says_so_and_writes_nothing() {
    // Markdown, two paragraphs in one div: not the sole-child shape twig
    // writes, so `block_attrs_at_caret` reads the paragraph and the
    // paragraph names none of it.
    let src = "<div class=\"center\" data-line-height=\"1.5\" data-size=\"large\">\n\nhello\n\nworld\n\n</div>\n";
    let mut md = fmt_doc(src, Format::Markdown);
    md.caret = md.source.find("hello").unwrap();
    assert_eq!(md.alignment_at_caret(), Some(Align::Center));

    md.set_alignment(None);
    assert_eq!(md.source, src, "nothing written");
    assert!(!md.dirty);
    assert_eq!(
        md.status.as_deref(),
        Some("alignment: set on the div around the block")
    );
    assert_eq!(md.alignment_at_caret(), Some(Align::Center));

    // The same for a `data-` key, at both levels — the block pair and the
    // run three, the run three at a bare caret being the block gesture.
    md.set_line_spacing(None);
    assert_eq!(md.source, src);
    assert_eq!(
        md.status.as_deref(),
        Some("line spacing: set on the div around the block")
    );
    md.set_font_size(None);
    assert_eq!(md.source, src);
    assert_eq!(
        md.status.as_deref(),
        Some("size: set on the div around the block")
    );

    // HTML has no sole-child fold at all: a block's attributes go on the
    // block, so the div around one is always out of reach.
    let html_src = "<div class=\"center\"><p>hi</p></div>\n";
    let mut html = fmt_doc(html_src, Format::Html);
    html.caret = html.source.find("hi").unwrap();
    assert_eq!(html.alignment_at_caret(), Some(Align::Center));
    html.set_alignment(None);
    assert_eq!(html.source, html_src);
    assert!(!html.dirty);
    assert_eq!(
        html.status.as_deref(),
        Some("alignment: set on the div around the block")
    );

    // And it is a refusal, not a rule against clearing: a block that names
    // the property itself still loses it, div or no div.
    let mut own = fmt_doc(
        "<div class=\"center\"><p class=\"right\">hi</p></div>\n",
        Format::Html,
    );
    own.caret = own.source.find("hi").unwrap();
    own.set_alignment(None);
    assert_eq!(own.source, "<div class=\"center\"><p>hi</p></div>\n");
    assert_eq!(own.status, None);
}

/// An edited key is rewritten **where it stands**, and an edited
/// declaration the same way inside `style`. Removing the key and pushing it
/// back shuffled a document's attributes on every press. A paragraph from
/// elsewhere, with its own `style`, keeps the attribute order and the
/// declarations it came with, and leaf's properties go in beside them.
#[test]
fn an_edited_key_keeps_its_place_among_the_attributes() {
    let mut html = fmt_doc(
        "<p id=\"intro\" style=\"color: #333; text-align: center\" class=\"lead\">hello</p>\n",
        Format::Html,
    );
    html.caret = html.source.find("hello").unwrap();
    html.set_alignment(Some(Align::Right));
    assert_eq!(
        html.source,
        "<p id=\"intro\" style=\"color: #333; text-align: right\" class=\"lead\">hello</p>\n"
    );

    // A property the block did not have goes on the end of `style`.
    html.caret = html.source.find("hello").unwrap();
    html.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
    assert_eq!(
        html.source,
        "<p id=\"intro\" style=\"color: #333; text-align: right; line-height: 2\" class=\"lead\">hello</p>\n"
    );

    // A leaf 0.2 paragraph: the alignment's token leaves `class` and its
    // property goes into a `style` on the end, and the spacing it did not
    // touch stays where it was, in the spelling it was in.
    let mut old = fmt_doc(
        "<p id=\"intro\" class=\"lead center\" data-line-height=\"1.5\">hello</p>\n",
        Format::Html,
    );
    old.caret = old.source.find("hello").unwrap();
    old.set_alignment(Some(Align::Right));
    assert_eq!(
        old.source,
        "<p id=\"intro\" class=\"lead\" data-line-height=\"1.5\" style=\"text-align: right\">hello</p>\n"
    );
    assert_eq!(
        old.line_spacing_at_caret(),
        Some(LineHeight::Step(LineSpacing::OneHalf))
    );

    // Djot writes the same list in its own spelling, and the order is the
    // author's there too.
    let mut dj = fmt_doc(
        "{#intro style=\"color: #333; text-align: center\" .lead}\nhello\n",
        Format::Djot,
    );
    dj.caret = dj.source.find("hello").unwrap();
    dj.set_alignment(Some(Align::Right));
    assert_eq!(
        dj.source,
        "{#intro style=\"color: #333; text-align: right\" .lead}\nhello\n"
    );
}

/// A page break is a block, so twig alone lands one after the caret's whole
/// block; the paragraph is parted at the caret first, exactly as
/// `insert_thematic_break` parts it, and each format spells the directive
/// its own way.
#[test]
fn insert_page_break_parts_the_paragraph_and_spells_the_directive() {
    let mut md = doc_with("page_break_md", "hello world\n");
    md.caret = 5;
    md.insert_page_break();
    assert_eq!(md.source, "hello\n\n::page-break\n\nworld\n");
    assert!(md.dirty);
    assert_eq!(md.status, None);

    let mut dj = fmt_doc("hello world\n", Format::Djot);
    dj.caret = 5;
    dj.insert_page_break();
    assert_eq!(dj.source, "hello\n\n::: page-break\n:::\n\nworld\n");

    // At a block's end there is no second half to mint, so the break simply
    // follows the block — the rule the rule button already has.
    let mut end = doc_with("page_break_end", "hello\n");
    end.caret = 5;
    end.insert_page_break();
    assert_eq!(end.source, "hello\n\n::page-break\n\n");
    assert_eq!(end.caret, end.source.len(), "on the line under the break");

    // And it reaches the map as the placeholder row a frontend paginates on.
    end.view = View::Wysiwyg;
    end.build_visual(80);
    assert_eq!(
        end.vmap
            .rows
            .iter()
            .find_map(|r| r.leaf_directive.as_ref())
            .map(|m| m.name.as_str()),
        Some(PAGE_BREAK)
    );

    // HTML and AsciiDoc spell it their own way, and draw it the same.
    for (fmt, src, spelled) in [
        (Format::Html, "<p>hello</p>\n", "<page-break></page-break>"),
        (Format::Asciidoc, "hello\n", "<<<"),
    ] {
        let mut d = fmt_doc(src, fmt);
        d.caret = d.source.find("hello").unwrap() + 5;
        d.insert_page_break();
        assert!(d.source.contains(spelled), "{fmt:?}: {:?}", d.source);
        d.view = View::Wysiwyg;
        d.build_visual(80);
        let marks = d
            .vmap
            .rows
            .iter()
            .filter_map(|r| r.leaf_directive.as_ref())
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(marks, [PAGE_BREAK], "{fmt:?}");
    }
}

/// The general gesture a host's catalogue calls: a directive named by the
/// host, with a label and attributes, written where the caret parts the
/// paragraph, and read back into the map with all three — the way the
/// view hands it to the host that draws it.
#[test]
fn insert_directive_round_trips_name_label_and_attrs() {
    let attrs: Attrs = vec![
        ("src".into(), Some("https://x.org/a".into())),
        ("title".into(), Some("a b".into())),
        ("wide".into(), None),
    ];
    let mut md = doc_with("directive_md", "hello world\n");
    md.caret = 5;
    md.insert_directive("x-card", Some("A card"), &attrs);
    assert_eq!(md.status, None);
    assert_eq!(
        md.source,
        "hello\n\n::x-card[A card]{src=\"https://x.org/a\" title=\"a b\" wide=\"\"}\n\nworld\n"
    );
    assert!(md.dirty);
    md.view = View::Wysiwyg;
    md.build_visual(80);
    let [d] = md.vmap.directives.as_slice() else {
        panic!("one directive, got {:?}", md.vmap.directives)
    };
    assert_eq!(d.name, "x-card");
    assert_eq!(d.label, "A card");
    // A bare attribute is written `wide=""` and reads back empty.
    assert_eq!(
        d.key().attrs,
        [
            ("src".to_string(), "https://x.org/a".to_string()),
            ("title".to_string(), "a b".to_string()),
            ("wide".to_string(), String::new()),
        ]
    );
    assert_eq!(d.attr("src"), Some("https://x.org/a"));

    // djot: the attributes on a line of their own over the fence, a bare
    // one spelled `key=""` as in every format, and a class kept apart from
    // the name the fence gives.
    let mut dj = fmt_doc("hello world\n", Format::Djot);
    dj.caret = 5;
    let dj_attrs: Attrs = vec![
        ("class".into(), Some("wide".into())),
        ("src".into(), Some("https://x.org/a".into())),
        ("flag".into(), None),
    ];
    dj.insert_directive("x-card", None, &dj_attrs);
    assert_eq!(dj.status, None);
    assert_eq!(
        dj.source,
        "hello\n\n{.wide src=\"https://x.org/a\" flag=\"\"}\n::: x-card\n:::\n\nworld\n"
    );
    dj.view = View::Wysiwyg;
    dj.build_visual(80);
    let [d] = dj.vmap.directives.as_slice() else {
        panic!("one directive, got {:?}", dj.vmap.directives)
    };
    assert_eq!(
        d.name, "x-card",
        "the fence's word, not the attribute line's class"
    );
    assert_eq!(d.label, "");
    assert_eq!(d.attr("class"), Some("wide"));
    assert_eq!(d.attr("src"), Some("https://x.org/a"));
    assert_eq!(d.attr("flag"), Some(""));
}

/// `insert_page_break` is `insert_directive` with leaf's own name: the
/// same bytes, the same caret, the same one undo step.
#[test]
fn insert_directive_places_the_caret_as_a_page_break_does() {
    for (fmt, body, caret) in [
        (Format::Markdown, "a\n\nb\n", 1),
        (Format::Markdown, "a\n", 1),
        (Format::Markdown, "hello world\n", 5),
        (Format::Djot, "a\n\nb\n", 1),
        (Format::Djot, "hello world\n", 5),
    ] {
        let run = |f: &dyn Fn(&mut Doc)| {
            let mut d = Doc::from_source(body.into(), fmt).unwrap();
            d.view = View::Wysiwyg;
            d.caret = caret;
            f(&mut d);
            (d.source.clone(), d.caret)
        };
        assert_eq!(
            run(&|d| d.insert_page_break()),
            run(&|d| d.insert_directive(PAGE_BREAK, None, &[])),
            "{fmt:?} {body:?}"
        );
    }

    // And a host's name lands the caret on a line under it, typed into
    // there, and taken back in one step with that line.
    let mut d = Doc::from_source("a\n\nb\n".into(), Format::Markdown).unwrap();
    d.view = View::Wysiwyg;
    d.caret = 1;
    d.insert_directive("x-card", Some("A"), &[("k".into(), Some("v".into()))]);
    assert_eq!(d.source, "a\n\n::x-card[A]{k=v}\n\n\n\nb\n");
    d.build_visual(80);
    let (row, col) = d.vmap.pos_of_offset(d.caret);
    assert_eq!(col, 0);
    assert!(d.vmap.rows[row].glyphs.is_empty(), "an empty line");
    assert!(d.vmap.rows[row - 2].leaf_directive.is_some(), "under it");
    d.insert("X");
    assert_eq!(d.source, "a\n\n::x-card[A]{k=v}\n\nX\n\nb\n");
    d.undo();
    d.undo();
    assert_eq!(
        d.source, "a\n\nb\n",
        "the directive and its line are one step"
    );
}

/// A name or a label twig will not write is refused before anything is
/// touched — the selection it would have replaced and the paragraph it
/// would have parted are both left as they were.
#[test]
fn insert_directive_refuses_a_bad_name_or_label_and_writes_nothing() {
    let src = "hello world\n";
    for (fmt, name, label) in [
        (Format::Markdown, "1x", None),
        (Format::Markdown, "a b", None),
        (Format::Markdown, "a:b", None),
        (Format::Markdown, "", None),
        (Format::Markdown, "ok", Some("a]b")),
        (Format::Markdown, "ok", Some("a\nb")),
        (Format::Djot, "a b", None),
        // djot has nowhere for a label to go.
        (Format::Djot, "ok", Some("A card")),
    ] {
        let mut d = fmt_doc(src, fmt);
        d.anchor = Some(2);
        d.caret = 5;
        d.insert_directive(name, label, &[]);
        assert_eq!(d.source, src, "{fmt:?} {name:?} {label:?}");
        assert!(!d.dirty, "{fmt:?} {name:?} {label:?}");
        assert!(!d.can_undo(), "{fmt:?} {name:?} {label:?}");
        let status = d.status.as_deref().unwrap_or("");
        assert!(
            status.starts_with("directive: "),
            "{fmt:?} {name:?} {label:?}: {status:?}"
        );
    }
}

/// `directives` is claimed only where what is written comes back as a
/// directive with its name. twig spells one in HTML and AsciiDoc too — and
/// `page_break` is true there — but the walker reads their spellings of
/// `page-break` alone, so an arbitrary name would be written and then
/// drawn as something else, or not at all.
#[test]
fn directives_are_offered_only_where_the_walker_reads_them_back() {
    for (fmt, src) in [
        (Format::Markdown, "hello\n"),
        (Format::Djot, "hello\n"),
        (Format::Html, "<p>hello</p>\n"),
        (Format::Asciidoc, "hello\n"),
    ] {
        // What twig writes for a host's name, read back by the walker.
        let mut ed =
            twig::Editor::new_with_flags(src.as_bytes(), fmt.twig(), parse_extensions(fmt))
                .unwrap();
        ed.insert_directive(src.len() - 1, "x-card", None, &[("k", Some("v"))])
            .unwrap_or_else(|e| panic!("{fmt:?}: twig spells it: {e}"));
        let written = String::from_utf8(ed.source().unwrap()).unwrap();
        let mut d = fmt_doc(&written, fmt);
        d.view = View::Wysiwyg;
        d.build_visual(80);
        let read_back = d
            .vmap
            .directives
            .iter()
            .any(|m| m.name == "x-card" && m.attr("k") == Some("v"));
        let c = Capabilities::of(fmt);
        assert_eq!(c.directives, read_back, "{fmt:?}: {written:?}");
        assert!(c.page_break, "{fmt:?}");

        // And where it is not offered, it is refused in the format's name.
        if !c.directives {
            let mut d = fmt_doc(src, fmt);
            d.caret = d.source.find("hello").unwrap() + 5;
            d.insert_directive("x-card", None, &[]);
            assert_eq!(d.source, src, "{fmt:?}");
            let status = d.status.as_deref().unwrap_or("");
            assert!(status.contains("not supported"), "{fmt:?}: {status:?}");
        }
    }
    assert!(!Capabilities::of(Format::Xml).directives);
}

/// A terminal draws a host's directive as lines of its own and reports how
/// many, keyed by what the directive says; the placeholder grows to that
/// height the way a picture does, with blank fillers holding no caret.
#[test]
fn set_directive_rows_reserves_filler_rows_for_a_host_drawing() {
    let body = "intro\n\n::x-card[A]{src=\"u\"}\n\n::x-card[B]\n\nend\n";
    let mut d = wysiwyg_doc("directive_rows", body);
    assert_eq!(d.vmap.directives.len(), 2);
    assert!(d.vmap.directives.iter().all(|i| i.rows_span.len() == 1));
    let stops = |d: &mut Doc| {
        d.caret = 0;
        let mut seen = vec![d.caret];
        loop {
            d.move_right(false);
            if *seen.last().unwrap() == d.caret {
                break;
            }
            seen.push(d.caret);
        }
        seen
    };
    let before = stops(&mut d);

    let key = d.vmap.directives[0].key();
    assert_eq!(
        key,
        wysiwyg::DirectiveKey {
            name: "x-card".into(),
            label: "A".into(),
            attrs: vec![("src".into(), "u".into())],
        }
    );
    d.set_directive_rows(HashMap::from([(key, 4)]));
    d.build_visual(80);
    let span = d.vmap.directives[0].rows_span.clone();
    assert_eq!(span.len(), 4, "the four rows asked for");
    assert_eq!(
        d.vmap.rows[span.start]
            .leaf_directive
            .as_ref()
            .map(|m| m.rows),
        Some(4)
    );
    for r in span.start + 1..span.end {
        let row = &d.vmap.rows[r];
        assert!(row.decoration && row.glyphs.is_empty(), "filler {r}");
        assert!(row.leaf_directive.is_none(), "only the first row is marked");
    }
    assert_eq!(
        d.vmap.directives[1].rows_span.len(),
        1,
        "another label is another key"
    );
    assert_eq!(stops(&mut d), before, "reserving rows adds no stops");

    // Keyed by what it says, not where it stands: an edit above keeps it.
    d.caret = 0;
    d.insert("more ");
    d.build_visual(80);
    assert_eq!(d.vmap.directives[0].rows_span.len(), 4);
}

/// The vocabulary's capabilities, per format. The two block properties are
/// `SetBlockAttrs` and the three run ones `WrapRangeAttrs`, which is why
/// AsciiDoc can align a paragraph and not size a run: its `[#id.role]#text#`
/// keeps an id and a role and has no slot for a `data-` key.
#[test]
fn the_presentation_capabilities_are_ragged_per_format() {
    for fmt in [Format::Markdown, Format::Djot, Format::Html] {
        let c = Capabilities::of(fmt);
        assert!(c.alignment, "{fmt:?} alignment");
        assert!(c.line_spacing, "{fmt:?} line spacing");
        assert!(c.font_size, "{fmt:?} size");
        assert!(c.font_family, "{fmt:?} face");
        assert!(c.text_color, "{fmt:?} colour");
    }
    // Markdown spells both only under the extensions leaf parses with — a
    // `<div>` and a `<span>` read back as containers under `html_elements`,
    // and `::page-break` as a directive under `directives`. Ask twig's own
    // defaults and the answer is no, which is why `Capabilities` is built
    // with `supports_with`.
    assert!(!Format::Markdown.supports(Gesture::SetBlockAttrs));
    assert!(!Format::Markdown.supports(Gesture::WrapRangeAttrs));
    assert!(!Format::Markdown.supports(Gesture::InsertDirective));

    let adoc = Capabilities::of(Format::Asciidoc);
    assert!(adoc.alignment && adoc.line_spacing, "AsciiDoc's `[…]` line");
    assert!(
        !adoc.font_size && !adoc.font_family && !adoc.text_color,
        "AsciiDoc has no inline spelling that keeps a data- key"
    );

    // XML spells none of it, and neither page break — nor has it blocks a
    // caret could name for a move.
    let xml = Capabilities::of(Format::Xml);
    assert!(!xml.alignment && !xml.font_size && !xml.page_break && !xml.move_block);
    for f in [Format::Markdown, Format::Djot, Format::Html] {
        assert!(Capabilities::of(f).move_block, "{f:?} moves blocks");
    }
    for f in [
        Format::Markdown,
        Format::Djot,
        Format::Html,
        Format::Asciidoc,
    ] {
        assert!(Capabilities::of(f).page_break, "{f:?} breaks pages");
    }
}

/// A format that cannot spell a property refuses in its own words and
/// writes nothing — the guard every other gesture has.
#[test]
fn a_presentation_gesture_a_format_cannot_spell_is_refused_with_a_reason() {
    let src = "<doc><p>hello</p></doc>\n";
    #[allow(clippy::type_complexity)]
    let ops: [(&str, &dyn Fn(&mut Doc)); 6] = [
        ("alignment", &|d: &mut Doc| {
            d.set_alignment(Some(Align::Center))
        }),
        ("line spacing", &|d: &mut Doc| {
            d.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)))
        }),
        ("size", &|d: &mut Doc| {
            d.set_font_size(Some(FontSize::Step(SizeStep::Large)))
        }),
        ("face", &|d: &mut Doc| {
            d.set_font_family(Some(FontFace::Generic(FontFamily::Serif)))
        }),
        ("colour", &|d: &mut Doc| {
            d.set_text_color(Some(TextColor::Named(MarkColor::Red)))
        }),
        ("page break", &|d: &mut Doc| d.insert_page_break()),
    ];
    for (name, op) in ops {
        let mut d = fmt_doc(src, Format::Xml);
        let at = d.source.find("hello").unwrap();
        d.caret = at;
        d.anchor = Some(at + 5);
        op(&mut d);
        assert_eq!(d.source, src, "{name} edited an XML document");
        assert!(!d.dirty, "{name} marked the document dirty");
        let status = d.status.as_deref().unwrap_or("");
        assert!(
            status.contains("xml"),
            "{name}: the refusal should name the format, got {status:?}"
        );
    }

    // AsciiDoc is the ragged one: the block gesture works where the run
    // gesture does not, and a *selection* is what tells the two apart.
    let mut adoc = fmt_doc("hello world\n", Format::Asciidoc);
    adoc.anchor = Some(0);
    adoc.caret = 5;
    adoc.set_font_size(Some(FontSize::Step(SizeStep::Large)));
    assert_eq!(adoc.source, "hello world\n", "no inline spelling");
    assert!(adoc.status.is_some());
}

/// A read-only document takes none of it, and a caret on a blank line has
/// no block to carry an attribute — both say so rather than writing.
#[test]
fn a_presentation_gesture_respects_read_only_and_a_blank_line() {
    let mut ro = fmt_doc("hello\n", Format::Djot);
    ro.read_only = true;
    ro.caret = 1;
    ro.set_alignment(Some(Align::Center));
    assert_eq!(ro.source, "hello\n");

    let mut blank = fmt_doc("a\n\n\nb\n", Format::Djot);
    blank.caret = 2; // the empty line between the two paragraphs
    blank.set_alignment(Some(Align::Center));
    assert_eq!(blank.source, "a\n\n\nb\n");
    assert!(
        blank.status.as_deref().unwrap_or("").contains("no block"),
        "got {:?}",
        blank.status
    );
}

/// A block attribute gesture keeps the caret on the **text** it was on, not
/// on the byte offset it had. Markdown has nowhere to put a paragraph's
/// attributes but a `<div>` around it, and twig splices the div and the
/// block it wraps as one region — so a caret that kept its offset landed in
/// the markup, and every press after the first answered "no block at the
/// caret" with the toolbar's queries reading nothing.
#[test]
fn a_markdown_block_gesture_keeps_the_caret_on_its_text() {
    let word = |d: &Doc| d.caret - d.source.find("brown").unwrap();
    let mut md = fmt_doc("the quick brown fox\n", Format::Markdown);
    md.caret = md.source.find("brown").unwrap() + 2; // "br|own"

    // Wrapping: the div and two blank lines open above the block.
    md.set_alignment(Some(Align::Center));
    assert_eq!(
        md.source,
        "<div style=\"text-align: center\">\n\nthe quick brown fox\n\n</div>\n"
    );
    assert_eq!(word(&md), 2, "the caret left its word: {}", md.caret);
    assert_eq!(md.alignment_at_caret(), Some(Align::Center));

    // Re-styling: the attribute line changes length under the same caret,
    // and the second press reaches the same block rather than nothing.
    md.set_alignment(Some(Align::Right));
    assert_eq!(
        md.source, "<div style=\"text-align: right\">\n\nthe quick brown fox\n\n</div>\n",
        "a second press re-styles the div"
    );
    assert_eq!(md.status, None);
    assert_eq!(word(&md), 2);

    // A second key on the same div — the line grows, the caret rides it.
    md.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
    assert_eq!(
        md.source,
        "<div style=\"text-align: right; line-height: 2\">\n\nthe quick brown fox\n\n</div>\n"
    );
    assert_eq!(word(&md), 2);
    assert_eq!(
        md.line_spacing_at_caret(),
        Some(LineHeight::Step(LineSpacing::Double))
    );

    // Unwrapping: the line shrinks, and then the div goes altogether.
    md.set_alignment(None);
    assert_eq!(
        md.source,
        "<div style=\"line-height: 2\">\n\nthe quick brown fox\n\n</div>\n"
    );
    assert_eq!(word(&md), 2);
    md.set_line_spacing(None);
    assert_eq!(md.source, "the quick brown fox\n", "the last key unwraps");
    assert_eq!(word(&md), 2, "the caret came back down with the block");
    assert_eq!(md.alignment_at_caret(), None);
    assert_eq!(md.status, None);
}

/// The same rule in djot, where the spelling is a `{…}` line *above* the
/// block rather than a wrapper around it: inserting it pushes the block
/// down, re-styling it changes the line's length, and clearing the last key
/// takes the line away again. The caret rides all three.
#[test]
fn a_djot_attribute_line_keeps_the_caret_on_its_text() {
    let word = |d: &Doc| d.caret - d.source.find("brown").unwrap();
    let mut dj = fmt_doc("the quick brown fox\n", Format::Djot);
    dj.caret = dj.source.find("brown").unwrap() + 2;

    dj.set_alignment(Some(Align::Center));
    assert_eq!(
        dj.source,
        "{style=\"text-align: center\"}\nthe quick brown fox\n"
    );
    assert_eq!(word(&dj), 2);
    assert_eq!(dj.alignment_at_caret(), Some(Align::Center));

    dj.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
    assert_eq!(
        dj.source, "{style=\"text-align: center; line-height: 2\"}\nthe quick brown fox\n",
        "a second press edits the line the first wrote"
    );
    assert_eq!(word(&dj), 2);

    dj.set_alignment(None);
    assert_eq!(
        dj.source,
        "{style=\"line-height: 2\"}\nthe quick brown fox\n"
    );
    assert_eq!(word(&dj), 2);

    dj.set_line_spacing(None);
    assert_eq!(dj.source, "the quick brown fox\n");
    assert_eq!(word(&dj), 2);
    assert_eq!(dj.status, None);
}

/// The run gestures with no selection are the block gesture, so they keep
/// the caret the same way — and a heading keeps it inside the heading's own
/// text, past the `# ` its content span starts after. A selection rides
/// along whole: a block gesture is not a run gesture, and what was selected
/// before the press is still selected after it.
#[test]
fn a_block_gesture_carries_a_selection_and_a_heading_caret_too() {
    // No selection: the run gesture goes through the block door.
    let mut md = fmt_doc("the quick brown fox\n", Format::Markdown);
    md.caret = md.source.find("brown").unwrap() + 2;
    md.set_font_size(Some(FontSize::Step(SizeStep::Large)));
    assert_eq!(
        md.source,
        "<div style=\"font-size: large\">\n\nthe quick brown fox\n\n</div>\n"
    );
    assert_eq!(md.caret - md.source.find("brown").unwrap(), 2);
    assert_eq!(
        md.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Large))
    );
    md.set_font_size(Some(FontSize::Step(SizeStep::Small)));
    assert_eq!(
        md.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::Small)),
        "the second press reached the same block"
    );

    // A selection: alignment is the block's whatever is selected, and the
    // words stay selected.
    let mut sel = fmt_doc("the quick brown fox\n", Format::Markdown);
    let at = sel.source.find("brown").unwrap();
    sel.anchor = Some(at);
    sel.caret = at + 5;
    sel.set_alignment(Some(Align::Center));
    let now = sel.source.find("brown").unwrap();
    assert_eq!(sel.selection(), Some((now, now + 5)), "the words moved out");

    // A heading: the content span starts past the `# `.
    let mut h = fmt_doc("# hi there\n\nbody\n", Format::Markdown);
    h.caret = h.source.find("there").unwrap() + 1;
    h.set_alignment(Some(Align::Right));
    assert_eq!(
        h.source,
        "<div style=\"text-align: right\">\n\n# hi there\n\n</div>\n\nbody\n"
    );
    assert_eq!(h.caret, h.source.find("there").unwrap() + 1);
    assert_eq!(h.alignment_at_caret(), Some(Align::Right));
}

/// A djot document open in the rich view, with its map built as
/// [`wysiwyg_doc`] builds a Markdown one's.
fn wysiwyg_djot(body: &str) -> Doc {
    let mut d = fmt_doc(body, Format::Djot);
    d.view = View::Wysiwyg;
    d.build_visual(80);
    d
}

/// Backspace at the start of a block whose presentation is spelled as
/// hidden markup before it strips that presentation, the way Backspace at
/// a heading's start strips its `#`. The ordinary delete fused djot's
/// `{.center}` line onto the text and took the blank line a Markdown div
/// needs between its tag and its paragraph.
#[test]
fn backspace_at_the_start_of_a_centred_paragraph_strips_its_attributes() {
    let mut md = wysiwyg_doc(
        "wys_attr_bksp",
        "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n",
    );
    md.caret = md.source.find("hello").unwrap();
    md.backspace();
    assert_eq!(
        md.source, "above\n\nhello\n\nbelow\n",
        "the div is unwrapped"
    );
    assert_eq!(md.caret, 7, "the caret stays at the start of its text");
    md.backspace();
    assert_eq!(
        md.source, "above\nhello\n\nbelow\n",
        "the next press joins the paragraphs, as it always did"
    );

    let mut dj = wysiwyg_djot("above\n\n{.center}\nhello\n\nbelow\n");
    dj.caret = dj.source.find("hello").unwrap();
    dj.backspace();
    assert_eq!(
        dj.source, "above\n\nhello\n\nbelow\n",
        "the attribute line goes"
    );
    assert_eq!(dj.caret, 7);

    // A heading's own marker is the nearer hidden markup, and goes first;
    // the attributes are the next press's.
    let mut dj = wysiwyg_djot("{.center}\n# Title\n");
    dj.caret = dj.source.find("Title").unwrap();
    dj.backspace();
    assert_eq!(dj.source, "{.center}\nTitle\n", "the `#` first");
    dj.build_visual(80);
    dj.backspace();
    assert_eq!(dj.source, "Title\n", "then the attributes");
    assert_eq!(dj.caret, 0);
}

/// A div around several blocks has no sole child for twig to unwrap, so
/// at its first block the caret steps back to the stop before rather than
/// taking the div apart; a later block has an ordinary paragraph above it
/// and joins as any paragraph does.
#[test]
fn backspace_at_the_first_of_a_div_s_blocks_steps_back_and_a_later_one_joins() {
    let src = "above\n\n<div class=\"center\">\n\nhello\n\nworld\n\n</div>\n";
    let mut d = wysiwyg_doc("wys_div_first", src);
    d.caret = d.source.find("hello").unwrap();
    d.backspace();
    assert_eq!(d.source, src, "nothing is deleted");
    assert_eq!(d.caret, 5, "the caret steps back to the end of `above`");

    let mut d = wysiwyg_doc("wys_div_later", src);
    d.caret = d.source.find("world").unwrap();
    d.backspace();
    assert_eq!(
        d.source, "above\n\n<div class=\"center\">\n\nhello\nworld\n\n</div>\n",
        "a later block joins the one above it"
    );
}

/// Backspace at the start of the paragraph after a Markdown div joins it
/// into the div's last paragraph — the join any two paragraphs make, with
/// the hidden `</div>` carried past the joined text. The ordinary delete
/// took the newline under the tag, which drew nothing different, and the
/// next press took the `>` and left the div unclosed.
#[test]
fn backspace_after_a_div_joins_the_paragraph_into_it() {
    let src = "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n";
    let mut d = wysiwyg_doc("wys_div_join", src);
    d.caret = d.source.find("below").unwrap();
    d.backspace();
    assert_eq!(
        d.source,
        "above\n\n<div class=\"center\">\n\nhello\nbelow\n\n</div>\n"
    );
    assert_eq!(
        d.caret,
        d.source.find("below").unwrap(),
        "the caret stays at the start of the joined text"
    );
    d.build_visual(80);
    assert_eq!(
        d.alignment_at_caret(),
        Some(Align::Center),
        "and is centred now"
    );
    d.undo();
    assert_eq!(d.source, src, "one undo step");

    // A list closes the div: the paragraph joins the last item's text,
    // under the item's continuation indent, inside the div.
    let src = "<div class=\"center\">\n\n- item\n\n</div>\n\nbelow\n";
    let mut d = wysiwyg_doc("wys_div_list", src);
    d.caret = d.source.find("below").unwrap();
    d.backspace();
    assert_eq!(
        d.source, "<div class=\"center\">\n\n- item\n  below\n\n</div>\n",
        "the paragraph joins the item"
    );
    assert_eq!(d.caret, d.source.find("below").unwrap());

    // And where twig has nothing to join into — a code block above — the
    // caret steps back to the stop before, and nothing is deleted.
    let src = "```\ncode\n```\n\nbelow\n";
    let mut d = wysiwyg_doc("wys_code_then_para", src);
    d.caret = d.source.find("below").unwrap();
    d.backspace();
    assert_eq!(d.source, src, "nothing is deleted");
    assert!(
        d.caret < d.source.find("below").unwrap(),
        "the caret stepped back"
    );
}

/// Backspace on a blank line collapses to the stop before it — but not
/// across hidden markup, which that collapse deleted whole: a `</div>`,
/// or a comment between two blocks. There the blank line goes alone, and
/// the caret lands where the collapse would have put it.
#[test]
fn backspace_on_a_blank_line_after_hidden_markup_keeps_the_markup() {
    let mut d = wysiwyg_doc(
        "wys_div_blank",
        "<div class=\"center\">\n\nhello\n\n</div>\n\n\n\nbelow\n",
    );
    d.caret = d.source.find("below").unwrap() - 2; // the empty paragraph
    assert!(
        d.vmap.is_stop(d.caret),
        "the empty paragraph is a caret home"
    );
    d.backspace();
    assert_eq!(
        d.source, "<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n",
        "the blank line goes and the div stays closed"
    );
    assert_eq!(
        d.caret,
        d.source.find("hello").unwrap() + 5,
        "onto the end of `hello`"
    );

    let mut d = wysiwyg_doc("wys_comment_blank", "above\n\n<!-- note -->\n\n\n\nbelow\n");
    d.caret = d.source.find("below").unwrap() - 2;
    assert!(d.vmap.is_stop(d.caret));
    d.backspace();
    assert_eq!(
        d.source, "above\n\n<!-- note -->\n\nbelow\n",
        "the comment stays"
    );
    assert_eq!(d.caret, 5);
}

/// Backspace at the end of an attributed span steps inside its hidden
/// closing tag the way it steps inside a `**`, and takes the span with
/// its last letter. Before, the byte-step took the `>` of `</span>`,
/// which left the paragraph unparseable: it vanished from the rich view,
/// and the Backspace after that joined the next block into the wreck.
#[test]
fn backspace_walks_into_a_sized_span_and_takes_the_emptied_span_with_its_space() {
    let src = "above\n\nThis <span data-size=\"x-large\">is</span> a test\n\nTest 2\n";
    let mut d = wysiwyg_doc("wys_span_bs", src);
    d.caret = d.source.find("a test").unwrap() + 6;
    for _ in 0..7 {
        d.backspace();
    }
    assert_eq!(
        d.source,
        "above\n\nThis <span data-size=\"x-large\">is</span>\n\nTest 2\n"
    );
    d.backspace();
    assert_eq!(
        d.source, "above\n\nThis <span data-size=\"x-large\">i</span>\n\nTest 2\n",
        "the first Backspace after the tag takes the letter, not the `>`"
    );
    d.backspace();
    assert_eq!(
        d.source, "above\n\nThis \n\nTest 2\n",
        "the last letter takes the span with it"
    );
    assert_eq!(d.caret, 12, "the caret is where the letter was");
    d.backspace();
    assert_eq!(d.source, "above\n\nThis\n\nTest 2\n");
    assert_eq!(d.caret, 11);
    d.build_visual(80);
    assert!(
        d.vmap
            .rows
            .iter()
            .any(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>() == "This"),
        "the paragraph is still drawn"
    );
}

#[test]
fn backspace_walks_into_a_djot_sized_span_too() {
    let mut d = wysiwyg_djot("This [is]{data-size=\"x-large\"}\n\nTest 2\n");
    d.caret = d.source.find("\n\nTest 2").unwrap();
    // The caret home at the paragraph's end is inside the span, before
    // its `]`: the map offers no stop after `]{…}`.
    d.build_visual(80);
    assert_eq!(d.vmap.stop_before(31), Some(8));
    d.caret = 8;
    d.backspace();
    assert_eq!(d.source, "This [i]{data-size=\"x-large\"}\n\nTest 2\n");
    d.backspace();
    assert_eq!(
        d.source, "This \n\nTest 2\n",
        "the attribute block outside the span goes with it"
    );
    d.backspace();
    assert_eq!(d.source, "This\n\nTest 2\n");
    assert_eq!(d.caret, 4);
}

/// A span that is empty as the file was written has no stop of its own;
/// Backspace reaching it from behind takes it with the character before
/// it, the character the key looked aimed at.
#[test]
fn backspace_over_an_already_empty_span_takes_it_with_the_character_before() {
    let src = "This <span data-size=\"x-large\"></span> a test\n";
    let mut d = wysiwyg_doc("wys_span_empty", src);
    d.caret = d.source.find(" a test").unwrap();
    d.backspace();
    assert_eq!(d.source, "This a test\n");
    assert_eq!(d.caret, 4);
}

/// The mirror: Delete in front of a span's opening tag takes its first
/// letter, and the span with its last.
#[test]
fn delete_walks_into_a_sized_span_and_takes_the_span_with_its_last_letter() {
    let src = "This <span data-size=\"x-large\">is</span> a test\n";
    let mut d = wysiwyg_doc("wys_span_del", src);
    d.caret = 5;
    d.delete_forward();
    assert_eq!(
        d.source,
        "This <span data-size=\"x-large\">s</span> a test\n"
    );
    d.delete_forward();
    assert_eq!(d.source, "This  a test\n");
    assert_eq!(d.caret, 5);
    d.delete_forward();
    assert_eq!(d.source, "This a test\n");
    assert_eq!(d.caret, 5);
}

/// The block version of the span's emptying rule. Centre a one-letter
/// paragraph — Markdown spells that as a `<div>` around it — and
/// Backspace the letter: the div goes with it, leaving a plain blank line
/// the caret is at home on, in the incremental map and the from-scratch
/// one alike. Before, the letter went alone; the emptied div drew a
/// caret home only the stale map had, and the next Backspace collapsed
/// the line and left `<div class="center">\n\n</div>` standing invisibly
/// in the file.
#[test]
fn backspace_that_empties_a_centred_paragraph_takes_its_div_with_the_letter() {
    let mut d = wysiwyg_doc("wys_div_empty", "Try the toolbar.\n\nT\n");
    d.caret = d.source.len() - 1;
    d.set_alignment(Some(Align::Center));
    assert_eq!(
        d.source,
        "Try the toolbar.\n\n<div style=\"text-align: center\">\n\nT\n\n</div>\n"
    );
    d.backspace();
    assert_eq!(d.source, "Try the toolbar.\n\n\n");
    assert_eq!(d.caret, 18, "on the blank line where the letter was");
    // The host rebuilds the map after every key; the spliced map and a
    // fresh one both give the line a caret home.
    d.build_visual_unwrapped();
    assert!(d.vmap.is_stop(18));
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the div goes");
    d.backspace();
    assert_eq!(
        d.source, "Try the toolbar.\n",
        "then the blank line collapses"
    );
    assert_eq!(d.caret, 16);

    // With a block after the div the blank line keeps a gap each side.
    let mut d = wysiwyg_doc(
        "wys_div_empty_mid",
        "Try the toolbar.\n\n<div class=\"center\">\n\nT\n\n</div>\n\nbelow\n",
    );
    d.caret = d.source.find("T\n").unwrap() + 1;
    d.backspace();
    assert_eq!(d.source, "Try the toolbar.\n\n\n\nbelow\n");
    assert_eq!(d.caret, 18);
    d.build_visual_unwrapped();
    assert!(d.vmap.is_stop(18));
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the div goes");
}

/// djot spells the same paragraph as a `{.center}` line above it, and
/// twig has no node at all for that line once the paragraph is gone —
/// so the line goes with the letter too.
#[test]
fn backspace_that_empties_a_centred_paragraph_takes_its_djot_attrs_line_too() {
    let mut d = fmt_doc("Try the toolbar.\n\nT\n", Format::Djot);
    d.build_visual(80);
    d.caret = d.source.len() - 1;
    d.set_alignment(Some(Align::Center));
    assert_eq!(
        d.source,
        "Try the toolbar.\n\n{style=\"text-align: center\"}\nT\n"
    );
    d.backspace();
    assert_eq!(d.source, "Try the toolbar.\n\n\n");
    assert_eq!(d.caret, 18);
    d.build_visual(80);
    assert!(d.vmap.is_stop(18));
}

/// The rule is for a block that would be no block: a div holding more
/// keeps its tags, and an emptied heading is still a heading.
#[test]
fn emptying_a_paragraph_keeps_a_div_that_holds_more_and_a_heading_its_marker() {
    let mut d = wysiwyg_doc(
        "wys_div_more",
        "<div class=\"center\">\n\nText\n\nT\n\n</div>\n",
    );
    d.caret = d.source.find("T\n").unwrap() + 1;
    d.backspace();
    assert_eq!(d.source, "<div class=\"center\">\n\nText\n\n\n\n</div>\n");
    assert_eq!(d.caret, 28, "the blank line inside the div, as after Enter");

    let mut d = wysiwyg_doc(
        "wys_div_heading",
        "<div class=\"center\">\n\n# T\n\n</div>\n",
    );
    d.caret = d.source.find("T\n").unwrap() + 1;
    d.backspace();
    assert_eq!(d.source, "<div class=\"center\">\n\n# \n\n</div>\n");
    assert_eq!(d.caret, 24);
    d.build_visual(80);
    assert!(
        d.vmap.is_stop(24),
        "the empty heading is still a caret home"
    );
}

/// The mirror: Delete in front of the letter takes the div with it.
#[test]
fn delete_that_empties_a_centred_paragraph_takes_its_div_with_the_letter() {
    let mut d = wysiwyg_doc(
        "wys_div_empty_del",
        "Try the toolbar.\n\n<div class=\"center\">\n\nT\n\n</div>\n",
    );
    d.caret = d.source.find("T\n").unwrap();
    d.delete_forward();
    assert_eq!(d.source, "Try the toolbar.\n\n\n");
    assert_eq!(d.caret, 18);
    d.build_visual(80);
    assert!(d.vmap.is_stop(18));

    let mut d = fmt_doc("Try the toolbar.\n\n{.center}\nT\n", Format::Djot);
    d.build_visual(80);
    d.caret = d.source.find("T\n").unwrap();
    d.delete_forward();
    assert_eq!(d.source, "Try the toolbar.\n\n\n");
    assert_eq!(d.caret, 18);
}

/// Backspace at a block's start is twig's join, spelled per format — so
/// the cases the one-newline delete got wrong come out right: a
/// paragraph joins onto a heading's line, HTML's `</p><p>` goes as one,
/// and a quote's prefix is written on the joined line.
#[test]
fn backspace_at_a_block_start_joins_it_the_format_s_way() {
    let mut d = wysiwyg_doc("wys_join_heading", "# Title\n\nbelow\n");
    d.caret = d.source.find("below").unwrap();
    d.backspace();
    assert_eq!(d.source, "# Title below\n", "onto the heading's line");
    assert_eq!(d.caret, d.source.find("below").unwrap());
    d.undo();
    assert_eq!(d.source, "# Title\n\nbelow\n", "one undo step");

    let mut d = wysiwyg_doc("wys_join_quote", "> a\n\nb\n");
    d.caret = d.source.find('b').unwrap();
    d.backspace();
    assert_eq!(d.source, "> a\n> b\n", "into the quote, with its prefix");
    assert_eq!(d.caret, d.source.find('b').unwrap());

    let mut h = fmt_doc("<p>above</p>\n<p class=\"x\">below</p>\n", Format::Html);
    h.view = View::Wysiwyg;
    h.build_visual(80);
    h.caret = h.source.find("below").unwrap();
    h.backspace();
    assert_eq!(
        h.source, "<p>above\nbelow</p>\n",
        "one paragraph, the tag gone whole"
    );
    assert_eq!(h.caret, h.source.find("below").unwrap());
}

/// A selection from the end of a run's text to the next block's start is
/// the run's closing delimiters and a blank line in the source, and one
/// `"\n"` on screen: deleting it — iOS's `replace(_:withText:)` one stop
/// back — joins the blocks and keeps the run closed. It took the `***`
/// with the newlines and left the opening ones on screen.
#[test]
fn deleting_a_selection_keeps_the_delimiters_of_a_run_it_cuts() {
    let src = "good.\n\n***HENRY B EYRING***\n\nWhy is life\n";
    let mut d = wysiwyg_doc("sel_cut_close", src);
    let end = d.source.find("G***").unwrap() + 1;
    d.place_caret(end, false);
    d.place_caret(d.source.find("Why").unwrap(), true);
    d.insert("");
    assert_eq!(d.source, "good.\n\n***HENRY B EYRING***Why is life\n");
    assert_eq!(d.caret, end, "at the end of the run's text");

    // On one line, from inside a run to past it, and typing over it.
    let mut d = wysiwyg_doc("sel_cut_line", "a **bold** text\n");
    d.place_caret(d.source.find("ld").unwrap(), false);
    d.place_caret(d.source.find("xt").unwrap(), true);
    d.backspace();
    assert_eq!(d.source, "a **bo**xt\n");
    let mut d = wysiwyg_doc("sel_cut_type", "a **bold** text\n");
    d.place_caret(d.source.find("ld").unwrap(), false);
    d.place_caret(d.source.find("xt").unwrap(), true);
    d.insert("X");
    assert_eq!(d.source, "a **boX**xt\n", "the text lands in the run");

    // Cut at its start, the run keeps its opening delimiters, and the
    // text typed goes in front of them.
    let mut d = wysiwyg_doc("sel_cut_open", "plain **bold**\n");
    d.place_caret(3, false);
    d.place_caret(d.source.find("ld").unwrap(), true);
    d.insert("X");
    assert_eq!(d.source, "plaX**ld**\n");

    // A run whose whole text goes, with more past it, goes whole.
    let mut d = wysiwyg_doc("sel_cut_whole", "a **bold** text\n");
    d.place_caret(d.source.find("bold").unwrap(), false);
    d.place_caret(d.source.find("xt").unwrap(), true);
    d.backspace();
    assert_eq!(d.source, "a xt\n");
}

/// A paste of several paragraphs over a selection that cuts runs: the run
/// cut at its end closes after the paste's first line, and the run cut
/// at its start opens after its last, so each stays on one paragraph.
#[test]
fn a_paste_of_paragraphs_over_a_cut_run_closes_it_on_its_own_line() {
    let src = "a **bold** and *italic* text\n";
    let mut d = wysiwyg_doc("sel_cut_paste", src);
    d.place_caret(d.source.find("ld").unwrap(), false);
    d.place_caret(d.source.find("alic").unwrap(), true);
    d.paste("one\n\ntwo");
    assert_eq!(d.source, "a **boone**\n\ntwo*alic* text\n");
    assert_eq!(d.caret, d.source.find("*alic").unwrap(), "after the paste");

    // A paste ending in a space puts the space outside the run, where a
    // closing delimiter can still close it.
    let mut d = wysiwyg_doc("sel_cut_paste_space", "a **bold** text\n");
    d.place_caret(d.source.find("ld").unwrap(), false);
    d.place_caret(d.source.find("xt").unwrap(), true);
    d.paste("X ");
    assert_eq!(d.source, "a **boX** xt\n");
}

/// Enter over a selection that cuts a run deletes it and splits there,
/// so the run closes on the line it is on.
#[test]
fn enter_over_a_selection_that_cuts_a_run_splits_where_it_was() {
    let mut d = wysiwyg_doc("sel_cut_enter", "a **bold** text\n");
    d.place_caret(d.source.find("ld").unwrap(), false);
    d.place_caret(d.source.find("xt").unwrap(), true);
    d.newline();
    assert_eq!(d.source, "a **bo**\n\nxt\n");
}

/// Delete at the end of a block's content is the same join aimed at the
/// block after it, and the caret stays where the joined text now begins.
#[test]
fn delete_at_a_block_end_joins_the_next_block_into_it() {
    let src = "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n";
    let mut d = wysiwyg_doc("wys_del_join", src);
    d.caret = d.source.find("hello").unwrap() + 5;
    d.delete_forward();
    assert_eq!(
        d.source, "above\n\n<div class=\"center\">\n\nhello\nbelow\n\n</div>\n",
        "below joins hello inside the div"
    );
    assert_eq!(
        d.caret,
        d.source.find("hello").unwrap() + 5,
        "the caret stays"
    );

    let mut d = wysiwyg_doc("wys_del_join_head", "above\n\n# Title\n");
    d.caret = 5;
    d.delete_forward();
    assert_eq!(
        d.source, "above\nTitle\n",
        "the heading's marker goes with the join"
    );
    assert_eq!(d.caret, 5);

    // A code block after the paragraph: nothing to join, the caret steps
    // forward onto the next stop and nothing is deleted.
    let src = "above\n\n```\ncode\n```\n";
    let mut d = wysiwyg_doc("wys_del_code", src);
    d.caret = 5;
    d.delete_forward();
    assert_eq!(d.source, src);
    assert!(d.caret > 5, "the caret stepped forward");
}

/// The vocabulary is read out of a `style` another app wrote: CSS's own
/// property names, its keywords in any case, `!important` and all. A
/// declaration this vocabulary cannot read is still what every other
/// renderer draws, so it wins over leaf 0.2's attribute beside it and the
/// menu ticks *Default*.
#[test]
fn a_style_from_elsewhere_is_read_as_css() {
    let mut d = fmt_doc(
        "<p style=\"TEXT-ALIGN:Center;font-size: X-Large !important;line-height:1.50\">hi</p>\n",
        Format::Html,
    );
    d.caret = d.source.find("hi").unwrap();
    assert_eq!(d.alignment_at_caret(), Some(Align::Center));
    assert_eq!(
        d.font_size_at_caret(),
        Some(FontSize::Step(SizeStep::XLarge))
    );
    assert_eq!(
        d.line_spacing_at_caret(),
        Some(LineHeight::Step(LineSpacing::OneHalf))
    );

    let mut px = fmt_doc(
        "<p style=\"font-size: 16px\" data-size=\"large\">hi</p>\n",
        Format::Html,
    );
    px.caret = px.source.find("hi").unwrap();
    assert_eq!(px.font_size_at_caret(), None, "the declaration wins");
}

/// A colour leaf names is written as that name's ink, so another app draws
/// it, and the ink is read back as the name, so the theme keeps both of its
/// inks for it. Any other triple is exact, and CSS's own colour keywords
/// that share a name with the vocabulary read as that name.
#[test]
fn a_named_colour_is_its_ink_in_style_and_its_name_in_leaf() {
    let colour = |style: &str| {
        let mut d = fmt_doc(&format!("<p style=\"{style}\">hi</p>\n"), Format::Html);
        d.caret = d.source.find("hi").unwrap();
        d.text_color_at_caret()
    };
    for c in MarkColor::ALL {
        assert_eq!(
            colour(&format!("color: {}", c.hex().to_uppercase())),
            Some(TextColor::Named(c))
        );
    }
    assert_eq!(colour("color: Red"), Some(TextColor::Named(MarkColor::Red)));
    assert_eq!(
        colour("color: #d24b40"),
        Some(TextColor::Rgb {
            r: 0xd2,
            g: 0x4b,
            b: 0x40
        })
    );
    assert_eq!(colour("color: rgb(1, 2, 3)"), None);
}

/// A face is the first family of CSS's list, unquoted; the rest are a
/// renderer's fallbacks. A family name is written in single quotes and
/// reads back as the name it was.
#[test]
fn a_family_is_the_first_of_the_list_and_is_written_quoted() {
    let face = |style: &str| {
        let mut d = fmt_doc(&format!("<p style='{style}'>hi</p>\n"), Format::Html);
        d.caret = d.source.find("hi").unwrap();
        d.font_family_at_caret()
    };
    assert_eq!(
        face("font-family: \"Gill Sans\", sans-serif"),
        Some(FontFace::Named("Gill Sans".into()))
    );
    assert_eq!(
        face("font-family: Times   New Roman, serif"),
        Some(FontFace::Named("Times New Roman".into()))
    );
    assert_eq!(
        face("font-family: SERIF"),
        Some(FontFace::Generic(FontFamily::Serif))
    );

    let mut d = fmt_doc("hello\n", Format::Djot);
    d.caret = 1;
    d.set_font_family(Some(FontFace::Named("Gill Sans".into())));
    assert_eq!(d.source, "{style=\"font-family: 'Gill Sans'\"}\nhello\n");
    assert_eq!(
        d.font_family_at_caret(),
        Some(FontFace::Named("Gill Sans".into()))
    );

    // A quote or a backslash in the name has no spelling every format reads
    // back — twig refuses a `"` in an attribute, and a backslash is an
    // escape to CSS and to djot both — so the press says so and writes
    // nothing.
    for name in ["O'Neil Sans", "Say \"Hi\"", "Back\\slash"] {
        let mut d = fmt_doc("hello\n", Format::Markdown);
        d.caret = 1;
        d.set_font_family(Some(FontFace::Named(name.into())));
        assert_eq!(d.source, "hello\n");
        assert_eq!(
            d.status.as_deref(),
            Some("font: a family name with a quote or a backslash in it cannot be written")
        );
    }
}

/// A `style` that is not CSS `fig-css` can edit is refused out loud rather
/// than rewritten whole, which would lose what another app wrote there.
#[test]
fn a_style_that_is_not_css_is_left_alone() {
    let src = "<p style=\"color: red; }\">hi</p>\n";
    let mut d = fmt_doc(src, Format::Html);
    d.caret = d.source.find("hi").unwrap();
    d.set_alignment(Some(Align::Center));
    assert_eq!(d.source, src);
    assert_eq!(
        d.status.as_deref(),
        Some("alignment: the style attribute is not CSS leaf can edit")
    );
}

/// AsciiDoc keeps leaf 0.2's spelling: its `style` is a positional
/// attribute that means something else, and a role and a named attribute
/// are what it has for the vocabulary.
#[test]
fn asciidoc_keeps_the_class_and_data_spelling() {
    let mut a = fmt_doc("hello\n", Format::Asciidoc);
    a.caret = 1;
    a.set_alignment(Some(Align::Center));
    a.caret = a.source.find("hello").unwrap();
    a.set_line_spacing(Some(LineHeight::Step(LineSpacing::OneHalf)));
    assert_eq!(a.source, "[.center,data-line-height=1.5]\nhello\n");
    assert_eq!(a.alignment_at_caret(), Some(Align::Center));
}
