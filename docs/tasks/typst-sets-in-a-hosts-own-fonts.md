---
title: Typst sets in a host's own fonts
part_of: '[Tasks](/docs/tasks/tasks.md)'
created: 2026-10-28
---
# Typst sets in a host's own fonts

A host of the Reflow and Pages views (`crates/leaf-pages`) hands it font
files through `leaf_pages::Setup::fonts`: Diaryx means to pass its four
faces, decoded to TrueType, which iOS has not got installed for Typst to
find. leaf-pages keeps them, and Typst never sees them: twig-export's
`Typesetter` reads its fonts itself when it is made (Typst's embedded ones,
then the system's, `typst_world::fonts`), and has no way to be given more.

**Done when** a document reflowed or paged in leaf-pages sets in a face
given in `Setup::fonts` that the system does not have.

**How.** In twig-export, a `Typesetter::with_fonts(fonts: impl
IntoIterator<Item = Vec<u8>>)` that puts each file's faces
(`typst::text::Font::iter`) into its `FontStore` before the embedded and the
system's ones; then, in leaf-pages's `typeset::typesetter()`, pass
`Setup::fonts` to it, and drop the trace that says they were not taken.
