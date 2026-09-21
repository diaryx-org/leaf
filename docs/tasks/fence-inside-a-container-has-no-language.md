---
status: open
created: 2026-09-21
updated: 2026-09-21
part_of: '[Tasks](/docs/tasks/tasks.md)'
---
# A fence inside a quote or list has no language

**Where.** `crates/leaf-core`, `wysiwyg::code_info_span` and the
`code_language` over it; read by both `wysiwyg` (the block's label and its
tokens) and `source::fill_tokens`.

**What.** `code_info_span` reads the opening fence at the code block's
`span.start` and allows it up to three spaces of indent. A fence inside a
block quote or a list item starts its line with the container's marker —
`> ```rust`, `- ```rust` — so the fence character is not where it looks and
the function answers `None`: the block has no language label in the rendered
view, and no syntax highlighting in either view. twig carries no `lang`
attribute on the node to fall back on.

**Repro.**

```markdown
> ```rust
> let x = 1;
> ```
```

Rendered: a code panel with no `rust` label and plain code colour. Source
view: the body is `Role::Code` with no token. The same block unquoted has
both.

**How.** Skip the container prefix before looking for the fence: the opening
line's fence is its first run of three or more `` ` `` or `~`, and no
container marker contains either — but an *indented* code block inside a
list item must still answer `None`, so the indent rule has to be measured
from after the marker rather than dropped. `code_info_span` is also what an
edit of the language replaces, so the span it returns has to be the info
string's exact bytes, as now. A test per container, and one for the
indented block inside a list.
