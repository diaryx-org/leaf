use super::*;

// ── ragged formats ───────────────────────────────────────────────────────
// No format spells every gesture. HTML writes the inline marks as a tag pair
// and no heading, list, quote or link; Markdown spells five of the eight
// marks — the highlight only because leaf parses with `highlight`, which is
// why the question is asked with the extensions; djot spells all eight and
// no in-cell break. leaf asks twig per
// gesture (`Doc::supports`) and refuses at the door, rather than letting each
// op discover the fact on its own — one of them didn't.

/// An HTML document in the rich view, ready for a gesture.
fn html_doc(body: &str) -> Doc {
    let mut d = Doc::from_source(body.to_string(), Format::Html).unwrap();
    d.view = View::Wysiwyg;
    d.build_visual(80);
    d
}

#[test]
fn a_table_gesture_leaves_an_html_table_alone() {
    // The regression this guard exists for. twig's table editor consults no
    // `Syntax` table — it spells a grid, not a delimiter — so it rebuilt an
    // HTML `<table>` as a *pipe table* and reported success: the whole
    // element replaced by `| a | b |`, silently, on one press of a toolbar
    // button. Every grid op went the same way.
    let src = "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>\n";
    // A table of named operations, which is what it looks like.
    #[allow(clippy::type_complexity)]
    let ops: [(&str, &dyn Fn(&mut Doc)); 7] = [
        ("insert row", &|d: &mut Doc| d.table_insert_row(true)),
        ("delete row", &|d: &mut Doc| d.table_delete_row()),
        ("insert column", &|d: &mut Doc| d.table_insert_column(true)),
        ("delete column", &|d: &mut Doc| d.table_delete_column()),
        ("align", &|d: &mut Doc| {
            d.table_set_alignment(Alignment::Right)
        }),
        ("move row", &|d: &mut Doc| d.table_move_row(true)),
        ("move column", &|d: &mut Doc| d.table_move_column(true)),
    ];
    for (name, op) in ops {
        let mut d = html_doc(src);
        d.caret = d.source.find('a').unwrap();
        assert!(d.caret_in_table(), "{name}: the caret really is in a table");
        op(&mut d);
        assert_eq!(d.source, src, "{name} rewrote an HTML table");
        assert!(
            !d.dirty,
            "{name} marked the document dirty without editing it"
        );
        assert!(d.status.is_some(), "{name} refused without saying why");
    }
}

#[test]
fn the_block_gestures_html_cannot_spell_are_refused_with_a_reason() {
    // A task box is a form control in HTML and a footnote has no native
    // spelling at all — the two gestures twig 3.5 still spells nothing
    // for, now that a quote, a list, a link and an image print through
    // its renderer (see the test below).
    let src = "<h1>Title</h1>\n<p>Hello world</p>\n<ul><li>one</li></ul>\n";
    // A table of named operations, which is what it looks like.
    #[allow(clippy::type_complexity)]
    let ops: [(&str, &dyn Fn(&mut Doc)); 3] = [
        ("task item", &|d: &mut Doc| d.toggle_task_item()),
        ("task tick", &|d: &mut Doc| d.toggle_task_checked()),
        ("footnote", &|d: &mut Doc| d.insert_footnote()),
    ];
    for (name, op) in ops {
        let mut d = html_doc(src);
        let at = d.source.find("Hello").unwrap();
        d.caret = at;
        d.anchor = Some(at + 5); // a selection, for the ops that want one
        op(&mut d);
        assert_eq!(d.source, src, "{name} edited an HTML document");
        assert!(
            !d.dirty,
            "{name} marked the document dirty without editing it"
        );
        let status = d.status.as_deref().unwrap_or("");
        assert!(
            status.contains("html"),
            "{name}: the refusal should name the format, got {status:?}"
        );
    }
}

