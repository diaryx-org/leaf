use super::*;

// ── the map is built at most once per (revision, wrap) ───────────────────
//
// A frontend repaints for reasons that have nothing to do with the text — a
// blinking caret, a scroll — and rebuilding the map is O(document). These
// pin *that the cache fires*, which a passing suite can't tell you: a cache
// that never hits is invisible to every other test in this file.
//
// The probe is to wreck the built map and ask for it again. A rebuild
// repairs it; a cache hit hands the wreckage straight back. Nothing else
// can distinguish the two from outside.

#[test]
fn a_rebuild_with_nothing_changed_reuses_the_map() {
    let mut d = doc_in(View::Wysiwyg, "cache_hit", "# Title\n\nbody\n");
    d.build_visual(80);
    assert!(!d.vmap.rows.is_empty());
    d.vmap.rows.clear(); // wreck it
    d.build_visual(80);
    assert!(
        d.vmap.rows.is_empty(),
        "the map was rebuilt though nothing changed — the cache never fired"
    );
}

#[test]
fn an_edit_rebuilds_the_map() {
    let mut d = doc_in(View::Wysiwyg, "cache_edit", "# Title\n\nbody\n");
    d.build_visual(80);
    let before = d.revision();
    d.vmap.rows.clear();
    d.insert("x");
    d.build_visual(80);
    assert!(d.revision() > before, "an edit must move the revision");
    assert!(
        !d.vmap.rows.is_empty(),
        "an edited document must not paint from a stale map"
    );
}

#[test]
fn a_width_change_rebuilds_the_map() {
    // The map is a function of the wrap width too, so a resize is a miss
    // even though the text is untouched.
    let mut d = doc_in(
        View::Wysiwyg,
        "cache_width",
        "one two three four five six\n",
    );
    d.build_visual(80);
    d.vmap.rows.clear();
    d.build_visual(12);
    assert!(!d.vmap.rows.is_empty(), "a resize must rebuild the map");
    // And the unwrapped map is its own key, not the same as any width.
    d.vmap.rows.clear();
    d.build_visual_unwrapped();
    assert!(!d.vmap.rows.is_empty(), "unwrapped is a different map");
}

#[test]
fn a_motion_does_not_rebuild_the_map() {
    // The whole point: moving the caret changes nothing the map is built
    // from. If a motion bumped the revision, every arrow key would cost a
    // full rebuild and the cache would be worthless.
    let mut d = doc_in(View::Wysiwyg, "cache_motion", "# Title\n\nbody text\n");
    d.build_visual(80);
    let rev = d.revision();
    d.move_right(false);
    d.move_right(true);
    d.move_down(false);
    assert_eq!(d.revision(), rev, "a motion must not move the revision");
    d.vmap.rows.clear();
    d.build_visual(80);
    assert!(
        d.vmap.rows.is_empty(),
        "a motion should not rebuild the map"
    );
}

#[test]
fn saving_does_not_rebuild_the_map() {
    // Saving changes `dirty`, not the text.
    let mut d = doc_in(View::Wysiwyg, "cache_save", "# Title\n\nbody\n");
    d.insert("x");
    d.build_visual(80);
    let rev = d.revision();
    d.save();
    assert_eq!(d.revision(), rev, "a save must not move the revision");
    assert!(!d.dirty, "the save should have cleaned the document");
}

#[test]
fn a_reload_rebuilds_the_map() {
    // Reload replaces the text without going through `refresh`, so it has to
    // move the revision itself — else the editor paints the old file.
    let mut d = doc_in(View::Wysiwyg, "cache_reload", "# Title\n\nbody\n");
    d.build_visual(80);
    let rev = d.revision();
    std::fs::write(&d.path, "# Other\n\nwholly new\n").unwrap();
    d.reload();
    assert!(d.revision() > rev, "a reload must move the revision");
    d.build_visual(80);
    let text: String = d
        .vmap
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.ch))
        .collect();
    assert!(
        text.contains("wholly new"),
        "the reloaded text should be on screen, got {text:?}"
    );
}
