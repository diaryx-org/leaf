# leaf-session

The editors leaf runs itself for a region of a document, behind one
interface. A frontend opens a session by the region's kind and content
(`open("math", tex, display)`), hands it keys, text and clicks, gets back
splices on the content, and draws the picture the session returns, with its
caret in it.

The bindings export this once — `EditSession` in leaf-ffi (Swift, Kotlin) and
in leaf-math-wasm (the web) — so a second editor written in Rust needs no new
binding: it implements `Editor` here and is added to `open` and `KINDS`.

| Kind | Editor |
|---|---|
| `math` | [`leaf-math-edit`](../leaf-math-edit): a formula edited as fractions, scripts and roots |

A region a host edits in a view of its own (a Mermaid fence) does not come
through here. See `docs/proposals/embedded-editors.md`.