#[test]
fn html_spells_a_quote_a_list_a_link_and_an_image_through_the_renderer() {
    // twig 3.5: where HTML has no marker alphabet it prints the fresh
    // node — a `<blockquote>` around the paragraph, a `<ul>`/`<ol>` with
    // the paragraph as its item, an `<a>` or `<img>` over the selection.
    // Until then every one of these was a refusal; now each is a real
    // edit, which is what the toolbar's capability flags say too.
    let src = "<h1>Title</h1>\n<p>Hello world</p>\n<ul><li>one</li></ul>\n";
    #[allow(clippy::type_complexity)]
    let ops: [(&str, &dyn Fn(&mut Doc), &str); 5] = [
        (
            "quote",
            &|d: &mut Doc| d.toggle_blockquote(),
            "<blockquote>",
        ),
        ("list", &|d: &mut Doc| d.toggle_list(false), "<ul>\n<li>"),
        (
            "ordered list",
            &|d: &mut Doc| d.toggle_list(true),
            "<ol>\n<li>",
        ),
        (
            "link",
            &|d: &mut Doc| d.insert_link("https://example.dev"),
            "<a href=\"https://example.dev\">Hello</a>",
        ),
        (
            "image",
            &|d: &mut Doc| d.insert_image("pic.png", "alt"),
            "<img alt=\"Hello\" src=\"pic.png\">",
        ),
    ];
    for (name, op, expect) in ops {
        let mut d = html_doc(src);
        let at = d.source.find("Hello").unwrap();
        d.caret = at;
        d.anchor = Some(at + 5);
        op(&mut d);
        assert!(d.source.contains(expect), "{name}: got {:?}", d.source);
        assert!(d.dirty, "{name}: a real edit");
        assert_eq!(
            d.status, None,
            "{name}: a supported gesture reports nothing"
        );
    }
}

#[test]
fn html_spells_a_heading_as_its_tag_pair() {
    // twig 3.4 rebuilds a heading or paragraph as its tag pair, attributes
    // along — the one block gesture whose HTML shape it can write. So ⌘2
    // in an HTML document is a real edit, and ⌘0 takes it back.
    let src = "<h1>Title</h1>\n<p>Hello world</p>\n";
    let mut d = html_doc(src);
    d.caret = d.source.find("Hello").unwrap();
    d.toggle_heading(2);
    assert_eq!(d.source, "<h1>Title</h1>\n<h2>Hello world</h2>\n");
    assert!(d.dirty);
    assert_eq!(d.status, None, "a supported gesture reports nothing");
    d.toggle_heading(2);
    assert_eq!(d.source, src, "the same level again is back to a paragraph");
}

#[test]
fn html_spells_the_inline_marks_and_the_rule() {
    // The other half, and why one per-document flag stopped being enough:
    // ⌘B in an HTML document writes `<strong>` — the tag the serializer
    // already emits and the parser reads straight back as the same mark —
    // and the rule button writes an `<hr>`. Refusing these on the old
    // "HTML is parse-only" reading would now be leaf's own limitation.
    let mut d = html_doc("<p>Hello world</p>\n");
    let at = d.source.find("world").unwrap();
    d.caret = at;
    d.anchor = Some(at + 5);
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "<p>Hello <strong>world</strong></p>\n");
    assert!(d.dirty);
    assert_eq!(d.status, None, "a supported gesture reports nothing");

    // And off again — the toggle reverses, which is the property that makes
    // authoring in HTML worth offering rather than a one-way trip.
    d.toggle(InlineKind::Strong);
    assert_eq!(d.source, "<p>Hello world</p>\n");

    let mut d = html_doc("<p>Hello world</p>\n");
    d.caret = d.source.find("world").unwrap();
    d.insert_thematic_break();
    assert!(d.source.contains("<hr>"), "got {:?}", d.source);
}

