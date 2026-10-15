use super::*;

// ── links ────────────────────────────────────────────────────────────────

#[test]
fn insert_link_wraps_the_selection_and_leaves_its_text_selected() {
    let mut d = doc_with("link_sel", "word here\n");
    d.anchor = Some(0);
    d.caret = 4;
    d.insert_link("http://x.dev");
    assert_eq!(d.source, "[word](http://x.dev) here\n");
    // The text, not the destination — so a second press re-points the link
    // the first one made rather than nesting one inside it.
    assert_eq!(d.selected_text(), Some("word"));
    d.insert_link("http://y.dev");
    assert_eq!(d.source, "[word](http://y.dev) here\n");
    assert_eq!(d.selected_text(), Some("word"));
}

#[test]
fn insert_image_at_the_caret_spells_the_markup_and_lands_past_it() {
    let mut d = doc_with("img_caret", "before after\n");
    d.caret = 7; // between "before " and "after"
    d.insert_image("cat.png", "a cat");
    assert_eq!(d.source, "before ![a cat](cat.png)after\n");
    // The caret sits just past the inserted image, nothing selected.
    assert_eq!(d.selection(), None);
    assert_eq!(d.caret, 7 + "![a cat](cat.png)".len());
}

/// The bug a real vault hit: a filename with spaces in it. Markdown ends a
/// destination at the first space, so the `format!` this used to be wrote
/// something that was not an image at all — and the reader saw the markup as
/// text. twig owns the spelling now, and moves it into the angle form.
#[test]
fn insert_image_spells_a_destination_with_spaces_so_it_stays_an_image() {
    let mut d = doc_with("img_space", "x\n");
    d.caret = 0;
    d.insert_image("Jesus Commands the Apostles to Rest.jpg", "");
    assert_eq!(
        d.source,
        "![](<Jesus Commands the Apostles to Rest.jpg>)x\n"
    );
    // And it reads back as an image pointing at the unescaped path — the angle
    // brackets are spelling, not part of the destination.
    d.caret = 2;
    assert_eq!(
        d.image_destination_at_caret(),
        Some("Jesus Commands the Apostles to Rest.jpg".to_string())
    );
}

/// A `)` in a caption or a filename must not close the image early.
#[test]
fn insert_image_escapes_a_paren_in_either_half() {
    let mut d = doc_with("img_paren", "x\n");
    d.caret = 0;
    d.insert_image("a)b.png", "");
    assert_eq!(d.source, "![](a\\)b.png)x\n");
    d.caret = 2;
    assert_eq!(d.image_destination_at_caret(), Some("a)b.png".to_string()));
}

#[test]
fn insert_image_uses_the_selection_as_alt_text() {
    let mut d = doc_with("img_sel", "caption here\n");
    d.anchor = Some(0);
    d.caret = 7; // "caption"
    d.insert_image("p.png", "ignored fallback");
    assert_eq!(d.source, "![caption](p.png) here\n");
}

#[test]
fn insert_image_with_no_alt_leaves_empty_brackets() {
    let mut d = doc_with("img_noalt", "\n");
    d.caret = 0;
    d.insert_image("logo.svg", "");
    assert_eq!(d.source, "![](logo.svg)\n");
}

#[test]
fn insert_link_repoints_the_link_at_a_bare_caret() {
    let mut d = doc_with("link_repoint", "[word](http://x.dev)\n");
    d.caret = 3; // in the link's text, nothing selected
    d.insert_link("http://y.dev");
    assert_eq!(d.source, "[word](http://y.dev)\n");
    assert_eq!(d.selected_text(), Some("word"));
}

#[test]
fn insert_link_on_an_empty_range_autolinks_a_url() {
    // A link with no text of its own is an autolink, and twig spells it —
    // `<…>` is the canonical form and needs no text typed into it, so the
    // caret lands after it rather than selecting a finished link.
    let mut d = doc_with("link_empty", "\n");
    d.caret = 0;
    d.insert_link("http://x.dev");
    assert_eq!(d.source, "<http://x.dev>\n");
    assert_eq!(d.selection(), None);
    assert_eq!(d.caret, 14);
}

#[test]
fn insert_link_on_an_empty_range_falls_back_for_a_non_url() {
    // `<./notes.md>` is literal text in both formats and `<foo>` is raw HTML
    // in Markdown, so a destination that can't autolink doubles as the text
    // instead — which is then selected, ready to be typed over.
    let mut d = doc_with("link_rel", "\n");
    d.caret = 0;
    d.insert_link("./notes.md");
    assert_eq!(d.source, "[./notes.md](./notes.md)\n");
    assert_eq!(d.selection(), Some((1, 11)));
    d.insert("Notes");
    assert_eq!(d.source, "[Notes](./notes.md)\n");
}

