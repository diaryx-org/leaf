use super::*;

#[test]
fn incremental_build_matches_a_fresh_build_across_edits() {
    // Every `Doc` edit rebuilds through `build_spliced` (the single-block
    // fast path, gated on twig's `dirty_range`) or falls back to
    // `build_cached`. After each edit the map must be byte-identical to a
    // from-scratch build — this is the correctness net under the splice.
    let docs = [
        "# Title\n\nThe quick brown fox jumps.\n\nAnother paragraph here.\n\n- a\n- b\n",
        "para one\n\n> quote **bold** text\n> continued line\n\ntail paragraph\n",
        "alpha\n\nbeta\n\ngamma\n\ndelta\n\nepsilon\n\nzeta\n",
        // A footnote definition is a root beside `doc`, merged back into the
        // top-level list by `wysiwyg::top_blocks`. The random edits below
        // make and unmake definitions as they go (a deleted `:` turns one
        // back into a paragraph, and vice versa), which is exactly the
        // structural churn the splice path has to notice and bail out of.
        "text[^1] here\n\n[^1]: the note\n\nmore text[^b]\n\n[^b]: second\n",
        // A comment is a top-level block that draws no rows — a layout entry
        // at zero rows either side of blocks that do. The edits below type
        // into the blocks around it (a splice past a hidden block), and
        // break the comment open into prose and back (a structural change).
        "intro\n\n<!-- exec -->\n```\ncode\n```\n\nafter the comment\n\n<!-- trail -->\n",
        // Link reference definitions: a hidden block that an edit can turn
        // into a paragraph (a deleted `:`) and back, and whose own bytes an
        // edit can land in.
        "see [a] and [b]\n\n[a]: /a\n\nmid text\n\n[b]: /b\n",
        // Blank lines above the first block draw as rows, counted as its
        // separator; above it past frontmatter and past a comment too.
        "\n\nfirst pushed down\n\nsecond\n",
        "---\ntitle: x\n---\n\n\nafter frontmatter\n\nmore\n",
        "<!-- lead -->\n\n\nafter a comment\n\nmore\n",
        // No `<div>` here yet: an edit that takes the blank line under
        // `</div>` leaves a map no fresh build draws. See
        // docs/tasks/text-glued-under-a-closing-div.md.
    ];
    // A deterministic mix: mostly single characters (which stay inside one
    // block → splice), plus edits that reshape structure (a paragraph break,
    // a heading marker, a code fence → fallback), so both paths are exercised.
    let inserts = ["x", "y", "\n\n", "#", "`", " ", "z"];
    for src in docs {
        let mut d = wysiwyg_doc("diff", src);
        d.build_visual_unwrapped();
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "initial");

        for step in 0..60usize {
            let len = d.source.len();
            let raw = (step * 13 + 5) % (len + 1);
            let pos = (raw..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
            let pre = d.source.clone();
            let action;
            if step % 3 == 0 && pos < len {
                let end = (pos + 1..=len)
                    .find(|&i| d.source.is_char_boundary(i))
                    .unwrap();
                action = format!("delete [{pos},{end})");
                d.edit(pos, end, "");
            } else {
                let ins = inserts[step % inserts.len()];
                action = format!("insert {ins:?} @ {pos}");
                d.edit(pos, pos, ins);
            }
            d.build_visual_unwrapped();
            if maps_differ(&d.vmap, &reference_map(&d.source)) {
                panic!(
                    "FIRST MISMATCH at step {step}: {action}\n  pre  = {pre:?}\n  post = {:?}",
                    d.source
                );
            }
        }
    }
}

