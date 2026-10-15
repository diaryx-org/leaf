use super::*;

// ── blank documents ───────────────────────────────────────────────────────

#[test]
fn a_blank_document_is_untitled_empty_and_markdown() {
    let mut d = Doc::blank().unwrap();
    assert!(d.is_untitled());
    assert_eq!(d.path, PathBuf::new());
    assert_eq!(
        d.file_name(),
        "untitled",
        "the header has to show something"
    );
    assert_eq!(d.format_name(), "markdown");
    assert_eq!(d.source, "");
    assert!(!d.dirty, "nothing typed yet is nothing to lose");
    assert_eq!(d.disk_state(), DiskState::Untitled);
    // And it's a document you can be in: the default view renders it.
    d.build_visual(80);
    assert_eq!(d.caret, 0);
}

#[test]
fn saving_an_untitled_document_asks_for_a_name_instead_of_writing() {
    let mut d = Doc::blank().unwrap();
    d.insert("hello");
    assert!(d.dirty);
    d.save();
    assert_eq!(d.status.as_deref(), Some("untitled — save as…"));
    assert!(d.dirty, "it must not come away believing it saved");
    assert!(d.is_untitled(), "and it still has no file");
}

#[test]
fn a_blank_document_becomes_a_real_one_at_the_first_save_as() {
    let p = temp_path("blank_save_as");
    let mut d = Doc::blank().unwrap();
    // Plain text — a blank doc opens in Hidden mode, where a typed `#` would
    // be kept literal (`\#`); this test is about save-as, not escaping (which
    // has its own test), so it types nothing that escaping would touch.
    d.insert("hi");
    d.save_as(p.clone());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi");
    assert!(!d.is_untitled());
    assert!(!d.dirty);
    assert_eq!(d.file_name(), p.file_name().unwrap().to_string_lossy());
    assert_eq!(
        d.disk_state(),
        DiskState::Unchanged,
        "the watermark is stamped"
    );
    // And ⌘S is a plain save from here on.
    d.insert("!");
    d.save();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi!");
    let _ = std::fs::remove_file(&p);
}

// ── a file that isn't there yet ───────────────────────────────────────────

/// A unique path in the temp dir with the given extension, guaranteed not to
/// exist — what `leaf notes.md` is handed when the file has never been made.
fn missing_path(name: &str, ext: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("leaf_test_new_{name}_{seq}.{ext}"));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn a_file_that_doesnt_exist_opens_as_an_empty_named_document() {
    let p = missing_path("named", "md");
    let mut d = Doc::open_or_create(p.clone()).unwrap();

    assert_eq!(d.source, "", "nothing was read, so there's nothing in it");
    assert!(!d.dirty, "an untouched new buffer has nothing to lose");
    assert!(
        !d.is_untitled(),
        "it has the name the user asked for — ^S must not detour to Save As"
    );
    assert_eq!(d.file_name(), p.file_name().unwrap().to_str().unwrap());
    assert!(d.path.is_absolute(), "the same absolute path `open` stores");
    assert!(!p.exists(), "and opening it wrote nothing");
    // And it's a document you can be in.
    d.build_visual(80);
    assert_eq!(d.caret, 0);
}

#[test]
fn a_new_file_is_created_by_its_first_save() {
    let p = missing_path("first_save", "md");
    let mut d = Doc::open_or_create(p.clone()).unwrap();
    d.insert("hello\n");
    assert!(d.dirty);
    d.save();

    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "hello\n",
        "a plain ^S wrote it — no Save As, no name to invent"
    );
    assert!(!d.dirty);
    assert_eq!(d.disk_state(), DiskState::Unchanged);
    let _ = std::fs::remove_file(&p);
}

#[test]
fn a_new_file_takes_its_format_from_the_extension() {
    // The one thing `blank` can't do: with no name it has to assume Markdown,
    // and typing djot into a Markdown parse is the wrong buffer.
    let dj = missing_path("format", "dj");
    assert_eq!(Doc::open_or_create(dj).unwrap().format_name(), "djot");
    let md = missing_path("format", "md");
    assert_eq!(Doc::open_or_create(md).unwrap().format_name(), "markdown");
}

