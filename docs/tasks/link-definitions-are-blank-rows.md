---
status: done
created: 2026-09-15
updated: 2026-09-15
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A link reference definition's lines are drawn as blank rows

**Status: done.** twig 3.3.3 (`dc0106f`) gives a `reference` node the span of
its own bytes, where it reported `0..0` for every one; leaf's `top_level` and
`top_blocks` merge a definition that has a span into the top-level walk as a
hidden block, stepped over the way `ed74a76` steps over a comment, through one
shared predicate (`is_placed_definition`) so the two walks cannot disagree.
Pinned by `a_link_reference_definition_is_stepped_over_like_a_comment`,
`link_reference_definitions_closing_the_document_are_not_trailing_blank_lines`
and `a_definition_glued_under_a_paragraph_stays_inside_it` in `wysiwyg`, and by
a definition-bearing fixture in the incremental-build parity test and the
Doc-driven splice test. Landed with the pin moved to `twig-doc 3.3.3`.

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
