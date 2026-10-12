// Public types for the framework-agnostic `LeafEditor`. The document-model and
// view-frame types (`DocView`, `Row`, `Run`, `LeafDoc`, …) are generated from
// Rust by tsify and live in `../pkg/leaf_wasm.d.ts`; this file types only the
// editor shell built on top of them.

import type {
  CapabilitiesView,
  DirectiveAttr,
  DocView,
  FootnoteDefView,
  FootnoteView,
  HighlightIn,
  HighlightOut,
  RegionView,
  SelectionQuote,
  TextCounts,
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
  /** Whether the caret stands in a code block — what lights a Code Block button. */
  codeBlock: boolean;
  /** Whether the caret stands in a verse — what lights a Verse button. */
  verse: boolean;
  /**
   * Whether the list item at the caret carries a checkbox, and which way it
   * faces — `true` ticked, `false` empty, null for a plain item or no item at
   * all. What lights a Checklist button, and ticks a Checked one.
   */
  task: boolean | null;
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

/** How a block is aligned across the measure. `null` is the theme's default,
 *  which is left — there is no `left` token, because absence is it. */
export type Align = "center" | "right" | "justify";
/** How far apart a block sets its lines, as a multiple of the theme's own line
 *  height — the three a menu offers. `null` is the theme's, which is why there
 *  is no `"1"`. */
export type LineSpacing = "1.15" | "1.5" | "2";
/** A size step, in CSS's `<absolute-size>` keywords less `medium` — which is
 *  absence, and so is `null`. */
export type SizeStep =
  | "xx-small" | "x-small" | "small" | "large" | "x-large" | "xx-large" | "xxx-large";
/** A face, in CSS's generic families. `null` is the theme's body face. */
export type FontFamily = "serif" | "sans-serif" | "monospace" | "cursive";

// The four open forms. Each property keeps its names as the first thing a menu
// offers — a name is portable under every theme and `presentation.css` has a
// rule for it — and grows one more row, *Other…*, for the author who wants a
// number, a family or a colour the names do not cover and takes the
// portability cost knowingly. A value is drawn by the inline style the editor
// sets beside the `data-` attribute; a page rendered with the published
// stylesheet alone shows the names and not the values.

/** A size: one of the seven steps, or a point size (`"14pt"`, `"13.5pt"`).
 *  Only `pt` is a unit — `px` is a screen's unit and a document is not a
 *  screen, and the relative units are what the steps already are. */
export type FontSize = SizeStep | `${number}pt`;
/** A line height: one of the three names, or any other positive decimal
 *  (`"1.3"`). A decimal that spells one of the three *is* that name. */
export type LineHeight = LineSpacing | `${number}`;
/** A face: one of the four generics, or a family the author named
 *  (`"Garamond"`), which draws in that family where it is installed and in the
 *  page's own face where it is not. */
export type FontFace = FontFamily | (string & {});
/** A text colour: one of the seven names, or a hex triple (`"#c03030"`, or the
 *  `"#f00"` shorthand, which is read and never written back). A name is the
 *  page's two inks, one per appearance; a triple is painted as written in
 *  both. */
export type TextColor = MarkColor | `#${string}`;

/** The alignment tokens in the order a control should offer them. */
export const ALIGNMENTS: readonly Align[];
/** The line-spacing tokens, likewise. */
export const LINE_SPACINGS: readonly LineSpacing[];
/** The size steps, smallest first. */
export const SIZE_STEPS: readonly SizeStep[];
/** The four generic faces. */
export const FONT_FAMILIES: readonly FontFamily[];

/**
 * leaf's presentation vocabulary as CSS — one rule per token, the same text as
 * the package's `src/presentation.css`, which is the file a page built from a
 * leaf document links to. The editor injects these rules scoped to its own
 * surface; a host rendering leaf documents elsewhere can inject them as they
 * are.
 */
export const PRESENTATION_CSS: string;

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
  /**
   * The directives this host names — its catalogue, for a toolbar or an
   * Insert menu to list (`directiveItems()`) and run
   * (`insertDirectiveItem()`), each row dimmed by `capabilities().directives`.
   * Empty by default, which offers nothing: leaf has no free-form "insert
   * directive", since a name no one draws is only a placeholder. How each
   * one looks is its renderer's, `regionRenderers["directive:<name>"]`.
   */
  directives?: DirectiveItem[];
  /**
   * How a formula is edited. `"structure"`, the default: → or ← onto a
   * formula, or a click on its picture, opens it in place — `/` makes a
   * fraction, `^` and `_` scripts, `sqrt` a root, the arrows walk its slots —
   * and what it writes is TeX, spliced into the document. `"source"`: a
   * formula on the caret's line reveals its TeX to be typed, as it does
   * while the math module has not loaded, or under `markupMode("full")`.
   */
  mathEditing?: "structure" | "source";
  /**
   * How each kind of region is drawn, by region kind: `"math"`,
   * `"fence:<language>"` (`"fence:mermaid"`) or `"directive:<name>"`
   * (`"directive:embed"`). A renderer turns a region's content into a
   * picture the layout places — inline on the baseline for a formula in a
   * line, or as a block standing in for the region's rows.
   *
   * Leaf registers one for `"math"`, the typesetter; a host's entry replaces
   * it, and draws every formula, edited or not. Every other kind named here
   * stops being drawn as its source: core keeps each such fence or directive
   * a placeholder row (`set_owned_regions`), and the renderer's picture is
   * drawn there, with a caret stop before it and one after. A kind with no
   * renderer, or a region its renderer answers null for, keeps leaf's
   * placeholder row. `"directive:page-break"` is leaf's own and cannot be
   * named.
   *
   * A picture is kept by kind, content, style, size, ink and scheme, so a
   * region whose content stands is not drawn again; `redrawRegions()` asks
   * afresh. Two identical block regions are asked about separately, and an
   * `{element}` picture must be a different element for each. A renderer
   * that answers with a promise is shown as the placeholder until the
   * promise settles. One that throws, rejects, or answers with something
   * that is not a `RegionPicture` is reported on the console and drawn as
   * the placeholder. An inline region — a formula in a line, or an inline
   * directive (`:date[2026-10-12]`) — is drawn only from `{svg}`, on the
   * baseline, in place of the one-glyph atom core lays out for it.
   */
  regionRenderers?: Partial<Record<RegionKind, RegionRenderer>>;
  /**
   * Editors by region kind: `"math"`, a fenced block's, `"fence:"` and its
   * language (`"fence:mermaid"`), or a leaf directive's, `"directive:"` and
   * its name (`"directive:embed"`, `"directive:date"`) — block
   * (`::name[label]{attrs}`) or inline (`:name[label]{attrs}`). A kind is
   * edited exactly when it has an editor here. Each named kind is a region
   * core keeps a picture or a placeholder (`set_owned_regions`), drawn by its
   * renderer, and → in front of it, ← behind it, or a click on its picture
   * opens it (`open`), in one of two sessions:
   *
   * - a `RegionPictureSession`, which leaf runs: keys by name, typed text,
   *   clicks and the clipboard are routed to it, its picture is drawn where
   *   the region's was, and leaf draws its caret, its selection and any IME
   *   composition over it. The only kind an inline region can open in.
   * - a `RegionSession`, a view of the host's own (`element`) placed over a
   *   block region's rows, which has its own keys and focus.
   *
   * What either writes comes back as splices on the region's content. A
   * fence's content is its lines between the fences; a directive's is its
   * whole source. Leaf registers its own editor for `"math"` — formulas
   * edited as structure — once the math module is here; a host's `"math"`
   * replaces it. `"directive:page-break"` is leaf's own and cannot be named.
   */
  regionEditors?: Partial<Record<RegionKind, RegionEditor>>;
}