#[test]
fn a_new_file_reports_itself_missing_until_it_is_saved() {
    // Not `Untitled` — that's the answer for a document with no path, and it
    // would tell a frontend there is nothing a save could collide with. Here
    // there is a path, and the file simply isn't at it yet.
    let p = missing_path("disk_state", "md");
    let mut d = Doc::open_or_create(p.clone()).unwrap();
    assert_eq!(d.disk_state(), DiskState::Missing);

    // Somebody else creates it while the buffer is open: that's an overwrite
    // the frontend has to be able to prompt about, exactly as for an opened
    // file. Their bytes, not ours, so `Changed`.
    std::fs::write(&p, "theirs\n").unwrap();
    assert_eq!(d.disk_state(), DiskState::Changed);

    // Saving makes the file ours and re-stamps the watermark.
    d.insert("ours\n");
    d.save();
    assert_eq!(d.disk_state(), DiskState::Unchanged);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "ours\n");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn open_or_create_still_opens_a_file_that_is_there() {
    let d = doc_with("open_or_create_existing", "body\n");
    let reopened = Doc::open_or_create(d.path.clone()).unwrap();
    assert_eq!(reopened.source, "body\n");
    assert_eq!(reopened.disk_state(), DiskState::Unchanged);
}

#[test]
fn a_missing_file_with_no_readable_extension_is_still_an_error() {
    // A mistyped flag or a stray argument must not become a buffer promising
    // to save somewhere — the same refusal `open` gives a real file.
    let mut p = std::env::temp_dir();
    p.push("leaf_test_new_bad_ext.wat");
    assert!(Doc::open_or_create(p).is_err());
    let mut none = std::env::temp_dir();
    none.push("leaf_test_new_no_ext");
    assert!(Doc::open_or_create(none).is_err());
}

#[test]
fn a_new_file_in_a_directory_that_doesnt_exist_opens_but_wont_save() {
    // Opening reads nothing, so there is nothing to fail on yet; the write is
    // where it fails, and it says so rather than claiming a save.
    let p = std::env::temp_dir().join("leaf_test_no_such_dir_c41/doc.md");
    let mut d = Doc::open_or_create(p).unwrap();
    d.insert("x");
    d.save();
    assert!(
        d.status.as_deref().unwrap().starts_with("save failed:"),
        "got {:?}",
        d.status
    );
    assert!(d.dirty, "it must not come away believing it saved");
}

// ── save as ───────────────────────────────────────────────────────────────

/// A unique path in the temp dir that no fixture wrote — a Save As target.
fn temp_path(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("leaf_test_target_{name}_{seq}.md"));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn save_as_moves_the_document_and_leaves_the_old_file_alone() {
    let mut d = doc_with("save_as_move", "original\n");
    let old = d.path.clone();
    let new = temp_path("save_as_move");
    d.insert("edited: ");
    d.save_as(new.clone());

    assert_eq!(std::fs::read_to_string(&new).unwrap(), "edited: original\n");
    assert_eq!(
        std::fs::read_to_string(&old).unwrap(),
        "original\n",
        "Save As doesn't touch the file it came from"
    );
    assert_eq!(d.path, new, "the document moved");
    assert!(!d.dirty);
    assert_eq!(
        d.status.as_deref(),
        Some(&*format!("saved {}", d.file_name()))
    );

    // Every later save follows it, which is the whole difference from a copy.
    d.caret = 0;
    d.insert("re-");
    d.save();
    assert_eq!(
        std::fs::read_to_string(&new).unwrap(),
        "re-edited: original\n"
    );
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "original\n");
    let _ = std::fs::remove_file(&new);
}

#[test]
fn save_as_overwrites_an_existing_target() {
    // The picker already asked; asking again down here is the same question
    // twice, and the second one has no way to be answered.
    let new = temp_path("save_as_over");
    std::fs::write(&new, "theirs\n").unwrap();
    let mut d = doc_with("save_as_over", "ours\n");
    d.save_as(new.clone());
    assert_eq!(std::fs::read_to_string(&new).unwrap(), "ours\n");
    let _ = std::fs::remove_file(&new);
}

#[test]
fn a_save_as_that_fails_leaves_the_document_where_it_was() {
    let mut d = doc_with("save_as_fail", "body\n");
    let old = d.path.clone();
    d.insert("x");
    // A directory that doesn't exist: the write can't land.
    let bad = std::env::temp_dir().join("leaf_test_no_such_dir_9f2/doc.md");
    d.save_as(bad);

    assert_eq!(
        d.path, old,
        "the document must not move to a file that isn't there"
    );
    assert!(d.dirty, "and must not believe it saved");
    assert!(
        d.status.as_deref().unwrap().starts_with("save failed:"),
        "the same failure a plain save reports, got {:?}",
        d.status
    );
    // The original is still the document's file, and still saveable.
    d.save();
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "xbody\n");
    assert!(!d.dirty);
}

