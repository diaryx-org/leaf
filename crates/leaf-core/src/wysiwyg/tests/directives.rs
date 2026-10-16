use super::*;

#[test]
fn a_directive_container_is_tinted_and_labeled_on_its_first_row() {
    // diaryx's `:::vis{.public .family}` visibility block, and any other
    // `:::name{.class}` fenced div — core is agnostic of `name`.
    let src = ":::vis{.public .family}\nhello\n\nworld\n:::\nafter\n";
    let m = map_directives(src);

    let content_rows: Vec<usize> = (0..m.rows.len()).filter(|&i| m.rows[i].directive).collect();
    assert!(!content_rows.is_empty(), "some row is flagged directive");

    let after_rows: Vec<usize> = (0..m.rows.len())
        .filter(|&i| !content_rows.contains(&i) && !m.rows[i].glyphs.is_empty())
        .collect();
    assert!(
        after_rows.iter().all(|&i| !m.rows[i].directive),
        "content outside the fence isn't tinted"
    );

    let labels: Vec<&str> = content_rows
        .iter()
        .filter_map(|&i| m.rows[i].directive_label.as_deref())
        .collect();
    assert_eq!(
        labels,
        vec!["public family"],
        "only the first row carries the label"
    );

    assert_eq!(
        rendered(&m)
            .lines()
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>(),
        vec!["hello", "world", "after"],
        "fence markers don't leak into the rendered text"
    );
}

#[test]
fn a_bare_word_directive_is_labeled_same_as_dot_classes() {
    // diaryx_core::visibility's own `:::vis{public family}` — no leading
    // dots — is what apps/web's directive serializer and the native
    // publish-time filter both actually write today, distinct from twig's
    // `.class` convention. Both must label the same way so every existing
    // diaryx `:::vis{...}` block reads, not just newly dot-authored ones.
    let src = ":::vis{public family}\nhello\n:::\n";
    let m = map_directives(src);
    let label = m.rows.iter().find_map(|r| r.directive_label.clone());
    assert_eq!(label.as_deref(), Some("public family"));
}

#[test]
fn a_text_directive_keeps_its_paragraph_visible() {
    // Regression: an inline `:name[label]{…}` used to make its paragraph
    // fail the "all children inline" test, so the whole line was walked as
    // a container of blocks and rendered as empty rows with NO caret stops —
    // the text vanished from the editor and the caret couldn't enter it.
    // diaryx's inline `:vis[…]` is exactly this shape.
    let src = "Text with :abbr[HTML]{title=\"HyperText\"} inline.\n";
    let m = map_directives(src);
    assert_eq!(rendered(&m).trim_end(), "Text with HTML inline.");
    // Every character of the line is a caret home, markup excluded — the
    // label reads as ordinary text, the way a link's does.
    let stops: usize = m
        .rows
        .iter()
        .map(|r| r.glyphs.iter().filter(|g| g.stop).count())
        .sum();
    assert_eq!(stops, "Text with HTML inline.".chars().count());
    // It is inline, so it is not the container form's tinted panel.
    assert!(m.rows.iter().all(|r| !r.directive));
}

#[test]
fn a_text_directives_label_maps_to_its_true_source_bytes() {
    // Regression (needs twig-doc >= 2.5.0): twig parses a `[label]` as a
    // detached slice, and until it rebased the enclosing scan's segments
    // onto it every node inside the label reported a span of `(0,0)`. Read
    // by anything that trusts a span that means "byte 0", so the label's
    // glyphs mapped to the START OF THE DOCUMENT — a click on the label put
    // the caret at the top of the file, its stops collided with the real
    // first line's, and an edit there landed on the wrong bytes entirely.
    //
    // The sibling test `a_text_directive_keeps_its_paragraph_visible` only
    // counts stops, which is exactly why this went unnoticed: the right
    // NUMBER of stops at completely wrong offsets.
    let src = "x :abbr[HTML]{title=\"y\"} z\n";
    let m = map_directives(src);
    let stops: Vec<(char, usize)> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| (g.ch, g.src))
        .collect();
    // `HTML` sits at 8..12. The name, brackets and `{…}` are hidden markup
    // the caret steps over, so the line's stops run 0, 1, 8..12, then 24.
    assert_eq!(
        stops,
        [
            ('x', 0),
            (' ', 1),
            ('H', 8),
            ('T', 9),
            ('M', 10),
            ('L', 11),
            (' ', 24),
            ('z', 25)
        ]
    );
}