/** A region kind a host may name: `"math"`, a fence's or a directive's
 *  (block or inline). */
export type RegionKind = "math" | `fence:${string}` | `directive:${string}`;

/**
 * What a region is drawn as, in CSS pixels. `{svg}` is a standalone SVG
 * document at `width` × `height`, whose baseline — where an inline one sits
 * on the line — is `baseline` from its top (default: its bottom). A block
 * one is centred on the measure at its own size, scaled down to fit it.
 * `{element}` is an element of the host's, placed as a block standing in for
 * the region's rows, as a `contenteditable="false"` atom; what it does with
 * a click or a key of its own is the host's.
 */
export type RegionPicture =
  | { svg: string; width: number; height: number; baseline?: number }
  | { element: HTMLElement };

/** What a renderer is told about where its picture goes. */
export interface RegionRenderContext {
  /** The size of the text around the region, in CSS pixels per em. */
  size: number;
  /** The colour of that text, as a CSS hex (`"#1c1f26"`). */
  ink: string;
  /** Whether the surface is dark: its text is light. */
  dark: boolean;
}

/**
 * A region kind's renderer: its picture of `region`, or null for leaf's
 * placeholder — at once, or once a promise settles. `region` is a copy.
 */
export type RegionRenderer = (
  region: RegionView,
  ctx: RegionRenderContext,
) => RegionPicture | null | Promise<RegionPicture | null>;