#[test]
fn a_mark_the_format_cannot_spell_arms_nothing() {
    // `toggle` with a collapsed caret doesn't reach twig at all — it arms a
    // sticky mark for the next text typed. Guarding only the twig call
    // leaves that path live, promising a mark the gesture will not write and
    // then swallowing the error inside `insert`.
    //
    // Markdown carries this, on the superscript now rather than on the
    // highlight: `^x^` is text there in any configuration, whereas twig
    // 3.3.1 authors `==x==` for an editor holding the `highlight` extension,
    // which every leaf document does.
    let mut d = doc_with("mark", "Hello world\n");
    d.view = View::Wysiwyg;
    d.build_visual(80);
    d.caret = d.source.find("world").unwrap();
    d.toggle(InlineKind::Superscript);
    assert!(d.pending_marks.is_empty(), "no mark should be armed");
    assert!(d.status.as_deref().unwrap_or("").contains("markdown"));
    d.insert("X");
    assert_eq!(d.source, "Hello Xworld\n");
}

#[test]
fn markdown_authors_a_highlight_and_a_strikethrough() {
    // twig 3.3.1: the two marks Markdown reads and, until it, refused to
    // write. `==x==` is authorable because leaf's own `parse_extensions`
    // turns `highlight` on — twig will only mint bytes this editor's reparse
    // reads back — and `~~x~~` because GFM strikethrough is parsed by
    // default, so the refusal there was never right for any leaf document.
    // twig 3.10 adds the underline, spelled `<u>x</u>` and paired back into
    // an `insert` under the `html_elements` leaf parses with.
    for (kind, marked) in [
        (InlineKind::Mark, "a ==word== b\n"),
        (InlineKind::Delete, "a ~~word~~ b\n"),
        (InlineKind::Insert, "a <u>word</u> b\n"),
    ] {
        let mut d = doc_with("author_mark", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(kind);
        assert_eq!(d.source, marked, "{kind:?}");
        assert_eq!(d.status, None, "{kind:?}: a supported gesture is silent");
        assert!(d.dirty, "{kind:?}");
        // The region stays selected, so the second press reverses it — the
        // property that separates authoring from a one-way trip.
        d.toggle(kind);
        assert_eq!(d.source, "a word b\n", "{kind:?}");
    }
}

#[test]
fn an_authored_highlight_reads_back_as_a_mark() {
    // The round trip the extension gate exists to protect: what the toggle
    // writes, the reparse must read back as a `mark` rather than as two
    // literal `=` pairs. A `Role::Mark` glyph is that answer, taken from the
    // rebuilt map rather than from the source text.
    let mut d = doc_with("mark_roundtrip", "a word b\n");
    d.view = View::Wysiwyg;
    d.build_visual(80);
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Mark);
    assert_eq!(d.source, "a ==word== b\n");
    d.build_visual(80);
    let w = d
        .vmap
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .find(|g| g.ch == 'w')
        .expect("the highlighted word");
    assert_eq!(w.style.role, crate::Role::Mark(None));
}

#[test]
fn an_authored_underline_reads_back_as_an_insert() {
    // The same round trip for `<u>…</u>`: under `html_elements` the tag pair
    // is one `insert`, so the word is underlined and the tags are markup the
    // caret-off line hides — not two `raw_inline` runs drawn as literal
    // HTML around plain text.
    let mut d = doc_with("underline_roundtrip", "a word b\nnext\n");
    d.view = View::Wysiwyg;
    d.build_visual(80);
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Insert);
    assert_eq!(d.source, "a <u>word</u> b\nnext\n");
    d.anchor = None;
    d.caret = d.source.find("next").unwrap();
    d.build_visual(80);
    let glyphs: Vec<_> = d.vmap.rows.iter().flat_map(|r| r.glyphs.iter()).collect();
    let w = glyphs
        .iter()
        .find(|g| g.ch == 'w')
        .expect("the underlined word");
    assert!(w.style.underline);
    assert!(!glyphs.iter().any(|g| g.ch == '<'), "the tags are hidden");
}

#[test]
fn a_highlight_takes_a_colour_changes_it_and_gives_it_back() {
    // The three states of one gesture, in the order a palette is pressed:
    // an uncoloured highlight takes the prefix, a coloured one has it
    // replaced, and `None` takes it away with the space that was part of the
    // spelling.
    let mut d = doc_with("mark_colour", "a ==word== b\n");
    d.caret = d.source.find("word").unwrap();
    d.set_mark_color(Some(MarkColor::Red));
    assert_eq!(d.source, "a ==🔴 word== b\n");
    assert_eq!(d.status, None);
    assert!(d.dirty);

    d.set_mark_color(Some(MarkColor::Blue));
    assert_eq!(d.source, "a ==🔵 word== b\n");

    d.set_mark_color(None);
    assert_eq!(d.source, "a ==word== b\n");
}

