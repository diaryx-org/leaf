use super::*;

#[test]
fn a_code_block_maps_each_line_to_its_own_source_text() {
    // Every glyph used to point at the block's start, which made the whole
    // block one offset — visible, but impossible to put a caret inside.
    let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
    let m = map(src);
    for row in &m.rows {
        for g in row.glyphs.iter().filter(|g| g.stop) {
            assert_eq!(
                src[g.src..].chars().next(),
                Some(g.ch),
                "glyph {:?} at {} isn't the source byte it claims",
                g.ch,
                g.src
            );
        }
    }
}

#[test]
fn an_indented_code_block_maps_past_its_stripped_indent() {
    // twig strips the four-space indent, so `text` isn't a source slice and
    // the lines have to be re-found. Offsets land on the code, not the indent.
    let src = "    indented\n    code\n";
    let m = map(src);
    let stops: Vec<(char, usize)> = m
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter().filter(|g| g.stop).map(|g| (g.ch, g.src)))
        .collect();
    assert_eq!(
        stops[0],
        ('i', 4),
        "first line should start past the indent"
    );
    assert!(
        stops.contains(&('c', 17)),
        "second line misplaced: {stops:?}"
    );
}

#[test]
fn a_fenced_block_whose_code_echoes_its_info_string_maps_to_the_code() {
    // The one case that defeats a forward search: the opening fence
    // ```` ```rust ```` ends with the same text as the code under it.
    let src = "```rust\nrust\n```\n";
    let m = map(src);
    let first = m.rows[0].glyphs.iter().find(|g| g.stop).unwrap();
    assert_eq!(first.src, 8, "matched the info string, not the code");
}

#[test]
fn a_code_block_carries_no_gutter_and_is_published_as_a_row_span() {
    // The old `▏ ` gutter is gone: a code row is the block prefix (none, at
    // the top level) plus the code text, and the whole run is named in
    // `code_blocks` so a frontend can box it.
    let src = "para\n\n```\ncode\nlines\n```\n\nafter\n";
    let m = map(src);
    assert_eq!(m.code_blocks.len(), 1, "one code block");
    let span = m.code_blocks[0].rows_span.clone();
    let rows: Vec<String> = m.rows[span.clone()]
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert_eq!(rows, vec!["code".to_string(), "lines".to_string()]);
    assert!(!rendered(&m).contains('▏'), "gutter still drawn");
    assert!(
        m.rows[span].iter().all(|r| r.code),
        "every row in the span is flagged code"
    );
}

#[test]
fn an_empty_last_line_in_a_code_block_is_a_row_of_its_own() {
    // `trim_end_matches('\n')` cut the block's terminator *and* the newline
    // that spells a trailing empty line, so the row the Return had just made
    // never appeared and the caret on it fell through to the block below.
    // Every empty line is a row, wherever in the block it falls.
    let src = "prose\n\n```\nalpha\nbeta\n\n```\n\nafter\n";
    let m = map(src);
    let span = m.code_blocks[0].rows_span.clone();
    let rows: Vec<String> = m.rows[span.clone()]
        .iter()
        .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
        .collect();
    assert_eq!(
        rows,
        vec!["alpha".to_string(), "beta".to_string(), String::new()],
        "the empty last line gets a row"
    );
    assert!(
        m.rows[span.clone()].iter().all(|r| r.code),
        "the empty row is flagged code like the rest of the block"
    );
    // And it is the *source's* empty line, not a coarse fallback to the
    // block start: the offset the caret resolves to is the one Return made.
    let empty = span.end - 1;
    assert_eq!(
        m.rows[empty].end_src,
        src.find("beta\n\n").unwrap() + "beta\n".len(),
        "the empty row maps to the line the Return opened"
    );

    // Nothing is invented where there is no empty line, and a second one is
    // a second row.
    assert_eq!(
        map("```\nalpha\nbeta\n```\n").code_blocks[0]
            .rows_span
            .len(),
        2,
        "a block that ends at its last code line keeps two rows"
    );
    assert_eq!(
        map("```\nalpha\n\n\n```\n").code_blocks[0].rows_span.len(),
        3,
        "two trailing empty lines are two rows"
    );
}

#[test]
fn a_code_block_leaves_exactly_one_blank_row_below_it() {
    // The closing fence line used to be miscounted as a blank separator,
    // opening a phantom second gap under the block. One block boundary is
    // one blank row, code block or not.
    let src = "para\n\n```\ncode\n```\n\nafter\n";
    let m = map(src);
    let code_end = m.code_blocks[0].rows_span.end;
    let after = m
        .rows
        .iter()
        .position(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>() == "after")
        .unwrap();
    assert_eq!(
        after - code_end,
        1,
        "exactly one row between code and 'after'"
    );
}

#[test]
fn a_fenced_block_publishes_its_language_on_its_code_block() {
    // The info string becomes the block's label; a bare fence and an indented
    // block carry none.
    assert_eq!(
        map("```rust\nlet x = 1;\n```\n").code_blocks[0]
            .lang
            .as_deref(),
        Some("rust")
    );
    assert_eq!(map("```\nplain\n```\n").code_blocks[0].lang, None);
    assert_eq!(map("    indented\n").code_blocks[0].lang, None);
}

fn lang_of(src: &str) -> Option<String> {
    map(src).code_blocks[0].lang.clone()
}