/**
 * Where the caret enters a region being opened: its first stop, its last, or
 * nearest a point in CSS pixels from its drawing's top left.
 */
export type RegionEntry =
  | { at: "home" }
  | { at: "end" }
  | { at: "point"; x: number; y: number; size: number };

/**
 * One splice on a region's content: replace the UTF-8 bytes `[from, to)` —
 * offsets into the content as the splice before this one left it — with
 * `text`, written exactly.
 */
export interface RegionSplice {
  from: number;
  to: number;
  text: string;
}

/**
 * What leaf hands a host's editor when it opens a region: the way back. Every
 * call is ignored once the region has closed.
 */
export interface RegionHost {
  /** Write splices on the content, as one undo step. Ends a typing step. */
  edit(splices: RegionSplice[]): void;
  /** Write the whole content anew; leaf splices only what differs, as one
   *  undo step. For a fence the content is whole lines, each ending in `\n`
   *  (empty for an empty fence) — keep it so. */
  replace(content: string): void;
  /** Close the region, the caret in front of it or behind it, and give the
   *  focus back to the document. */
  leave(side: "before" | "after"): void;
  /** Undo or redo in the document. The region stays open, and a change to its
   *  content reaches the session as `reload`. */
  undo(): void;
  redo(): void;
  /**
   * Where an open picture session's caret is drawn, in viewport (client)
   * coordinates — to anchor a popover of the host's, a completion list or a
   * calendar, which leaf does not draw. Null before the picture is laid
   * out (during `open`; ask after it, as in a `requestAnimationFrame`), for
   * a session with no caret, for a host view, and once the region closed.
   */
  caretRect(): DOMRect | null;
  /** The open region's drawing — its picture, or the atom a host's view
   *  stands in — in viewport coordinates; null once it closed. */
  regionRect(): DOMRect | null;
}

/** A rect on a session's picture, in CSS pixels from its top left. */
export interface RegionRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * An open picture session's picture: `RegionPicture`'s `{svg}` form, in CSS
 * pixels, and where on it leaf draws the session's caret and selection.
 * Neither belongs in the SVG: leaf draws them over it, with its own blink,
 * `--leaf-caret` and `--leaf-sel`. `caret` is a bar at `x` (`width` 0 is
 * fine), `height` tall; null or absent draws none. A picture that is not
 * scaled is drawn at exactly this size; a block one scaled down to fit the
 * measure has its rects scaled with it.
 */
export interface RegionSessionPicture {
  svg: string;
  width: number;
  height: number;
  baseline?: number;
  caret?: RegionRect | null;
  selection?: RegionRect[];
}

