// Public types for the framework-agnostic `LeafEditor`. The document-model and
// view-frame types (`DocView`, `Row`, `Run`, `LeafDoc`) are generated from Rust
// by tsify and live in `../pkg/leaf_wasm.d.ts`; this file types only the editor
// shell built on top of them.

import type { DocView } from "../pkg/leaf_wasm.js";

/** Presentation knobs, mirroring `leaf-gpui`'s `EditorStyle`. All optional. */
export interface EditorTheme {
  /** Proportional body family — prose and headings. */
  fontFamily: string;
  /** Monospace family — inline code and fenced blocks. */
  monoFamily: string;
  /** Body font size in px. */
  fontSize: number;
  /** Body line height in px. */
  lineHeight: number;
  /** Per-level heading size multipliers `[h1…h6]`, relative to `fontSize`. */
  headingScale: [number, number, number, number, number, number];
}

/** The default theme (gpui-parity: Helvetica-class body, Menlo-class mono, 16/24). */
export const DEFAULT_THEME: EditorTheme;

/** A summary of caret/document state, emitted after every repaint. */
export interface EditorState {
  /** Which surface is showing. */
  view: "wysiwyg" | "source";
  /** Whether the buffer differs from the last saved bytes. */
  dirty: boolean;
  /** Heading level at the caret (1–6), or null outside a heading. */
  heading: number | null;
  /** Inline marks active at the caret (`"bold"`, `"italic"`, `"code"`, …). */
  active: string[];
}

export interface EditorOptions {
  /** Initial document text. Defaults to empty. */
  source?: string;
  /** Source format: `"markdown"` | `"djot"` | `"html"` | `"xml"`. Default markdown. */
  format?: string;
  /** Presentation overrides; any omitted field keeps its `DEFAULT_THEME` value. */
  theme?: Partial<EditorTheme>;
  /** Called after every repaint with the new caret/document state. */
  onChange?: (state: EditorState) => void;
}

/**
 * A framework-agnostic rich-text editor over a `leaf_core::Doc`, compiled to
 * wasm. Renders proportionally (real body font, sized headings, monospace code)
 * while core stays the authority on text, wrapping, and caret math.
 *
 * `LeafEditor.init()` must resolve before the first construction.
 */
export class LeafEditor {
  /** Load and instantiate the wasm module once. `wasmUrl` overrides its location. */
  static init(wasmUrl?: string | URL): Promise<void>;

  constructor(container: HTMLElement, opts?: EditorOptions);

  /** Give the editing surface keyboard focus. */
  focus(): void;
  /** Remove listeners, free the wasm handle, and empty the container. */
  destroy(): void;
  /** Register (or replace) the repaint callback. Returns `this`. */
  onChange(cb: (state: EditorState) => void): this;

  /** The current source text. */
  source(): string;
  /** Whether the buffer differs from the last saved bytes. */
  isDirty(): boolean;
  /** Which surface is showing. */
  viewName(): "wysiwyg" | "source";
  /** Clear the dirty flag after the host persisted `source()` itself. */
  markSaved(): void;
  /** Recompute the wrap width from the viewport and repaint. */
  refit(): void;

  /** Paint a frame from a model `DocView` (rarely called directly). */
  render(view: DocView): void;

  // Formatting commands — mirror leaf-gpui's EditorCommand.
  toggleBold(): void;
  toggleItalic(): void;
  toggleCode(): void;
  toggleMark(): void;
  toggleUnderline(): void;
  toggleStrike(): void;
  setParagraph(): void;
  /** Toggle the block to a heading of `level` (1–6); the active level toggles off. */
  setHeading(level: number): void;
  toggleBlockquote(): void;
  toggleList(ordered: boolean): void;
  insertLink(dest: string): void;
  undo(): void;
  redo(): void;
  selectAll(): void;
  /** Switch between the WYSIWYG surface and the raw source. */
  toggleView(): void;
}

// Selection gestures are handled internally on mousedown by click count
// (1 = caret, 2 = word, 3 = block, 4 = document) and don't need a public method;
// hosts drive selection through the caret/command API above.
