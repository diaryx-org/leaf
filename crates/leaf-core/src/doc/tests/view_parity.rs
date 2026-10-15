use super::*;

// ── view parity ──────────────────────────────────────────────────────────
// `doc_with` pins the source view, so most of these suites test a view users
// never start in — `Doc::open` opens in WYSIWYG. These run the motion and
// deletion golden cases through *both*, plus the WYSIWYG cases the two
// can't share: where the source carries markup the rendered text is a
// different string, and the views agreeing would itself be the bug.

/// Run `action` in both views on one `|`-marked fixture and assert they
/// agree. Plain prose only: with no markup to hide, WYSIWYG renders the
/// source verbatim, so the two views are looking at the same text and any
/// disagreement is one of them having lost the plot.
fn both_views(name: &str, marked: &str, action: fn(&mut Doc)) -> String {
    let (src, caret) = parse_caret(marked);
    let run = |view: View, tag: &str| {
        let mut d = doc_in(view, &format!("{name}_{tag}"), &src);
        d.caret = caret;
        action(&mut d);
        render_caret(&d)
    };
    let source = run(VIEWS[0].0, VIEWS[0].1);
    let wysiwyg = run(VIEWS[1].0, VIEWS[1].1);
    assert_eq!(source, wysiwyg, "the views disagree on {marked:?}");
    source
}

#[test]
fn word_motion_agrees_across_the_views_on_plain_prose() {
    let g = both_views;
    assert_eq!(
        g("par_wl", "hello wor|ld", |d| d.move_word_left(false)),
        "hello |world"
    );
    assert_eq!(
        g("par_wl2", "hello| world", |d| d.move_word_left(false)),
        "|hello world"
    );
    assert_eq!(
        g("par_wr", "hel|lo world", |d| d.move_word_right(false)),
        "hello| world"
    );
    assert_eq!(
        g("par_wr2", "hello| world", |d| d.move_word_right(false)),
        "hello world|"
    );
    assert_eq!(
        g("par_punct", "|foo.bar", |d| d.move_word_right(false)),
        "foo|.bar"
    );
    assert_eq!(
        g("par_ext", "hello |world", |d| d.move_word_right(true)),
        "hello [world|]"
    );
}

#[test]
fn word_deletion_agrees_across_the_views_on_plain_prose() {
    let g = both_views;
    assert_eq!(
        g("par_db", "hello world|", |d| d.delete_word_back()),
        "hello |"
    );
    assert_eq!(
        g("par_df", "hello |world", |d| d.delete_word_forward()),
        "hello |"
    );
    assert_eq!(
        g("par_db2", "foo |bar baz", |d| d.delete_word_back()),
        "|bar baz"
    );
    assert_eq!(g("par_utf8", "café |ok", |d| d.delete_word_back()), "|ok");
}

#[test]
fn character_motion_and_deletion_agree_across_the_views_on_plain_prose() {
    let g = both_views;
    assert_eq!(g("par_r", "he|llo", |d| d.move_right(false)), "hel|lo");
    assert_eq!(g("par_l", "he|llo", |d| d.move_left(false)), "h|ello");
    assert_eq!(g("par_bs", "hel|lo", |d| d.backspace()), "he|lo");
    assert_eq!(g("par_del", "hel|lo", |d| d.delete_forward()), "hel|o");
}

#[test]
fn wysiwyg_motion_steps_a_grapheme_cluster_the_way_the_source_view_does() {
    // The reproduction: the stop table was built one stop per `char`, so
    // Right parked the caret 4 bytes into a ZWJ sequence — a place the
    // source view, which steps by grapheme, can't reach and backspace can't
    // survive. The two views must land on the same offset.
    let family = "👨‍👩‍👧"; // three emoji strung together with joiners: one cluster
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("cluster_{tag}"), &format!("a{family}b\n"));
        d.caret = 1;
        d.move_right(false);
        assert_eq!(d.caret, 1 + family.len(), "{tag} parked inside the cluster");

        // ...and the edit that used to sever a joiner off the front of it.
        d.backspace();
        assert_eq!(d.source, "ab\n", "{tag} split the cluster");
        assert_eq!(d.caret, 1);
    }
}