/**
 * The name of a key leaf hands a picture session (`key`). An open set: leaf
 * may add names in any release, so a session answers null to a name it does
 * not know, and leaf closes the region and lets the key run. Leaf names:
 *
 * - `ArrowLeft` … `ArrowDown`: the arrows; with ⇧, `ExtendLeft` …
 *   `ExtendDown`
 * - `WordLeft`, `WordRight`: ⌥← / ⌥→ on a Mac, Ctrl← / Ctrl→ elsewhere
 * - `LineStart`, `LineEnd`: ⌘← / ⌘→ on a Mac, and Home / End
 * - `Home`, `End`: ⌘↑ / ⌘↓ on a Mac, Ctrl Home / Ctrl End
 * - `SelectAll`: ⌘A (Ctrl A)
 * - `Backspace`, `Delete`, `Escape`, `Enter`, `Tab`
 *
 * A printable character is not a key but text, through `type`. A key with
 * no name — a shortcut such as ⌘Z, F5 — is not asked about: it closes the
 * region and runs. Copy, cut and paste arrive as `copy` and `paste`.
 */
export type RegionKey =
  | "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown"
  | "ExtendLeft" | "ExtendRight" | "ExtendUp" | "ExtendDown"
  | "WordLeft" | "WordRight" | "LineStart" | "LineEnd" | "Home" | "End"
  | "Backspace" | "Delete" | "SelectAll" | "Escape" | "Enter" | "Tab"
  | (string & {});

/**
 * What a picture session answers an input with: splices it made on the
 * content (`edit`, applied through `edit_source` exactly, one undo step —
 * or, for typed text, part of the typing step), a move inside it that
 * changed only its picture (`moved`), or leaving it, the caret in front of
 * the region or behind it (`leave`).
 */
export type RegionOutcome =
  | { edit: RegionSplice[] }
  | { moved: true }
  | { leave: "before" | "after" };

/**
 * A region open in a session leaf runs and draws: what leaf's own formula
 * editor is, and what a host's editor may answer `open` with, for a region of
 * any kind, inline or block. Leaf has the focus (a hidden field at the
 * caret, which an input method composes into), names the keys, draws the
 * picture in the line or the block, and draws the caret, the selection and
 * marked text over it. Every method is synchronous.
 *
 * Undo: consecutive `type` edits are one undo step, as typing in the
 * document is; any other key, a click, a paste, a `host.edit`, leaving and
 * closing end it. Every other edit is a step of its own. ⌘Z is no name: it
 * closes the region and undoes in the document.
 */
export interface RegionPictureSession {
  /**
   * The picture at `size` CSS pixels per em, in `ink` (a CSS hex), with its
   * caret and selection. Asked again after every key, click and edit,
   * synchronously, so it must be cheap: a kind whose drawing is slow edits
   * through a view. Null draws the kind's renderer's picture.
   */
  picture(size: number, ink: string): RegionSessionPicture | null;
  /** A key, by name (`RegionKey`). Null: not the session's — leaf closes the
   *  region and lets the key run. Unknown names must answer null. */
  key?(name: RegionKey): RegionOutcome | null;
  /** Text typed, or committed by an input method — never text still being
   *  composed, which leaf shows itself. Null: closes the region, and the
   *  text goes into the document in front of it. */
  type?(text: string): RegionOutcome | null;
  /** A click on the open picture, at `x, y` CSS pixels from its top left at
   *  `size` pixels per em. Null counts as `{ moved: true }`. */
  click?(x: number, y: number, size: number): RegionOutcome | null;
  /** The selection as text, for copy and cut (which then sends `Delete`);
   *  null copies nothing. */
  copy?(): string | null;
  /** Paste `text`. Null refuses it, and nothing is pasted. */
  paste?(text: string): RegionOutcome | null;
  /** The content changed under the session — an undo, another writer.
   *  `false` closes the region. */
  reload?(content: string): boolean | void;
  /** The region closed: by a leave, a key it did not know, a click
   *  elsewhere, or because it went. */
  close?(): void;
  /** What a screen reader is told the open region holds — on opening and
   *  after each edit, politely, and as the picture's label. Default: the
   *  region's content. */
  accessibilityValue?(): string | null;
}

