---
title: Proposals
description: Arguments for a change to leaf, one file each — a case that may lose
created: 2026-09-07
updated: 2026-09-12
contents:
- '[HTML containers leaf does not author fold to atoms](html-containers-fold-to-atoms.md)'
- '[Math in leaf](math-rendering.md)'
- '[SVG as vectors in leaf-swift, through usvg](svg-as-vectors-on-apple.md)'
part_of: '[leaf](/README.md)'
---
# Proposals

A document lands here when it argues for a change rather than describing what
leaf already does. It is the place an idea is discussed *before* it is settled
and the record of the argument *after*, so a proposal that lost is still worth
keeping — the reasoning is the value, and the outcome is part of it.

Each carries `status` in its frontmatter — `draft`, `accepted`, `implemented`,
`deferred`, `rejected` — which `dx tasks` reads. A proposal leaves the list
above only by reaching `implemented` or `rejected`; `accepted` is a decision
made and not yet built, and stays visible. Closing one is an edit that names the
release or commit that resolved it, with the outcome in a **Status** section at
the top and the body left as argued.

What this is not:

- A commitment with a done state, which is a file in [`../tasks/`](../tasks/tasks.md).
- A commitment to consumers, which is the [CHANGELOG](../CHANGELOG.md)'s
  unreleased region or a `Behavioural-change:` trailer on the commit that
  causes it.
- A description of what shipped, which belongs in the READMEs and the crate
  docs — never here.
