---
status: done
created: 2026-09-21
updated: 2026-09-22
part_of: '[Closed tasks](/docs/tasks/closed/closed.md)'
---
# A fence inside a quote or list has no language

**Status.** Done, in `fix(core): a fence inside a quote or list item has
its language`. `code_info_span` skips the line's quote and list markers
before looking for the fence, and measures the three-space allowance from
the container's content column — past the marker on the fence's own line,
or from the list item above on a continuation line — so an indented block
in a list item still answers `None`. Tests per container in `wysiwyg.rs`,
the source view's tokens in `source.rs`, and the edit in `doc.rs`.

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
