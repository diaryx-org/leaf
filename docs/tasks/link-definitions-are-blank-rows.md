---
status: in-progress
created: 2026-09-15
updated: 2026-09-15
---
# A link reference definition's lines are drawn as blank rows

**Status: in-progress.** The fix is written and verified on both sides, and
waits on a twig release. twig `dc0106f` gives a `reference` node the span of
its own bytes (it reported `0..0` for every one); leaf's branch
`link-definitions` merges a definition that has a span into the top-level walk
as a hidden block, stepped over the way `ed74a76` steps over a comment. Against
the published `twig-doc 3.3.1` the merge is a no-op and two of its tests fail
— `a_link_reference_definition_is_stepped_over_like_a_comment` and
`link_reference_definitions_closing_the_document_are_not_trailing_blank_lines`
— so the branch is not on `main`. Verified green with `cargo xtask ci` under
`[patch.crates-io] twig-doc` at the checkout and `TWIG_SYS_FORCE_SOURCE=1`.

**Remaining.** Release twig with the fix (the version is Adam's to name), then
in leaf: bump `twig-doc` in `Cargo.toml` to that release, fast-forward `main`
onto `link-definitions` (or cherry-pick it), run `cargo xtask ci` against the
published crate, and close this task in the same commit.

**Where.** `crates/leaf-core`, the visual map's top-level walk
(`top_level` / `top_blocks` in `wysiwyg.rs`); and twig's Markdown block
parser, which is where the span comes from.

**Repro.** Open

    see [a] and [b]

    <!-- links -->
    [a]: /a
    [b]: /b

in the WYSIWYG view. The paragraph is followed by three blank rows — a gap and
two empty paragraphs, one per definition line — because twig hands leaf no
block over those bytes: a link reference definition is resolved by label, so it
hangs off nothing, and until twig `dc0106f` it had no span either. The same
happens between blocks (`para\n\n[a]: /a\n\nafter` draws three gap rows where
one belongs). A definition glued to the top of a paragraph is fine, since the
paragraph's span swallows it.

**Done when.** Those lines draw as nothing, the blocks either side meet across
one boundary, and the two tests above pass against the published twig.