#[test]
fn wysiwyg_motion_treats_a_combining_accent_as_one_character() {
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("combining_{tag}"), "e\u{0301}x\n");
        d.caret = 0;
        d.move_right(false);
        assert_eq!(
            d.caret,
            "e\u{0301}".len(),
            "{tag} stopped on the combining mark"
        );
    }
}

#[test]
fn no_wysiwyg_motion_can_park_the_caret_inside_a_cluster() {
    // The general form: whatever route the caret takes through a document
    // full of clusters, it never lands between the codepoints of one — so no
    // motion-then-backspace sequence can leave a dangling joiner behind.
    use unicode_segmentation::UnicodeSegmentation;

    let src = "a👨‍👩‍👧b e\u{0301}mo👨‍👩‍👧ji\n\nnext 👩‍🚀 line\n";
    let mut d = wysiwyg_doc("cluster_walk", src);
    d.caret = 0;
    let boundaries: Vec<usize> = src
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(src.len()))
        .collect();
    for off in walk_right(&mut d) {
        assert!(
            boundaries.contains(&off),
            "Right stopped at {off}, inside a grapheme cluster"
        );
    }
}

#[test]
fn wysiwyg_word_motion_stays_out_of_hidden_delimiters() {
    // The reproduction: ⌥→ from inside the opening `**` computed its
    // boundary over the raw source and landed on byte 8 — inside the
    // *closing* `**`, which `caret_pos` draws at column 6, immediately after
    // "bold". The caret drew past the bold word and sat inside it.
    let mut d = wysiwyg_doc("wys_word_delim", "a **bold** c\n");
    d.caret = 2;
    d.move_word_right(false);
    assert!(
        d.vmap.is_stop(d.caret),
        "landed at {}, not a caret stop",
        d.caret
    );
    assert_eq!(d.caret, 10, "should land on the space after \"bold\"");
    // The rendered row is "a bold c": column 6 is the space just past "bold",
    // and now the caret is really there rather than only drawn there.
    assert_eq!(d.caret_pos(), (0, 6));

    // ...and back again: ⌥← returns to the "b", not into the opening `**`.
    d.move_word_left(false);
    assert_eq!(d.caret, 4);
    assert_eq!(d.caret_pos(), (0, 2));
}

#[test]
fn wysiwyg_word_delete_takes_the_markup_with_the_word() {
    // The reproduction: ⌥⌫ from after "bold" walked the raw source, stopped
    // inside the closing `**`, and left "a ** c\n" — delimiters with no
    // opener. Glyph space covers the word alone, which would leave
    // "a **** c": markup wrapped around nothing. The word and the styling
    // that was only ever the word's go together.
    let mut d = wysiwyg_doc("wys_word_del_back", "a **bold** c\n");
    d.caret = 10;
    d.delete_word_back();
    assert_eq!(d.source, "a  c\n");
    assert_eq!(d.caret, 2);

    let mut d = wysiwyg_doc("wys_word_del_fwd", "a **bold** c\n");
    d.caret = 4; // the "b"
    d.delete_word_forward();
    assert_eq!(d.source, "a  c\n");
}

#[test]
fn wysiwyg_word_delete_empties_a_nested_mark_and_a_code_span_too() {
    let src = "a ***bold*** c\n";
    let mut d = wysiwyg_doc("wys_word_del_nest", src);
    d.caret = src.find(" c").unwrap();
    d.delete_word_back();
    assert_eq!(
        d.source, "a  c\n",
        "the emph inside the strong empties it too"
    );

    let src = "a `code` c\n";
    let mut d = wysiwyg_doc("wys_word_del_code", src);
    d.caret = src.find(" c").unwrap();
    d.delete_word_back();
    assert_eq!(d.source, "a  c\n");
}

#[test]
fn wysiwyg_word_delete_keeps_a_mark_that_still_has_text() {
    // Only an *emptied* node goes. Take one word of two and the `**` still
    // has a job to do — over the word that's left, with the space the delete
    // pushed against the opening delimiter moved out in front of it, or the
    // run would be no run at all (`** words**` is literal asterisks — see
    // the mark-edge rule on `splice`).
    let src = "a **two words** c\n";
    let mut d = wysiwyg_doc("wys_word_del_partial", src);
    d.caret = src.find(" words").unwrap();
    d.delete_word_back();
    assert_eq!(d.source, "a  **words** c\n");
}