/// A frontend is handed [`Doc::vmap`] and may present it differently:
/// leaf-ratatui splices blank filler rows under an oversized heading so the
/// raster it paints there has somewhere to stand, and leaves them in the map
/// because the caret and the mouse both read it between frames. The splice
/// path addresses that map by *row index*, against the block layout the last
/// build recorded — so handed a map with rows in it that no block owns, it
/// laid the re-rendered block over one of the fillers and carried the rows
/// the block really occupied into the suffix. One stranded copy of the
/// edited line, and everything below it a row further down, per keystroke.
///
/// A map that isn't the one the layout describes is a map this path can't
/// patch, whoever changed it and for whatever reason. It rebuilds instead.
#[test]
fn an_edit_over_a_map_a_frontend_reshaped_rebuilds_it_whole() {
    let mut d = wysiwyg_doc("reshaped", "# Title\n\nThe quick brown fox jumps.\n");
    d.build_visual_unwrapped();

    // Stand in for the heading filler rows: two blank rows past the heading
    // that no block accounts for. Cloning a real row keeps every field
    // plausible — it is the row *count* the splice can't survive.
    let filler = d.vmap.rows[0].clone();
    d.vmap.rows.insert(1, filler.clone());
    d.vmap.rows.insert(1, filler);

    // An edit inside the last block: the single-block case the splice path
    // is for, and the one the frontend hits on every keystroke.
    let at = d.source.len() - 1;
    d.edit(at, at, "!");
    d.build_visual_unwrapped();

    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the edit");
}

/// A glyph's [`FaceId`] has to mean the same thing however its row was
/// built. A row comes three ways — a fresh walk, a [`BlockCache`] hit
/// cloned at a shifted offset, and a previous map's rows a splice kept
/// untouched — and only the first of those walks a `data-font` at all. An
/// index into a per-build table would have had the same glyph naming two
/// families the moment a second one appeared; the id is the name's own
/// hash, so nothing is remapped and the table is merged rather than rebuilt.
///
/// Two families, because one cannot tell a wrong id from a right one.
///
/// [`FaceId`]: crate::style::FaceId
/// [`BlockCache`]: crate::wysiwyg::BlockCache
#[test]
fn a_spliced_rebuild_still_says_which_family_each_glyph_is_set_in() {
    use crate::style::{FaceId, FaceRef};
    let garamond = FaceId::of("Garamond");
    let futura = FaceId::of("Futura");
    let mut d = wysiwyg_doc(
        "two_faces",
        "x <span data-font=\"Garamond\">alpha</span>\n\ny <span data-font=\"Futura\">beta</span>\n",
    );
    d.build_visual_unwrapped();

    // What the map has to keep saying, whichever path built it.
    let check = |d: &Doc, ctx: &str| {
        let face_of = |ch: char| {
            d.vmap
                .rows
                .iter()
                .flat_map(|r| r.glyphs.iter())
                .find(|g| g.ch == ch)
                .map(|g| g.style.font)
        };
        assert_eq!(face_of('a'), Some(Some(FaceRef::Named(garamond))), "{ctx}");
        assert_eq!(face_of('b'), Some(Some(FaceRef::Named(futura))), "{ctx}");
        assert_eq!(d.vmap.face_name(garamond), Some("Garamond"), "{ctx}");
        assert_eq!(d.vmap.face_name(futura), Some("Futura"), "{ctx}");
        assert_eq!(d.vmap.face_name(FaceId::of("Bodoni")), None, "{ctx}");
    };
    check(&d, "fresh");

    // An edit inside the second block: the single-block case the splice
    // path is for. The first block's rows are carried over untouched, so
    // its glyphs' ids are the previous build's and the table has to be too.
    let at = d.source.find("beta").unwrap();
    d.edit(at, at, "z");
    d.build_visual_unwrapped();
    check(&d, "after an edit in the second block");
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "spliced");

    // And the other way round, so the block that was kept is the one that
    // is now re-rendered.
    let at = d.source.find("alpha").unwrap();
    d.edit(at, at, "z");
    d.build_visual_unwrapped();
    check(&d, "after an edit in the first block");
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "spliced again");

    // A structural edit is one the splice bails out of, so the map is
    // reassembled by `build_cached` — where an untouched block is a *cache
    // hit* and its rows are cloned without a `data-font` being walked
    // again. The names the entry stored are what keeps the table honest
    // there.
    let at = d.source.find("\n\ny ").unwrap();
    d.edit(at, at, "\n\nmiddle");
    d.build_visual_unwrapped();
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "cached");
    // Twice, because that first `build_cached` is what stores the entries:
    // this one is the build where the Garamond block is a *hit*, its rows
    // cloned with their ids and no attribute walked to explain them.
    let at = d.source.len() - 1;
    d.edit(at, at, "\n\ntail");
    d.build_visual_unwrapped();
    check(&d, "after a structural edit, through the block cache");
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "cached again");

    // A family the edit took the last glyph of leaves the glyphs with no
    // face and the table with a name nothing asks for — harmless, and the
    // price of not walking the rows the splice exists to avoid walking.
    let span = d.source.find("<span data-font=\"Futura\">").unwrap();
    let end = d.source.rfind("</span>").unwrap() + "</span>".len();
    d.edit(span, end, "beta");
    d.build_visual_unwrapped();
    assert!(!d.source.contains("Futura"), "{:?}", d.source);
    wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "the face removed");
}

