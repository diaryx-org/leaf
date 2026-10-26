# leaf-mermaid-ffi

The Swift binding for [`leaf-mermaid`](../leaf-mermaid): `renderMermaid(source:dark:measure:)`,
a Mermaid diagram to a standalone SVG with its size, its labels measured by
the host (`MermaidMeasure`, Core Text on Apple).

It is not part of leaf's default build. A host that draws Mermaid fences
builds leaf-ffi with its `mermaid` feature, which links this crate into the one
archive the app force-loads, and compiles the generated Swift module
`LeafMermaidFFI` beside `LeafFFI`. The leaf-editor app does both: see
`apps/leaf-editor/project.yml` and `apps/leaf-editor/LeafMermaid/`.
`scripts/gen-bindings.sh` writes that module, and CI holds it to this crate.