#[test]
fn save_as_renames_without_reparsing_the_format() {
    // `.dj` on the name doesn't make the buffer djot: it was parsed as
    // Markdown and still is, and saying otherwise would be a conversion the
    // user never asked for (and an undo history thrown away to do it).
    let mut d = doc_with("save_as_format", "**b**\n");
    let mut new = temp_path("save_as_format");
    new.set_extension("dj");
    d.save_as(new.clone());
    assert_eq!(d.format_name(), "markdown");
    let _ = std::fs::remove_file(&new);
}

// ── external change / reload ──────────────────────────────────────────────

#[test]
fn an_untouched_file_reports_unchanged() {
    let mut d = doc_with("disk_clean", "body\n");
    assert_eq!(d.disk_state(), DiskState::Unchanged);
    // Editing the buffer is not editing the file.
    d.insert("x");
    assert_eq!(d.disk_state(), DiskState::Unchanged);
    assert!(d.dirty);
    // Saving re-stamps the watermark rather than reporting our own bytes back.
    d.save();
    assert_eq!(d.disk_state(), DiskState::Unchanged);
}

#[test]
fn a_file_written_underneath_reports_changed() {
    let mut d = doc_with("disk_changed", "body\n");
    std::fs::write(&d.path, "someone else\n").unwrap();
    assert_eq!(d.disk_state(), DiskState::Changed);
    // Dirty *and* changed is the clobber: both halves are readable, and
    // leaf-core takes neither side.
    d.insert("x");
    assert!(d.dirty && d.disk_state() == DiskState::Changed);
    // Saving anyway is allowed — the frontend asked, or chose not to.
    d.save();
    assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "xbody\n");
    assert_eq!(d.disk_state(), DiskState::Unchanged);
}

#[test]
fn a_file_rewritten_with_the_same_bytes_is_unchanged() {
    // The hash is what makes this honest: the file was written (a fresh
    // mtime), and nothing about the document is stale.
    let d = doc_with("disk_same_bytes", "body\n");
    std::fs::write(&d.path, "body\n").unwrap();
    assert_eq!(d.disk_state(), DiskState::Unchanged);
}

#[test]
fn a_deleted_file_reports_missing() {
    let mut d = doc_with("disk_missing", "body\n");
    std::fs::remove_file(&d.path).unwrap();
    assert_eq!(d.disk_state(), DiskState::Missing);
    // A save recreates it, and the document is whole again.
    d.save();
    assert_eq!(d.disk_state(), DiskState::Unchanged);
    assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "body\n");
}

#[test]
fn reload_replaces_the_document_with_the_file() {
    for (view, tag) in VIEWS {
        let mut d = doc_in(view, &format!("reload_{tag}"), "one\n\ntwo\n");
        d.insert("edited ");
        assert!(d.dirty);
        std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
        d.reload();

        assert_eq!(d.source, "one\n\ntwo\n\nthree\n", "{tag}");
        assert!(!d.dirty, "{tag}: the file is what we have");
        assert_eq!(d.disk_state(), DiskState::Unchanged, "{tag}");
        assert_eq!(
            d.status.as_deref(),
            Some(&*format!("reloaded {}", d.file_name()))
        );
        // The reloaded tree is live, not the old parse.
        d.caret = d.source.find("three").unwrap();
        assert_eq!(d.breadcrumb(), "doc › para › str", "{tag}");
    }
}

#[test]
fn reload_clamps_the_caret_and_drops_the_selection() {
    let mut d = doc_with("reload_caret", "a long first line\n");
    d.caret = 12;
    d.anchor = Some(4);
    std::fs::write(&d.path, "short\n").unwrap();
    d.reload();
    assert_eq!(d.caret, d.source.len(), "clamped into the shorter file");
    assert_eq!(
        d.anchor, None,
        "a selection over bytes that changed is a lie"
    );
    assert!(d.selection().is_none());

    // A caret the file still has room for stays put.
    let mut d = doc_with("reload_caret_keep", "one\n\ntwo\n");
    d.caret = 2;
    std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
    d.reload();
    assert_eq!(d.caret, 2);
}

