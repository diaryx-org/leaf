use super::*;

#[test]
fn blockquote_after_a_list_is_not_bulleted() {
    // twig nests a following top-level block quote under the `bullet_list`
    // (a direct child, not a `list_item`). The map must render it de-nested —
    // `│ quote`, never `• │ quote` — with a blank separator, like any block
    // that follows a list. Regression for the "combined list + blockquote" bug.
    let mut d = doc_in(View::Wysiwyg, "bq_after_list", "- item\n\n> quote\n");
    d.build_visual(80);
    let rows: Vec<String> = d
        .vmap
        .rows
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert!(
        rows.iter().any(|r| r == "│ quote"),
        "block quote should render on its own gutter, got rows: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains('•') && r.contains('│')),
        "no row should carry both a bullet and a quote gutter, got rows: {rows:?}"
    );
}

// ── block containers (quote / list) ──────────────────────────────────────

#[test]
fn toggle_blockquote_wraps_the_block_at_the_caret_and_reverses() {
    let g = |m, f: fn(&mut Doc)| golden("quote", m, f);
    assert_eq!(g("hel|lo\n", |d| d.toggle_blockquote()), "> hel|lo\n");
    assert_eq!(g("> hel|lo\n", |d| d.toggle_blockquote()), "hel|lo\n");
    // A caret at a line end sits at the doc level; the block is still found.
    assert_eq!(g("hello|\n", |d| d.toggle_blockquote()), "> hello|\n");
}

#[test]
fn toggle_blockquote_keeps_the_caret_in_a_hard_wrapped_paragraph() {
    // Every source line of the paragraph gets its own `> `, so a caret left
    // on its old byte offset falls one prefix per line above it too far
    // back — inside the markup it just asked for rather than in its word.
    assert_eq!(
        golden("quote_wrap", "aaa\nb|bb\nccc\n", |d| d.toggle_blockquote()),
        "> aaa\n> b|bb\n> ccc\n"
    );
}

#[test]
fn toggle_blockquote_works_in_wysiwyg_view() {
    let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
    assert_eq!(
        g("q_wys", "hel|lo\n", |d| d.toggle_blockquote()),
        "> hel|lo\n"
    );
    assert_eq!(
        g("q_wys2", "> hel|lo\n", |d| d.toggle_blockquote()),
        "hel|lo\n"
    );
}