#[test]
fn insert_link_repoints_the_autolink_the_caret_stands_in() {
    // The autolink's text is its URL, so re-pointing replaces the whole
    // node — the caret must not splice a second link inside the first.
    let mut d = doc_with("link_repoint_auto", "see <https://x.dev> ok\n");
    d.caret = 10;
    d.insert_link("https://y.dev");
    assert_eq!(d.source, "see <https://y.dev> ok\n");
}

#[test]
fn code_language_reads_and_edits_through_the_fence() {
    let mut d = doc_with("code_lang", "```rust\nlet x = 1;\n```\n");
    d.caret = 10; // inside the code body
    assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
    assert!(d.caret_in_fenced_code());

    d.set_code_language("python");
    assert!(
        d.source.starts_with("```python\n"),
        "source: {:?}",
        d.source
    );
    assert_eq!(d.code_language_at_caret().as_deref(), Some("python"));

    // Clearing it leaves a bare fence and no label.
    d.set_code_language("");
    assert!(d.source.starts_with("```\n"), "source: {:?}", d.source);
    assert_eq!(d.code_language_at_caret(), None);

    // A caret outside any code block edits nothing.
    let mut p = doc_with("code_lang_none", "just prose\n");
    assert!(!p.caret_in_fenced_code());
    p.set_code_language("rust");
    assert_eq!(p.source, "just prose\n");
}

#[test]
fn code_language_reads_and_edits_through_a_quoted_fence() {
    let mut d = doc_with("code_lang_quote", "> ```rust\n> let x = 1;\n> ```\n");
    d.caret = d.source.find("let").unwrap();
    assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
    assert!(d.caret_in_fenced_code());
    d.set_code_language("python");
    assert!(
        d.source.starts_with("> ```python\n"),
        "source: {:?}",
        d.source
    );
    assert_eq!(d.code_language_at_caret().as_deref(), Some("python"));
}

#[test]
fn a_language_the_fence_cannot_carry_is_refused_not_written() {
    // Markdown's info string ends at whitespace, so `two words` would write
    // a fence that reads back with a different language than the one asked
    // for. twig refuses it; leaf reports that and leaves the source alone.
    // The old splice trimmed the ends and wrote whatever was left.
    let mut d = doc_with("code_lang_bad", "```rust\nx\n```\n");
    d.caret = 10;
    d.set_code_language("two words");
    assert_eq!(d.source, "```rust\nx\n```\n", "source should be untouched");
    assert!(d.status.is_some(), "the refusal should be reported");
    assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
}

#[test]
fn link_destination_at_caret_reads_both_spellings() {
    let mut d = doc_with("link_dest", "see [t](https://x.dev) ok\n");
    d.caret = 5;
    assert_eq!(
        d.link_destination_at_caret().as_deref(),
        Some("https://x.dev")
    );
    d.caret = 0;
    assert_eq!(d.link_destination_at_caret(), None);

    // An autolink has no `destination`; its text is the URL.
    let mut a = doc_with("link_dest_auto", "see <https://x.dev> ok\n");
    a.caret = 10;
    assert_eq!(
        a.link_destination_at_caret().as_deref(),
        Some("https://x.dev")
    );
    a.caret = 21;
    assert_eq!(a.link_destination_at_caret(), None);
}

#[test]
fn heading_at_caret_is_the_nearest_heading_above() {
    let src = "intro\n\n# Title\n\n## The *Second* Part\n\nbody here\n";
    let mut d = doc_with("heading_at", src);
    // Under no heading at all.
    d.caret = 2;
    assert_eq!(d.heading_at_caret(), None);
    // In a paragraph under a level-2 heading: that heading, its markup
    // stripped for the slug, and its own span.
    d.caret = src.find("body").unwrap() + 2;
    let h = d.heading_at_caret().expect("under `## The Second Part`");
    assert_eq!(h.text, "The Second Part");
    assert_eq!(h.level, 2);
    assert_eq!(&src[h.span.clone()].trim_end(), &"## The *Second* Part");
    // Standing in a heading answers with that heading.
    let h = d.heading_at(src.find("Title").unwrap()).unwrap();
    assert_eq!((h.text.as_str(), h.level), ("Title", 1));
    // And the text is what `locate` lands a slug of.
    let landing = d.locate("the-second-part").unwrap();
    assert_eq!(landing.start, d.heading_at_caret().unwrap().span.start);
}

#[test]
fn locate_finds_the_block_a_declared_id_names() {
    // The Book of Mormon shape: one document per chapter, one `{#v…}` per
    // verse. The locator has to land on the *verse*, which is the whole
    // reason a link carries one.
    let src = "{#v1}\nI, Nephi, having been born of goodly parents.\n\n\
                   {#v2}\nYea, I make a record in the language of my father.\n";
    let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
    let v2 = d.locate("v2").expect("the document declares `{#v2}`");
    assert_eq!(
        d.source[v2.start..v2.end].trim_end(),
        "Yea, I make a record in the language of my father."
    );
    // The attribute line is not part of it: `start` is a place to put a
    // caret, and `{#v2}` is markup the caret has no business landing in.
    assert!(d.source[..v2.start].ends_with("{#v2}\n"));
    assert_eq!(d.locate("v99"), None);
}