#[test]
fn the_caret_keeps_its_place_in_the_text_across_a_colour() {
    // The prefix is written *before* the word, so an offset in the word has
    // to ride its width — a caret that stayed put would be a caret that
    // walked backwards through the text it was standing in.
    let mut d = doc_with("mark_colour_caret", "a ==word== b\n");
    let word = d.source.find("word").unwrap();
    d.caret = word + 2; // between `wo` and `rd`
    d.set_mark_color(Some(MarkColor::Red));
    assert_eq!(&d.source[d.caret..d.caret + 2], "rd", "still before `rd`");

    // And back the other way when the prefix goes.
    d.set_mark_color(None);
    assert_eq!(&d.source[d.caret..d.caret + 2], "rd");
}

#[test]
fn the_colour_at_the_caret_is_what_the_palette_lights() {
    let mut d = doc_with("mark_colour_read", "a ==🔴 red== and ==plain== b\n");
    d.caret = d.source.find("red").unwrap();
    assert!(d.caret_in_mark());
    assert_eq!(d.mark_color_at_caret(), Some(MarkColor::Red));

    d.caret = d.source.find("plain").unwrap();
    assert!(d.caret_in_mark(), "a highlight with no colour is still one");
    assert_eq!(d.mark_color_at_caret(), None);

    d.caret = d.source.find(" and ").unwrap() + 2;
    assert!(!d.caret_in_mark());
    assert_eq!(d.mark_color_at_caret(), None);
}

#[test]
fn a_colour_without_a_highlight_says_so_and_writes_nothing() {
    // The gesture colours a highlight that exists; it does not make one.
    // Two presses is the price of a coloured highlight from bare text, and
    // the reason is undo — one press that spliced twice would take two
    // presses to take back.
    let mut d = doc_with("mark_colour_none", "a word b\n");
    d.caret = d.source.find("word").unwrap();
    d.set_mark_color(Some(MarkColor::Red));
    assert_eq!(d.source, "a word b\n");
    assert!(d.status.is_some(), "it should say why");
    assert!(!d.dirty);

    // Clearing where there is nothing to clear is the same refusal, not a
    // quiet success — the caret is in no highlight either way.
    d.status = None;
    d.set_mark_color(None);
    assert_eq!(d.source, "a word b\n");
    assert!(d.status.is_some());
}

#[test]
fn clearing_an_uncoloured_highlight_is_a_quiet_no_op() {
    // twig answers this one *successfully* with a `Change` describing some
    // earlier edit, so a caller that trusted the change would jump the caret
    // to wherever that was. Core answers it before asking.
    let mut d = doc_with("mark_colour_noop", "a ==word== b\n");
    d.toggle(InlineKind::Strong); // an earlier edit for a stale change to name
    d.caret = d.source.find("word").unwrap();
    let (source, caret) = (d.source.clone(), d.caret);
    d.set_mark_color(None);
    assert_eq!(d.source, source);
    assert_eq!(
        d.caret, caret,
        "the caret must not ride a change that isn't one"
    );
    assert_eq!(d.status, None, "and it is not an error either");
}

#[test]
fn djot_spells_the_highlight_and_not_its_colour() {
    // The reason the palette is its own capability rather than the Highlight
    // button's: `{=word=}` is a highlight djot writes happily, and there is
    // no djot spelling for a colour on it.
    assert!(Capabilities::of(Format::Djot).mark);
    assert!(!Capabilities::of(Format::Djot).mark_color);
    assert!(Capabilities::of(Format::Markdown).mark_color);

    let mut d = Doc::from_source("a {=word=} b\n".into(), Format::Djot).unwrap();
    d.caret = d.source.find("word").unwrap();
    assert!(
        d.caret_in_mark(),
        "the caret is in a highlight all the same"
    );
    d.set_mark_color(Some(MarkColor::Red));
    assert_eq!(d.source, "a {=word=} b\n");
    assert!(
        d.status.as_deref().unwrap_or("").contains("djot"),
        "and the refusal names the document's format: {:?}",
        d.status
    );
}