#[test]
fn source_view_word_motion_still_walks_the_markup() {
    // The other half of the decision: in the source view the `**` are
    // characters like any other — they're on the screen, so word motion has
    // to stop at them and a word-delete has to leave them behind. Only
    // WYSIWYG hides them, so only WYSIWYG steps over them.
    let g = |n, m, f: fn(&mut Doc)| golden(n, m, f);
    assert_eq!(
        g("src_word_motion", "a |**bold** c\n", |d| d
            .move_word_right(false)),
        "a **bold|** c\n"
    );
    // The same caret as the WYSIWYG reproduction, and the opposite outcome:
    // here "a ** c\n" is right, because `bold**` is what's to the left of it.
    assert_eq!(
        g("src_word_del", "a **bold**| c\n", |d| d.delete_word_back()),
        "a **| c\n"
    );
}

#[test]
fn every_wysiwyg_motion_lands_on_a_caret_stop() {
    // The single invariant both bugs violated: the caret draws and edits at
    // the same place only when it's on a stop. `debug_assert_on_a_stop`
    // makes the same claim in-place; this pins it from the outside, over a
    // document with every kind of thing the map has to be careful about.
    // At two widths: the wide one every other test builds at, where no
    // fixture folds, and one narrow enough that they all do. A soft wrap is
    // where an offset stops being on exactly one row, and testing only the
    // width that never wraps is how the caret came to be pinned at the first
    // one Down reached.
    let src = "# Title\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` c\n\n\
                   - item one\n\n| A | B |\n|---|---|\n| x | y |\n";
    // A table of named operations, which is what it looks like.
    #[allow(clippy::type_complexity)]
    let motions: [(&str, fn(&mut Doc)); 8] = [
        ("right", |d| d.move_right(false)),
        ("left", |d| d.move_left(false)),
        ("word_right", |d| d.move_word_right(false)),
        ("word_left", |d| d.move_word_left(false)),
        ("down", |d| d.move_down(false)),
        ("up", |d| d.move_up(false)),
        ("home", |d| d.move_home(false)),
        ("end", |d| d.move_end(false)),
    ];
    for width in [80, 12] {
        let mut d = wysiwyg_doc("stop_invariant", src);
        d.build_visual(width);
        let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
        assert!(stops.len() > 20, "fixture should have plenty of stops");
        for start in stops {
            for (name, motion) in &motions {
                d.caret = start;
                d.anchor = None;
                motion(&mut d);
                assert!(
                    d.vmap.is_stop(d.caret),
                    "{name} from {start} at width {width} landed at {} — not a caret stop",
                    d.caret
                );
            }
        }
    }
}

#[test]
fn no_wysiwyg_motion_is_a_dead_end() {
    // Down held to the bottom of a document reaches the bottom, and Up held
    // to the top reaches the top — from anywhere, at a width that wraps. The
    // invariant above says a motion lands somewhere legal; this one says it
    // gets somewhere at all, which is what a caret pinned at a wrap boundary
    // was quietly failing to do while every assertion around it held.
    let src = "# Title\n\none two three four five six seven eight nine ten\n\n\
                   - item one two three four five\n\nlast\n";
    for width in [80, 12] {
        let mut d = wysiwyg_doc("no_dead_end", src);
        d.build_visual(width);
        let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
        let (first, last) = (stops[0], stops[stops.len() - 1]);
        for &start in &stops {
            for (name, motion, want) in [
                (
                    "down",
                    (|d: &mut Doc| d.move_down(false)) as fn(&mut Doc),
                    last,
                ),
                ("up", |d: &mut Doc| d.move_up(false), first),
            ] {
                d.caret = start;
                d.anchor = None;
                d.goal_col = None;
                // Every row, plus the presses the edges take, plus slack.
                for _ in 0..d.vmap.num_rows() + 4 {
                    motion(&mut d);
                }
                assert_eq!(
                    d.caret, want,
                    "{name} held from {start} at width {width} never arrived"
                );
            }
        }
    }
}