#[test]
fn locate_reads_a_heading_by_its_words_when_the_format_mints_no_ids() {
    // Markdown has no ids at all — twig mints none, and `{#custom}` in a
    // Markdown heading is literal text. So `#the-second-part` can only be
    // the heading's own words, which is the rule every Markdown renderer
    // already follows and therefore the one a link was authored against.
    let src = "# Title\n\nintro\n\n## The Second Part\n\nbody\n\n## Third\n\nmore\n";
    let mut d = doc_with("locate_md", src);
    let hit = d.locate("the-second-part").expect("the heading's slug");
    assert!(d.source[hit.start..].starts_with("## The Second Part"));
    // Bounded by the next heading that isn't under it, so a peek shows the
    // section rather than only its title.
    assert_eq!(
        &d.source[hit.start..hit.end],
        "## The Second Part\n\nbody\n\n"
    );

    // A subsection does not end its parent: `# Title` runs to `## Third`'s
    // sibling only because there is no other `#`, so it covers the lot.
    let title = d.locate("title").expect("the top heading");
    assert_eq!(title.end, d.source.len());
}

#[test]
fn locate_reads_a_djot_auto_id_however_the_link_spelled_it() {
    // djot mints `Some-Heading-Here`; a link to it is written
    // `#some-heading-here` by nearly everything that writes links. Both
    // spellings are one question.
    let src = "## Some Heading Here\n\nbody\n";
    let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
    let exact = d.locate("Some-Heading-Here").expect("djot's own spelling");
    let slugged = d.locate("some-heading-here").expect("the link's spelling");
    assert_eq!(exact, slugged);
    // The section, not the heading line — there is more to show than a title.
    assert_eq!(&d.source[exact.start..exact.end], src);
}

#[test]
fn locate_ignores_an_empty_locator_and_one_that_slugs_to_nothing() {
    let mut d = doc_with("locate_empty", "# Title\n\nbody\n");
    assert_eq!(d.locate(""), None);
    assert_eq!(d.locate("   "), None);
    // All punctuation: it names nothing, and must not be read as "match the
    // first heading whose slug is also empty".
    assert_eq!(d.locate("!!!"), None);
}

#[test]
fn following_a_fragment_puts_the_caret_on_the_heading_not_before_it() {
    // `landing.start` is the heading's first byte, the `#` of a hidden
    // `## `. The paragraph's end is two bytes back and the heading's `E`
    // three forward, so snapping to the *nearest* stop put the reader at
    // the end of the paragraph above the place they were sent to.
    let src = "see [below](#end) now\n\n## End\n\nlast\n";
    let mut d = doc_in(View::Wysiwyg, "locate_lands", src);
    let landing = d.locate("end").expect("the heading's slug");
    d.place_caret(landing.start, false);
    assert_eq!(d.caret, src.find("End").unwrap());

    // A list item's marker opens its line the same way.
    let src = "intro\n\n- item\n";
    let mut d = doc_in(View::Wysiwyg, "locate_lands_item", src);
    d.place_caret(src.find("- item").unwrap(), false);
    assert_eq!(d.caret, src.find("item").unwrap());

    // The blank line between blocks opens nothing, so a click there still
    // goes to the nearer stop.
    let src = "foo\n\nbar\n";
    let mut d = doc_in(View::Wysiwyg, "locate_lands_gap", src);
    d.place_caret(4, false);
    assert_eq!(d.caret, 3);
}

#[test]
fn locate_gives_a_duplicated_id_to_the_first_block_that_claims_it() {
    // The document's mistake, and the answer every other anchor
    // implementation gives — the alternative is for a link to mean whichever
    // of the two a walk happened to reach first.
    let src = "{#dup}\nfirst.\n\n{#dup}\nsecond.\n";
    let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
    let hit = d.locate("dup").expect("the first `{#dup}`");
    assert_eq!(d.source[hit.start..hit.end].trim_end(), "first.");
}

#[test]
fn insert_footnote_writes_both_halves_and_lands_the_caret_in_the_note() {
    // The button's whole job: a reference where the caret was, a definition
    // to give it meaning, and the caret waiting in the empty note so the
    // next keystroke is the note's first word.
    let mut d = doc_with("fn_insert", "A claim and more.\n");
    d.caret = 7; // just past "A claim"
    d.insert_footnote();
    assert!(
        d.source.starts_with("A claim[^1] and more."),
        "{:?}",
        d.source
    );
    assert!(
        d.source.contains("[^1]:"),
        "the definition too: {:?}",
        d.source
    );
    assert_eq!(d.status, None);

    let reference = d.source.find("[^1]").unwrap();
    let note = d
        .footnote_at(reference + 2)
        .expect("the reference just written");
    assert_eq!(note.label, "1");
    assert_eq!(note.text.as_deref(), Some(""), "the note starts empty");
    assert_eq!(Some(d.caret), note.offset, "the caret waits in the note");
    // …and typing there is typing into the note, not near it.
    d.insert("the note");
    assert_eq!(
        d.footnote_at(reference + 2).and_then(|f| f.text),
        Some("the note".to_string())
    );
}