#[test]
fn a_coloured_highlight_is_one_undo_step_and_reads_back_as_its_colour() {
    // The round trip that matters for a palette: the bytes twig writes are
    // bytes its own reparse reads back as a colour, so the swatch that was
    // pressed is the swatch that lights afterwards.
    let mut d = doc_with("mark_colour_undo", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Mark);
    d.caret = d.source.find("word").unwrap();
    d.set_mark_color(Some(MarkColor::Green));
    assert_eq!(d.source, "a ==🟢 word== b\n");
    assert_eq!(d.mark_color_at_caret(), Some(MarkColor::Green));

    // One splice, one step: the colour comes off and the highlight stays.
    d.undo();
    assert_eq!(d.source, "a ==word== b\n");
    d.undo();
    assert_eq!(d.source, "a word b\n");
}

#[test]
fn every_colour_leaf_names_is_one_twig_writes() {
    // The two enums are one vocabulary, and this is what says so: each of
    // leaf's colours writes an emoji twig's reparse reads back as *that*
    // colour, so `twig_mark_color`'s table cannot quietly pair red with
    // orange.
    for color in MarkColor::ALL {
        let mut d = doc_with("mark_colour_all", "a ==word== b\n");
        d.caret = d.source.find("word").unwrap();
        d.set_mark_color(Some(color));
        assert_eq!(d.status, None, "{color:?}");
        assert_eq!(d.mark_color_at_caret(), Some(color), "{color:?}");
    }
}