#[test]
fn a_fence_in_a_quote_has_its_language() {
    // The block's span starts at the line, `> ` and all; the fence is past
    // the quote marker, and so is its info string.
    let src = "> ```rust\n> let x = 1;\n> ```\n";
    assert_eq!(lang_of(src).as_deref(), Some("rust"));
    let info = code_info_span(src, 0).unwrap();
    assert_eq!(&src[info], "rust", "the info string's exact bytes");
    assert_eq!(
        lang_of("> > ~~~ py\n> > x\n> > ~~~\n").as_deref(),
        Some("py")
    );
}

#[test]
fn a_fence_in_a_list_item_has_its_language() {
    // On the item's own line, past its marker…
    assert_eq!(
        lang_of("- ```rust\n  let x = 1;\n  ```\n").as_deref(),
        Some("rust")
    );
    assert_eq!(
        lang_of("10. ```rust\n    let x = 1;\n    ```\n").as_deref(),
        Some("rust")
    );
    // …and on a line of its own inside the item, where the indent is the
    // item's content column and not the fence's.
    assert_eq!(
        lang_of("10. a\n\n    ```rust\n    let x = 1;\n    ```\n").as_deref(),
        Some("rust")
    );
    assert_eq!(
        lang_of("- a\n  - b\n\n    ```rust\n    x\n    ```\n").as_deref(),
        Some("rust")
    );
    // In a list in a quote.
    assert_eq!(
        lang_of("> - a\n>\n>   ```rust\n>   x\n>   ```\n").as_deref(),
        Some("rust")
    );
}

#[test]
fn an_indented_block_in_a_list_item_still_has_no_language() {
    // Four spaces past the item's content column is an indented code
    // block, whatever its text looks like — the allowance is measured from
    // the item, not dropped.
    let src = "- a\n\n      ```rust\n";
    assert_eq!(map(src).code_blocks.len(), 1);
    assert_eq!(lang_of(src), None);
    assert_eq!(lang_of("- a\n\n      indented\n"), None);
}

/// The token every glyph spelling `ch` carries, in row order — how a test
/// reads a block's highlighting off the map.
fn tokens_of(m: &VisualMap, ch: char) -> Vec<Option<Token>> {
    m.rows
        .iter()
        .flat_map(|r| r.glyphs.iter())
        .filter(|g| g.ch == ch)
        .map(|g| g.style.token)
        .collect()
}

#[cfg(feature = "syntax")]
#[test]
fn a_fenced_block_in_a_known_language_carries_tokens() {
    // `let` is a keyword, the string literal a string, and the plain
    // identifier `x` nothing at all — it draws in the code colour. Every
    // glyph is still `Role::Code`: a token is beside the role, not instead.
    let m = map("```rust\nlet x = \"s\";\n```\n");
    assert_eq!(tokens_of(&m, 'l'), vec![Some(Token::Keyword)]);
    assert_eq!(tokens_of(&m, 'x'), vec![None]);
    assert_eq!(tokens_of(&m, '"'), vec![Some(Token::String); 2]);
    assert!(
        m.rows
            .iter()
            .filter(|r| r.code)
            .flat_map(|r| r.glyphs.iter())
            .all(|g| g.style.role == Role::Code),
        "a token replaced the code role"
    );
}

#[cfg(feature = "syntax")]
#[test]
fn a_token_changes_nothing_about_where_a_glyph_is() {
    // The same block with and without a language it can be highlighted in
    // lays out identically: same rows, same offsets, same stops. Only the
    // token differs, so the caret walks a highlighted block as it walked an
    // unhighlighted one.
    let hl = map("```rust\nlet x = 1; // c\nfn f() {}\n```\n");
    let plain = map("```text\nlet x = 1; // c\nfn f() {}\n```\n");
    assert_eq!(hl.rows.len(), plain.rows.len());
    for (a, b) in hl.rows.iter().zip(&plain.rows) {
        assert_eq!(a.end_src, b.end_src);
        assert_eq!(a.glyphs.len(), b.glyphs.len());
        for (ga, gb) in a.glyphs.iter().zip(&b.glyphs) {
            assert_eq!((ga.ch, ga.src, ga.stop), (gb.ch, gb.src, gb.stop));
            assert_eq!(ga.style.token(None), gb.style);
        }
    }
    assert!(tokens_of(&hl, 'l').iter().any(Option::is_some));
    assert!(tokens_of(&plain, 'l').iter().all(Option::is_none));
}

#[test]
fn a_block_with_no_language_to_highlight_in_carries_no_tokens() {
    // A bare fence, an indented block, a fence in a language no grammar
    // covers, and inline code all draw as plain code — and so does a
    // `rust` fence when the `syntax` feature is off.
    for src in [
        "```\nlet x = 1;\n```\n",
        "    let x = 1;\n",
        "```no-such-language\nlet x = 1;\n```\n",
        "a `let x` b\n",
    ] {
        assert!(
            tokens_of(&map(src), 'l').iter().all(Option::is_none),
            "{src:?} was highlighted"
        );
    }
    #[cfg(not(feature = "syntax"))]
    assert!(
        tokens_of(&map("```rust\nlet x = 1;\n```\n"), 'l')
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn inline_code_is_not_a_code_block() {
    // A `code` span inside prose is styled by role, not boxed: it's part of a
    // normal paragraph row, so it names no `code_blocks` entry.
    let m = map("a `snippet` b\n");
    assert!(m.code_blocks.is_empty(), "inline code wrongly boxed");
    assert!(
        m.rows.iter().all(|r| !r.code),
        "inline code flagged a code row"
    );
}