#[test]
fn insert_footnote_numbers_past_the_notes_already_written() {
    // A second press must not hand back a label somebody else is using: twig
    // reuses a defined label rather than appending a rival definition, so a
    // repeat of `1` would quietly point the new reference at the old note.
    let mut d = doc_with("fn_insert_number", "One[^1] two.\n\n[^1]: first\n");
    d.caret = 7; // past `[^1]`, before " two."
    d.insert_footnote();
    assert!(d.source.starts_with("One[^1][^2] two."), "{:?}", d.source);
    assert_eq!(d.source.matches("[^2]:").count(), 1);
}

#[test]
fn insert_footnote_counts_a_dangling_reference_and_ignores_a_named_one() {
    // `[^2]` with no definition is still a 2 that means something to whoever
    // wrote it — stepping over it would mint a note for their reference. A
    // word label takes no number, so it blocks none.
    let mut d = doc_with("fn_insert_dangling", "a[^2] b[^why] c\n\n[^why]: named\n");
    d.caret = d.source.find(" c").unwrap();
    d.insert_footnote();
    assert!(d.source.contains("[^1]:"), "1 is free: {:?}", d.source);
    assert!(
        d.source.starts_with("a[^2] b[^why][^1] c"),
        "{:?}",
        d.source
    );
}

#[test]
fn insert_footnote_marks_the_selection_rather_than_replacing_it() {
    // A reference annotates the words before it. Consuming the selection —
    // which is what an insert normally does — would delete the very claim
    // the author selected in order to footnote.
    let mut d = doc_with("fn_insert_sel", "A claim and more.\n");
    d.anchor = Some(2);
    d.caret = 7; // "claim" selected
    d.insert_footnote();
    assert!(
        d.source.starts_with("A claim[^1] and more."),
        "{:?}",
        d.source
    );
}

#[test]
fn a_note_just_written_still_knows_where_its_reference_is() {
    // The authoring loop in one test: press the button, type the note, ask to
    // go back. The caret ends at the note's last byte — which is the *end* of
    // the definition's span, the one offset the query used to exclude — so
    // this is where the round trip either works or doesn't.
    let mut d = doc_with("fn_insert_return", "A claim and more.\n");
    d.caret = 7;
    d.insert_footnote();
    d.insert("the note");
    assert_eq!(d.source, "A claim[^1] and more.\n\n[^1]: the note\n");
    let back = d
        .footnote_definition_at_caret()
        .expect("still in the note we just typed");
    assert_eq!(back.label, "1");
    // …and following it lands on the reference's label, where a reader's
    // return leg lands.
    assert_eq!(back.offset, Some(9));
    assert_eq!(&d.source[9..10], "1");
}

#[test]
fn insert_footnote_takes_one_undo_for_both_halves() {
    // twig writes the pair as a single edit; the point of that is here.
    let before = "A claim and more.\n";
    let mut d = doc_with("fn_insert_undo", before);
    d.caret = 7;
    d.insert_footnote();
    assert_ne!(d.source, before);
    d.undo();
    assert_eq!(d.source, before, "one undo takes back both halves");
}

#[test]
fn insert_footnote_refuses_a_format_that_cannot_spell_one() {
    // HTML is authorable — it spells the inline marks — and has no footnote.
    // The refusal says so rather than writing brackets that would render as
    // brackets.
    let src = "<p>A claim.</p>\n";
    let mut d = Doc::from_source(src.to_string(), Format::Html).unwrap();
    assert!(!Capabilities::of(Format::Html).footnote);
    d.caret = 5;
    d.insert_footnote();
    assert_eq!(d.source, src, "nothing written");
    assert!(d.status.is_some_and(|s| s.starts_with("footnote:")));
}

#[test]
fn insert_footnote_leaves_the_caret_on_a_real_stop_in_the_rich_view() {
    // The empty body is the one place this could go wrong: the definition
    // renders as a `[1] ` marker the caret cannot occupy, so a caret aimed a
    // byte early would draw up in the paragraph above the note it belongs to.
    let mut d = doc_in(View::Wysiwyg, "fn_insert_stop", "A claim and more.\n");
    d.place_caret(7, false);
    d.insert_footnote();
    d.build_visual(80); // the frame a frontend draws after the edit
    assert_eq!(
        d.vmap.snap_to_stop(d.caret),
        d.caret,
        "the caret sits on a stop"
    );
    let (row, _) = d.caret_pos();
    assert!(
        drawn_rows(&d)[row].contains("[1]"),
        "the caret is on the note's row, not above it: {:?}",
        drawn_rows(&d)
    );
}

