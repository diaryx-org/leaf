// Public types for the framework-agnostic `LeafEditor`. The document-model and
// view-frame types (`DocView`, `Row`, `Run`, `LeafDoc`, …) are generated from
// Rust by tsify and live in `../pkg/leaf_wasm.d.ts`; this file types only the
// editor shell built on top of them.

import type {
  CapabilitiesView,
  DocView,
  FootnoteDefView,
  FootnoteView,
  HighlightIn,
  HighlightOut,
  SelectionQuote,
} from "../pkg/leaf_wasm.js";

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
  /** Height of a between-blocks gap row, as a fraction of `lineHeight` (default 0.5). */
  blockGapScale: number;
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
  /** Whether there is an edit to undo — what a history button enables by. */
  canUndo: boolean;
  /** Whether there is an undone edit to redo. */
  canRedo: boolean;
  /** Whether the document refuses edits — see `setReadOnly`. */
  readOnly: boolean;
  /** Heading level at the caret (1–6), or null outside a heading. */
  heading: number | null;
  /** Inline marks active at the caret (`"bold"`, `"italic"`, `"code"`, …). */
  active: string[];
  /** Destination of the link the caret stands in, or null. */
  link: string | null;
  /**
   * Colour of the highlight the caret stands in, or null — both outside a
   * highlight and inside one that names no colour, which are the same answer to
   * "which swatch is current".
   */
  markColor: MarkColor | null;
  /** Whether a non-empty selection is live — with `caretInMark()`, what tells a
   *  colour picker whether it would recolour a highlight or make one. */
  hasSelection: boolean;
  /** The caret's source byte offset. */
  caretSrc: number;
}

/** A colour a highlight can be — the closed palette core reads and writes. */
export type MarkColor = "red" | "orange" | "yellow" | "green" | "blue" | "purple" | "brown";

/**
 * The colours in the order a picker should show them. Also the suffixes of the
 * `.leaf-mk-*` classes the renderer paints with, so a picker built from this
 * and the stylesheet cannot disagree.
 */
export const MARK_COLORS: readonly MarkColor[];

/** A source format the model can parse. */
export type Format = "markdown" | "djot" | "html" | "xml";

export interface EditorOptions {
  /** Initial document text. Defaults to empty. */
  source?: string;
  /** Source format. Default markdown. */
  format?: Format | string;
  /** Presentation overrides; any omitted field keeps its `DEFAULT_THEME` value. */
  theme?: Partial<EditorTheme>;
  /** Whether to focus the surface on construction. Default true. */
  autofocus?: boolean;
  /** Called after every repaint with the new caret/document state. */
  onChange?: (state: EditorState) => void;
}