/// A silent reload is something that happened *to* a reader — a formatter,
/// a `git checkout` — so it has to be undoable like anything else that
/// changes the document, and undoable as one step rather than as however
/// many the file happens to differ by.
#[test]
fn reload_is_one_undo_step_and_keeps_the_history_under_it() {
    let mut d = doc_with("reload_undo", "body\n");
    d.insert("x");
    assert_eq!(d.source, "xbody\n");
    std::fs::write(&d.path, "replaced\n").unwrap();
    d.reload();
    assert_eq!(d.source, "replaced\n");
    assert!(!d.dirty, "a reload lands clean");

    // One ^Z takes the whole swap off, and hands back the unsaved work it
    // replaced — which is unsaved again, because the file no longer says it.
    d.undo();
    assert_eq!(d.source, "xbody\n", "the reload comes off in one step");
    assert!(d.dirty, "and what it comes back to is unsaved");
    // …and the history under it is still there.
    d.undo();
    assert_eq!(
        d.source, "body\n",
        "the typing before the reload undoes too"
    );
    // Redo walks back up through the reload.
    d.redo();
    d.redo();
    assert_eq!(d.source, "replaced\n");
}

/// A file rewritten with the bytes it already had is not an edit, so it
/// must not leave an undo step behind for something nobody did.
#[test]
fn reloading_identical_bytes_pushes_no_undo_step() {
    let mut d = doc_with("reload_same", "body\n");
    d.insert("x");
    std::fs::write(&d.path, "xbody\n").unwrap();
    d.reload();
    assert_eq!(d.source, "xbody\n");
    assert!(!d.dirty, "the file now says what the buffer does");
    d.undo();
    assert_eq!(
        d.source, "body\n",
        "one step back is the typing, not a no-op"
    );
}

#[test]
fn a_reload_that_cant_read_leaves_the_document_alone() {
    let mut d = doc_with("reload_gone", "body\n");
    d.insert("x");
    std::fs::remove_file(&d.path).unwrap();
    d.reload();
    assert_eq!(d.source, "xbody\n", "the unsaved work is still here");
    assert!(d.dirty);
    assert!(
        d.status.as_deref().unwrap().starts_with("reload failed:"),
        "{:?}",
        d.status
    );

    // And an untitled document has nothing to reload from.
    let mut d = Doc::blank().unwrap();
    d.insert("typed");
    d.reload();
    assert_eq!(d.source, "typed");
    assert_eq!(d.status.as_deref(), Some("no file to reload"));
}

#[test]
fn a_read_only_document_refuses_every_door() {
    let mut d = doc_with("readonly", "one two three\n");
    d.insert("x");
    assert!(d.dirty, "writable first, so the undo step exists");
    d.set_read_only(true);
    let before = d.source.clone();
    d.insert("y");
    d.backspace();
    d.undo();
    d.redo();
    assert_eq!(d.source, before, "no door moved a byte");
    d.set_read_only(false);
    d.undo();
    assert_ne!(d.source, before, "off again, the same doors work");
}

/// The doors that go to twig's own verbs rather than through the splice.
/// Typed text in the rendered view under the default markup mode is the
/// everyday one — it is what a keystroke in leaf-web or the Apple views
/// becomes — and it walked straight past the gate.
#[test]
fn a_read_only_document_refuses_the_doors_around_the_splice() {
    let mut d = wysiwyg_doc(
        "readonly-doors",
        "one two three\n\n| a | b |\n|---|---|\n| c | d |\n",
    );
    d.set_markup_mode(MarkupMode::None);
    d.set_read_only(true);
    let before = d.source.clone();
    d.place_caret(3, false);
    d.insert("y");
    d.insert_link("https://example.com");
    d.insert_image("a.png", "alt");
    d.insert_thematic_break();
    d.insert_footnote();
    d.place_caret(0, false);
    d.place_caret(3, true);
    d.toggle(InlineKind::Strong);
    d.toggle_heading(2);
    d.set_block(BlockKind::Paragraph);
    d.toggle_list(false);
    d.toggle_blockquote();
    d.toggle_task_item();
    d.newline();
    d.indent();
    d.set_code_language("rust");
    let in_cell = d.source.find("| c").unwrap() + 2;
    d.place_caret(in_cell, false);
    assert!(d.caret_in_table(), "the caret is in the grid");
    assert!(!d.cell_line_break(), "the cell break reports the refusal");
    assert_eq!(d.source, before, "no door moved a byte");
    assert!(!d.dirty, "nothing to save");
    d.set_read_only(false);
    d.place_caret(3, false);
    d.insert("y");
    assert_ne!(d.source, before, "off again, the same doors work");
}