#[test]
fn footnote_at_caret_resolves_a_reference_to_its_note() {
    // `[^1]` spans 7..11; its label byte is at 9. The definition follows a
    // blank line, as one has to.
    let mut d = doc_with("fn_at_caret", "A claim[^1] and more.\n\n[^1]: the note\n");
    d.caret = 9;
    let f = d
        .footnote_at_caret()
        .expect("the caret stands in a reference");
    assert_eq!(f.label, "1");
    assert_eq!(f.text.as_deref(), Some("the note"));
    // The offset points at the note's first word, not at the definition's
    // `[` — the marker is decoration with no caret stop on it.
    assert_eq!(f.offset, Some(29));
    assert_eq!(&d.source[29..37], "the note");
    // …and `end` closes the range, so a frontend can ask which rendered rows
    // the note occupies rather than re-deriving them from the text.
    assert_eq!(f.end, Some(37));
    assert_eq!(&d.source[f.offset.unwrap()..f.end.unwrap()], "the note");
}

/// Two definitions in a row: each is its own note, and neither reaches into
/// the other.
///
/// A djot definition's span used to run past the blank line into the first
/// byte of whatever followed, so this answered `"first note.\n\n["` — and the
/// offsets named the *next* note's rows too, showing a reader two footnotes
/// when they had asked about one. twig 3.1 ends the span after the block's
/// own last line; the test outlives the workaround leaf carried for it.
#[test]
fn footnote_at_stops_a_note_at_the_definition_after_it() {
    let src = "Claim[^2a] and [^2b].\n\n[^2a]: first note.\n\n[^2b]: second note.\n";
    for format in [Format::Markdown, Format::Djot] {
        let mut d = Doc::from_source(src.to_string(), format).unwrap();
        d.caret = 7;
        let f = d.footnote_at_caret().expect("a reference");
        assert_eq!(f.text.as_deref(), Some("first note."), "in {format:?}");
        assert_eq!(
            &src[f.offset.unwrap()..f.end.unwrap()],
            "first note.",
            "in {format:?}"
        );
    }
}

/// The other side of that boundary: a blank line *inside* a definition is
/// interior to it, and the note keeps its second paragraph.
///
/// This is what the old body scan cost. It stopped at the first line not
/// indented under the note — a blank line is not — so a two-paragraph note
/// came back as its first paragraph, and "go to note" framed half of it.
/// Reading the span twig gives is both simpler and right.
#[test]
fn footnote_at_keeps_a_notes_second_paragraph() {
    let src = "Claim[^1].\n\n[^1]: first para.\n\n    second para.\n\nAfter.\n";
    let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
    d.caret = 7;
    let f = d.footnote_at_caret().expect("a reference");
    assert_eq!(f.text.as_deref(), Some("first para.\n\n    second para."));
    // And it stops there — `After.` is the next block, not more note.
    assert_eq!(
        &src[f.offset.unwrap()..f.end.unwrap()],
        f.text.as_deref().unwrap()
    );
    assert!(!f.text.as_deref().unwrap().contains("After"));
}

#[test]
fn footnote_at_bounds_a_note_whose_body_is_empty() {
    // `[^1]:` with nothing after it. The range is empty rather than
    // inverted, and still points inside the definition — which is what keeps
    // a frontend's row lookup from walking off into the block above.
    let src = "A claim[^1].\n\n[^1]:\n";
    let mut d = doc_with("fn_empty_body", src);
    d.caret = 9;
    let f = d.footnote_at_caret().expect("a reference");
    assert_eq!(f.text.as_deref(), Some(""));
    assert_eq!(f.offset, f.end, "an empty note is an empty range");
    assert!(f.offset.unwrap() >= src.find("[^1]:").unwrap());
}

#[test]
fn footnote_at_caret_ignores_a_caret_that_stands_in_no_reference() {
    let mut d = doc_with(
        "fn_at_caret_none",
        "A claim[^1] and more.\n\n[^1]: the note\n",
    );
    d.caret = 2; // in the prose
    assert_eq!(d.footnote_at_caret(), None);
}

#[test]
fn footnote_at_caret_is_not_a_link_query_and_vice_versa() {
    // The two are deliberately separate: a reference names a note in this
    // document, a link names somewhere to leave for, and answering one with
    // the other is what made a reference click do nothing at all.
    let mut d = doc_with("fn_vs_link", "a[^1] b [t](https://x.dev)\n\n[^1]: note\n");
    d.caret = 3; // the `1` of `[^1]`
    assert!(d.footnote_at_caret().is_some());
    assert_eq!(
        d.link_destination_at_caret(),
        None,
        "a reference is not a link"
    );

    d.caret = 10; // inside the link's label
    assert_eq!(d.footnote_at_caret(), None, "a link is not a reference");
    assert_eq!(
        d.link_destination_at_caret().as_deref(),
        Some("https://x.dev")
    );
}