/** A host-painted range of the source — see `LeafEditor.setHighlights`. */
export type Highlight = HighlightIn;

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

  // ── lifecycle ──────────────────────────────────────────────────────────

  /** Give the editing surface keyboard focus. */
  focus(): void;
  /** Remove listeners, free the wasm handle, and empty the container. */
  destroy(): void;
  /**
   * Replace the document with another, in place. The reader's preferences
   * (markup mode, line flow, read-only, colour scheme) carry over; the
   * history does not. The caret starts at the top.
   */
  load(source: string, format?: Format | string): void;
  /** Register (or replace) the repaint callback. Returns `this`. */
  onChange(cb: (state: EditorState) => void): this;

  // ── the document ───────────────────────────────────────────────────────

  /** The current source text. */
  source(): string;
  /** Whether the buffer differs from the last saved bytes. */
  isDirty(): boolean;
  /** Which surface is showing. */
  viewName(): "wysiwyg" | "source";
  /** Clear the dirty flag after the host persisted `source()` itself. */
  markSaved(): void;
  /** Whether the document refuses edits. */
  isReadOnly(): boolean;
  /**
   * Turn the read-only gate on or off: the same rendering, selection, and
   * navigation, refusing every edit. The surface stops being contenteditable
   * (no caret, no keyboard on a phone) while staying focusable and selectable.
   */
  setReadOnly(on: boolean): void;
  /** The selected source, verbatim (markup included), or null with nothing selected. */
  selectedText(): string | null;
  /**
   * The selection cited out of the source with up to `context` characters
   * of what surrounds it and its byte range, or null with nothing selected.
   */
  selectionQuote(context?: number): SelectionQuote | null;
  /**
   * Which formatting controls this document's format can spell — one flag per
   * toolbar button. Depends only on the format, so read once per document.
   */
  capabilities(): CapabilitiesView;
  /** Whether the format offers any way to author at all (false for XML). */
  isAuthorable(): boolean;
  /** Whether the caret is inside a table — gate the grid commands on this and on `capabilities().table`. */
  caretInTable(): boolean;
  /** Whether the caret is inside a highlight — gate a colour picker on this and
   *  on `capabilities().mark_color`, which asks whether the format spells a
   *  colour at all (djot writes the highlight and no colour on it). */
  caretInMark(): boolean;
  /** Recompute the wrap width from the viewport and repaint. */
  refit(): void;

  // ── navigation ─────────────────────────────────────────────────────────

  /**
   * Put the caret at source `offset`, scrolled into view. With `end`, the
   * block through it is flashed so the reader sees where they were sent.
   */
  reveal(offset: number, end?: number | null): void;
  /** Land on what a locator names (a fragment, a heading id). Returns whether it named anything. */
  goTo(locator: string): boolean;
  /** The footnote reference at `offset` and the note it names, or null. */
  footnoteAt(offset: number): FootnoteView | null;
  /** `footnoteAt` for the caret. */
  footnoteAtCaret(): FootnoteView | null;
  /** The footnote definition the caret stands in and its first reference, or null. */
  footnoteDefinitionAtCaret(): FootnoteDefView | null;
  /**
   * Follow the footnote at the caret: a reference down to its note, a note
   * back up to its reference. Returns whether there was one to follow.
   */
  followFootnote(): boolean;
  /**
   * What to do when a link is followed (⌘-click / Ctrl-click) instead of
   * opening a new tab. A fragment into this document is followed internally
   * first and never reaches this.
   */
  onFollowLink(cb: (destination: string) => void): this;

  // ── host highlights ────────────────────────────────────────────────────

  /** Replace the host-painted ranges wholesale and repaint. */
  setHighlights(highlights: Highlight[]): void;
  /** The highlights as last set, sorted by start. */
  highlights(): HighlightOut[];
  /** The id of the highlight covering `offset`, or null. */
  highlightAt(offset: number): string | null;
  /** Called with a highlight's id when the reader clicks it; the click still places the caret. */
  onActivateHighlight(cb: (id: string) => void): this;

  // ── drag and drop ──────────────────────────────────────────────────────

  /**
   * What to do with files dropped on the surface, given the `File`s and the
   * source offset under the pointer. Without a handler they are ignored.
   */
  onDropFiles(cb: (files: File[], offset: number) => void): this;

  // ── formatting commands (mirror leaf-gpui's EditorCommand) ─────────────

  toggleBold(): void;
  toggleItalic(): void;
  toggleCode(): void;
  toggleMark(): void;
  /**
   * One press of a colour swatch: colour the highlight at the caret, or — over
   * a selection that isn't highlighted yet — highlight it and colour it, as one
   * undo step. `null` clears the colour, and over a plain selection means
   * simply "highlight this".
   */
  highlight(color?: MarkColor | null): void;
  /** The exact gesture behind `highlight`: recolour the highlight the caret is
   *  already in (or clear it). Writes nothing where there is no highlight. */
  setMarkColor(color?: MarkColor | null): void;
  toggleUnderline(): void;
  toggleStrike(): void;
  setParagraph(): void;
  /** Toggle the block to a heading of `level` (1–6); the active level toggles off. */
  setHeading(level: number): void;
  toggleBlockquote(): void;
  toggleList(ordered: boolean): void;
  insertLink(dest: string): void;
  toggleTaskItem(): void;
  toggleTaskChecked(): void;
  /** Write a footnote reference at the caret, and the definition it needs. */
  insertFootnote(): void;
  insertThematicBreak(): void;
  /** Insert block media; any selection becomes the alt text. */
  insertMedia(kind: "image" | "video" | "audio", destination: string, alt?: string): void;
  tableInsertRow(below?: boolean): void;
  tableDeleteRow(): void;
  tableInsertColumn(right?: boolean): void;
  tableDeleteColumn(): void;
  tableMoveRow(down?: boolean): void;
  tableMoveColumn(right?: boolean): void;
  tableSetAlignment(align: "left" | "right" | "center" | "default"): void;
  /** The line-wrapping preference. */
  lineFlow(): "fold" | "preserve";
  setLineFlow(mode: "fold" | "preserve"): void;
  undo(): void;
  redo(): void;
  selectAll(): void;
  /** Switch between the WYSIWYG surface and the raw source. */
  toggleView(): void;
  /** The markup-exposure preference. */
  markupMode(): "none" | "shortcuts" | "full";
  /**
   * Set the markup-exposure preference. `"none"` (the default) hides
   * delimiters and keeps typed syntax literal; `"shortcuts"` still hides them
   * but lets typing author markup; `"full"` also shows the caret line's raw
   * markup, whose delimiters arrive as runs with `role: "delimiter"`.
   */
  setMarkupMode(mode: "none" | "shortcuts" | "full"): void;

  /** Paint a frame from a model `DocView` (rarely called directly). */
  render(view: DocView): void;
}

// Selection gestures are handled internally on mousedown by click count
// (1 = caret, 2 = word, 3 = block, 4 = document) and don't need a public method;
// hosts drive selection through the caret/command API above.
//
// Keyboard: ⌘B/I/U, ⌘⇧C code, ⌘⇧M highlight, ⌘⇧X strike, ⌘⌥0–6 paragraph and
// headings, ⌘⇧7/8 lists, ⌘[ / ⌘] outdent/indent, ⌘E view, ⌘Z/⌘⇧Z/⌘Y history,
// ⌘⇧V paste as plain text, Tab/⇧Tab indent or the next cell. Ctrl for ⌘ off a
// Mac. ⌘-click follows a link or a footnote.