#[test]
fn every_glyph_in_a_directive_label_points_at_its_source_byte() {
    // The `every_glyph_points_at_its_source_byte` invariant, extended over
    // directive labels now that their offsets are real. Nested markup is
    // included: its delimiters are hidden, so the visible glyphs must skip
    // them and still name their own bytes.
    let src = "x :abbr[a *b* c] y and :vis[family only] z\n";
    let m = map_directives(src);
    for g in m.rows.iter().flat_map(|r| &r.glyphs).filter(|g| g.stop) {
        let at = src[g.src..].chars().next();
        assert_eq!(
            at,
            Some(g.ch),
            "glyph {:?} claims byte {}, which is {at:?}",
            g.ch,
            g.src
        );
    }
    assert_eq!(rendered(&m).trim_end(), "x a b c y and family only z");
}

#[test]
fn a_directive_labels_nested_emphasis_keeps_both_its_style_and_its_offsets() {
    let src = "x :abbr[a *b* c] y\n";
    let m = map_directives(src);
    let b = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .find(|g| g.ch == 'b')
        .expect("the emphasised char");
    assert!(b.style.italic, "the label's *b* lost its emphasis");
    assert_eq!(b.src, 11, "the label's *b* lost its source byte");
}

#[test]
fn a_bare_colon_word_renders_as_the_prose_it_almost_always_is() {
    // Regression: twig matches a colon followed by any letter-led word, so
    // ordinary prose is full of "text directives" nobody meant to write.
    // With no `[label]` there are no children, and the arm recursed into
    // them — rendering *nothing*. The word vanished from the document with
    // no caret stop left behind, so it could not even be deleted.
    for src in ["a :word b\n", "note :see below\n", ":smile: hi\n"] {
        let m = map_directives(src);
        assert_eq!(
            rendered(&m).trim_end(),
            src.trim_end(),
            "prose was eaten: {src:?}"
        );
    }
}