#[test]
fn footnote_at_caret_reports_an_undefined_reference_rather_than_nothing() {
    // A `[^99]` the document never defines is a real state — a note deleted
    // out from under its reference — and the label is what lets a frontend
    // say so. `None` here would be indistinguishable from "not on a
    // reference", which is the wrong thing to tell a reader.
    let mut d = doc_with("fn_undefined", "A claim[^99] and more.\n");
    d.caret = 9;
    let f = d
        .footnote_at_caret()
        .expect("the reference is still a reference");
    assert_eq!(f.label, "99");
    assert_eq!(f.text, None);
    assert_eq!(f.offset, None);
}

#[test]
fn footnote_at_caret_reads_a_word_label_and_a_multiline_note() {
    // Labels are not always numbers, and a note's body runs past its first
    // line — the indented continuation belongs to the note, so it comes back
    // with it (source bytes, verbatim, as documented).
    let src = "see[^note] here\n\n[^note]: first line\n    second line\n";
    let mut d = doc_with("fn_word_label", src);
    d.caret = 6;
    let f = d
        .footnote_at_caret()
        .expect("the caret stands in a reference");
    assert_eq!(f.label, "note");
    assert_eq!(f.text.as_deref(), Some("first line\n    second line"));
}

#[test]
fn footnote_at_answers_for_an_offset_the_caret_is_nowhere_near() {
    // The point of the offset form: a pointer hovering a reference asks what
    // note it names, and must not drag the caret along to ask.
    let mut d = doc_with("fn_at_off", "A claim[^1] and more.\n\n[^1]: the note\n");
    d.caret = 0;
    let f = d.footnote_at(9).expect("offset 9 stands in the reference");
    assert_eq!(f.label, "1");
    assert_eq!(f.text.as_deref(), Some("the note"));
    assert_eq!(d.caret, 0, "asking must not move the caret");
    assert_eq!(d.footnote_at(2), None, "offset 2 is prose");
}

#[test]
fn footnote_definition_at_caret_points_back_at_the_reference() {
    // The return leg. `[^1]` spans 7..11, so its label — the only byte of it
    // the caret can rest on — is at 9.
    let mut d = doc_with("fn_def", "A claim[^1] and more.\n\n[^1]: the note\n");
    d.caret = 30; // inside the note's body
    let f = d
        .footnote_definition_at_caret()
        .expect("the caret stands in a definition");
    assert_eq!(f.label, "1");
    assert_eq!(f.offset, Some(9));
    assert_eq!(&d.source[7..11], "[^1]");
}

#[test]
fn footnote_definition_at_covers_where_a_go_to_note_actually_lands() {
    // The two legs have to meet: wherever `footnote_at` sends the caret, the
    // definition query must answer for — otherwise arriving at a note leaves
    // the reader somewhere the way back isn't offered.
    let src = "A claim[^1] and more.\n\n[^1]: the note\n";
    let mut d = doc_with("fn_def_marker", src);
    let landed = d.footnote_at(9).unwrap().offset.unwrap();
    assert_eq!(
        d.footnote_definition_at(landed).and_then(|f| f.offset),
        Some(9),
        "the note a reference sends you to offers the way back"
    );
}

#[test]
fn footnote_definition_at_caret_ignores_prose_and_the_reference_itself() {
    // The two queries answer for disjoint places, which is what lets one
    // gesture mean "down to the note" in one and "back up" in the other
    // without either having to remember which way the reader is going.
    let mut d = doc_with("fn_def_none", "A claim[^1] and more.\n\n[^1]: the note\n");
    d.caret = 2; // prose
    assert_eq!(d.footnote_definition_at_caret(), None);
    d.caret = 9; // the reference
    assert_eq!(d.footnote_definition_at_caret(), None);
    assert!(
        d.footnote_at_caret().is_some(),
        "which is the reference's own query"
    );
}

#[test]
fn footnote_definition_at_caret_reports_an_orphan_note_rather_than_nothing() {
    // Nothing cites `[^2]`. Answering `None` would say "you are not in a
    // note", which is false and leaves a frontend unable to explain why the
    // way back is missing.
    let src = "A claim[^1].\n\n[^1]: cited\n\n[^2]: orphan\n";
    let mut d = doc_with("fn_def_orphan", src);
    d.caret = src.find("orphan").unwrap();
    let f = d
        .footnote_definition_at_caret()
        .expect("an orphan is still a definition");
    assert_eq!(f.label, "2");
    assert_eq!(f.offset, None);
}

#[test]
fn footnote_definition_at_caret_returns_to_the_first_of_repeated_references() {
    // One label, cited twice. The first is where the reader most likely came
    // from, and the only answer that doesn't depend on how they got here.
    let src = "One[^a] and two[^a].\n\n[^a]: the note\n";
    let mut d = doc_with("fn_def_repeat", src);
    d.caret = src.find("the note").unwrap();
    let f = d.footnote_definition_at_caret().expect("a definition");
    assert_eq!(
        f.offset,
        Some(5),
        "the first `[^a]`'s label, not the second's"
    );
    assert_eq!(&src[3..7], "[^a]");
}

