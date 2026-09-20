---
title: A typed dollar mints math in the hidden mode
status: open
created: 2026-09-19
updated: 2026-09-19
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A typed dollar mints math in the hidden mode

`MarkupMode::None` promises that typed syntax stays literal — twig escapes
anything that would open markup, and formatting comes from commands. Since
Markdown reads math (`parse_extensions` turns the `math` extension on), a
`$` that would open a formula is markup too, and twig's `insert_literal`
does not yet escape it. So typing `$x$` in the default mode makes a formula,
which reveals to its source while the caret is on the line and folds to a
picture when it leaves — behaviour `Shortcuts` wants and `None` does not.

Only Markdown: djot's math needs a backtick after the dollar, and the
backtick is already escaped.

**Repro.** Open a Markdown document in the rich view, leave the markup mode
at `None`, type `$x$`. The source is `$x$`, not `\$x\$`, and the paragraph's
breadcrumb reads `doc › para › inline_math`.

Leaf does not paper over it: the escaping is positional and per-format,
and `insert_literal`'s own docs say it is not the caller's to reproduce.
The fix is twig's — `twig/docs/tasks/insert-literal-does-not-escape-a-dollar-under-the-math-extension.md`
— and this closes when leaf's `twig-doc` pin carries it, with the
`None`-mode half of `a_dollar_typed_in_shortcuts_authors_math` restored.