/** A region open in a host's editor: the view leaf places over it. Block
 *  regions only. */
export interface RegionSession {
  /** The view, placed over the region's rows in place of its drawing while it
   *  is open. Its keys, clicks, input and clipboard are its own. */
  element: HTMLElement;
  /** Called once the view is in the document, to take the focus. */
  focus?(): void;
  /** The content changed under the view — an undo, a redo, another writer.
   *  `false` closes the region. */
  reload?(content: string): boolean | void;
  /** The region closed: by `leave`, a click elsewhere, or because it went. */
  close?(): void;
}

/**
 * An editor for one kind of region. Closed, the region is drawn by its
 * kind's renderer (`EditorOptions.regionRenderers`); open, it is the
 * session `open` answers with: a picture session leaf runs, or — for a block
 * region only — a view of the host's own. An inline region whose editor
 * answers with a view is closed again, and the refusal reported on the
 * console.
 */
export interface RegionEditor {
  /** Open `region`, or answer null to leave it closed. */
  open(region: RegionView, entry: RegionEntry, host: RegionHost): RegionSession | RegionPictureSession | null;
}

/**
 * A directive's attributes as `insertDirective` takes them: the `{key, value}`
 * list a `DirectiveView` carries — an empty value is a bare attribute — or a
 * plain object of the same pairs.
 */
export type DirectiveAttrs = DirectiveAttr[] | Record<string, string>;

/** One row of a host's directive catalogue — see `EditorOptions.directives`. */
export interface DirectiveItem {
  /** Stable, as a tool id is (`"directive.embed"`), so a saved arrangement survives. */
  id: string;
  /** The directive's name — what `insertDirective` writes. */
  name: string;
  /** The row's text, localised by the host. */
  title?: string;
  /** A glyph for the row. */
  icon?: string;
  /** The `[label]` to write. djot has nowhere to put one and refuses it. */
  label?: string | null;
  /** The attributes to write. */
  attrs?: DirectiveAttrs;
  /**
   * Ask the author what to write — an embed's URL — and answer it, or null to
   * cancel. What it answers wins over `label` and `attrs`. The caret is read
   * before it is called, so a dialog that takes the focus does not move where
   * the directive goes.
   */
  ask?: (item: DirectiveItem) => Promise<{ label?: string | null; attrs?: DirectiveAttrs } | null>;
}

/** A host-painted range of the source — see `LeafEditor.setHighlights`. */
export type Highlight = HighlightIn;

/**
 * How the typesetter — a second wasm module, `@diaryx/leaf/math`, two
 * megabytes most documents never need — is loaded.
 */
export interface InitOptions {
  /**
   * `"lazy"` (the default): fetched on the first frame that shows a formula,
   * which is drawn as its placeholder (the `∑` atom, the `∑ tex` row) until
   * it lands, then repainted. `"eager"`: fetched now, alongside the editor's
   * own module, and `init` waits for both — though a typesetter that cannot
   * be fetched does not fail `init`; formulas draw as marked faults, and
   * `loadMath()` rejects with the reason. `"off"`: never; every formula stays
   * its placeholder.
   */
  math?: "lazy" | "eager" | "off";
  /**
   * Where its binary is — `wasmUrl`'s counterpart. By default fetched relative
   * to the module, which a bundler has moved, so a bundled host passes the URL
   * it emitted for `@diaryx/leaf/math/wasm`.
   */
  mathUrl?: string | URL;
}

/**
 * A framework-agnostic rich-text editor over a `leaf_core::Doc`, compiled to
 * wasm. Renders proportionally (real body font, sized headings, monospace code)
 * while core stays the authority on text, wrapping, and caret math.
 *
 * `LeafEditor.init()` must resolve before the first construction.
 */
