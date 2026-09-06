// Public type surface for the leaf web editor package.
export {
  LeafEditor,
  DEFAULT_THEME,
  MARK_COLORS,
  type MarkColor,
  type EditorTheme,
  type EditorOptions,
  type EditorState,
  type Format,
  type Highlight,
} from "./editor.js";
export {
  LeafDoc,
  type DocView,
  type Row,
  type Run,
  type CapabilitiesView,
  type FootnoteView,
  type FootnoteDefView,
  type HighlightOut,
  type SelectionQuote,
} from "../pkg/leaf_wasm.js";