#[test]
fn footnote_navigation_is_a_round_trip_through_placed_carets() {
    // Down and back up, each leg found from the document rather than from a
    // memory of the other — so it still works for a reader who scrolled to
    // the notes instead of jumping there.
    //
    // `place_caret` rather than assigning `caret`, because that is what a
    // frontend calls: it snaps to a real caret stop, and a jump that lands
    // on a byte the caret can't rest on would arrive somewhere the return
    // leg no longer answers for. `build_map` first, since snapping is a
    // no-op until the map exists — which is exactly how this went unnoticed
    // when the offsets pointed at the `[^` markers.
    let mut d = doc_with("fn_round", "A claim[^1] and more.\n\n[^1]: the note\n");
    d.build_map(None);
    d.place_caret(9, false);
    let down = d
        .footnote_at_caret()
        .expect("a reference")
        .offset
        .expect("a note");
    d.place_caret(down, false);
    let up = d
        .footnote_definition_at_caret()
        .expect("a definition")
        .offset
        .expect("a reference");
    d.place_caret(up, false);
    assert_eq!(d.caret, up, "the way back is a stop the caret can occupy");
    assert_eq!(
        d.footnote_at_caret().expect("back on the reference").label,
        "1"
    );
}

#[test]
fn footnote_navigation_survives_a_surface_that_drops_the_brackets() {
    // With the brackets gone the label is the whole of what is drawn, so
    // everything that finds a reference — the round trip, a caret walking
    // through it, the reveal — has only the label's own glyphs to go by.
    let src = "A claim[^1] and more.\n\n[^1]: the note\n";
    let mut d = doc_in(View::Wysiwyg, "fn_round_raised", src);
    d.set_raised_superscripts(true);
    d.build_visual(80);
    assert!(drawn_rows(&d).iter().any(|r| r == "A claim1 and more."));
    assert!(drawn_rows(&d).iter().any(|r| r == "1 the note"));

    d.place_caret(9, false);
    assert_eq!(d.caret, 9, "the label is a stop");
    let down = d
        .footnote_at_caret()
        .expect("a reference")
        .offset
        .expect("a note");
    d.place_caret(down, false);
    let up = d
        .footnote_definition_at_caret()
        .expect("a definition")
        .offset
        .expect("a reference");
    d.place_caret(up, false);
    assert_eq!(d.caret, up);
    assert_eq!(
        d.footnote_at_caret().expect("back on the reference").label,
        "1"
    );

    // The caret walks prose, label, prose, never resting on the markup
    // either side: from before the `m`, onto the label, past it, and past
    // the space — `claim|1| and`, with the `[^` and `]` stepped over.
    d.place_caret(src.find("m[").unwrap(), false);
    let mut walk = vec![];
    for _ in 0..3 {
        d.move_right(false);
        walk.push(d.caret);
    }
    assert_eq!(walk, [9, 11, 12], "{walk:?}");

    // Revealed, the reference is its source, brackets and all, raised or not.
    d.set_markup_mode(MarkupMode::Full);
    caret_at(&mut d, "claim");
    assert!(
        drawn_rows(&d).iter().any(|r| r == "A claim[^1] and more."),
        "{:?}",
        drawn_rows(&d)
    );
}

#[test]
fn insert_link_hands_the_destination_to_twig_raw() {
    // Escaping is twig's, and format-specific: Markdown ends a destination
    // at the first space and needs the `<…>` form, where djot would read
    // those angle brackets as part of the URL.
    let mut d = doc_with("link_space", "word\n");
    d.anchor = Some(0);
    d.caret = 4;
    d.insert_link("a b");
    assert_eq!(d.source, "[word](<a b>)\n");
}

#[test]
fn insert_link_reports_a_destination_no_format_can_carry() {
    let mut d = doc_with("link_bad", "word\n");
    d.anchor = Some(0);
    d.caret = 4;
    d.insert_link("a\nb");
    assert_eq!(d.source, "word\n"); // untouched, not quietly rewritten
    assert!(
        d.status.is_some(),
        "InvalidArgument should reach the status line"
    );
    assert!(!d.dirty);
}

#[test]
fn insert_link_works_in_wysiwyg_view() {
    let mut d = wysiwyg_doc("link_wys", "word here\n");
    d.anchor = Some(0);
    d.caret = 4;
    d.insert_link("http://x.dev");
    assert_eq!(d.source, "[word](http://x.dev) here\n");
    assert_eq!(d.selected_text(), Some("word"));
    // The map the caret has to keep riding is rebuilt each frame; motion
    // over the fresh one must still land on a real stop (the debug_assert).
    d.build_visual(80);
    d.move_right(false);
    d.move_left(false);
}