#[test]
fn toggle_list_makes_a_list_and_converts_between_the_kinds() {
    let g = |m, f: fn(&mut Doc)| golden("list", m, f);
    assert_eq!(g("hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
    assert_eq!(g("hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
    // The *other* kind converts in place instead of nesting, which is what
    // makes the two buttons one three-state control.
    assert_eq!(g("- hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
    assert_eq!(g("1. hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
    // Its own kind, over the only item the list holds, takes it off.
    assert_eq!(g("- hel|lo\n", |d| d.toggle_list(false)), "hel|lo\n");
}

#[test]
fn toggle_list_works_in_wysiwyg_view() {
    let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
    assert_eq!(
        g("l_wys", "hel|lo\n", |d| d.toggle_list(true)),
        "1. hel|lo\n"
    );
    assert_eq!(
        g("l_wys2", "1. hel|lo\n", |d| d.toggle_list(false)),
        "- hel|lo\n"
    );
    assert_eq!(
        g("l_wys3", "- hel|lo\n", |d| d.toggle_list(false)),
        "hel|lo\n"
    );
}

#[test]
fn a_list_over_a_selection_numbers_each_block_and_stays_selected() {
    // The selection has to grow with the markup: twig takes a container off
    // only a range covering every block it holds, so the second press can
    // reverse the first only if the result is what's selected.
    let mut d = doc_with("list_sel", "abc\n\ndef\n");
    d.select_all();
    d.toggle_list(true);
    assert_eq!(d.source, "1. abc\n\n2. def\n");
    assert_eq!(d.selection(), Some((0, d.source.len())));
    d.toggle_list(true);
    assert_eq!(d.source, "abc\n\ndef\n");
}

#[test]
fn toggle_blockquote_nests_a_partly_covered_quote() {
    // twig's rule: covering only some of a container's blocks nests, because
    // taking the quote off would drag its uncovered siblings out with it.
    let mut d = doc_with("quote_nest", "> a\n>\n> b\n");
    d.caret = 2; // in the first quoted paragraph only
    d.toggle_blockquote();
    assert_eq!(d.source, "> > a\n>\n> b\n");
}

#[test]
fn a_container_toggle_opens_an_empty_one_on_a_blank_line() {
    // A blank line used to be no block for twig to wrap —
    // `toggle_block_container` answered `NotFound` — so Quote and the list
    // buttons did nothing on the very line the H1 button works on, and leaf
    // lent twig a scratch paragraph to wrap and took it back out again.
    // twig 3.2.0 opens an empty container there itself, so what is left here
    // is where the caret lands: inside the marker that was just written.
    let mut d = doc_with("quote_blank", "\nabc\n");
    d.caret = 0;
    d.toggle_blockquote();
    assert_eq!(d.source, "> \nabc\n");
    assert_eq!(
        d.caret, 2,
        "the caret belongs inside the quote it just opened"
    );
    assert!(d.status.is_none(), "{:?}", d.status);
    assert!(d.dirty);

    // And the paragraph below is still its own block: an empty container one
    // soft break from `abc` would take that paragraph into the quote with it.
    let mut d = wysiwyg_doc("quote_blank_rows", "\nabc\n");
    d.caret = 0;
    d.toggle_blockquote();
    d.build_visual(80);
    assert_eq!(drawn_rows(&d), ["│ ", "", "abc"]);

    // The same from the other side: a blank line directly under a paragraph
    // earns the blank line an empty block needs, rather than being read as a
    // soft break inside that paragraph.
    let mut d = doc_with("list_blank_below", "abc\n");
    d.caret = 4;
    d.toggle_list(false);
    assert_eq!(d.source, "abc\n\n- ");
    assert_eq!(d.caret, 7);
}

#[test]
fn enter_at_the_end_of_a_quote_stays_in_the_quote() {
    // The gesture the rendering fix is for. `newline` inside a quote already
    // wrote the right source — `> a\n` becomes `> a\n>\n> \n`, twig's own
    // spelling — but the two marker lines it adds belonged to no node until
    // twig 3.2.0, so the gutter stopped at `a` and the line the writer had
    // just made drew as plain prose under the quote.
    let mut d = wysiwyg_doc("quote_enter", "> a\n");
    d.caret = 3; // past `a`, at the end of the quoted line
    d.newline();
    assert_eq!(d.source, "> a\n>\n> \n");
    d.build_visual(80);
    assert_eq!(drawn_rows(&d), ["│ a", "│ ", "│ "]);
    // And the caret is on the new line, not stranded on the old one.
    assert_eq!(d.caret, 8);
}

#[test]
fn a_second_enter_at_the_end_of_a_quote_leaves_it() {
    // Double-Enter, as it leaves a list. The second Enter used to split the
    // empty quoted line like any other, so the writer pressing it to get
    // out watched blank lines pile up under the quote with the caret still
    // in it — and the empty quoted line and its separator stayed behind.
    let mut d = wysiwyg_doc("quote_exit", "> hello\n");
    d.caret = 7;
    d.newline();
    assert_eq!(d.source, "> hello\n>\n> \n");
    d.newline();
    assert_eq!(d.source, "> hello\n\n\n");
    d.insert("x");
    assert_eq!(d.source, "> hello\n\nx\n");
    let quotes = d
        .nodes()
        .iter()
        .filter(|n| n.kind == Kind::BlockQuote)
        .count();
    assert_eq!(quotes, 1, "the quote keeps only `hello`");

    // Out of the inner quote only, into a paragraph of the outer one.
    let mut d = wysiwyg_doc("quote_exit_nested", "> > a\n");
    d.caret = 5;
    d.newline();
    d.newline();
    assert_eq!(d.source, "> > a\n>\n> \n");

    // Above more of the document, the quote's end is still its end.
    let mut d = wysiwyg_doc("quote_exit_mid", "> a\n\nnext\n");
    d.caret = 3;
    d.newline();
    d.newline();
    d.insert("x");
    assert_eq!(d.source, "> a\n\nx\n\nnext\n");

    // And one undo puts the empty quoted line back.
    let mut d = wysiwyg_doc("quote_exit_undo", "> a\n");
    d.caret = 3;
    d.newline();
    d.newline();
    d.undo();
    assert_eq!(d.source, "> a\n>\n> \n");
}

#[test]
fn the_line_a_quote_is_left_on_is_spaced_as_the_paragraph_it_becomes() {
    // Above a heading, the gap over the empty line was labelled by the
    // heading beyond it, which a frontend spaces wider: the line sat low,
    // and jumped up as soon as a character made it a paragraph.
    let boundaries = |d: &mut Doc| {
        d.build_visual(80);
        d.vmap.rows.iter().map(|r| r.boundary).collect::<Vec<_>>()
    };
    let mut d = wysiwyg_doc("quote_exit_gap", "> a\n\n## Math\n");
    d.caret = 3;
    d.newline();
    d.newline();
    let empty = boundaries(&mut d);
    d.insert("hello");
    assert_eq!(boundaries(&mut d), empty);
}

#[test]
fn undoing_enter_in_a_quote_puts_the_caret_back_where_it_was() {
    // Enter in a quote or a list goes to twig's `split_block`, which used to
    // be handed no pre-edit caret. Undo restored whatever the last edit had
    // filed — here the line under the quote, a gap the view moved the caret
    // off and onto the heading below.
    for src in ["> a\n\n## Math\n", "- a\n\n## Math\n"] {
        let mut d = wysiwyg_doc("split_undo_caret", src);
        d.caret = 3;
        d.newline();
        assert_ne!(d.source, src);
        d.undo();
        assert_eq!(d.source, src);
        assert_eq!(d.caret, 3, "{src:?}");
    }
}

#[test]
fn opening_a_container_on_a_blank_line_is_one_undo_step() {
    // It was three edits — scratch, wrap, unscratch — coalesced into one, and
    // now it is twig's single edit. Either way one ⌘z has to put the blank
    // line back rather than undoing into a half-built document.
    for open in [
        &(|d: &mut Doc| d.toggle_blockquote()) as &dyn Fn(&mut Doc),
        &|d: &mut Doc| d.toggle_list(false),
        &|d: &mut Doc| d.toggle_list(true),
    ] {
        let mut d = doc_with("container_blank_undo", "a\n\n\n\nb\n");
        d.caret = 3;
        open(&mut d);
        assert_ne!(d.source, "a\n\n\n\nb\n");
        d.undo();
        assert_eq!(d.source, "a\n\n\n\nb\n");
    }
}

#[test]
fn a_container_toggle_is_one_undo_step() {
    let mut d = doc_with("quote_undo", "hello\n");
    d.caret = 3;
    d.insert("X"); // a typing run the structural edit must not fold into
    d.toggle_blockquote();
    assert_eq!(d.source, "> helXlo\n");
    d.undo();
    assert_eq!(d.source, "helXlo\n");
}
