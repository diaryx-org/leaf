// Public entry for the leaf web editor package.
//
// Most consumers want `LeafEditor` — the full editor (render + input) over a
// container element. `LeafDoc` is the lower-level document model it wraps,
// re-exported for hosts that want to drive the model directly (e.g. a custom
// renderer). Call `LeafEditor.init()` (or the model's `init`) once before use.

export { LeafEditor, DEFAULT_THEME } from "./editor.js";
export { LeafDoc } from "../pkg/leaf_wasm.js";