#[test]
fn insert_link_labelled_writes_the_label_and_lands_after_the_link() {
    // A host that knows the title of what it links: the label is the
    // text, and the link is finished — caret past it, nothing selected.
    for fmt in [Format::Markdown, Format::Djot] {
        let mut d = Doc::from_source("see \n".into(), fmt).unwrap();
        d.caret = 4;
        d.insert_link_labelled("2026-10-02.md", "Title");
        assert_eq!(d.source, "see [Title](2026-10-02.md)\n", "{fmt:?}");
        assert_eq!(d.selection(), None, "{fmt:?}");
        assert_eq!(d.caret, "see [Title](2026-10-02.md)".len(), "{fmt:?}");
        assert_eq!(
            d.link_destination_at(5).as_deref(),
            Some("2026-10-02.md"),
            "{fmt:?}"
        );
        assert!(d.dirty);
    }
}

#[test]
fn insert_link_labelled_escapes_the_label_so_it_round_trips() {
    // A `]` must not close the link early, nor a `*` open emphasis: the
    // label goes in as literal text in the body's own grammar.
    for fmt in [Format::Markdown, Format::Djot] {
        let label = "a] *b* [c` <d>";
        let mut d = Doc::from_source("\n".into(), fmt).unwrap();
        d.caret = 0;
        d.insert_link_labelled("x.md", label);
        assert_eq!(d.selection(), None, "{fmt:?}");
        assert_eq!(d.caret, d.source.len() - 1, "{fmt:?}: {:?}", d.source);
        let link = d
            .nodes()
            .into_iter()
            .find(|n| n.kind == Kind::Link)
            .unwrap_or_else(|| panic!("{fmt:?}: no link in {:?}", d.source));
        assert_eq!(link.span, 0..d.source.len() - 1, "{fmt:?}: {:?}", d.source);
        assert_eq!(d.link_destination_at(1).as_deref(), Some("x.md"));
        // What the link's text reads as, markup and escapes aside.
        let text: String = d
            .nodes()
            .into_iter()
            .filter(|n| n.span.start >= link.span.start && n.span.end <= link.span.end)
            .inspect(|n| {
                assert!(
                    matches!(n.kind, Kind::Para | Kind::Link | Kind::Str),
                    "{fmt:?}: the label minted {:?} in {:?}",
                    n.kind,
                    d.source
                )
            })
            .filter(|n| n.kind == Kind::Str)
            .filter_map(|n| n.text)
            .collect();
        assert_eq!(text, label, "{fmt:?}: {:?}", d.source);
    }
}

#[test]
fn insert_link_labelled_ignores_the_label_over_a_selection() {
    let mut d = doc_with("link_lab_sel", "word here\n");
    d.anchor = Some(0);
    d.caret = 4;
    d.insert_link_labelled("http://x.dev", "Ignored");
    assert_eq!(d.source, "[word](http://x.dev) here\n");
    assert_eq!(d.selected_text(), Some("word"));
}

#[test]
fn insert_link_labelled_repoints_the_link_the_caret_stands_in() {
    // A host's Edit Link calls the labelled verb too: inside a link it
    // re-points and keeps the text there is, as `insert_link` does.
    let mut d = doc_with("link_lab_repoint", "[word](http://x.dev)\n");
    d.caret = 3;
    d.insert_link_labelled("http://y.dev", "Ignored");
    assert_eq!(d.source, "[word](http://y.dev)\n");
    assert_eq!(d.selected_text(), Some("word"));

    let mut d = doc_with("link_lab_repoint_auto", "see <https://x.dev> ok\n");
    d.caret = 10;
    d.insert_link_labelled("https://y.dev", "Ignored");
    assert_eq!(d.source, "see <https://y.dev> ok\n");
}

#[test]
fn insert_link_labelled_just_past_a_link_writes_a_new_one() {
    // The caret at a link's end is not in it: nothing to re-point.
    let mut d = doc_with("link_lab_after", "[a](x.md)\n");
    d.caret = 9;
    d.insert_link_labelled("y.md", "B");
    assert_eq!(d.source, "[a](x.md)[B](y.md)\n");
    assert_eq!(d.caret, 18);
}

#[test]
fn insert_link_labelled_is_one_undo_step() {
    let mut d = doc_with("link_lab_undo", "see \n");
    d.caret = 4;
    let steps = d.undo_steps;
    d.insert_link_labelled("x.md", "Title");
    assert_eq!(d.undo_steps, steps + 1);
    assert!(d.undo());
    assert_eq!(d.source, "see \n");
    assert_eq!(d.caret, 4);
    assert!(d.redo());
    assert_eq!(d.source, "see [Title](x.md)\n");
}

#[test]
fn insert_link_labelled_refuses_a_bad_destination_and_leaves_no_label() {
    let mut d = doc_with("link_lab_bad", "see \n");
    d.caret = 4;
    d.insert_link_labelled("a\nb", "Title");
    assert_eq!(d.source, "see \n");
    assert!(d.status.is_some());
    assert!(!d.dirty);
}

#[test]
fn insert_link_labelled_refuses_a_read_only_document() {
    let mut d = doc_with("link_lab_ro", "see \n");
    d.set_read_only(true);
    d.caret = 4;
    d.insert_link_labelled("x.md", "Title");
    assert_eq!(d.source, "see \n");
    assert!(!d.dirty);
}