#[test]
fn a_fresh_highlight_takes_a_colour_without_moving_the_caret_first() {
    // The two presses a coloured highlight is made of, in the state the
    // first one leaves: `toggle` selects the whole `==word==` and puts the
    // caret one past the closing `==`, which is *not* in the mark. Asking at
    // the caret alone would refuse to colour the highlight just written —
    // the selection's start is what answers.
    let mut d = doc_with("mark_colour_fresh", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.toggle(InlineKind::Mark);
    assert_eq!(d.source, "a ==word== b\n");
    assert_eq!(d.caret, 10, "the caret twig leaves, past the closing `==`");

    assert!(d.caret_in_mark(), "the selected highlight is the one meant");
    d.set_mark_color(Some(MarkColor::Yellow));
    assert_eq!(d.source, "a ==🟡 word== b\n");
    assert_eq!(d.status, None);
}

#[test]
fn one_press_highlights_a_selection_and_colours_it() {
    // What a toolbar swatch means over a plain selection, and the undo it
    // has to have: one press, one step. Two steps would leave an uncoloured
    // highlight behind on the way back, which is a state the author never
    // asked for and never saw.
    let mut d = doc_with("highlight_one", "a word b\n");
    d.anchor = Some(2);
    d.caret = 6;
    d.highlight(Some(MarkColor::Purple));
    assert_eq!(d.source, "a ==\u{1F7E3} word== b\n");
    assert_eq!(d.status, None);

    d.undo();
    assert_eq!(d.source, "a word b\n", "one press, one undo");
}

#[test]
fn one_press_on_an_existing_highlight_only_recolours_it() {
    // The other half: inside a highlight there is nothing to make, so the
    // compound is the plain gesture and the text is untouched.
    let mut d = doc_with("highlight_recolour", "a ==\u{1F534} word== b\n");
    d.caret = d.source.find("word").unwrap();
    d.highlight(Some(MarkColor::Blue));
    assert_eq!(d.source, "a ==\u{1F535} word== b\n");
    d.undo();
    assert_eq!(d.source, "a ==\u{1F534} word== b\n", "the highlight stays");
}

#[test]
fn one_press_with_no_colour_over_a_selection_just_highlights_it() {
    // `None` means "no colour", and over bare text that is the Highlight
    // button's own job. The fold must not happen here — there is no second
    // splice, and folding would take the *previous* edit into this one.
    let mut d = doc_with("highlight_none", "a word b and more\n");
    d.caret = d.source.find("more").unwrap() + 4; // after "more"
    d.insert("!"); // an earlier edit for a wrong fold to swallow
    d.anchor = Some(2);
    d.caret = 6;
    d.highlight(None);
    assert_eq!(d.source, "a ==word== b and more!\n");

    d.undo();
    assert_eq!(
        d.source, "a word b and more!\n",
        "only the highlight came off"
    );
    d.undo();
    assert_eq!(
        d.source, "a word b and more\n",
        "and the edit before it survived"
    );
}

#[test]
fn one_press_at_a_bare_caret_in_no_highlight_writes_nothing() {
    // `toggle` at a collapsed caret arms a mark for text not yet typed, and
    // a colour cannot be armed with it — so the compound declines rather
    // than leaving half a promise.
    let mut d = doc_with("highlight_bare", "a word b\n");
    d.caret = 4;
    d.highlight(Some(MarkColor::Red));
    assert_eq!(d.source, "a word b\n");
    assert!(d.pending_marks.is_empty(), "and nothing armed");
    assert!(d.status.is_some());
}

#[test]
fn a_read_only_document_takes_no_colour() {
    let mut d = doc_with("mark_colour_ro", "a ==word== b\n");
    d.caret = d.source.find("word").unwrap();
    d.set_read_only(true);
    d.set_mark_color(Some(MarkColor::Red));
    assert_eq!(d.source, "a ==word== b\n");
}

#[test]
fn a_sticky_highlight_wraps_the_next_typed_text_in_markdown() {
    // The other door into `toggle`: no selection, so nothing reaches twig
    // until `insert` realises the armed mark. It is armed now — the guard
    // above asks `Doc::supports`, which asks with the extensions — and what
    // it writes is the same `==…==`.
    let mut d = doc_with("sticky_mark", "xy\n");
    d.caret = 1;
    d.toggle(InlineKind::Mark);
    assert!(d.pending_marks.contains(InlineKind::Mark));
    d.insert("Z");
    assert_eq!(d.source, "x==Z==y\n");
}

#[test]
fn html_documents_still_take_typed_text() {
    // The guard covers *markup* gestures and must not touch plain editing:
    // twig's splicer is language-neutral, and typing into an HTML document
    // is the thing that does work today.
    let mut d = html_doc("<p>Hello world</p>\n");
    d.caret = d.source.find("world").unwrap();
    d.insert("big ");
    assert_eq!(d.source, "<p>Hello big world</p>\n");
    assert!(d.dirty);
    d.backspace();
    assert_eq!(d.source, "<p>Hello bigworld</p>\n");
    d.undo();
    d.undo();
    assert_eq!(d.source, "<p>Hello world</p>\n");
}

#[test]
fn authorable_is_the_coarse_question_and_capabilities_the_useful_one() {
    // `authorable` only separates "there is a door in" from "there is not",
    // and HTML is on the near side of that line — which is exactly why a
    // toolbar must not be built from it.
    let html = Doc::from_source("<p>x</p>\n".into(), Format::Html).unwrap();
    assert!(html.authorable());
    assert!(
        !Doc::from_source("<r>x</r>".into(), Format::Xml)
            .unwrap()
            .authorable()
    );

    let caps = html.capabilities();
    assert!(caps.bold && caps.italic && caps.code && caps.mark);
    assert!(caps.thematic_break && caps.cell_line_break);
    // A heading is a tag pair twig rebuilds (3.4), and since 3.5 so are a
    // quote, a list, a code block's language, a link and an image — each
    // printed as a fresh node where HTML has no marker to rewrite. A task
    // box is a form control and a footnote has no spelling, so those two
    // are what keeps the record ragged.
    assert!(caps.heading && caps.blockquote && caps.bullet_list);
    assert!(caps.link && caps.image && caps.code_language);
    assert!(!caps.task && !caps.footnote);
    // The one flag that isn't twig's answer: an HTML `<table>` is a grid
    // twig's table editor would happily re-emit as `| a | b |`.
    assert!(!caps.table);

    // The two lightweight formats spell everything leaf offers — and still
    // differ from each other, which is the other half of why one boolean
    // can't serve.
    for fmt in [Format::Markdown, Format::Djot] {
        let caps = Capabilities::of(fmt);
        assert!(
            caps.heading && caps.blockquote && caps.ordered_list,
            "{fmt:?}"
        );
        assert!(
            caps.task && caps.link && caps.image && caps.table,
            "{fmt:?}"
        );
    }
    // Both spell the highlight, the strikethrough and the underline: djot
    // natively, and Markdown because `Capabilities` asks with
    // `parse_extensions` rather than with twig's defaults — `==x==` is text
    // under those, and a mark under the `highlight` leaf always parses
    // with; `<u>x</u>` likewise pairs into an `insert` only under
    // `html_elements`.
    for fmt in [Format::Markdown, Format::Djot] {
        let caps = Capabilities::of(fmt);
        assert!(caps.mark && caps.strike && caps.underline, "{fmt:?}");
    }
    // What still separates them, now that the highlight doesn't: djot has
    // no in-cell break, and Markdown spells neither of the scripts.
    assert!(Capabilities::of(Format::Djot).superscript);
    assert!(!Capabilities::of(Format::Markdown).superscript);
    assert!(Capabilities::of(Format::Markdown).cell_line_break);
    assert!(!Capabilities::of(Format::Djot).cell_line_break);

    // A parse-only format answers no to every one of them, so the coarse
    // predicate and the record agree there.
    let caps = Capabilities::of(Format::Xml);
    assert!(!caps.bold && !caps.heading && !caps.table && !caps.thematic_break);
}

#[test]
fn a_refused_gesture_says_so_where_twig_would_have_said_it() {
    // The guard exists to name the *document's* format rather than twig's
    // internals, so the message has to survive being one leaf writes itself.
    // Checked against a gesture twig also refuses, since that is the pair
    // most at risk of drifting apart — the task box, once the code
    // language stopped being one (twig 3.5).
    let mut d = html_doc("<p>Hello</p>\n");
    d.caret = d.source.find("Hello").unwrap();
    d.toggle_task_item();
    assert_eq!(d.status.as_deref(), Some("task: not supported in html"));
    assert!(!d.dirty);
}

#[test]
fn table_set_alignment_respells_the_delimiter() {
    let mut d = doc_with("tbl_align", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
    d.caret = d.source.find('b').unwrap();
    d.table_set_alignment(Alignment::Right);
    assert_eq!(d.source, "| a | b |\n| --- | ---: |\n| 1 | 2 |\n");
}

#[test]
fn each_empty_table_cell_has_its_own_editable_home() {
    // Regression: an empty cell has no twig content_span, so both cells of a
    // `|  |  |` row collapsed onto the row's start (before the first `│`).
    // Typing there inserted *before* the table (`hello|  |  |`); nav couldn't
    // tell the cells apart. Each empty cell must now have a distinct home
    // inside it.
    let mut d = wysiwyg_doc("tbl_empty", "| a | b |\n| --- | --- |\n|  |  |\n");
    let (c0, c1) = {
        let cells = &d.vmap.tables[0].grid[1].cells;
        (cells[0].start, cells[1].start)
    };
    assert!(
        c0 < c1,
        "the two empty cells have distinct homes: {c0} < {c1}"
    );
    d.caret = c0;
    d.insert("x");
    assert_eq!(
        d.source, "| a | b |\n| --- | --- |\n| x |  |\n",
        "typed inside the cell"
    );
}

#[test]
fn arrows_step_into_each_empty_table_cell() {
    let mut d = wysiwyg_doc("tbl_empty_nav", "| a | b |\n| --- | --- |\n|  |  |\n");
    let (c0, c1) = {
        let cells = &d.vmap.tables[0].grid[1].cells;
        (cells[0].start, cells[1].start)
    };
    d.caret = d.source.find('b').unwrap(); // in the header's second cell
    let mut seen = std::collections::HashSet::new();
    for _ in 0..6 {
        d.move_right(false);
        seen.insert(d.caret);
    }
    assert!(
        seen.contains(&c0),
        "right arrow reaches the first empty cell"
    );
    assert!(
        seen.contains(&c1),
        "right arrow reaches the second empty cell"
    );
}

#[test]
fn table_op_off_a_table_is_a_no_op_with_a_status() {
    let mut d = doc_with("tbl_none", "just text\n");
    d.caret = 3;
    d.table_insert_row(true);
    assert_eq!(d.source, "just text\n", "nothing changed");
    assert!(d.status.is_some(), "a status explains why");
    assert!(!d.caret_in_table());
}

#[test]
fn enter_in_an_ordered_list_renumbers_the_following_items() {
    // Inserting an item mid-list left the source markers stale (`1. 2. 2. 3.`);
    // the renumber pass keeps them sequential, matching what the view draws.
    let mut d = wysiwyg_doc("enter_renumber", "1. a\n2. b\n3. c\n");
    d.caret = d.source.find('a').unwrap() + 1; // end of item a
    d.newline();
    d.insert("x");
    assert_eq!(d.source, "1. a\n2. x\n3. b\n4. c\n");
}

#[test]
fn outdent_with_nothing_to_give_back_records_no_undo_step() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "outdent_noop", "hello\n");
        d.caret = 2;
        d.outdent();
        assert_eq!(d.source, "hello\n");
        assert!(!d.dirty, "a no-op is not a modification");
        d.undo();
        assert_eq!(
            d.status.as_deref(),
            Some("nothing to undo"),
            "spends no undo step"
        );
        assert_eq!(d.source, "hello\n");
    }
}

