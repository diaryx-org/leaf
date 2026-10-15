use super::*;

// ── Text statistics ──────────────────────────────────────────────────────

#[test]
fn counts_tally_plain_prose() {
    let c = counts_of(
        "counts_prose",
        "The quick brown fox jumps over the lazy dog.\n",
    );
    assert_eq!(
        c,
        TextCounts {
            words: 9,
            characters: 44,
            characters_without_spaces: 36,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_read_the_text_and_not_the_markup() {
    // The `**` are four bytes of source and no part of the word.
    assert_eq!(
        counts_of("counts_marks", "a **bold** word\n"),
        TextCounts {
            words: 3,
            characters: 11,
            characters_without_spaces: 9,
            paragraphs: 1,
        }
    );
    // A link is its label; the destination is plumbing, however long.
    assert_eq!(
        counts_of(
            "counts_link",
            "see [the label](https://example.com/a/b/c) here\n"
        ),
        TextCounts {
            words: 4,
            characters: 18,
            characters_without_spaces: 15,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_spend_nothing_on_a_picture() {
    // A block image renders as a `🖼 alt` placeholder — a picture, not a
    // sentence, and not a paragraph either.
    assert_eq!(
        counts_of("counts_image", "![a long caption](pic.png)\n"),
        TextCounts::default()
    );
    // And it adds nothing to the prose around it.
    assert_eq!(
        counts_of("counts_image_prose", "text\n\n![a long caption](pic.png)\n"),
        TextCounts {
            words: 1,
            characters: 4,
            characters_without_spaces: 4,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_spend_nothing_on_drawn_furniture() {
    // A thematic break is drawn, not written, and an empty paragraph has
    // nothing in it — neither is a paragraph of the document.
    assert_eq!(
        counts_of("counts_rule", "one\n\n---\n\ntwo\n"),
        TextCounts {
            words: 2,
            characters: 6,
            characters_without_spaces: 6,
            paragraphs: 2,
        }
    );
}

#[test]
fn counts_measure_characters_as_a_reader_does() {
    // Four Han characters (each its own word under UAX#29), one ZWJ emoji
    // family that is a single grapheme cluster, and two letters.
    let c = counts_of(
        "counts_graphemes",
        "你好世界 👩\u{200d}👩\u{200d}👧\u{200d}👦 ok\n",
    );
    assert_eq!(
        c,
        TextCounts {
            words: 5,
            characters: 9,
            characters_without_spaces: 7,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_take_a_code_block_as_one_paragraph() {
    let c = counts_of("counts_code", "```rust\nlet x = 1;\n\nlet y = 2;\n```\n");
    assert_eq!(
        c,
        TextCounts {
            words: 6,
            characters: 20,
            characters_without_spaces: 14,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_take_a_table_as_one_paragraph() {
    // The box-drawn borders and the column padding are the renderer's, not
    // the author's; the cells are what was written.
    let c = counts_of("counts_table", "| a b | c |\n| - | - |\n| d | e |\n");
    assert_eq!(
        c,
        TextCounts {
            words: 5,
            characters: 6,
            characters_without_spaces: 5,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_give_every_item_and_every_quoted_paragraph_its_own_paragraph() {
    let c = counts_of(
        "counts_blocks",
        "- one\n- two\n- three\n\n> first quoted\n>\n> second quoted\n",
    );
    assert_eq!(
        c,
        TextCounts {
            words: 7,
            characters: 36,
            characters_without_spaces: 34,
            paragraphs: 5,
        }
    );
}

#[test]
fn counts_leave_the_frontmatter_out() {
    // The WYSIWYG view doesn't render it and a writer didn't write it.
    let c = counts_of(
        "counts_frontmatter",
        "---\ntitle: Hidden\n---\n\nvisible words here\n",
    );
    assert_eq!(
        c,
        TextCounts {
            words: 3,
            characters: 18,
            characters_without_spaces: 16,
            paragraphs: 1,
        }
    );
}

#[test]
fn counts_of_an_empty_document_are_all_zero() {
    assert_eq!(counts_of("counts_empty", ""), TextCounts::default());
}

/// UAX#29 puts a boundary at the hyphen, so a hyphenated compound is two
/// words. Recorded rather than corrected: it is what the algorithm says,
/// and what every other UAX#29 counter reports.
#[test]
fn counts_split_a_hyphenated_compound_in_two() {
    let c = counts_of("counts_hyphen", "well-known example\n");
    assert_eq!(c.words, 3);
    assert_eq!(c.characters, 18);
    // Punctuation on its own is no word, and an apostrophe doesn't split one.
    assert_eq!(counts_of("counts_punct", "don't ... stop\n").words, 2);
}

#[test]
fn selection_counts_measure_the_selection_and_nothing_without_one() {
    let src = "alpha beta\n\ngamma delta\n";
    let mut d = doc_in(View::Wysiwyg, "counts_sel", src);
    assert_eq!(d.selection_counts(), None, "no selection, no counts");

    // From the `b` of `beta` to the end of `gamma`: two blocks clipped.
    d.select_range(6, 17);
    assert_eq!(
        d.selection_counts(),
        Some(TextCounts {
            words: 2,
            characters: 9,
            characters_without_spaces: 9,
            paragraphs: 2,
        })
    );
}

/// The count is of the document, not of the window it is shown in — so
/// ⌘E must not move it, and neither must a resize or an edit made with no
/// map built at all.
#[test]
fn counts_agree_across_the_views() {
    let src =
        "# Head\n\nA **bold** word, a [label](http://x), and more.\n\n- item one\n- item two\n";
    let mut d = doc_in(View::Wysiwyg, "counts_views", src);
    let wysiwyg = d.counts();
    assert!(wysiwyg.words > 0 && wysiwyg.paragraphs == 4);

    d.toggle_view();
    assert_eq!(d.view, View::Source);
    d.build_source();
    assert_eq!(d.counts(), wysiwyg, "the source view counts the same text");

    // A narrower measure is a narrower window, not a shorter document.
    d.toggle_view();
    d.build_visual(24);
    assert_eq!(d.counts(), wysiwyg, "wrapping is not an input");

    // And an edit made in the source view, with the visual map left stale,
    // still counts the document as it now stands.
    d.toggle_view();
    d.caret = d.source.len();
    d.insert("\n\ntail words\n");
    let after = d.counts();
    assert_eq!(after.paragraphs, wysiwyg.paragraphs + 1);
    assert_eq!(after.words, wysiwyg.words + 2);
}