export class LeafEditor {
  /**
   * Load and instantiate the wasm module once. `wasmUrl` overrides its
   * location: by default the binary is fetched relative to the package's own
   * module, which a bundler has moved, so a bundled host passes the URL it
   * emitted for `@diaryx/leaf/wasm` (in Vite, `import url from
   * "@diaryx/leaf/wasm?url"`).
   */
  static init(wasmUrl?: string | URL, opts?: InitOptions): Promise<void>;

  /**
   * Fetch the typesetter now rather than on the first formula — what
   * `init(url, { math: "eager" })` does, for a host that decides later.
   * Resolves when formulas can be typeset; rejects if the module could not
   * be loaded, after which every formula is drawn as a marked fault with the
   * reason in its title. `url` is `InitOptions.mathUrl`.
   */
  static loadMath(url?: string | URL): Promise<void>;

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
  /** `markSaved` for a write that took a while: `saved` is the `source()` read before writing. */
  markSavedAs(saved: string): void;
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
   * Words, characters, and paragraphs over the whole document, counted over the
   * text a reader sees and not the markup that spells it (`**bold**` is one
   * word and four characters; a link is its label, not its destination).
   * O(document), so ask when the typing settles rather than on every keystroke.
   */
  counts(): TextCounts;
  /** The same statistics over the selection alone, or null with nothing selected. */
  selectionCounts(): TextCounts | null;
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
  /**
   * Toggle a fenced code block over the selection or the block at the caret; on
   * a blank line, open an empty one with the caret inside. Gate on
   * `capabilities().code_block`; `EditorState.codeBlock` lights the button.
   */
  toggleCodeBlock(): void;
  /**
   * Make the selected paragraphs — or the caret's — a verse, or the verse at
   * the caret prose again. Gate on `capabilities().verse`; `EditorState.verse`
   * lights the button.
   */
  toggleVerse(): void;
  toggleList(ordered: boolean): void;
  insertLink(dest: string): void;
  /**
   * Link to `dest` with `label` as the link's text. At a bare caret the label
   * is written, escaped for the body, and the caret lands after the link with
   * nothing selected; over a selection, or with the caret in a link it
   * re-points, this is `insertLink` and the label is ignored.
   */
  insertLinkLabelled(dest: string, label: string): void;
  /**
   * Give the list item at the caret a checkbox, or take its away. Gate on
   * `capabilities().task`; `EditorState.task` is non-null while the caret
   * stands in one.
   */
  toggleTaskItem(): void;
  /**
   * Tick or untick the task item at the caret — the keyboard half of the
   * click a rendered checkbox already answers. A no-op outside a task item.
   */
  toggleTaskChecked(): void;
  /** Write a footnote reference at the caret, and the definition it needs. */
  insertFootnote(): void;
  insertThematicBreak(): void;
  /** Insert block media; any selection becomes the alt text. */
  insertMedia(kind: "image" | "video" | "audio", destination: string, alt?: string): void;
  /**
   * Append media at the end of the document, as a block of its own — for
   * media that arrives (an import, a placed attachment) rather than media the
   * writer puts at the caret.
   */
  appendMedia(kind: "image" | "video" | "audio", destination: string, alt?: string): void;
  // ── the presentation vocabulary ────────────────────────────────────────
  // Alignment and spacing are the caret's block; size, face and colour are the
  // selection, or the block with no selection. `null` clears the key and
  // returns the property to the theme's own; a name outside the vocabulary
  // throws. Dim each control by its own `capabilities()` flag — `alignment`,
  // `line_spacing`, `font_size`, `font_family`, `text_color`, `page_break`,
  // `directives`, `move_block`.