#[test]
fn indent_shifts_every_selected_line_and_keeps_them_selected() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "indent_sel", "one\n\ntwo\n");
        d.anchor = Some(0);
        d.caret = 7; // through "two"
        d.indent();
        assert_eq!(
            d.source, "  one\n\n  two\n",
            "the blank line keeps no trailing pad"
        );
        // Selected, so a second Tab lands on the same lines rather than on
        // whatever the shifted offsets now cover.
        assert_eq!(d.selection(), Some((0, 12)));
        d.indent();
        assert_eq!(d.source, "    one\n\n    two\n");
    }
}

#[test]
fn outdent_takes_what_each_line_has_and_leaves_the_rest_alone() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "outdent_sel", "  two\n one\nnone\n");
        d.anchor = Some(0);
        d.caret = 15;
        d.outdent();
        assert_eq!(d.source, "two\none\nnone\n");
    }
}

#[test]
fn a_tab_undoes_as_one_step_however_many_lines_it_moved() {
    for view in [View::Source, View::Wysiwyg] {
        let mut d = doc_in(view, "indent_undo", "one\n\ntwo\n");
        d.anchor = Some(0);
        d.caret = 7;
        d.indent();
        assert_eq!(d.source, "  one\n\n  two\n");
        d.undo();
        assert_eq!(d.source, "one\n\ntwo\n", "one step, not one per line");
        assert_eq!(
            d.selection(),
            Some((0, 7)),
            "with the selection it was aimed at"
        );
        d.redo();
        assert_eq!(d.source, "  one\n\n  two\n");
        assert_eq!(
            d.selection(),
            Some((0, 12)),
            "redo replays the caret the indent placed, not the one splice left"
        );
    }
}

#[test]
fn vertical_motion_keeps_the_column() {
    let mut d = doc_with("move", "abcd\nef\n");
    d.caret = 3; // "abc|d" on row 0, col 3
    d.move_down(false); // row 1 "ef" only has cols 0..2 -> clamps to end
    assert_eq!(d.caret, 7); // just after "ef"
}