#[test]
fn incremental_build_matches_a_fresh_build_under_full_reveal() {
    // The same correctness net as `incremental_build_matches_a_fresh_build_
    // across_edits`, under `MarkupMode::Full` — where the map depends on
    // the caret's *line* as well as the text, so the two caches have a new
    // way to be wrong. Both are exercised: the block cache can hand back
    // rows built for a line that is no longer the revealed one, and the
    // splice path can reuse a suffix that still has yesterday's line raw.
    //
    // Caret motion is interleaved with the edits deliberately, because a
    // caret that only ever moved with the edit would never cross a line
    // without also dirtying it — the case where a stale reveal survives.
    let docs = [
        "# Title\n\n*one* and **two**\n\n[lk](http://x) and `code`\n\n- a *b*\n",
        "para *em* one\n\n> quote **bold** text\n\ntail ~~del~~ paragraph\n",
    ];
    let inserts = ["x", "*", "\n\n", "#", "`", " ", "_"];
    for src in docs {
        let mut d = wysiwyg_doc("reveal_diff", src);
        d.set_markup_mode(MarkupMode::Full);

        for step in 0..60usize {
            let len = d.source.len();
            let raw = (step * 13 + 5) % (len + 1);
            let pos = (raw..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
            let pre = d.source.clone();
            let action;
            if step % 3 == 0 && pos < len {
                let end = (pos + 1..=len)
                    .find(|&i| d.source.is_char_boundary(i))
                    .unwrap();
                action = format!("delete [{pos},{end})");
                d.edit(pos, end, "");
            } else {
                let ins = inserts[step % inserts.len()];
                action = format!("insert {ins:?} @ {pos}");
                d.edit(pos, pos, ins);
            }
            // Walk the caret somewhere else in the document, independently
            // of where the edit landed.
            let want = (step * 29 + 11) % (d.source.len() + 1);
            d.caret = (want..=d.source.len())
                .find(|&i| d.source.is_char_boundary(i))
                .unwrap();
            d.build_visual_unwrapped();

            let want = reference_map_revealing(&d.source, d.reveal_line());
            if maps_differ(&d.vmap, &want) {
                panic!(
                    "FIRST MISMATCH at step {step}: {action}, caret {}\n  pre  = {pre:?}\n  post = {:?}",
                    d.caret, d.source
                );
            }
        }
    }
}

#[test]
fn caret_motion_across_lines_rebuilds_only_under_full() {
    // The cache-key change has to earn its keep in both directions: `Full`
    // must rebuild when the caret changes line (or the reveal would never
    // move), and the hidden modes must *not* (or every arrow key would pay
    // for a feature they don't use). The existing `cache_motion` test pins
    // the second for the default mode; this pins the pair against a mode
    // change alone.
    let body = "*one* here\n\n*two* there\n";

    let mut full = doc_in(View::Wysiwyg, "motion_full", body);
    full.set_markup_mode(MarkupMode::Full);
    caret_at(&mut full, "one");
    let before = full.revision();
    caret_at(&mut full, "two");
    assert_eq!(full.revision(), before, "motion is not an edit");
    assert!(
        drawn_rows(&full).iter().any(|r| r == "*two* there"),
        "the map followed the caret: {:?}",
        drawn_rows(&full)
    );

    let mut hidden = doc_in(View::Wysiwyg, "motion_hidden", body);
    caret_at(&mut hidden, "one");
    let key = hidden.vmap_key.clone();
    caret_at(&mut hidden, "two");
    assert_eq!(
        hidden.vmap_key, key,
        "a hidden mode rebuilds nothing on motion"
    );
}

#[test]
fn wysiwyg_down_crosses_a_paragraph_boundary() {
    // Regression: the blank separator row used to share the previous
    // paragraph's end offset, so Down got pinned at the boundary (while Up
    // still crossed). Both directions must step through it symmetrically.
    //
    // It's now stepped *over* rather than onto: the blank line between two
    // paragraphs is the boundary being drawn, not a line of the document, so
    // one press of Down crosses it. The goal column survives the crossing —
    // col 3 at the end of "abc" is col 3 at the end of "def".
    let mut d = wysiwyg_doc("wys_down", "abc\n\ndef\n");
    d.caret = 3; // end of "abc" (row 0)
    d.move_down(false);
    assert_eq!(d.caret_pos().0, 2, "Down should reach the second paragraph");
    assert_eq!(d.caret, 8); // end of "def", col 3 kept
    d.move_up(false);
    assert_eq!(d.caret_pos().0, 0, "Up should come back symmetrically");
    assert_eq!(d.caret, 3);
}

#[test]
fn wysiwyg_up_and_down_are_inverse_across_paragraphs() {
    // The second Up and the second Down here run off the ends of the
    // document, which is no longer a place a press is swallowed: they carry
    // the caret to the start and the end of the text. The claim in the
    // middle — that a Down retraces the Up that crossed the paragraph gap —
    // is the one this test is for, and it is asserted where it is made.
    let mut d = wysiwyg_doc("wys_updown", "abc\n\ndef\n");
    d.caret = 5; // start of "def"
    let start = d.caret_pos();
    d.move_up(false);
    assert_eq!(d.caret_pos().0, 0, "Up reaches the first paragraph");
    d.move_up(false);
    assert_eq!(d.caret, 0, "a second Up runs on to the document's start");
    d.move_down(false);
    assert_eq!(d.caret_pos(), start, "Down retraces Up exactly");
    d.move_down(false);
    assert_eq!(d.caret, 8, "a second Down runs on to the document's end");
}

#[test]
fn wysiwyg_new_paragraph_shows_before_typing() {
    // Regression: two Enters at the end of a paragraph produced trailing
    // newlines with no AST node, so the caret appeared stuck on the old line
    // until a character was typed. It must ride down onto the new line now.
    let mut d = doc_with("wys_newpara", "abc\n");
    d.view = View::Wysiwyg;
    d.caret = 3;
    d.insert("\n");
    d.insert("\n"); // source is now "abc\n\n\n", caret at 5
    assert_eq!(d.source, "abc\n\n\n");
    d.build_visual(80);
    let (row, _) = d.caret_pos();
    assert!(
        row >= 2,
        "caret should have moved down to the new line, got row {row}"
    );
    assert!(
        d.vmap.num_rows() >= 3,
        "the blank lines should render as rows"
    );
}

#[test]
fn wysiwyg_enter_between_paragraphs_lands_on_an_empty_line() {
    // The reported bug: Enter at the end of a paragraph that has another
    // paragraph below put the caret at the *start of the next paragraph* —
    // the empty paragraph it opened had no row, so the caret snapped onto
    // "World". It must now sit on its own empty line, with a blank spacer
    // above it (the paragraph gap).
    let mut d = wysiwyg_doc("wys_gap_mid", "Hello\n\nWorld\n");
    d.caret = 5; // end of "Hello"
    d.newline();
    d.build_visual(80);
    let (row, col) = d.caret_pos();
    assert_eq!(col, 0, "caret should start an empty line, not sit in text");
    assert_eq!(
        d.vmap.row_width(row),
        0,
        "caret's row must be empty, not 'World'"
    );
    assert!(
        row >= 2,
        "a blank spacer row should sit above the caret, got row {row}"
    );
    // The row above the caret is a real (empty) gap, and "Hello" stays put.
    assert_eq!(
        d.vmap.row_width(row - 1),
        0,
        "the row above the caret is a gap"
    );
    let row0: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
    assert_eq!(row0, "Hello", "the paragraph above the caret must not move");
}

#[test]
fn wysiwyg_enter_at_eof_shows_a_gap_before_typing() {
    // At the document end a single Enter must also show the paragraph gap —
    // a blank spacer row above the caret — so the layout already matches how
    // it will look once the new paragraph has text.
    let mut d = wysiwyg_doc("wys_gap_eof", "Hello");
    d.caret = 5; // end of "Hello", no trailing newline
    d.newline(); // source becomes "Hello\n\n"
    d.build_visual(80);
    let (row, col) = d.caret_pos();
    assert_eq!(col, 0);
    assert!(
        row >= 2,
        "caret should sit below a blank spacer, got row {row}"
    );
    assert_eq!(
        d.vmap.row_width(row - 1),
        0,
        "the row above the caret is a gap"
    );
}

#[test]
fn wysiwyg_typing_after_enter_does_not_shift_the_caret_row() {
    // The spacer is view-only: typing the new paragraph must not reflow the
    // caret onto a different row — the transient view already matched the
    // settled one.
    let mut d = wysiwyg_doc("wys_no_reflow", "Hello\n\nWorld\n");
    d.caret = 5;
    d.newline();
    d.build_visual(80);
    let before = d.caret_pos();
    d.insert("New");
    d.build_visual(80);
    let after = d.caret_pos();
    assert_eq!(
        after.0, before.0,
        "typing must not move the caret to another row ({before:?} -> {after:?})"
    );
}

#[test]
fn wysiwyg_return_on_the_last_code_line_keeps_the_caret_in_the_block() {
    // Return at the end of the block's last line writes an empty line the
    // map used to drop, so the caret landed on `after` and the next
    // keystroke went into the paragraph below instead of into the code.
    let mut d = wysiwyg_doc("code_return", "prose\n\n```\nalpha\nbeta\n```\n\nafter\n");
    d.caret = d.source.find("beta").unwrap() + "beta".len();
    d.build_visual(80);
    let before = d.caret_pos().0;

    d.newline();
    d.build_visual(80);
    assert_eq!(d.source, "prose\n\n```\nalpha\nbeta\n\n```\n\nafter\n");

    let (row, col) = d.caret_pos();
    assert_eq!(row, before + 1, "the caret moves down one row");
    assert_eq!(col, 0, "onto the head of the empty line");
    let span = d.vmap.code_blocks[0].rows_span.clone();
    assert!(
        span.contains(&row),
        "caret row {row} is outside the block's rows {span:?}"
    );

    // The whole point: what is typed next is code.
    d.insert("gamma");
    assert_eq!(d.source, "prose\n\n```\nalpha\nbeta\ngamma\n```\n\nafter\n");
}

#[test]
fn wysiwyg_hides_frontmatter_from_the_caret_and_copy() {
    let fm = "---\ntitle: hi\n---\n";
    let body = format!("{fm}# leaf\n\nbody\n");
    let mut d = wysiwyg_doc("wys_fm", &body);
    // Opening lifts the caret out of the now-hidden frontmatter.
    assert_eq!(
        d.caret,
        fm.len(),
        "caret should start at the first real block"
    );
    // Left at the content start can't step back into frontmatter.
    d.move_left(false);
    assert_eq!(d.caret, fm.len(), "left must not enter frontmatter");
    // Doc-start lands on the content floor, not offset 0.
    d.move_doc_start(false);
    assert_eq!(d.caret, fm.len());
    // Select-all + copy never include the frontmatter bytes.
    d.select_all();
    let sel = d.selected_text().unwrap().to_string();
    assert!(!sel.contains("title"), "copy leaked frontmatter: {sel:?}");
    assert!(
        sel.starts_with("# leaf"),
        "selection should begin at content: {sel:?}"
    );
}

#[test]
fn typing_in_a_frontmatter_only_document_lands_after_the_frontmatter() {
    // A fresh note is frontmatter and nothing else. With no rendered block
    // to floor the caret it opened at offset 0 — before the opening `---` —
    // so the first keystroke wrote itself in front of the metadata and the
    // file came out as `This---\ntitle: …`.
    let fm = "---\ntitle: 2026-08-29\nid: f8s32cd\n---\n";
    let mut d = wysiwyg_doc("wys_fm_only", fm);
    assert_eq!(d.caret, fm.len(), "caret must open past the frontmatter");
    // Nothing is rendered, so the caret draws at the origin of an empty view
    // — the same place an empty document puts it.
    assert_eq!(d.caret_pos(), (0, 0));
    d.insert("This");
    assert_eq!(d.source, format!("{fm}This"));
}

/// `select_range` is the verb for a range a host already knows the bytes of,
/// so it must not snap — and must still hold every invariant `place_caret`
/// holds, the frontmatter floor above all.
#[test]
fn select_range_takes_the_range_as_given_but_still_floors_it() {
    let fm = "---\ntitle: foo\n---\n\n";
    let body = format!("{fm}body foo here\n");
    let mut d = wysiwyg_doc("wys_select_range", &body);

    // The `foo` in the body: taken exactly, not snapped to a caret stop.
    let at = body.rfind("foo").unwrap();
    d.select_range(at, at + 3);
    assert_eq!(d.selection(), Some((at, at + 3)));
    assert_eq!(d.selected_text(), Some("foo"));

    // The `foo` in the hidden frontmatter: below the floor, so both ends
    // come up to it rather than parking the caret in the metadata, where a
    // later keystroke would rewrite `title:`.
    let hidden = body.find("foo").unwrap();
    assert!(hidden < d.vmap.content_start);
    d.select_range(hidden, hidden + 3);
    assert!(
        d.caret >= d.vmap.content_start && d.anchor.unwrap() >= d.vmap.content_start,
        "a range under the floor must not leave the caret in the frontmatter"
    );

    // Past the end, and mid-character, are both brought back to something
    // sliceable rather than panicking the next reader of the range.
    let multi = wysiwyg_doc("wys_select_range_utf8", "héllo\n");
    let mut d = multi;
    d.select_range(2, 9_999);
    assert_eq!(d.caret, d.source.len());
    assert!(d.source.is_char_boundary(d.anchor.unwrap()));
    assert!(d.source.is_char_boundary(d.caret));
}

/// The bug `select_range` exists for: a match butting up against a hidden
/// delimiter. `place_caret` snaps to the nearest *visible* stop, which is
/// the one before the `**`.
#[test]
fn select_range_does_not_snap_off_a_hidden_delimiter() {
    let mut d = wysiwyg_doc("wys_select_range_bold", "a **needle** in it\n");
    let at = d.source.find("needle").unwrap();
    d.select_range(at, at + 6);
    assert_eq!(d.selected_text(), Some("needle"), "not \"needl\"");
}

#[test]
fn wysiwyg_backspace_at_content_start_leaves_frontmatter_intact() {
    // Backspace deletes `prev_boundary..caret` directly; at the first real
    // block that boundary is inside the hidden frontmatter, so it must be a
    // no-op rather than eating the closing `---`.
    let fm = "---\ntitle: hi\n---\n";
    let body = format!("{fm}leaf\n");
    let mut d = wysiwyg_doc("wys_fm_bs", &body);
    assert_eq!(d.caret, fm.len());
    d.backspace();
    assert_eq!(d.source, body, "backspace must not touch frontmatter");
    d.delete_word_back();
    assert_eq!(
        d.source, body,
        "word-delete must not touch frontmatter either"
    );
}

#[test]
fn wysiwyg_edits_inside_a_vis_directive_block_without_disturbing_its_fences() {
    // diaryx's `:::vis{.audience}` visibility block — any `:::name{.class}`
    // fenced div, really, since core parses these on for every document
    // now (`parse_extensions`). The container is a `directive` node, an
    // `is_block_container` kind like `block_quote`, so the caret works
    // inside its child paragraph exactly as it would inside a quote: typing
    // edits the paragraph, and the `:::vis{...}` / `:::` fences round-trip
    // untouched.
    let body = ":::vis{.public .family}\nhello\n:::\nafter\n";
    let mut d = wysiwyg_doc("wys_vis", body);
    d.caret = body.find("hello").unwrap() + "hello".len();
    d.insert("!");
    assert_eq!(
        d.source, ":::vis{.public .family}\nhello!\n:::\nafter\n",
        "typing inside the block edits its content in place"
    );
    assert!(
        d.source.contains(":::vis{.public .family}"),
        "opening fence survives"
    );
    assert!(d.source.contains(":::\nafter"), "closing fence survives");
}

#[test]
fn source_view_still_reaches_frontmatter() {
    // The metadata is only *hidden*, never lost: the source view edits and
    // selects it in full, and it's always preserved on save.
    let fm = "---\ntitle: hi\n---\n";
    let body = format!("{fm}# leaf\n");
    let mut d = doc_with("src_fm", &body);
    d.select_all();
    let sel = d.selected_text().unwrap();
    assert!(
        sel.contains("title"),
        "source view should select everything"
    );
    d.move_doc_start(false);
    assert_eq!(d.caret, 0, "source view can reach offset 0");
}