  /** Align the caret's block, or `null` for the theme's default (left). */
  setAlignment(align?: Align | null): void;
  /** Set the block's line spacing as a multiple of the theme's, or `null`. */
  setLineSpacing(spacing?: LineHeight | null): void;
  /** Set the selection a step larger or smaller, or to an exact point size, or
   *  `null` for the theme's size. */
  setFontSize(size?: FontSize | null): void;
  /** Set the selection's face — a generic, or a family by name — or `null` for
   *  the theme's body face. */
  setFontFamily(font?: FontFace | null): void;
  /** Colour the selection's letters, by name or by hex triple, or `null` for
   *  the theme's text colour — not `setMarkColor`, which colours a highlight's
   *  background. */
  setTextColor(color?: TextColor | null): void;
  /** Write a page break at the caret (the `::page-break` leaf directive). */
  insertPageBreak(): void;
  /**
   * Write the leaf directive `::name[label]{attrs}` at the caret, placed as a
   * page break is. A name twig will not write, a label in djot, and a format
   * whose `capabilities().directives` is false are refused, writing nothing.
   */
  insertDirective(name: string, label?: string | null, attrs?: DirectiveAttrs): void;
  /** The host's catalogue as given in `EditorOptions.directives`, frozen. */
  directiveItems(): readonly DirectiveItem[];
  /**
   * Run a catalogue item, by id or itself: `ask()` if it has one, then write
   * it at the caret. Resolves true once written; false when the author
   * cancelled, the id names nothing, or core refused it.
   */
  insertDirectiveItem(item: string | DirectiveItem): Promise<boolean>;
  /**
   * Forget the pictures the renderers drew — all, or those of one `kind` —
   * and ask the renderers again for each region on the page, for a host
   * whose drawing changed without the document changing.
   */
  redrawRegions(kind?: RegionKind): void;
  /** The alignment in force at the caret, or null — which swatch is lit. */
  alignmentAtCaret(): Align | null;
  /** The line spacing in force at the caret, or null. */
  lineSpacingAtCaret(): LineHeight | null;
  /** The size at the caret — a step, or a point size — or null. */
  fontSizeAtCaret(): FontSize | null;
  /** The face at the caret — a generic, or a family name — or null. */
  fontFamilyAtCaret(): FontFace | null;
  /** The *text* colour at the caret, or null — not `EditorState.markColor`. */
  textColorAtCaret(): TextColor | null;

  /** Move the caret's block one place up — ⌥↑: above the block before it,
   *  and out of its container to just above it when it is the first block
   *  there. A list item goes with its children; the caret rides the block.
   *  Gate on `capabilities().move_block`. */
  moveBlockUp(): void;
  /** The mirror of `moveBlockUp` — ⌥↓. */
  moveBlockDown(): void;
  /** Move the block at source offset `from` to the boundary `to` — a block
   *  drag's drop, `to` from `dropTargetAt`. One undo step; the caret rides
   *  the block; a drop back onto the block's own boundary is nothing. */
  moveBlock(from: number, to: number): void;
  /** Where a block dragged over rendered row `row` would land: the offset to
   *  hand `moveBlock`, and the row to draw an indicator above
   *  (`rowEls.length` for a drop below everything). Null for a row with no
   *  block under it. */
  dropTargetAt(row: number): { offset: number; row: number } | null;

  tableInsertRow(below?: boolean): void;
  tableDeleteRow(): void;
  tableInsertColumn(right?: boolean): void;
  tableDeleteColumn(): void;
  tableMoveRow(down?: boolean): void;
  tableMoveColumn(right?: boolean): void;
  tableSetAlignment(align: "left" | "right" | "center" | "default"): void;
  /** A fresh table at the caret — `rows` body rows under a header, `cols`
   *  wide — with the caret left in its first header cell. Needs no table
   *  under the caret; `capabilities().table` is the whole gate. */
  insertTable(rows?: number, cols?: number): void;
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
// headings, ⌘⌥C code block, ⌘⇧7/8 lists, ⌘[ / ⌘] outdent/indent, ⌘E view,
// ⌘Z/⌘⇧Z/⌘Y history,
// ⌘⇧V paste as plain text, Tab/⇧Tab indent or the next cell. Ctrl for ⌘ off a
// Mac. ⌘-click follows a link or a footnote.