#[test]
fn a_bare_colon_word_keeps_every_byte_a_caret_stop() {
    let src = "a :word b\n";
    let m = map_directives(src);
    // Nothing here is markup, so nothing is hidden: each byte maps to
    // itself and can be stood on, which is what makes the colon deletable.
    let stops: Vec<(char, usize)> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| (g.ch, g.src))
        .collect();
    assert_eq!(
        stops,
        "a :word b"
            .chars()
            .enumerate()
            .map(|(i, c)| (c, i))
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_attribute_bearing_text_directive_draws_a_chip() {
    // `{…}` is deliberate in a way a bare colon is not — diaryx writes
    // `:vis{.family}` inline — so this one reads as an embed, on the same
    // `⧉ label` recipe the leaf form's placeholder row uses.
    // Both attribute conventions label it: twig's dot-prefixed classes and
    // the bare pandoc-style words diaryx also writes.
    for src in ["a :vis{.family} b\n", "a :vis{family} b\n"] {
        let m = map_directives(src);
        assert_eq!(rendered(&m).trim_end(), "a ⧉ vis family b", "{src:?}");
    }
    // A `key=value` attr is configuration, not a name, so it adds nothing.
    let m = map_directives("a :foo{title=\"x\"} b\n");
    assert_eq!(rendered(&m).trim_end(), "a ⧉ foo b");
}

#[test]
fn a_directive_chip_is_one_atomic_caret_stop_at_its_own_offset() {
    let src = "a :vis{.family} b\n";
    let m = map_directives(src);
    let stops: Vec<usize> = m
        .rows
        .iter()
        .flat_map(|r| &r.glyphs)
        .filter(|g| g.stop)
        .map(|g| g.src)
        .collect();
    // The chip contributes exactly one stop, at the directive's start (2),
    // so the caret steps over it whole instead of walking hidden markup a
    // byte at a time. `{.family}`'s bytes (3..15) are never stood on.
    assert_eq!(stops, [0, 1, 2, 15, 16]);
}

#[test]
fn a_paragraph_holding_only_a_chip_is_still_navigable() {
    // With no stop of its own the row would be unreachable — the caret
    // could never be put on the line to edit or delete the directive.
    let m = map_directives(":vis{.family}\n");
    assert!(
        m.row_is_navigable(0),
        "a chip-only paragraph has no caret home"
    );
    assert_eq!(
        m.offset_of_pos(0, 0),
        0,
        "its caret home isn't the directive's start"
    );
}

#[test]
fn a_ratio_or_a_clock_time_is_never_a_directive() {
    // twig requires a letter after the colon, so these stay prose — the
    // verbatim arm must not be reached for them at all.
    let src = "ratio 3:4 and 10:30\n";
    assert_eq!(
        rendered(&map_directives(src)).trim_end(),
        "ratio 3:4 and 10:30"
    );
}

#[test]
fn a_leaf_directive_is_a_placeholder_row_with_its_attrs_published() {
    // `::name{…}` is a standalone block with no body — an embed, a table of
    // contents. It used to emit no rows at all: invisible, no caret home,
    // vertical motion crossing a void. Now it draws the image recipe's
    // placeholder and publishes what the host app needs to paint the real
    // thing.
    let src = "before\n\n::embed{src=\"demo.html\" height=\"400\"}\n\nafter\n";
    let m = map_directives(src);

    let row = m
        .rows
        .iter()
        .position(|r| r.leaf_directive.is_some())
        .expect("a placeholder row");
    assert_eq!(
        m.rows[row].glyphs.iter().map(|g| g.ch).collect::<String>(),
        "⧉ embed"
    );
    assert!(
        m.rows[row].glyphs.iter().any(|g| g.stop),
        "the caret can land on it"
    );
    assert!(
        m.rows[row].directive,
        "a frontend frames it like the container form"
    );

    assert_eq!(m.directives.len(), 1);
    let info = &m.directives[0];
    assert_eq!(info.name, "embed");
    assert_eq!(info.rows_span, row..row + 1);
    assert_eq!(info.attr("src"), Some("demo.html"));
    assert_eq!(info.attr("height"), Some("400"));
    assert_eq!(info.attr("nope"), None);
    // The prose around it is untouched.
    assert!(rendered(&m).contains("before") && rendered(&m).contains("after"));
}

#[test]
fn a_leaf_directive_shows_its_label_and_honours_its_prefix() {
    // A `[label]` names the placeholder (the way an image's alt does), and a
    // quoted directive keeps the quote's gutter — it is a block like any
    // other, not a special case that escapes its container.
    let m = map_directives("::embed[Audience demo]{src=\"demo.html\"}\n");
    assert_eq!(rendered(&m).trim_end(), "⧉ Audience demo");
    assert_eq!(m.directives[0].label, "Audience demo");

    let quoted = map_directives("> ::embed{src=\"x.html\"}\n");
    assert_eq!(rendered(&quoted).trim_end(), "│ ⧉ embed");
    assert_eq!(quoted.directives[0].name, "embed");
}

#[test]
fn a_container_directive_is_still_a_panel_not_a_placeholder() {
    // The three forms must not bleed into each other: only the leaf form is
    // a placeholder, and only the container form tints the blocks it wraps.
    let m = map_directives(":::note{.warning}\nBody\n:::\n");
    assert!(
        m.directives.is_empty(),
        "a container publishes no placeholder"
    );
    assert!(m.rows.iter().all(|r| r.leaf_directive.is_none()));
    assert_eq!(rendered(&m).trim_end(), "Body");
    assert!(
        m.rows
            .iter()
            .any(|r| r.directive && r.directive_label.as_deref() == Some("warning"))
    );
}

/// Every `container` node in `src`, parsed the way production does (both
/// extensions on), paired with what [`container_is_directive`] makes of it.
fn containers(src: &str) -> Vec<(String, bool, Option<DirectiveForm>)> {
    let mut ed = Editor::new_with_features(
        src.as_bytes(),
        Format::Markdown.twig(),
        &["directives", "html_elements"],
    )
    .unwrap();
    ed.nodes()
        .unwrap()
        .iter()
        .filter(|n| n.kind == Kind::Container)
        .map(|n| {
            (
                n.name.clone().unwrap_or_default(),
                container_is_directive(n),
                n.directive_form,
            )
        })
        .collect()
}

#[test]
fn a_directive_and_an_html_element_are_told_apart_by_spelling_not_by_form() {
    // twig 2.8 folded `div`/`span`/`directive`/`element` into one `container`
    // kind. `directive_form` reads as though it separates them and does not:
    // a block-level `<div>` reports `Some(DirectiveForm::Container)` exactly
    // as a `:::note` does. Trusting it would draw directive chrome — a tinted
    // panel, a `.class` audience label — on every pasted Slack/Docs div.
    for (src, name, want) in [
        (":::note{.a}\nbody\n:::\n", "note", true),
        ("::embed{src=x}\n", "embed", true),
        ("a :vis[hi]{.b} b\n", "vis", true),
        ("<div class=\"x\">\nhi\n</div>\n", "div", false),
        ("<video src=\"v.mp4\" controls></video>\n", "video", false),
        ("<audio src=\"a.mp3\" controls></audio>\n", "audio", false),
        ("<figure>\n\nhi\n\n</figure>\n", "figure", false),
        // The `:` in an attribute must not read as a directive opener: the
        // `<` of the tag comes first, and first one wins.
        (
            "<video src=\"http://x.test/v.mp4\" controls></video>\n",
            "video",
            false,
        ),
        (
            "<source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\">\n",
            "source",
            false,
        ),
    ] {
        let found = containers(src);
        let hit = found.iter().find(|(n, ..)| n == name);
        let Some((_, is_directive, form)) = hit else {
            panic!("no `{name}` container in {src:?} — found {found:?}");
        };
        assert_eq!(*is_directive, want, "{name} in {src:?} (form was {form:?})");
    }

    // And the reason this can't just read the field: for the one collision
    // that matters, the field says the same thing for both.
    let div = containers("<div class=\"x\">\nhi\n</div>\n");
    let note = containers(":::note{.a}\nbody\n:::\n");
    assert_eq!(
        div[0].2, note[0].2,
        "if these ever differ, `directive_form` became usable and this rule can go"
    );
}

#[test]
fn a_directive_nested_in_a_quote_or_list_is_still_a_directive() {
    // A container's span opens with its *block prefix*, not its own markup —
    // `> ::embed{…}` starts at the `>`. Reading only the first byte to tell a
    // directive from an element (both `container` since 2.8) therefore misses
    // every nested one, and the placeholder silently renders as nothing.
    for (src, ctx) in [
        ("> ::embed{src=\"x\"}\n", "quoted"),
        ("- ::embed{src=\"x\"}\n", "listed"),
        (">> ::embed{src=\"x\"}\n", "twice quoted"),
    ] {
        let m = map_directives(src);
        assert_eq!(m.directives.len(), 1, "{ctx} directive was lost");
        assert_eq!(m.directives[0].name, "embed", "{ctx}");
    }
}

#[test]
fn a_video_is_still_media_and_not_a_directive() {
    // The other side of the same coin: `<video>` is a `container` too, and
    // must reach `block_media` rather than the directive arms.
    let doc = doc_built("<video src=\"clip.mp4\" controls></video>\n");
    assert_eq!(doc.vmap.media.len(), 1, "the video is block media");
    assert!(
        doc.vmap.rows.iter().all(|r| !r.directive),
        "the video drew directive chrome"
    );
}

#[test]
fn a_directive_needs_the_extension_flag() {
    // `map` (twig's default extensions) leaves `directives` off — the fence
    // renders as literal paragraph text, same as any other unrecognized
    // punctuation, never corrupting or panicking.
    let src = ":::vis{.public}\nhello\n:::\n";
    let m = map(src);
    assert!(m.rows.iter().all(|r| !r.directive));
    assert!(rendered(&m).contains(":::vis{.public}"));
}