#[test]
fn a_selection_quote_carries_its_context_on_char_boundaries() {
    let mut d = doc_with("quote", "before 你好 exact 世界 after\n");
    let start = d.source.find("exact").unwrap();
    d.place_caret(start, false);
    d.place_caret(start + "exact".len(), true);
    let q = d.selection_quote(3).unwrap();
    assert_eq!(q.exact, "exact");
    assert_eq!(
        q.prefix, "你好 ",
        "chars, not bytes — the multibyte pair counts as two"
    );
    assert_eq!(q.suffix, " 世界");
    assert_eq!(&d.source[q.start..q.end], "exact");
    // At the edges the context clips rather than erring.
    d.place_caret(0, false);
    d.place_caret(6, true);
    let q = d.selection_quote(40).unwrap();
    assert_eq!(q.prefix, "");
    assert_eq!(q.exact, "before");
    // No selection is no quote.
    d.place_caret(0, false);
    assert!(d.selection_quote(3).is_none());
}

#[test]
fn highlights_are_kept_sorted_and_answer_point_queries() {
    let mut d = doc_with("hl", "one two three\n");
    d.set_highlights(vec![
        Highlight {
            start: 8,
            end: 13,
            id: "b".into(),
            color: None,
            marker: None,
        },
        Highlight {
            start: 0,
            end: 3,
            id: "a".into(),
            color: Some("#ffe066".into()),
            marker: None,
        },
        Highlight {
            start: 5,
            end: 5,
            id: "empty".into(),
            color: None,
            marker: None,
        },
    ]);
    assert_eq!(
        d.highlights()
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"],
        "sorted by start, the empty range dropped"
    );
    assert_eq!(d.highlight_at(1).map(|h| h.id.as_str()), Some("a"));
    assert_eq!(d.highlight_at(3), None, "end is exclusive");
    assert_eq!(d.highlight_at(8).map(|h| h.id.as_str()), Some("b"));
    d.set_highlights(Vec::new());
    assert!(d.highlights().is_empty(), "a replace is a replace");
}

/// `Highlight::covering` and the cursor over it are what both painters ask
/// per glyph, so they have to answer the same as the scan they replaced —
/// including in the gaps, which is where most glyphs are.
#[test]
fn covering_answers_from_a_sorted_list_without_scanning_all_of_it() {
    let hl = |start: usize, end: usize, id: &str| Highlight {
        start,
        end,
        id: id.into(),
        color: None,
        marker: None,
    };
    // Disjoint, as search hits are: in a range, in a gap, and past the end.
    let hits: Vec<Highlight> = (0..20).map(|i| hl(i * 10, i * 10 + 3, "hit")).collect();
    assert_eq!(Highlight::covering(&hits, 0).map(|h| h.start), Some(0));
    assert_eq!(Highlight::covering(&hits, 102).map(|h| h.start), Some(100));
    assert_eq!(
        Highlight::covering(&hits, 105),
        None,
        "a gap covers nothing"
    );
    assert_eq!(Highlight::covering(&hits, 103), None, "end is exclusive");
    assert_eq!(Highlight::covering(&hits, 9_999), None);
    assert_eq!(Highlight::covering(&[], 0), None);

    // Nested: first by start, so a hit inside an annotation still resolves
    // to the annotation — and the range that stops short doesn't mask it.
    let nested = vec![hl(0, 20, "outer"), hl(5, 10, "inner")];
    assert_eq!(
        Highlight::covering(&nested, 7).map(|h| h.id.as_str()),
        Some("outer")
    );
    assert_eq!(
        Highlight::covering(&nested, 15).map(|h| h.id.as_str()),
        Some("outer")
    );
}

/// The cursor is an optimisation, so the only thing worth asserting is that
/// it is not also a change of answer — at every offset, over a list with a
/// nest in it, walked forwards and then backwards.
#[test]
fn the_highlight_cursor_answers_exactly_what_a_fresh_scan_would() {
    let hl = |start: usize, end: usize, id: &str| Highlight {
        start,
        end,
        id: id.into(),
        color: None,
        marker: None,
    };
    let mut list = vec![
        hl(0, 20, "outer"),
        hl(5, 10, "inner"),
        hl(30, 33, "hit"),
        hl(40, 43, "hit"),
    ];
    list.sort_by_key(|h| (h.start, h.end));

    let mut cursor = HighlightCursor::new(&list);
    for offset in 0..50 {
        assert_eq!(
            cursor.at(offset).map(|h| h.id.as_str()),
            Highlight::covering(&list, offset).map(|h| h.id.as_str()),
            "cursor disagrees at {offset}"
        );
    }
    // Backwards: the cursor re-seats rather than answering from where it
    // had got to, so a painter that revisits a row is still told the truth.
    for offset in (0..50).rev() {
        assert_eq!(
            cursor.at(offset).map(|h| h.id.as_str()),
            Highlight::covering(&list, offset).map(|h| h.id.as_str()),
            "cursor disagrees walking back at {offset}"
        );
    }
}
