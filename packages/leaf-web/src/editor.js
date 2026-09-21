// leaf-wasm — the framework-agnostic web editor.
//
// `LeafDoc` (from the wasm module) is only the document *model*: parse, edit,
// caret math, and a frame of style runs. This class is the *editor* around it —
// it renders those runs to the DOM and routes the browser's editing intent back
// into core. It is the web analogue of `leaf-tui`'s event loop + `leaf-gpui`'s
// widget, and owns exactly what those own: presentation and input, never the
// model. Core stays the single source of truth.
//
// ## Proportional, not a monospace grid
//
// The renderer is proportional — a real body font, headings sized by a scale
// ramp, code in a monospace family — the peer of `leaf-gpui`'s `style.rs`. Core
// wraps each line to a *column* budget and hands back rows plus a caret at
// `(row, col)`; a column is a semantic position, not a pixel. The browser shapes
// each row, and we map between core's columns and DOM text offsets (UTF-16, what
// a `Range` counts) with `DocView.caret_ch` / `set_selection` — wide-glyph
// correct, since a column and a character index diverge past CJK and emoji.
//
// ## The surface is contenteditable
//
// The rows live in one `contenteditable` element, so the browser owns the caret
// and the selection natively — which is what makes word/line select, drag,
// right-click Look Up, macOS force-click, mobile selection handles, and IME all
// behave like a real native field. But the rendered DOM is a *projection* of
// core's model (WYSIWYG hides markup; list markers and quote gutters are
// synthetic), so the browser must never actually mutate it. Instead:
//
//   * every `beforeinput` is `preventDefault`ed and its *intent* (the inputType:
//     insertText, deleteContentBackward, insertParagraph, formatBold, …) is
//     translated into a core operation; core edits, then we repaint and restore
//     the native selection to core's new caret;
//   * `selectionchange` mirrors the browser's caret/selection back into core, so
//     a command (bold, copy) always acts where the user actually is;
//   * IME is the exception the browser won't let us prevent — composition is
//     allowed to happen in the DOM and reconciled into core on `compositionend`.
//
// This is the CodeMirror-6 shape (native selection + intercepted beforeinput)
// rather than Monaco's hidden-textarea; it's the only way to get native
// selection and IME together, since both must live on one focused element.
//
//   import { LeafEditor } from "./src/editor.js";
//   await LeafEditor.init();                       // load the wasm once
//   const ed = new LeafEditor(el, { source, format: "markdown" });
//   ed.onChange((s) => updateToolbar(s));          // reflect active marks
//   ed.toggleBold();                               // imperative commands
//
// The class is deliberately headless of chrome: it renders and edits the
// document surface and exposes commands + a change event, leaving the toolbar,
// footer, and save affordances to the host app.

import init, { LeafDoc } from "../pkg/leaf_wasm.js";

/**
 * The presentation knobs, mirroring `leaf-gpui`'s `EditorStyle`. Everything here
 * is *look*, never model. Any subset can be passed to the constructor; omitted
 * fields keep the defaults below (which match gpui's: Helvetica-class body,
 * Menlo-class mono, 16/24, the same heading ramp). Colours default from the
 * stylesheet (light/dark aware) unless overridden here.
 */
export const DEFAULT_THEME = {
  /** Proportional body family — prose and headings shape with this. */
  fontFamily:
    '-apple-system, BlinkMacSystemFont, "Helvetica Neue", Helvetica, Arial, system-ui, sans-serif',
  /** Monospace family — inline `code` and fenced blocks, so columns line up. */
  monoFamily: 'ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace',
  /** Body font size in px. A heading is this scaled by `headingScale`. */
  fontSize: 16,
  /** Body line height in px. Heading rows scale taller in proportion. */
  lineHeight: 24,
  /**
   * The height of a between-blocks gap row, as a fraction of `lineHeight`. Core
   * spells a block boundary with an empty decoration row; at a full line box it
   * reads as a blank line the user never typed, so it's drawn short — ordinary
   * paragraph spacing. Set to `1` to restore the old full-line gap.
   */
  blockGapScale: 0.5,
  /**
   * How much larger than the body each heading level is drawn, `[h1…h6]`.
   * Headings are told apart by size and weight alone (no colour), so this ramp
   * is the whole hierarchy — 26 / 22 / 19 / 17 / 16 / 15 px against a 16px body.
   */
  headingScale: [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
};

/** The `<style>` is injected once for all editors on the page. */
const STYLE_ID = "leaf-editor-styles";

/** A representative sample for measuring the body font's average glyph width —
 *  lowercase-heavy so the wrap budget tracks real prose, not capitals. */
const WIDTH_SAMPLE = "the quick brown fox jumps over the lazy dog ";

/** How many times `_fitWidth` may correct the column budget against what the
 *  browser drew before it accepts the overflow. Each pass re-wraps the whole
 *  document, and in practice one is enough — the second exists for the case
 *  where re-wrapping surfaces a row that was previously hidden mid-line. */
const MAX_FIT_PASSES = 4;

/** How much of the overflow a correction pass has to remove to count as
 *  progress, as a fraction. Below this the binding constraint is a row nothing
 *  can wrap, and the pass is rolled back — see `_fitWidth`. */
const MIN_FIT_PROGRESS = 0.25;

/**
 * Zero-width space. A media row's only editable text is one of these on each
 * side of the element, giving the caret somewhere to land in front of and past
 * a picture that is itself not editable. See `_mediaRowEl`.
 */
const ZWSP = "​";

/**
 * The typesetter is a second wasm module, `leaf-math-wasm`, and it is not
 * loaded with the editor's own. It is two megabytes of RaTeX and KaTeX's
 * outlines — a third of what the binding weighed with it inside — and most
 * documents never show a formula. So the editor's module publishes each
 * formula as a `MathView` and knows nothing of TeX; the first frame that
 * carries one starts the fetch (`loadMath`), draws the formula as core's
 * placeholder meanwhile — the `∑` atom in a line, the `∑ tex` row for a
 * block — and every editor that was waiting repaints when the module lands.
 * A host that knows its documents have formulas asks for it up front
 * (`LeafEditor.init(url, { math: "eager" })`); one that never wants the
 * weight turns it off (`math: "off"`), and a formula stays its placeholder.
 *
 * `mathPolicy` is that choice; `mathUrl` is where the binary is, for a
 * bundled host that has moved it (`@diaryx/leaf/math/wasm`). `mathModule` is
 * the loaded module's exports, `mathLoading` the fetch in flight, `mathFailed`
 * why it did not arrive — after which a formula is drawn as TeX the
 * typesetter could not read, with that reason in its title, rather than
 * waited for again.
 */
let mathPolicy = "lazy";
let mathUrl = undefined;
let mathModule = null;
let mathLoading = null;
let mathFailed = null;
/** Editors with a formula on screen that the module has not reached yet;
 *  each repaints when it does. @type {Set<LeafEditor>} */
const MATH_WAITERS = new Set();

/**
 * Fetch and instantiate the typesetter once. Resolves when `typeset_math` is
 * callable; rejects — once, and every later call the same way — when the
 * module could not be fetched or instantiated. Either way the editors that
 * were waiting are repainted, so a formula settles into a picture or into a
 * marked fault, never into a placeholder for good.
 * @returns {Promise<void>}
 */
function loadMath() {
  if (mathModule) return Promise.resolve();
  if (!mathLoading) {
    const settle = () => {
      const waiting = [...MATH_WAITERS];
      MATH_WAITERS.clear();
      for (const ed of waiting) ed._mathSettled();
    };
    mathLoading = import("../pkg-math/leaf_math_wasm.js")
      .then((mod) => mod.default(mathUrl ? { module_or_path: mathUrl } : undefined).then(() => mod))
      .then(
        (mod) => {
          mathModule = mod;
          settle();
        },
        (e) => {
          mathFailed = e instanceof Error ? e : new Error(String(e));
          mathFailed.message = "the math typesetter did not load: " + mathFailed.message;
          settle();
          throw mathFailed;
        },
      );
  }
  return mathLoading;
}

/** One word for where the typesetter stands, for a row key to carry: a row
 *  drawn while it was absent must not be reused once it is here. */
function mathState() {
  return mathModule ? "ready" : mathFailed ? "failed" : "absent";
}

/**
 * Typeset formulas, keyed by everything that changes the picture — the TeX,
 * the style, the size, and the ink — so a frame that shows the same formula
 * as the last one typesets nothing. Shared across editors: a formula is the
 * same picture whichever surface asks. An entry is the module's `MathPicture`
 * copied out to a plain object, or an `Error` for TeX the typesetter could
 * not read, which is cached too so the fault is paid once.
 * @type {Map<string, {svg: string, width: number, height: number, depth: number} | Error>}
 */
const MATH_CACHE = new Map();
const MATH_CACHE_MAX = 512;

/**
 * Typeset `tex` at `size` CSS pixels in `color` (a CSS hex), through the
 * cache. `display` is TeX's display style, the `$$` switch. Returns the
 * picture; an `Error` for TeX the typesetter could not read, or for a
 * typesetter that could not be loaded; or `null` while the typesetter is
 * absent — not yet fetched, in flight, or turned off — which is the caller's
 * cue to draw the placeholder. Under the `lazy` policy the first `null` is
 * also what starts the fetch.
 */
function typesetMath(tex, display, size, color) {
  if (!mathModule) {
    if (mathFailed) return mathFailed;
    if (mathPolicy !== "off") loadMath().catch(() => {});
    return null;
  }
  const key = (display ? "D" : "T") + size + color + "\u0001" + tex;
  let hit = MATH_CACHE.get(key);
  if (hit === undefined) {
    if (MATH_CACHE.size >= MATH_CACHE_MAX) MATH_CACHE.clear();
    try {
      // A wasm-bindgen object over the module's heap: copy its four fields
      // out and free it, so the cache holds plain objects and no SVG stays
      // in that heap.
      const p = mathModule.typeset_math(tex, display, size, color);
      hit = { svg: p.svg, width: p.width, height: p.height, depth: p.depth };
      p.free();
    } catch (e) {
      hit = e instanceof Error ? e : new Error(String(e));
    }
    MATH_CACHE.set(key, hit);
  }
  return hit;
}

/** A standalone SVG document as a `data:` URL an `<img>` or a background can load. */
function svgDataUrl(svg) {
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}

/**
 * The character an inline formula's run holds — core's one-glyph atom. Its
 * advance in the row's font is what the run is stretched from to the picture's
 * width, so the caret's two homes land either side of the picture; see
 * `_mathRunEl`.
 */
const MATH_ATOM = "∑";

let wasmReady = null;


/**
 * The colours a highlight can be — the closed palette `setMarkColor` and
 * `highlight` accept, in the order a picker should show them (the spectrum,
 * which is the order core's own table is written in).
 *
 * These are the names the document records (`==\u{1F534} text==` is `"red"`),
 * the values `EditorState.markColor` reports, and the suffixes of the
 * `.leaf-mk-*` classes the renderer already paints with — so a picker built
 * from this list and the stylesheet cannot come to disagree. A name outside it
 * is an error rather than a silent clearing, since `null` (clear the colour)
 * and a typo are one keystroke apart and mean opposite things.
 */
export const MARK_COLORS = Object.freeze([
  "red", "orange", "yellow", "green", "blue", "purple", "brown",
]);

/**
 * The presentation vocabulary: the tokens the four non-colour properties take,
 * in the order a control should offer them. `MARK_COLORS` is the fifth list —
 * `setTextColor` takes exactly the names `setMarkColor` does, because a
 * frontend with a red for a highlight has a red for text.
 *
 * Each is a *name*, never a measurement, and each is what CSS already calls the
 * same thing — so `presentation.css` is a transcription of these and a control
 * built from the list cannot offer a token the document could not carry.
 * `null` is the vocabulary's absent value everywhere: it clears the key and
 * returns the property to the theme's own. There is no `left` and no `1`,
 * because those *are* absence.
 * @type {readonly string[]}
 */
export const ALIGNMENTS = Object.freeze(["center", "right", "justify"]);
/** The line-spacing multiples, as ratios of the theme's own line height. */
export const LINE_SPACINGS = Object.freeze(["1.15", "1.5", "2"]);
/** CSS's `<absolute-size>` keywords, less `medium` — which is absence. */
export const SIZE_STEPS = Object.freeze([
  "xx-small", "x-small", "small", "large", "x-large", "xx-large", "xxx-large",
]);
/** CSS's generic families, less `fantasy` and `system-ui`, which no author asks for. */
export const FONT_FAMILIES = Object.freeze(["serif", "sans-serif", "monospace", "cursive"]);

/**
 * Is this token one of the names above, or the exact value an author asked for?
 *
 * The vocabulary is open at one end: beside its names each key also takes a
 * measurement — `data-size="14pt"`, `data-font="Garamond"`,
 * `data-color="#c03030"` — which the author wrote knowing it is exact and not
 * portable. A name is a rule `presentation.css` already has; a value cannot be
 * enumerated by any stylesheet, so `_runEl` transcribes it into an inline
 * style beside the `data-` attribute the document carries either way.
 *
 * One predicate per property, here beside the list it reads, so that "is this
 * a name" is decided in exactly one place per key. Spacing needs none: the
 * three names *are* ratios, and `_rowEl` computes the row's line box from the
 * number whichever it was given.
 */
const isSizeStep = (token) => SIZE_STEPS.includes(token);
/** A generic family the stylesheet has a rule for, rather than a family named. */
const isFontGeneric = (token) => FONT_FAMILIES.includes(token);
/** One of the seven colour names, rather than a hex triple. */
const isColorName = (token) => MARK_COLORS.includes(token);

/** The name of the leaf directive a page break is — `Doc::insert_page_break`
 *  writes `::page-break`, and the row arrives in `DocView.directives` under
 *  this name whichever format spelled it. */
const PAGE_BREAK = "page-break";

export class LeafEditor {
  /**
   * Load and instantiate the wasm module. Call once before constructing any
   * editor; repeated calls share one instantiation. `wasmUrl` overrides where
   * the `.wasm` is fetched from (defaults to the file next to the JS glue).
   * @param {string | URL} [wasmUrl]
   * @returns {Promise<void>}
   */
  /**
   * Load and instantiate the wasm module once. `wasmUrl` overrides where it is
   * fetched from — a bundled host passes the URL it emitted for
   * `@diaryx/leaf/wasm`, since the default is relative to this module and a
   * bundler has moved it.
   *
   * The typesetter is a second module, and `opts.math` says when it is
   * fetched: `"lazy"` (the default) on the first frame that shows a formula,
   * `"eager"` now, alongside the editor's own — this promise then waits for
   * both — or `"off"` never, leaving every formula as its placeholder.
   * `opts.mathUrl` is `wasmUrl`'s counterpart for it, the URL a bundler
   * emitted for `@diaryx/leaf/math/wasm`. A typesetter that cannot be
   * fetched does not fail `init`: the editor works without it and draws each
   * formula as a marked fault with the reason; `loadMath()` is the call that
   * rejects with it.
   * @param {string | URL} [wasmUrl]
   * @param {{ math?: "lazy" | "eager" | "off", mathUrl?: string | URL }} [opts]
   */
  static init(wasmUrl, opts = {}) {
    if (opts.math !== undefined) {
      if (!["lazy", "eager", "off"].includes(opts.math)) {
        throw new Error(`math must be "lazy", "eager" or "off", not ${JSON.stringify(opts.math)}`);
      }
      mathPolicy = opts.math;
    }
    if (opts.mathUrl !== undefined) mathUrl = opts.mathUrl;
    if (!wasmReady) wasmReady = init(wasmUrl ? { module_or_path: wasmUrl } : undefined);
    const ready =
      mathPolicy === "eager" ? Promise.all([wasmReady, loadMath().catch(() => {})]) : wasmReady;
    return ready.then(() => undefined);
  }

  /**
   * Fetch the typesetter now rather than on the first formula — what
   * `init(url, { math: "eager" })` does, for a host that decides later (a
   * document is about to open that it knows has formulas). Resolves when
   * formulas can be typeset; rejects if the module could not be loaded, in
   * which case every formula is drawn as a marked fault with the reason.
   * @param {string | URL} [url]  where the binary is, as `init`'s `mathUrl`
   * @returns {Promise<void>}
   */
  static loadMath(url) {
    if (url !== undefined) mathUrl = url;
    return loadMath();
  }

  /**
   * @param {HTMLElement} container  the element to mount into (becomes the
   *   scroll viewport; its contents are replaced).
   * @param {{ source?: string, format?: string, theme?: Partial<typeof DEFAULT_THEME>,
   *           autofocus?: boolean, onChange?: (state: EditorState) => void }} [opts]
   */
  constructor(container, opts = {}) {
    if (!wasmReady) {
      throw new Error("LeafEditor.init() must be awaited before constructing an editor");
    }
    this.container = container;
    this.theme = { ...DEFAULT_THEME, ...(opts.theme || {}) };
    this._onChange = opts.onChange || null;
    /** @type {HTMLElement[]} the row elements, indexed 1:1 with core's rows. */
    this.rowEls = [];
    /** @type {{start: number, end: number, el: HTMLElement}[]} every rendered
     *  table-cell line and the source range it covers — how a caret inside a
     *  drawn grid is placed and read back. See `_tableEl`. */
    this.cellLines = [];
    /** @type {[number, number][]} the `rows` spans a drawn grid stands in for,
     *  so a caret row can be recognised as belonging to a table. */
    this.tableSpans = [];
    /** @type {{key: string, el: HTMLElement}[]} what the surface holds, in
     *  order, each under the key it was built from — the previous frame, for
     *  the next one to reuse from. See `render`. */
    this._keyed = [];
    /** Set when the DOM may have been touched by something other than
     *  `render` — an IME composition — so the next frame trusts nothing. */
    this._rebuildAll = false;
    this._composing = false;
    /** Guard so our own selection restores don't echo back through selectionchange. */
    this._settingSelection = false;
    /** Re-entrancy guards for the wrap-fitting loop, which repaints as it works. */
    this._fitting = false;
    this._fitQueued = false;
    this._refitQueued = false;
    /** Set once a row has been found that no budget can wrap narrower — see
     *  `_fitWidth`. Cleared by `refit`, since a resize can make room again. */
    this._acceptedOverflow = false;

    this.doc = new LeafDoc(opts.source ?? "", opts.format ?? "markdown");
    // A browser paints a picture in a line, so an inline formula arrives as an
    // atom to draw over rather than as its TeX — see _mathRunEl.
    this.doc.set_inline_pictures(true);
    // Every frame after the first is the change since the frame before —
    // the rows that differ, and where they go — spliced into the last whole
    // one by `_whole`. See `render`.
    this.doc.set_incremental_frames(true);

    ensureStylesheet();
    this._buildDom();
    this._applyTheme();
    this._bindEvents();

    // First paint at the wrap width the viewport implies.
    this._fitWidth();
    // Focus on construction is what a page with one editor wants, and what a
    // page with a form around it, or an editor below the fold, does not.
    if (opts.autofocus ?? true) this.focus();
  }

  // ── lifecycle ─────────────────────────────────────────────────────────────

  /** Give the editing surface keyboard focus. */
  focus() {
    this.contentEl.focus({ preventScroll: true });
  }

  /**
   * Tear down: remove listeners and empty the container. The `LeafDoc` wasm
   * handle is freed too. Safe to call once.
   */
  destroy() {
    if (this._destroyed) return;
    this._destroyed = true;
    for (const [t, fn, tgt] of this._listeners) tgt.removeEventListener(t, fn);
    this._resizeObs?.disconnect();
    MATH_WAITERS.delete(this);
    this.doc.free?.();
    this.container.innerHTML = "";
    this.container.classList.remove("leaf-editor");
  }

  /**
   * Replace the document with another, in place — for a host that opens a
   * second file into the same editor rather than tearing this one down. The
   * preferences a reader set on the editor (markup exposure, line flow,
   * read-only, the colour scheme) carry over; the history does not, since it
   * belonged to the old text. The caret starts at the top.
   * @param {string} source
   * @param {string} [format]  defaults to markdown, as the constructor does
   */
  load(source, format = "markdown") {
    const old = this.doc;
    const doc = new LeafDoc(source ?? "", format);
    doc.set_markup_mode(old.markup_mode());
    doc.set_line_flow(old.line_flow());
    doc.set_read_only(old.read_only());
    doc.set_inline_pictures(true);
    doc.set_incremental_frames(true);
    if (this._darkQuery) doc.set_color_scheme(this._darkQuery.matches ? "dark" : "light");
    this.doc = doc;
    // A frame of the new document is no change to the old one's rows.
    this._lastView = null;
    old.free?.();
    // A new document is a new measure: the surrender to an unwrappable row
    // belonged to the old one.
    this._acceptedOverflow = false;
    this._fitWidth();
  }

  /** Register (or replace) the change callback fired after every repaint. */
  onChange(cb) {
    this._onChange = cb;
    return this;
  }

  // ── host-facing model access ──────────────────────────────────────────────

  /** The current source text — for save / download / a source panel. */
  source() {
    return this.doc.source();
  }

  /** Whether the buffer differs from the last saved bytes. */
  isDirty() {
    return this._lastView?.dirty ?? false;
  }

  /** Which surface is showing: `"wysiwyg"` or `"source"`. */
  viewName() {
    return this._lastView?.view ?? "wysiwyg";
  }

  /** Clear the dirty flag after the host persisted `source()` its own way. */
  markSaved() {
    this.render(this.doc.mark_saved());
  }

  /** Whether the document refuses to change — see `setReadOnly`. */
  isReadOnly() {
    return this.doc.read_only();
  }

  /**
   * Turn the read-only gate on or off: a *reading* surface with the same
   * rendering, selection, and navigation as the editor, that refuses every
   * edit. Enforced in core, at the doors every mutation goes through; what
   * this layer adds is the chrome — the surface stops being contenteditable,
   * so no caret blinks in it and no keyboard rises to meet it on a phone,
   * while staying focusable and selectable.
   */
  setReadOnly(on) {
    on = !!on;
    const ce = this.contentEl;
    ce.setAttribute("contenteditable", on ? "false" : "true");
    ce.setAttribute("aria-readonly", on ? "true" : "false");
    // A non-editable div is not in the tab order on its own.
    if (on) ce.setAttribute("tabindex", "0");
    else ce.removeAttribute("tabindex");
    this.container.classList.toggle("leaf-readonly", on);
    this.render(this.doc.set_read_only(on));
  }

  /** The selected source, verbatim — markup included, as a copy takes it —
   *  or null with nothing selected. */
  selectedText() {
    this._syncFromDom();
    return this.doc.selected_text() ?? null;
  }

  /**
   * The selection cited out of the source — `{exact, prefix, suffix, start,
   * end}`: the selected bytes verbatim, up to `context` characters of what
   * surrounds them, and where they sit — enough for a host to anchor a
   * comment or a quotation that survives edits nearby. Null with nothing
   * selected.
   */
  selectionQuote(context = 30) {
    this._syncFromDom();
    return this.doc.selection_quote(context) ?? null;
  }

  /**
   * Words, characters, and paragraphs over the whole document — `{words,
   * characters, characters_without_spaces, paragraphs}`, the numbers a status
   * bar or an inspector puts next to a piece of writing.
   *
   * Counted over the text a reader sees rather than the markup that spells it:
   * `**bold**` is one word and four characters, a link is its label and not its
   * destination, a picture counts nothing, and frontmatter is not writing. The
   * same answer in both views.
   *
   * O(document) and not free (about 4 ms on a 45 KB file), so ask when the
   * typing settles rather than on every keystroke.
   */
  counts() {
    return this.doc.counts();
  }

  /** The same statistics over the selection alone — null with nothing
   *  selected. */
  selectionCounts() {
    this._syncFromDom();
    return this.doc.selection_counts() ?? null;
  }

  /**
   * Which formatting controls this document's format can actually spell — one
   * flag per toolbar button.
   *
   * Read once when a document opens: the answer depends only on the format, so
   * no edit can change it. A toolbar that ignores this stays correct — every
   * gesture refuses on its own — but it offers buttons that do nothing, which is
   * how the demo once shipped a superscript button no Markdown document can
   * spell. (The highlight was that example until core learned to author
   * `==text==`; `mark_color` is the flag that still separates the two
   * lightweight formats, since djot writes the highlight and no colour on it.)
   */
  capabilities() {
    return this.doc.capabilities();
  }

  /** Whether the format offers any door in at all — false only for a wholly
   *  parse-only one (XML), where a toolbar can be hidden outright. */
  isAuthorable() {
    return this.doc.authorable();
  }

  /** Whether the caret is inside a table — gate the grid controls on this *and*
   *  on `capabilities().table`, which asks whether this format's tables are
   *  editable at all. An HTML table answers yes to the first and no to the
   *  second. */
  caretInTable() {
    return this.doc.caret_in_table();
  }

  /** Whether the caret is inside a highlight — gate a colour picker on this
   *  *and* on `capabilities().mark_color`, which asks whether this format
   *  spells a colour at all. Djot writes `{=word=}` and answers yes to the
   *  first and no to the second. A selection is the third case a picker cares
   *  about: see `highlight`, which makes the highlight it colours. */
  caretInMark() {
    return this.doc.caret_in_mark();
  }

  // ── navigation ────────────────────────────────────────────────────────────

  /**
   * Put the caret at source `offset`, scrolled into view, and land the reader
   * on it. `end` bounds the block that was named — pass it whenever the
   * arrival is at a *block* rather than a point — and the rows it covers are
   * flashed, so the reader is told which words they were sent to.
   */
  reveal(offset, end = null) {
    // Landing the reader somewhere means the editor is where they are going;
    // the caret is only restored into a focused surface.
    this.focus();
    this.render(this.doc.set_selection_offsets(offset, offset));
    if (end == null || end <= offset) return;
    const rows = this.doc.row_range_for(offset, end);
    for (let r = rows.first; r <= rows.last; r++) {
      const rowEl = this.rowEls[r];
      if (!rowEl || !rowEl.isConnected) continue;
      rowEl.classList.remove("leaf-landed");
      void rowEl.offsetWidth; // restart the animation on a row still flashing
      rowEl.classList.add("leaf-landed");
    }
  }

  /**
   * Land on what a locator names — the fragment of an in-document link, a
   * heading's id — and say whether it named anything. Resolving `chapter.md`
   * to a document is the host's business; this is the rest of the address.
   */
  goTo(locator) {
    const landing = this.doc.locate(locator);
    if (!landing) return false;
    this.reveal(landing.start, landing.end);
    return true;
  }

  /** The footnote reference at source `offset` and the note it names, or
   *  null — `{label, text, offset, end}`, `text` null when nothing defines
   *  it. */
  footnoteAt(offset) {
    return this.doc.footnote_at(offset) ?? null;
  }

  /** `footnoteAt` for the caret. */
  footnoteAtCaret() {
    this._syncFromDom();
    return this.doc.footnote_at_caret() ?? null;
  }

  /** The footnote *definition* the caret stands in and where its first
   *  reference is — `{label, offset}` — or null. The return leg. */
  footnoteDefinitionAtCaret() {
    this._syncFromDom();
    return this.doc.footnote_definition_at_caret() ?? null;
  }

  /**
   * Follow the footnote the caret stands on: a reference down to its note, a
   * note back up to its first reference. Returns whether there was one to
   * follow — false on no footnote, on a reference nothing defines, and in a
   * note nothing cites, which all have nowhere to go.
   */
  followFootnote() {
    this._syncFromDom();
    const ref = this.doc.footnote_at_caret();
    if (ref && ref.offset != null) {
      this.reveal(ref.offset, ref.end ?? null);
      return true;
    }
    const def = this.doc.footnote_definition_at_caret();
    if (def && def.offset != null) {
      this.reveal(def.offset);
      return true;
    }
    return false;
  }

  // ── host highlights ───────────────────────────────────────────────────────

  /**
   * Replace the host-painted ranges wholesale and repaint — search hits, a
   * reviewer's annotations, whatever the host wants washed over a span of the
   * source. Each is `{start, end, id, color?}`: byte offsets, the host's own
   * name for it (handed back by `onActivateHighlight`), and an optional
   * `#RRGGBB` for the wash in place of the theme's. The whole set every time,
   * as core takes it; there is no add-one.
   * @param {{start: number, end: number, id: string, color?: string, marker?: string}[]} highlights
   */
  setHighlights(highlights) {
    this.render(this.doc.set_highlights(highlights));
  }

  /** The highlights as last set, sorted by start. */
  highlights() {
    return this.doc.highlights();
  }

  /** The id of the highlight covering source `offset`, or null. */
  highlightAt(offset) {
    return this.doc.highlight_at(offset) ?? null;
  }

  /**
   * What to do when the reader clicks a highlight — open the annotation it
   * stands for, step to the search hit. Called with the highlight's id. The
   * click still places the caret; this rides alongside.
   */
  onActivateHighlight(cb) {
    this._onActivateHighlight = cb;
    return this;
  }

  /**
   * Recompute the wrap width from the current viewport and repaint. Called
   * automatically on container resize; expose it for hosts that resize the
   * editor programmatically.
   */
  refit() {
    // A resize is the one event that can make room again, so it is also the one
    // that retracts a previous decision to live with an overflowing row.
    this._acceptedOverflow = false;
    this._fitWidth();
  }

  // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────
  // Each syncs core's selection from the browser first, so the command acts on
  // exactly what the user has selected, then repaints.

  toggleBold() { this._command((d) => d.toggle_bold()); }
  toggleItalic() { this._command((d) => d.toggle_italic()); }
  toggleCode() { this._command((d) => d.toggle_code()); }
  toggleMark() { this._command((d) => d.toggle_mark()); }
  /**
   * One press of a colour swatch: colour the highlight at the caret, or — over
   * a selection that isn't highlighted yet — highlight it and colour it, as
   * **one** undo step. `null` clears the colour, and over a plain selection
   * means simply "highlight this".
   *
   * `color` is one of `MARK_COLORS`. A bare caret in no highlight is left
   * alone, because there is nothing for a colour to belong to — which is what
   * `caretInMark()` and `EditorState.hasSelection` let a picker dim itself by.
   */
  highlight(color = null) { this._command((d) => d.highlight(color ?? undefined)); }
  /**
   * The exact gesture behind `highlight`: colour the highlight the caret is
   * already in, or clear its colour with `null`. Writes nothing where there is
   * no highlight — it does not make one.
   */
  setMarkColor(color = null) { this._command((d) => d.set_mark_color(color ?? undefined)); }
  toggleUnderline() { this._command((d) => d.toggle_underline()); }
  toggleStrike() { this._command((d) => d.toggle_strike()); }
  setParagraph() { this._command((d) => d.set_paragraph()); }
  /** Toggle the block to a heading of `level` (1–6); the active level toggles off to a paragraph. */
  setHeading(level) { this._command((d) => d.set_heading(level)); }
  toggleBlockquote() { this._command((d) => d.toggle_blockquote()); }
  toggleList(ordered) { this._command((d) => d.toggle_list(!!ordered)); }
  /**
   * Toggle a fenced code block over the selection or the block at the caret —
   * the rich view's one door into a code block, since a typed backtick is
   * escaped there. On a blank line it opens an empty block with the caret
   * inside. Gate on `capabilities().code_block`; `EditorState.codeBlock` is
   * lit while the caret stands in one.
   */
  toggleCodeBlock() { this._command((d) => d.toggle_code_block()); }
  insertLink(dest) { this._command((d) => d.insert_link(dest)); }
  toggleTaskItem() { this._command((d) => d.toggle_task_item()); }
  toggleTaskChecked() { this._command((d) => d.toggle_task_checked()); }
  /** Write a footnote reference at the caret, and the definition it needs. */
  insertFootnote() { this._command((d) => d.insert_footnote()); }
  insertThematicBreak() { this._command((d) => d.insert_thematic_break()); }
  /** Insert block media — `kind` is `"image"`, `"video"`, or `"audio"`. Any
   *  selection becomes the alt text. */
  insertMedia(kind, destination, alt = "") {
    this._command((d) => d.insert_media(kind, destination, alt));
  }
  // ── the presentation vocabulary ───────────────────────────────────────────
  // Alignment and spacing are the block's; size, face and colour are the
  // selection's, or the block's with no selection. Each takes one of the
  // vocabulary's names — or, for all but alignment, the exact value the names
  // cannot spell — or `null` to clear the key and return the property to the
  // theme's own; and each refuses a token outside its grammar, because a typo
  // and "clear this" are one keystroke apart and mean opposite things. A value
  // comes back from the query in its canonical spelling. Dim each control by
  // its own `capabilities()` flag.

  /** Align the caret's block: `"center"`, `"right"`, `"justify"`, or `null`
   *  for the theme's default, which is left. */
  setAlignment(align = null) { this._command((d) => d.set_alignment(align ?? undefined)); }
  /** Open the caret's block up to a multiple of the theme's line height:
   *  `"1.15"`, `"1.5"`, `"2"`, any other positive decimal (`"1.3"`), or `null`
   *  for the theme's own. */
  setLineSpacing(spacing = null) { this._command((d) => d.set_line_spacing(spacing ?? undefined)); }
  /** Set the selection a step larger or smaller — one of `SIZE_STEPS` — or to
   *  an exact size in points (`"14pt"`, `"13.5pt"`), which is that many points
   *  and not the heading's ramp scaled; `null` is the theme's size. `pt` is
   *  the only unit. With no selection it is the whole block. */
  setFontSize(size = null) { this._command((d) => d.set_font_size(size ?? undefined)); }
  /** Set the selection's face — one of `FONT_FAMILIES`, or a family the author
   *  names (`"Garamond"`), which draws where it is installed and falls back to
   *  the page's own face where it is not; `null` is the theme's body face.
   *  `setFontSize`'s peer. */
  setFontFamily(font = null) { this._command((d) => d.set_font_family(font ?? undefined)); }
  /**
   * Colour the selection's *letters* — one of `MARK_COLORS`, a hex triple
   * (`"#c03030"`, painted as written in both appearances), or `null` for the
   * theme's text colour. Not `setMarkColor`, which colours a highlight's
   * background: same seven names, and the two never collide because a mark is a
   * mark and a span is a span.
   */
  setTextColor(color = null) { this._command((d) => d.set_text_color(color ?? undefined)); }
  /** Write a page break at the caret — the `::page-break` leaf directive, drawn
   *  here as a dashed rule and a `break-after: page` when the page is printed. */
  insertPageBreak() { this._command((d) => d.insert_page_break()); }

  /** The alignment in force at the caret, or null for the theme's default —
   *  which swatch of an alignment control is lit. */
  alignmentAtCaret() {
    this._syncFromDom();
    return this.doc.alignment_at_caret() ?? null;
  }
  /** The line spacing in force at the caret, or null. */
  lineSpacingAtCaret() {
    this._syncFromDom();
    return this.doc.line_spacing_at_caret() ?? null;
  }
  /** The size at the caret — a step, or a point size in its canonical
   *  spelling — or null for the theme's size, which is also the answer where
   *  the document carries something the grammar does not cover. */
  fontSizeAtCaret() {
    this._syncFromDom();
    return this.doc.font_size_at_caret() ?? null;
  }
  /** The face at the caret, or null for the theme's body face. */
  fontFamilyAtCaret() {
    this._syncFromDom();
    return this.doc.font_family_at_caret() ?? null;
  }
  /** The *text* colour at the caret, or null — not `EditorState.markColor`,
   *  which is the colour of the highlight around it. */
  textColorAtCaret() {
    this._syncFromDom();
    return this.doc.text_color_at_caret() ?? null;
  }

  tableInsertRow(below = true) { this._command((d) => d.table_insert_row(below)); }
  tableDeleteRow() { this._command((d) => d.table_delete_row()); }
  tableInsertColumn(right = true) { this._command((d) => d.table_insert_column(right)); }
  tableDeleteColumn() { this._command((d) => d.table_delete_column()); }
  tableMoveRow(down = true) { this._command((d) => d.table_move_row(down)); }
  tableMoveColumn(right = true) { this._command((d) => d.table_move_column(right)); }
  /** `"left"`, `"right"`, `"center"`, or `"default"`. */
  tableSetAlignment(align) { this._command((d) => d.table_set_alignment(align)); }
  /** A fresh table at the caret: `rows` body rows under a header, `cols` wide. */
  insertTable(rows = 2, cols = 2) { this._command((d) => d.insert_table(rows, cols)); }
  /** The line-wrapping preference: `"fold"` or `"preserve"`. */
  lineFlow() { return this.doc.line_flow(); }
  setLineFlow(mode) { this._command((d) => d.set_line_flow(String(mode))); }
  undo() { this._command((d) => d.undo()); }
  redo() { this._command((d) => d.redo()); }
  selectAll() { this._command((d) => d.select_all()); }
  /** Switch between the rendered WYSIWYG surface and the raw source. */
  toggleView() { this._command((d) => d.toggle_view()); }

  /**
   * The markup-exposure preference: `"none"`, `"shortcuts"`, or `"full"`.
   */
  markupMode() { return this.doc.markup_mode(); }

  /**
   * Set the markup-exposure preference. `"none"` (the clean default Diaryx
   * ships) hides delimiters and keeps typed syntax literal; `"shortcuts"` still
   * hides them but lets typing author markup; `"full"` also shows the caret
   * line's raw markup, whose delimiters arrive as runs with
   * `role: "delimiter"`.
   */
  setMarkupMode(mode) { this._command((d) => d.set_markup_mode(String(mode))); }

  /** Sync core's selection from the DOM, run a model op, and repaint. */
  _command(op) {
    this._syncFromDom();
    this.render(op(this.doc));
  }

  // ── DOM scaffolding ───────────────────────────────────────────────────────

  _buildDom() {
    const c = this.container;
    c.classList.add("leaf-editor");
    c.innerHTML = "";

    // The one contenteditable surface: the browser owns caret + selection here,
    // and every edit intent is intercepted (see _bindEvents). The a11y hints and
    // input attributes live on it because it is the focus target.
    this.contentEl = el("div", "leaf-content");
    this.contentEl.setAttribute("contenteditable", "true");
    this.contentEl.setAttribute("role", "textbox");
    this.contentEl.setAttribute("aria-multiline", "true");
    this.contentEl.setAttribute("aria-label", "leaf document");
    this.contentEl.setAttribute("autocorrect", "on");
    this.contentEl.setAttribute("autocapitalize", "sentences");
    this.contentEl.spellcheck = false;

    // Hidden probe for the body font's average glyph width (wrap budget).
    this.measureEl = el("span", "leaf-measure");
    this.measureEl.textContent = WIDTH_SAMPLE;

    c.appendChild(this.contentEl);
    c.appendChild(this.measureEl);
  }

  _applyTheme() {
    const t = this.theme;
    const s = this.container.style;
    s.setProperty("--leaf-font", t.fontFamily);
    s.setProperty("--leaf-mono", t.monoFamily);
    s.setProperty("--leaf-size", t.fontSize + "px");
    s.setProperty("--leaf-line", t.lineHeight + "px");
    // Per-level heading sizes, precomputed from the ramp so a CSS rule per level
    // can pick one up. Line height tracks size (the row grows proportionally).
    this._ratio = t.lineHeight / t.fontSize;
    for (let i = 0; i < 6; i++) {
      s.setProperty(`--leaf-h${i + 1}-size`, (t.fontSize * t.headingScale[i]).toFixed(2) + "px");
    }
  }

  // ── rendering ─────────────────────────────────────────────────────────────

  /**
   * Paint one frame. Takes a `DocView` (returned by every model method), rebuilds
   * the rows, restores the native selection to the model's caret/selection, and
   * fires the change callback.
   * @param {import("../pkg/leaf_wasm.js").DocView} view
   */
  render(view) {
    if (!view) return; // an unhandled key returns undefined; nothing to repaint
    view = this._whole(view);
    this._lastView = view;
    // Which surface this is, for the stylesheet: the source view sets every
    // row in the mono family at one size, and colours by role the way a
    // fenced block colours by token. A class on the container rather than on
    // each row, so the rows the pool reuses across the flip are right as they
    // stand.
    this.container.classList.toggle("leaf-source", view.view === "source");

    // The previous frame's elements, pooled under the key each was built from.
    // A row whose key comes up again is reused as it stands, moved if it has to
    // be; only a row that is new to the frame is built. Rebuilding everything
    // on every keystroke was the simple thing and cost the whole document each
    // time — forty milliseconds of node creation for a few thousand rows, on a
    // key that changed one of them. A key spells everything the row's DOM is
    // made from (its text and styles, its block kind, its neighbours' where
    // the CSS looks at them) and nothing else — not the source offsets, which
    // every row after an edit shifts by, and which are carried as a property
    // on each run instead so they can be refreshed in place.
    const force = this._rebuildAll;
    this._rebuildAll = false;
    const pool = new Map();
    if (!force) {
      for (const { key, el } of this._keyed) {
        let q = pool.get(key);
        if (!q) pool.set(key, (q = []));
        q.push(el);
      }
    }
    const take = (key) => {
      const q = pool.get(key);
      return q && q.length ? q.shift() : null;
    };
    /** @type {{key: string, el: HTMLElement}[]} */
    const keyed = [];
    this.rowEls = [];
    this.cellLines = [];
    this.tableSpans = [];
    // Block media by its first row, so `_rowEl` can build a real element there
    // instead of core's `🖼`/`🎬`/`🔊` placeholder glyphs, and skip the blank
    // filler rows underneath it.
    const mediaAt = new Map();
    const covered = new Set();
    for (const m of view.media || []) {
      mediaAt.set(m.row, m);
      for (let r = m.row + 1; r < m.row + m.rows; r++) covered.add(r);
    }
    // Formulas, two ways. A display block is a media row's shape — a real
    // element on the first row, the fillers skipped. An inline atom rides its
    // row: `_runEl` finds it by the source offset the `math` run carries, and
    // the row's key carries its TeX so a reused row never shows a stale
    // picture. What the ink is comes from the surface's own text colour, so
    // a theme change re-typesets through the cache.
    const mathBlockAt = new Map();
    const mathInlineAt = new Map();
    // Every row a formula stands on, in whatever state its picture is: the
    // row's key carries the typesetter's state, so a row drawn while the
    // module was still on its way is rebuilt when it lands.
    const mathRows = new Set();
    const ink = this._inkColor();
    for (const m of view.math || []) {
      mathRows.add(m.row);
      if (m.inline) {
        mathInlineAt.set(m.src, m);
        continue;
      }
      const pic = typesetMath(m.tex, m.display, this.theme.fontSize, ink);
      // TeX that will not typeset stays core's `∑ tex` placeholder row, drawn
      // as text with the fault in its title — see `_runEl`. So does every
      // block while the typesetter is absent; `_mathSettled` repaints then.
      if (pic && !(pic instanceof Error)) {
        mathBlockAt.set(m.row, m);
        for (let r = m.row + 1; r < m.row + m.rows; r++) covered.add(r);
      }
    }
    if (mathRows.size && mathState() === "absent") MATH_WAITERS.add(this);
    this._mathInline = mathInlineAt;
    this._ink = ink;
    // Tables the same way, but replacing the *whole* span rather than the first
    // row of it: core draws a table as a monospace box-glyph picture, which is
    // exact on a fixed-cell surface and shears in a proportional font — the `│`
    // of one row and the `│` of the next land at different x. So the picture
    // rows are all skipped and one real grid is built from the structural
    // `TableView` core publishes alongside them.
    const tableAt = new Map();
    for (const t of view.tables || []) {
      tableAt.set(t.start_row, t);
      this.tableSpans.push([t.start_row, t.end_row]);
      for (let r = t.start_row + 1; r < t.end_row; r++) covered.add(r);
    }
    // The one leaf directive this frontend has a drawing for: a page break,
    // which is a rule across the measure rather than the `⧉ page-break`
    // placeholder every other directive gets. The narrow half of the host hook
    // `docs/tasks/web-directive-hook.md` argues for — the vocabulary here is
    // leaf's own, so it needs no hook to know what the name means.
    const breakAt = new Set();
    for (const d of view.directives || []) {
      if (d.name === PAGE_BREAK) breakAt.add(d.start_row);
    }
    for (let i = 0; i < view.rows.length; i++) {
      // A filler row core reserved under the media: the element built on the
      // first row already occupies that height in the flow, so drawing these
      // would add blank lines below it. `rowEls` still needs an entry per core
      // row — every caret/click path indexes it by core's row number — so the
      // element is created and tracked, just never put in the document.
      if (covered.has(i)) {
        // `_rowEl` records it in `rowEls` itself, as it does for every row —
        // pushing the return value here as well would double-count the fillers
        // and shift every later row's index off by one.
        this._rowEl(view.rows[i], i, view.rows, null, true);
        continue;
      }
      const table = tableAt.get(i);
      if (table) {
        // The picture row still needs its `rowEls` entry — every caret and click
        // path indexes that array by core's row number — but it is the grid that
        // goes in the document. A caret inside the table is placed by source
        // offset instead; see `_restoreSelection`.
        this._rowEl(view.rows[i], i, view.rows, null, true);
        const key = tableKey(table);
        let el = take(key);
        if (el) this._adoptTable(el, table);
        else el = this._tableEl(table);
        keyed.push({ key, el });
        continue;
      }
      const row = view.rows[i];
      const media = mediaAt.get(i) || null;
      const mathBlock = mathBlockAt.get(i) || null;
      const pageBreak = breakAt.has(i);
      const runs = canonicalRuns(row.runs);
      const key = pageBreak
        ? pageBreakKey(row)
        : media
          ? mediaKey(media, row)
          : mathBlock
            ? mathBlockKey(mathBlock, row, ink)
            : rowKey(row, runs, i, view.rows) + mathRowKey(runs, mathInlineAt, ink, mathRows.has(i));
      let el = take(key);
      if (el) {
        this._adoptRow(el, runs);
        this.rowEls.push(el);
      } else {
        el = this._rowEl(row, i, view.rows, media, false, runs, pageBreak, mathBlock);
      }
      keyed.push({ key, el });
    }
    this._reconcile(keyed);
    this._keyed = keyed;

    this._restoreSelection(view);
    this._scrollCaretIntoView(view);
    this._emitChange(view);
    this._checkFit();
  }

  /**
   * The frame as the whole document. Core answers every gesture with the
   * change since the frame before — the rows that differ, `row_start` and
   * `replaced` for where they go, and `src_shift` for how far the source of
   * every row below moved (`set_incremental_frames`) — because lifting every
   * row of a long document across the wasm boundary on every keystroke cost
   * more than the repaint. A change is spliced into the last whole frame,
   * which is `_lastView`; a whole frame (`basis` of 0) is itself. A change
   * against a frame this editor did not keep — a gesture whose frame was
   * dropped on the floor, as a drop's own reselection is — means the rows
   * here have lost step, and core is asked for a whole frame instead.
   *
   * The rows before the span are the same objects as before, and the rows
   * after it are too, moved in place: each run's `src` is the offset the
   * renderer refreshes on a reused element (`_adoptRow`), and nothing else
   * about those rows changed.
   * @param {import("../pkg/leaf_wasm.js").DocView} view
   * @returns {import("../pkg/leaf_wasm.js").DocView}
   */
  _whole(view) {
    if (!view.basis) return view;
    const last = this._lastView;
    if (!last || last.frame !== view.basis) return this.doc.view();
    const start = view.row_start;
    const rows = last.rows.slice(0, start);
    for (const row of view.rows) rows.push(row);
    const kept = last.rows.slice(start + view.replaced);
    if (view.src_shift) {
      for (const row of kept) for (const run of row.runs) run.src += view.src_shift;
    }
    for (const row of kept) rows.push(row);
    return { ...view, rows, basis: 0, row_start: 0, replaced: 0, src_shift: 0 };
  }

  /**
   * Put the surface's children in the order `keyed` gives, touching only what
   * moved: a child already in place is stepped over, one that belongs earlier
   * is moved up, a new one is inserted, and whatever the previous frame had
   * that this one doesn't is removed off the end.
   *
   * Everything before the cursor has been placed, so an element the frame
   * still wants is always at or after it — which is why what is left past the
   * last placed child is exactly the set to discard.
   */
  _reconcile(keyed) {
    const parent = this.contentEl;
    let cursor = parent.firstChild;
    for (const { el } of keyed) {
      if (el === cursor) {
        cursor = cursor.nextSibling;
        continue;
      }
      parent.insertBefore(el, cursor);
    }
    while (cursor) {
      const next = cursor.nextSibling;
      cursor.remove();
      cursor = next;
    }
  }

  /** Refresh what a reused row carries that its key leaves out: the source
   *  offset of each run, which every row past an edit shifts by. */
  _adoptRow(rowEl, runs) {
    const spans = rowEl._leafRuns;
    if (!spans) return; // a media row: no runs of its own
    for (let i = 0; i < spans.length; i++) spans[i]._src = runs[i].src;
  }

  /** `_adoptRow` for a reused grid: re-register its cell lines under the source
   *  ranges this frame gives them, and refresh each run's offset. The key
   *  guarantees the grid's shape — rows, cells, lines, runs — is unchanged, so
   *  the two walks stay in step. */
  _adoptTable(table, t) {
    const lines = table._leafLines;
    let n = 0;
    for (const row of t.grid) {
      for (const cell of row.cells) {
        for (const line of cell.lines) {
          const lineEl = lines[n++];
          this._adoptRow(lineEl, canonicalRuns(line.runs));
          this.cellLines.push({ start: line.start, end: line.end, el: lineEl });
        }
      }
    }
  }

  /**
   * Build one row element from a `Row`.
   *
   * `media` is the `MediaView` whose placeholder starts on this row, if any —
   * the row then carries a real element instead of core's label glyphs.
   * `detached` marks a filler row that is tracked but never inserted (see
   * `render`). `runs` is the row's runs with selection splits merged back
   * (`canonicalRuns`), passed in when the caller has already done it.
   * `pageBreak` marks the one leaf directive this frontend draws itself.
   */
  _rowEl(
    row,
    i,
    rows,
    media = null,
    detached = false,
    runs = null,
    pageBreak = false,
    mathBlock = null
  ) {
    const div = el("div", "leaf-row");
    if (detached) {
      this.rowEls.push(div);
      return div;
    }
    if (media) return this._mediaRowEl(div, media, row);
    if (mathBlock) return this._mathRowEl(div, mathBlock, row);
    if (pageBreak) return this._pageBreakRowEl(div, row);
    runs ??= canonicalRuns(row.runs);
    // A block-boundary gap row holds no caret. Left editable, the browser's own
    // ArrowUp/ArrowDown lands in its short line box on the way between blocks, so
    // a step from a paragraph into the list or code block below it moves only
    // sometimes. Marking it non-editable (like the code-language label) makes
    // native vertical motion step straight over it to the next real line.
    const gap = isBlockGap(row);
    if (gap) div.setAttribute("contenteditable", "false");
    // The block's half of the presentation vocabulary, on every row the block
    // emits. The alignment token *is* the CSS class and the spacing token is
    // the attribute value, so `presentation.css` — the same rules this editor
    // injects, scoped to its surface — does the aligning with nothing
    // translated on the way.
    if (row.align) div.classList.add(row.align);
    if (row.line_height) div.dataset.lineHeight = row.line_height;
    // The spacing is a multiple of the *theme's* line height, not of the font
    // size: opening a paragraph to 1.5 means half again the leading this
    // editor draws with, whatever that is. So the row's line box is computed
    // here, in px, the way a heading's already is — an inline style, which
    // also settles which of it and the stylesheet's unitless ratio wins.
    //
    // A ratio the menu does not offer (`1.3`) is a number like the three that
    // it does, so it goes through this same arithmetic and needs no path of
    // its own: the token *is* the multiple, which is why spacing has no
    // is-it-a-name predicate beside the other three. Anything the grammar does
    // not cover reads as the theme's own.
    const spacing = row.line_height ? Number(row.line_height) || 1 : 1;
    // Sizing the *whole* row from its heading level (not per run) mirrors gpui
    // shaping a heading's line at one size: an inline `code` run inside a
    // heading still reads at the heading's size.
    if (row.heading) {
      const size = this.theme.fontSize * this.theme.headingScale[Math.min(row.heading, 6) - 1];
      div.classList.add("h");
      div.style.fontSize = size + "px";
      div.style.lineHeight = size * this._ratio * spacing + "px";
    } else if (spacing !== 1) {
      div.style.lineHeight = this.theme.lineHeight * spacing + "px";
    }
    // Keep empty rows occupying their line so the caret has somewhere to sit —
    // except a block-boundary gap row (empty, holds no caret), drawn short so a
    // paragraph break reads as spacing rather than a blank line.
    div.style.minHeight = (row.heading
      ? this.theme.fontSize * this.theme.headingScale[Math.min(row.heading, 6) - 1] * this._ratio * spacing
      : gap
        ? this.theme.lineHeight * this.theme.blockGapScale * gapScale(row)
        : this.theme.lineHeight * spacing) + "px";

    if (row.code) {
      div.classList.add("code");
      if (i === 0 || !rows[i - 1].code) div.classList.add("code-first");
      if (i === rows.length - 1 || !rows[i + 1].code) div.classList.add("code-last");
      if (row.code_lang) {
        // contenteditable=false + excluded from the text walkers, so it's not part
        // of the row's editable text and never counts toward an offset.
        const lab = el("span", "leaf-code-lang");
        lab.setAttribute("contenteditable", "false");
        lab.textContent = row.code_lang;
        div.appendChild(lab);
      }
    }

    // The row's font size, for an inline formula set at the size of the text
    // around it — a heading's ramp, else the body's.
    const fontPx = row.heading
      ? this.theme.fontSize * this.theme.headingScale[Math.min(row.heading, 6) - 1]
      : this.theme.fontSize;
    div._leafRuns = runs.map((run) => div.appendChild(this._runEl(run, fontPx)));

    // A contenteditable block needs a placeholder to hold a caret when it has no
    // text of its own (an empty paragraph) — but a non-editable gap row holds no
    // caret, so it needs none.
    if (runs.length === 0 && !gap) div.appendChild(document.createElement("br"));

    this.rowEls.push(div);
    return div;
  }

  /**
   * Build the row for a block image, video, or audio: a real element in place of
   * the placeholder glyphs core drew for surfaces that can't paint one.
   *
   * The element is `contenteditable="false"` — an atom the browser will not let
   * the caret enter, edit, or split, the same trick the code-language label
   * uses. Around it sit two zero-width spaces, and they are load-bearing rather
   * than cosmetic: they are the row's only editable text, so they give the caret
   * a place to land on each side of the media. Core publishes exactly two stops
   * for a media row (one in front, one past it) and the two spaces are what
   * those map onto — with no text at all, `rangeAtOffset` would collapse both to
   * the row start and the caret could never be seen *after* a picture.
   *
   * Nothing here calls `set_media_rows`. On a proportional surface the element
   * sits in the normal flow and the row grows to fit it, exactly as leaf-gpui
   * lays images out in pixels and leaves core's reservation at one row. The
   * measure-and-report loop is for a fixed-cell host (the terminal), and using
   * it here would reserve blank rows the CSS has already accounted for.
   */
  _mediaRowEl(div, media, row) {
    div.classList.add("leaf-media-row");
    // What core thinks this row's text is, in the units a caret column is
    // counted in. The row *drawn* here is a picture and two zero-width spaces;
    // the row core is addressing is the label glyphs it put there for a surface
    // that can't paint one (`🖼 a leaf`). The two lengths have nothing to do with
    // each other, and every offset that crosses between them goes through this
    // number. See `atomCoreLen`.
    div.dataset.leafCoreLen = String(
      (row?.runs || []).reduce((n, r) => n + r.text.length, 0)
    );
    div.appendChild(document.createTextNode(ZWSP));

    let node;
    if (media.kind === "image") {
      node = el("img", "leaf-media");
      node.src = media.src;
      node.alt = media.alt;
    } else {
      node = el(media.kind === "audio" ? "audio" : "video", "leaf-media");
      node.controls = true;
      // A `<video>`'s poster is a still the browser can show before the movie
      // is ready — or instead of one it turns out not to be able to play.
      if (media.kind === "video" && media.poster) node.poster = media.poster;
      // An element-level `src` and child `<source>`s are alternatives, not
      // partners: giving the element a `src` makes the browser ignore every
      // `<source>` under it. So the candidate list wins when there is one,
      // since it is the more specific statement of what will actually decode.
      if (media.sources && media.sources.length) {
        for (const s of media.sources) {
          const src = document.createElement("source");
          src.src = s.src;
          if (s.mime) src.type = s.mime;
          if (s.media) src.media = s.media;
          node.appendChild(src);
        }
      } else {
        node.src = media.src;
      }
      // The element's own text is its no-support fallback. It is excluded from
      // the row's editable text by `textWalker`, so it can never count toward a
      // caret offset — it is chrome the browser may show, not document content.
      if (media.alt) node.appendChild(document.createTextNode(media.alt));
      node.setAttribute("aria-label", media.alt || media.src);
    }
    node.setAttribute("contenteditable", "false");
    // Cheap and correct: a media that fails to load leaves the row visibly
    // empty otherwise, with no hint of what was meant to be there.
    node.addEventListener("error", () => div.classList.add("leaf-media-broken"), { once: true });
    div.appendChild(node);

    div.appendChild(document.createTextNode(ZWSP));
    this.rowEls.push(div);
    return div;
  }

  /**
   * Build the row for a display formula on lines of its own: the typeset
   * picture, centred, in place of the `∑ tex` placeholder core drew for a
   * surface that cannot paint one. The media row's shape exactly — an atom the
   * caret sits either side of, a zero-width space on each side for it to land
   * on, and `leafCoreLen` for the offsets to cross by — because that is what a
   * block formula is to the caret. The picture is set at the body size; the
   * row grows to fit it, so nothing here calls `set_math_rows`.
   */
  _mathRowEl(div, m, row) {
    div.classList.add("leaf-media-row", "leaf-math-row");
    div.dataset.leafCoreLen = String((row?.runs || []).reduce((n, r) => n + r.text.length, 0));
    div.appendChild(document.createTextNode(ZWSP));
    const pic = typesetMath(m.tex, m.display, this.theme.fontSize, this._ink);
    const img = el("img", "leaf-media leaf-math-block");
    img.src = svgDataUrl(pic.svg);
    img.alt = m.tex.trim();
    img.setAttribute("contenteditable", "false");
    // The picture's own size, so it neither scales to the measure nor blurs.
    img.style.width = (pic.width * this.theme.fontSize).toFixed(2) + "px";
    img.style.height = ((pic.height + pic.depth) * this.theme.fontSize).toFixed(2) + "px";
    div.appendChild(img);
    div.appendChild(document.createTextNode(ZWSP));
    this.rowEls.push(div);
    return div;
  }

  /**
   * Build the row for a `::page-break` directive: a dashed rule across the
   * measure, in place of the `⧉ page-break` placeholder glyphs core draws for a
   * surface that knows nothing of the name.
   *
   * The row holds no caret — it is `contenteditable="false"` with no text at
   * all, the block-gap row's shape rather than the media row's. A picture is
   * something the caret sits either side of, because the author may want to
   * type in front of it or behind it; a page break is a seam between two pages
   * with nothing on it to edit, and the blank boundary rows on each side of it
   * are already the caret's homes there. So native ArrowUp/ArrowDown steps
   * straight across it, in one press, rather than resting on a line with
   * nothing to stand on.
   *
   * The `leafCoreLen` marker is the media row's, for the same reason: what core
   * counts on this row (`⧉ page-break`, twelve columns) and what is drawn here
   * (nothing) have nothing to do with each other, and every offset that crosses
   * between the two goes through that number.
   */
  _pageBreakRowEl(div, row) {
    div.classList.add("leaf-page-break");
    div.setAttribute("contenteditable", "false");
    div.dataset.leafCoreLen = String(
      (row?.runs || []).reduce((n, r) => n + r.text.length, 0)
    );
    const rule = el("div", "leaf-page-break-rule");
    rule.setAttribute("role", "separator");
    rule.setAttribute("aria-label", "page break");
    div.appendChild(rule);
    this.rowEls.push(div);
    return div;
  }

  /**
   * Build a real `<table>` from core's structural [`TableView`], in place of the
   * box-drawn rows it names.
   *
   * The two descriptions are alternatives, not layers: they describe the same
   * cells at the same source offsets, so the caret lands identically whichever
   * one a frontend paints. The terminal paints the picture; the browser is
   * proportional and cannot, so it draws its own geometry — columns sized to
   * content, real borders, alignment honoured — and lets the browser's own table
   * layout do the work core did in character cells.
   *
   * Each cell line records the source range it covers in `cellLines`. That list
   * is how a caret gets into and out of a cell: inside a table the editor stops
   * addressing the document in `(row, ch)` — those coordinates belong to the
   * picture, which isn't in the DOM — and works in source offsets, which both
   * descriptions agree on.
   */
  _tableEl(t) {
    const table = el("table", "leaf-table");
    // No `contenteditable` of its own, in either direction. Marking the table
    // false and its cells true reads like the right shape — chrome around
    // editable content — but it makes each cell a *separate editing host*, and a
    // browser will not deliver a keystroke to the focused host when the
    // selection is inside a different one: the caret sits in the cell and typing
    // goes nowhere. The whole surface is one host, as the rows are, and the
    // browser is kept from restructuring the grid the same way it is kept from
    // restructuring anything else here — every `beforeinput` is prevented and
    // translated into a core edit.
    const body = document.createElement("tbody");
    // Every cell line in grid order, for `_adoptTable` to walk when the grid
    // is reused by a later frame.
    table._leafLines = [];

    for (const row of t.grid) {
      const tr = document.createElement("tr");
      if (row.head) tr.className = "leaf-thead";
      for (const cell of row.cells) {
        const td = document.createElement(row.head ? "th" : "td");
        if (cell.align && cell.align !== "default") td.style.textAlign = cell.align;
        for (const line of cell.lines) {
          const lineEl = el("div", "leaf-cell-line");
          const runs = canonicalRuns(line.runs);
          lineEl._leafRuns = runs.map((run) => lineEl.appendChild(this._runEl(run)));
          // An empty cell still needs somewhere for the caret to sit.
          if (runs.length === 0) lineEl.appendChild(document.createElement("br"));
          this.cellLines.push({ start: line.start, end: line.end, el: lineEl });
          table._leafLines.push(lineEl);
          td.appendChild(lineEl);
        }
        tr.appendChild(td);
      }
      body.appendChild(tr);
    }
    table.appendChild(body);
    return table;
  }

  /**
   * One styled span for a run, carrying the source offset its first glyph came
   * from as the `_src` property — what makes a rendered link or footnote marker
   * followable, and what the cell-offset mapping counts from.
   *
   * A property rather than a `data-` attribute on purpose. The offset is the
   * one thing about a run that changes without the run changing: type a
   * character at the top and every run below it moves by one byte. Rows are
   * reused across frames (see `render`), so the offset is refreshed on each,
   * and a property write is nothing where an attribute write is a DOM mutation
   * — thousands of them on a keystroke, for the document to end up looking the
   * same.
   */
  _runEl(run, fontPx = this.theme.fontSize) {
    const span = document.createElement("span");
    if (run.role === "math") return this._mathRunEl(span, run, fontPx);
    let cls = "leaf-run leaf-r-" + run.role;
    if (run.bold) cls += " leaf-b";
    if (run.italic) cls += " leaf-i";
    if (run.underline) cls += " leaf-u";
    if (run.strike) cls += " leaf-s";
    if (run.sup) cls += " leaf-sup";
    if (run.sub) cls += " leaf-sub";
    if (run.hl) {
      cls += " leaf-hl";
      span.dataset.hl = run.hl;
      if (run.hl_color) span.style.setProperty("--leaf-hl", run.hl_color);
    }
    // A highlight the author coloured (`==🔴 text==`). A class rather than an
    // inline custom property, unlike `--leaf-hl` above: the name comes from a
    // closed vocabulary the stylesheet already has a rule for, where a host
    // highlight's colour is an arbitrary value only the host knows.
    if (run.mark_color) cls += " leaf-mk-" + run.mark_color;
    // What a code run is to its block's language — a class from the same
    // closed vocabulary `plates` styles its pages with, so a stylesheet rule
    // per token colours it. A run without one is plain code.
    if (run.token) cls += " leaf-t-" + run.token;
    span.className = cls;
    // The run's half of the presentation vocabulary. `data-` attributes rather
    // than classes because each takes a value, which is twig's own
    // recommendation and what the document itself carries — so the span the
    // editor draws and the span a published page carries are spelled the same,
    // and `presentation.css` styles both.
    //
    // A *value* — `14pt`, `Garamond`, `#c03030` — keeps the attribute, because
    // the attribute is what the document says, and gains the inline style the
    // stylesheet cannot hold: a rule per possible measurement is not a file
    // anyone can write. That is the portability the author traded away at the
    // *Other…* row, and the editor's own surface is the one place it is whole.
    // An inline declaration also settles the ramp below: an exact size is that
    // many points, not the heading's size scaled.
    if (run.size) {
      span.dataset.size = run.size;
      if (!isSizeStep(run.size)) span.style.fontSize = run.size;
    }
    if (run.font) {
      span.dataset.font = run.font;
      if (!isFontGeneric(run.font)) span.style.fontFamily = cssFamily(run.font);
    }
    if (run.text_color) {
      span.dataset.color = run.text_color;
      if (!isColorName(run.text_color)) span.style.color = run.text_color;
    }
    span._src = run.src;
    span.textContent = run.text;
    return span;
  }

  /**
   * An inline formula's run: the one-character atom core drew, with its
   * typeset picture painted where it stands.
   *
   * The atom's text stays — a single `∑`, the one UTF-16 unit the row's text
   * has for the formula, which is what keeps every offset the caret and the
   * selection cross the row by in step with core's columns. What changes is
   * how it draws: the glyph is made transparent, its advance is stretched by
   * `letter-spacing` to the picture's width, and the picture is the span's
   * background. So the caret's two homes — before the atom and after it — fall
   * at the picture's left and right edges, native Left and Right step over it
   * as over any character, and a click on it lands the caret at the formula's
   * start, exactly as core's `src` says. The span is an inline-block hidden
   * overflow, whose baseline is its bottom edge, dropped by the formula's
   * depth so the text baseline runs through the picture where TeX put it.
   *
   * A formula on the caret's line never arrives here: core draws it as its
   * TeX in code and delimiter runs. TeX the typesetter cannot read keeps the
   * `∑`, tinted, with the fault in its title — the surface says "this is not
   * yet a formula" where it stands. So does a formula whose typesetter has
   * not arrived, untinted, until `_mathSettled` repaints the row.
   */
  _mathRunEl(span, run, fontPx) {
    span.className = "leaf-run leaf-r-math" + (run.bold ? " leaf-b" : "") + (run.italic ? " leaf-i" : "");
    span._src = run.src;
    span.textContent = run.text;
    const m = this._mathInline?.get(run.src);
    if (!m) return span;
    const pic = typesetMath(m.tex, m.display, fontPx, this._ink);
    if (pic === null) return span; // the typesetter is on its way, or off: the atom stands
    if (pic instanceof Error) {
      span.classList.add("leaf-math-error");
      span.title = pic.message;
      return span;
    }
    const w = pic.width * fontPx;
    const h = (pic.height + pic.depth) * fontPx;
    span.classList.add("leaf-math");
    span.title = m.tex.trim();
    span.style.width = w.toFixed(2) + "px";
    span.style.height = h.toFixed(2) + "px";
    span.style.verticalAlign = (-pic.depth * fontPx).toFixed(2) + "px";
    span.style.letterSpacing = (w - this._atomAdvance(fontPx, run.bold)).toFixed(2) + "px";
    span.style.backgroundImage = "url(\"" + svgDataUrl(pic.svg) + "\")";
    return span;
  }

  /**
   * The advance of the atom character in the body face at `fontPx` — what
   * `_mathRunEl` subtracts from the picture's width to find the letter-spacing
   * that stretches the atom to it. Measured once per size on a canvas and
   * remembered: a frame with a dozen formulas measures nothing.
   */
  _atomAdvance(fontPx, bold) {
    const key = (bold ? "b" : "") + fontPx;
    this._atomWidths ??= new Map();
    let w = this._atomWidths.get(key);
    if (w === undefined) {
      const ctx = (this._measureCanvas ??= document.createElement("canvas")).getContext("2d");
      ctx.font = (bold ? "700 " : "") + fontPx + "px " + this.theme.fontFamily;
      w = ctx.measureText(MATH_ATOM).width;
      this._atomWidths.set(key, w);
    }
    return w;
  }

  /**
   * The typesetter has arrived, or has failed to: repaint the last frame, so
   * every formula drawn as a placeholder while it was on its way becomes its
   * picture — or its marked fault. Only the rows that carried a formula are
   * rebuilt, because only their keys carry the typesetter's state.
   */
  _mathSettled() {
    if (this._destroyed || !this._lastView) return;
    this.render(this._lastView);
  }

  /**
   * The ink a formula is set in: the surface's own text colour, as a CSS hex
   * the typesetter reads. Read off the computed style so a host that overrode
   * `--leaf-text`, or a scheme change, is followed without being told.
   */
  _inkColor() {
    const rgb = getComputedStyle(this.contentEl).color;
    const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(rgb);
    if (!m) return "#000000";
    return "#" + [m[1], m[2], m[3]].map((v) => Number(v).toString(16).padStart(2, "0")).join("");
  }

  // ── native selection (model ⇄ browser) ────────────────────────────────────

  /** Paint the model's caret/selection onto the browser's native selection. */
  _restoreSelection(view) {
    const sel = window.getSelection();
    if (!sel) return;
    // Only where the selection already lives here. Putting a range into a
    // contenteditable focuses it, so restoring the caret on every repaint
    // would pull focus out of whatever the reader was typing in — a search
    // box, a toolbar's URL field — the moment the host repainted, which a
    // setHighlights does. An editor that was told not to autofocus stays
    // unfocused for the same reason.
    const inside =
      document.activeElement === this.contentEl ||
      (sel.anchorNode != null && this.contentEl.contains(sel.anchorNode));
    if (!inside) return;
    // A caret inside a table sits on a row that isn't in the document — the
    // box-drawn picture was replaced by a grid — so `(row, ch)` addresses
    // nothing to put a range in. The source offset does, and both descriptions
    // of a table agree on it.
    const f = this._rangeForSrc(view.caret_src) || this._rangeForRow(view.caret_row, view.caret_ch);
    const a = view.has_selection
      ? this._rangeForSrc(this._anchorSrc(view)) || this._rangeForRow(view.anchor_row, view.anchor_ch)
      : f;
    if (!f || !a) return;
    this._settingSelection = true;
    try {
      // base/extent (not start/end) keeps the model's selection direction.
      sel.setBaseAndExtent(a.startContainer, a.startOffset, f.startContainer, f.startOffset);
    } catch {
      /* endpoints in an edge/detached node — leave the selection as the browser has it */
    }
    this._settingSelection = false;
  }

  /** The frame's anchor as a source offset. The frame carries the caret's, and
   *  the anchor only as `(row, ch)`; ask core to convert when it is needed. */
  _anchorSrc(view) {
    if (!view.has_selection) return view.caret_src;
    return this.doc.offset_for_pos(view.anchor_row, view.anchor_ch);
  }

  /** A collapsed range at a row's UTF-16 offset, or null if that row is gone. */
  _rangeForRow(row, ch) {
    let rowEl = this.rowEls[row];
    // A page break holds no caret (see `_pageBreakRowEl`), and core's caret
    // lands on it the moment one is inserted — `insert_page_break` leaves it at
    // the end of what it wrote. Put the browser's caret on the first row past
    // the break that does hold one, so the author carries on typing below the
    // seam; the next `selectionchange` tells core where that was.
    if (rowEl && rowEl.classList.contains("leaf-page-break")) {
      for (let r = row + 1; r < this.rowEls.length; r++) {
        const next = this.rowEls[r];
        if (next && next.isConnected && !next.classList.contains("leaf-page-break")
            && next.getAttribute("contenteditable") !== "false") {
          rowEl = next;
          ch = 0;
          break;
        }
      }
      if (rowEl === this.rowEls[row]) return null; // a break with nothing after it
    }
    return rowEl && rowEl.isConnected ? rangeAtOffset(rowEl, ch) : null;
  }

  /**
   * A collapsed range at a source offset, if it falls in a drawn table cell —
   * and null otherwise, which is the ordinary case and means "use the row".
   *
   * The cell's own range is a byte span; a `Range` counts UTF-16 units. The two
   * are bridged one run at a time, since a run's glyphs are contiguous in the
   * source and carry the offset the first of them came from.
   */
  _rangeForSrc(src) {
    const line = this._cellLineForSrc(src);
    return line ? rangeAtOffset(line.el, utf16InLine(line.el, src, line.start)) : null;
  }

  /** The drawn cell line whose source range covers `src`, or null — the
   *  ordinary answer, meaning the offset is in prose and has a row. */
  _cellLineForSrc(src) {
    if (src == null) return null;
    for (const line of this.cellLines) {
      if (src >= line.start && src <= line.end && line.el.isConnected) return line;
    }
    return null;
  }

  /** Whether a frame row is one a drawn grid stands in for. */
  _rowIsTable(row) {
    return this.tableSpans.some(([a, b]) => row >= a && row < b);
  }

  /** Read the browser's selection into core (no repaint). Returns whether it mapped. */
  _syncFromDom() {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return false;
    const r = sel.getRangeAt(0);
    if (!this.contentEl.contains(r.commonAncestorContainer)) return false;
    const view = this._selectionToCore(sel);
    if (!view) return false;
    this._lastView = this._whole(view);
    return true;
  }

  /**
   * Push the browser's selection into core, in whichever coordinates both ends
   * can be spelled in.
   *
   * `(row, ch)` is the everyday one. But a table's rows are the box-drawn
   * picture, which isn't in the document, so an endpoint in a cell has no row to
   * name — and a selection can have one end in a cell and the other in the
   * prose above it. When either end is in a grid, both are converted to source
   * offsets, the coordinate the two descriptions of a table share.
   */
  _selectionToCore(sel) {
    return this._pointsToCore(sel.anchorNode, sel.anchorOffset, sel.focusNode, sel.focusOffset);
  }

  /** `_selectionToCore` for any two DOM points — a `StaticRange`'s ends as
   *  much as a `Selection`'s. */
  _pointsToCore(anchorNode, anchorOffset, focusNode, focusOffset) {
    const ac = this._cellPoint(anchorNode, anchorOffset);
    const fc = this._cellPoint(focusNode, focusOffset);
    const a = this._domPoint(anchorNode, anchorOffset);
    const f = this._domPoint(focusNode, focusOffset);

    if (ac != null || fc != null) {
      const anchor = ac != null ? ac : a && this.doc.offset_for_pos(a.row, a.ch);
      const focus = fc != null ? fc : f && this.doc.offset_for_pos(f.row, f.ch);
      if (anchor == null || focus == null) return null;
      return this.doc.set_selection_offsets(anchor, focus);
    }
    if (!a || !f) return null;
    return this.doc.set_selection(a.row, a.ch, f.row, f.ch);
  }

  /**
   * Map a DOM selection endpoint inside a drawn table cell to a source offset,
   * or null if it isn't in one.
   *
   * Approximate then snap: the offset is counted from the enclosing run's own
   * `src`, which is exact whenever the run's glyphs are contiguous in the source
   * — the ordinary case, since a hidden delimiter has a different role and so
   * forms its own run — and core's `snap_offset` cleans up the rest by pulling
   * the answer onto a real caret stop.
   */
  _cellPoint(node, offset) {
    const el = node.nodeType === 1 ? node : node.parentElement;
    const lineEl = el && el.closest(".leaf-cell-line");
    if (!lineEl) return null;
    const line = this.cellLines.find((l) => l.el === lineEl);
    if (!line) return null;
    return this.doc.snap_offset(srcInLine(lineEl, node, offset, line.start, line.end));
  }

  /** Map a DOM selection endpoint to `{row, ch}`, or null if it isn't in a row. */
  _domPoint(node, offset) {
    // An endpoint on the content root sits at a row boundary: its children are the
    // rows in order, so `offset` addresses a row start (clamped to the last end).
    if (node === this.contentEl) {
      if (offset < this.rowEls.length) return { row: offset, ch: 0 };
      const row = this.rowEls.length - 1;
      const rowEl = this.rowEls[row];
      return rowEl ? { row, ch: rowTextLength(rowEl) } : null;
    }
    const rowEl = this._rowOf(node);
    if (!rowEl) {
      // An endpoint on a row element itself (offset = child index): treat it as
      // that row's start.
      const idx = this.rowEls.indexOf(node);
      if (idx >= 0) return { row: idx, ch: 0 };
      return null;
    }
    return { row: this.rowEls.indexOf(rowEl), ch: offsetTo(rowEl, node, offset) };
  }

  /**
   * Keep the caret's line inside the viewport.
   *
   * The caret's row is the ordinary case. A caret inside a drawn grid has no
   * row in the document — the picture row it belongs to was replaced by the
   * `<table>`, and its element is tracked but never inserted — so its
   * `offsetTop` reads as zero, and scrolling to that sent the viewport to the
   * top of the document on every keystroke in a cell. The cell line the caret
   * is actually in is what to measure there.
   */
  _scrollCaretIntoView(view) {
    const c = this.container;
    let top, bottom;
    const rowEl = this.rowEls[view.caret_row];
    if (rowEl && rowEl.isConnected) {
      top = rowEl.offsetTop;
      bottom = top + rowEl.offsetHeight;
    } else {
      const line = this._cellLineForSrc(view.caret_src);
      if (!line) return;
      // Measured against the viewport rather than `offsetTop`, whose reference
      // inside a table is the cell, not the scrolling container.
      const r = line.el.getBoundingClientRect();
      top = r.top - c.getBoundingClientRect().top + c.scrollTop;
      bottom = top + r.height;
    }
    if (top < c.scrollTop) c.scrollTop = top;
    else if (bottom > c.scrollTop + c.clientHeight) c.scrollTop = bottom - c.clientHeight;
  }

  /** The `.leaf-row` ancestor of a node, or null if it isn't one of ours. */
  _rowOf(node) {
    let n = node;
    while (n && n !== this.contentEl) {
      if (n.nodeType === 1 && n.classList.contains("leaf-row")) {
        return this.rowEls.includes(n) ? n : null;
      }
      n = n.parentNode;
    }
    return null;
  }

  // ── hit testing (only for the triple-click block gesture) ─────────────────

  /** Map a viewport point to `{row, ch}` (used to seed a logical-block select). */
  _hitTest(clientX, clientY) {
    const hit = caretFromPoint(clientX, clientY);
    const rowEl = hit ? this._rowOf(hit.node) : null;
    if (rowEl) return { row: this.rowEls.indexOf(rowEl), ch: offsetTo(rowEl, hit.node, hit.offset) };
    return null;
  }

  /**
   * Whether `clientY` is under every row drawn — the blank space the surface's
   * `min-height` and padding leave under the document, where a click lands on
   * no row at all. Measured against the lowest row rather than the last one: a
   * table's picture rows share one grid element, and a row a frame hid
   * measures nothing.
   */
  _isPastEnd(clientY) {
    let bottom = -Infinity;
    for (const el of this.rowEls) {
      if (!el || !el.isConnected) continue;
      const r = el.getBoundingClientRect();
      if (r.height > 0 && r.bottom > bottom) bottom = r.bottom;
    }
    return Number.isFinite(bottom) && clientY >= bottom;
  }

  // ── wrap width ────────────────────────────────────────────────────────────
  //
  // Core wraps to a *column* budget; the browser shapes the result in pixels.
  // Turning one into the other is a guess, because a column has no fixed width
  // in a proportional font — so it is made, then checked against what the
  // browser actually drew, exactly as core's media reservation is checked
  // against the measured height of a real `<img>`.

  /**
   * The column budget the viewport implies, from the body font's average glyph
   * width. Proportional text means this is a good average rather than exact —
   * a line of capitals or of inline `code` runs wider per column than the
   * lowercase sample — so it is only the opening bid; `_fitWidth` corrects it.
   */
  _cols() {
    const avg = this.measureEl.getBoundingClientRect().width / WIDTH_SAMPLE.length;
    if (!(avg > 0)) return 80;
    const avail = this._availWidth();
    return avail > 0 ? Math.max(1, Math.floor(avail / avg)) : 80;
  }

  /** The pixels a row has to fit in: the viewport less the surface's padding. */
  _availWidth() {
    const cs = getComputedStyle(this.contentEl);
    const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
    return this.container.clientWidth - padX;
  }

  /** The widest row as laid out, in pixels. Forces layout, so it is only asked
   *  when something has already been seen to overflow. */
  _widestRow() {
    let widest = 0;
    for (const rowEl of this.rowEls) {
      if (rowEl.isConnected && rowEl.scrollWidth > widest) widest = rowEl.scrollWidth;
    }
    return widest;
  }

  /**
   * Paint at the estimated budget, then correct it against the pixels the
   * browser produced, until nothing overflows.
   *
   * The estimate divides the measure by the body font's *average* glyph, so a
   * row whose glyphs run wider than average — capitals, a monospace `code` run,
   * a heading at 1.625× — exceeds the viewport even though it is within budget
   * in columns. The symptom is text clipped at the right edge and a horizontal
   * scrollbar under the whole document.
   *
   * Each pass is one Newton step from what was actually drawn: the budget that
   * would have fitted, had a column kept the width it turned out to have. It
   * only ever shrinks — growing back toward a budget just seen to overflow is
   * how a fitting loop oscillates — and it stops as soon as a pass wins nothing,
   * which is the honest answer for a row that cannot be wrapped narrower at all
   * (one long word, a URL). That row is then left to scroll, and
   * `_acceptedOverflow` records the surrender so `_checkFit` doesn't spend every
   * subsequent paint rediscovering it.
   */
  _fitWidth() {
    if (this._fitting) return;
    this._fitting = true;
    try {
      let cols = this._cols();
      this.render(this.doc.set_width(cols));
      for (let pass = 0; pass < MAX_FIT_PASSES; pass++) {
        const avail = this._availWidth();
        if (!(avail > 0)) break;
        const widest = this._widestRow();
        // A pixel of slack: scrollWidth is a rounded integer, and re-wrapping
        // the whole document to chase a rounding error helps nobody.
        if (widest <= avail + 1) break;
        const corrected = Math.max(1, Math.floor((cols * avail) / widest));
        if (corrected >= cols) break;
        this.render(this.doc.set_width(corrected));
        // Did narrowing the budget actually narrow the document? A row that is
        // one unbreakable token — a URL, a long identifier, a line of capitals
        // with no space in it — is as wide as it is at every budget, and core
        // will not split it. Chasing it drags every *other* line in the document
        // narrower for nothing, so when a pass buys almost no ground, put the
        // budget back and let that one row scroll.
        if (this._widestRow() > widest - (widest - avail) * MIN_FIT_PROGRESS) {
          this.render(this.doc.set_width(cols));
          break;
        }
        cols = corrected;
      }
      // Out of passes, or out of progress. Whether that is a surrender depends
      // on what the last pass drew, not on how the loop happened to leave — a
      // correction on the final pass can fit perfectly, and calling that an
      // accepted overflow would switch `_checkFit` off for a surface that is fine.
      const avail = this._availWidth();
      this._acceptedOverflow = !(avail > 0) || this._widestRow() > avail + 1;
    } finally {
      this._fitting = false;
    }
  }

  /**
   * Notice, after a paint, that an edit has produced a row wider than the
   * measure, and re-fit on the next frame.
   *
   * Typing can outgrow a budget that fitted a moment ago — hold down a capital
   * letter on a full line — so the fit cannot be a one-time measurement at
   * startup. But it also cannot re-measure every row on every keystroke, which
   * is a forced layout over the whole document. So the per-paint question is
   * the cheapest one that can be asked: a single read of the surface's own
   * `scrollWidth`. The per-row scan behind `_fitWidth` only runs once that has
   * already said yes.
   */
  /**
   * Re-fit after a viewport change, on the frame *after* the one that reported
   * it.
   *
   * Re-wrapping repaints, and repainting changes the very sizes the observer is
   * watching; doing that inside its own callback is what makes a browser log
   * "ResizeObserver loop completed with undelivered notifications" and drop the
   * remaining notifications for that frame. Landing on the next frame breaks
   * the cycle, at the cost of one frame of stale wrapping during a live drag —
   * which is what a resize looks like anyway.
   */
  _scheduleRefit() {
    if (this._refitQueued || this._destroyed) return;
    this._refitQueued = true;
    const run = () => {
      this._refitQueued = false;
      if (!this._destroyed) this.refit();
    };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(run);
    else run();
  }

  _checkFit() {
    if (this._fitting || this._fitQueued || this._acceptedOverflow || this._destroyed) return;
    const ce = this.contentEl;
    if (ce.scrollWidth <= ce.clientWidth + 1) return;
    this._fitQueued = true;
    const run = () => {
      this._fitQueued = false;
      if (!this._destroyed) this._fitWidth();
    };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(run);
    else run();
  }

  // ── input ─────────────────────────────────────────────────────────────────

  _bindEvents() {
    this._listeners = [];
    const on = (tgt, type, fn) => {
      tgt.addEventListener(type, fn);
      this._listeners.push([type, fn, tgt]);
    };
    const ce = this.contentEl;

    // The editing intent stream. Every input is prevented and translated to a
    // core op — the browser never mutates our projected DOM (IME excepted below).
    on(ce, "beforeinput", (e) => this._onBeforeInput(e));

    // IME: the browser *will* compose into the DOM (we can't prevent it), so we
    // let it, freeze core's caret at the start, and reconcile on end.
    on(ce, "compositionstart", () => {
      this._composing = true;
      this._syncFromDom();
    });
    on(ce, "compositionend", (e) => {
      this._composing = false;
      const data = e.data || "";
      // Rebuild from core (with the composed text inserted), replacing whatever
      // the browser left in the DOM during composition — all of it: the browser
      // has been writing into the projection, so no row can be taken as still
      // matching the key it was built from.
      this._rebuildAll = true;
      this.render(data ? this.doc.insert(data) : this.doc.view());
    });

    // Shortcuts the browser doesn't deliver as a beforeinput intent (view toggle,
    // tab), plus the formatting/history shortcuts routed here so they work even
    // where `formatBold`/`historyUndo` beforeinput isn't emitted.
    on(ce, "keydown", (e) => this._onKeyDown(e));

    // Selection: mirror the browser's caret/selection into core so a command acts
    // where the user is. Skipped while composing (core's caret is frozen) and
    // while we're the ones setting the selection (our own restore).
    on(document, "selectionchange", () => {
      if (this._settingSelection || this._composing) return;
      const sel = window.getSelection();
      if (!sel || sel.rangeCount === 0) return;
      if (!this.contentEl.contains(sel.getRangeAt(0).commonAncestorContainer)) return;
      const view = this._selectionToCore(sel);
      if (!view) return;
      // A caret move is not an edit, but it can still change what the rows
      // show: crossing onto a line that reveals its markup — every line under
      // the "full" markup mode, a formula's line in every mode — or off one.
      // Core says so through the map's key, and only then is a frame painted;
      // ordinary motion keeps costing nothing.
      if (view.map_key !== this._lastView?.map_key) this.render(view);
      else this._emitChange((this._lastView = this._whole(view)));
    });

    // Triple-click: the browser's is a *visual line*; leaf's is the *logical
    // block* (a paragraph across its soft-wraps). Intercept just that count.
    on(ce, "mousedown", (e) => {
      if (e.button !== 0 || e.detail !== 3) return;
      const hit = this._hitTest(e.clientX, e.clientY);
      if (!hit) return;
      e.preventDefault();
      this.focus();
      this.render(this.doc.select_block_ch(hit.row, hit.ch));
    });

    // A plain click under the last row is on nothing: the caret goes onto an
    // empty paragraph under the last block, opened if the document has none —
    // wherever the pointer was horizontally — which is also the way out from
    // under a fenced code block, where Enter is a literal newline. Left to
    // itself the browser would put the caret at the pointer's x on the last
    // line, which is what a click on that line means and not what a click under
    // it does. Only the plain click: a shift-click extends to the end through
    // the browser's own gesture, and a double- or triple-click still selects.
    // And only in the rich view — the source view is a text editor, where a
    // click under the last line lands on it at the pointer's x.
    on(ce, "mousedown", (e) => {
      if (e.button !== 0 || e.detail !== 1 || e.shiftKey || primaryModifier(e)) return;
      if (this.viewName() === "source" || !this._isPastEnd(e.clientY)) return;
      e.preventDefault();
      this.focus();
      this.render(this.doc.click_past_end());
    });

    on(ce, "focus", () => this.container.classList.add("leaf-focus"));
    on(ce, "blur", () => this.container.classList.remove("leaf-focus"));

    // A rendered link and a rendered checkbox are the two things on this surface
    // that are worth clicking rather than merely worth putting a caret in. Both
    // are answered from the run's own source offset — the run says where it came
    // from, and core turns that back into a destination or a tick.
    on(ce, "click", (e) => this._onClick(e));

    // Where a link goes, shown on hover as a native tooltip. Asked on each
    // pass rather than at build: a link's text can stay the same while its
    // destination is edited underneath it, and the row would be reused as it
    // stands.
    on(ce, "mouseover", (e) => {
      const runEl = e.target.closest?.(".leaf-r-link");
      if (!runEl) return;
      const dest = this.doc.link_destination_at(runEl._src);
      if (dest) runEl.title = dest;
    });

    // Core resolves a `<picture>`'s `prefers-color-scheme` sources itself, and
    // has no theme of its own — so the page answers on its behalf, and keeps
    // answering. A repaint at the same scheme resolves to the same URLs, so this
    // is cheap to fire on every change.
    if (typeof matchMedia === "function") {
      this._darkQuery = matchMedia("(prefers-color-scheme: dark)");
      const scheme = () => this.render(this.doc.set_color_scheme(this._darkQuery.matches ? "dark" : "light"));
      on(this._darkQuery, "change", scheme);
      this.doc.set_color_scheme(this._darkQuery.matches ? "dark" : "light");
    }

    // Rich clipboard (mirrors leaf-tui / leaf-gpui): copy/cut write both the
    // plain source and twig's HTML; paste prefers the HTML flavor.
    on(ce, "copy", (e) => {
      const text = this.doc.selected_text();
      if (text == null) return;
      e.clipboardData.setData("text/plain", text);
      const html = this.doc.selection_html();
      if (html != null) e.clipboardData.setData("text/html", html);
      e.preventDefault();
    });
    on(ce, "cut", (e) => {
      const text = this.doc.selected_text();
      if (text == null) return;
      e.clipboardData.setData("text/plain", text);
      const html = this.doc.selection_html();
      if (html != null) e.clipboardData.setData("text/html", html);
      this._syncFromDom();
      this.render(this.doc.backspace());
      e.preventDefault();
    });
    on(ce, "paste", (e) => {
      const plain = this._plainPaste;
      this._plainPaste = false;
      const html = plain ? "" : e.clipboardData.getData("text/html");
      const text = e.clipboardData.getData("text/plain");
      if (!html && !text) return;
      this._syncFromDom();
      this.render(this.doc.paste_rich(html || undefined, text || ""));
      e.preventDefault();
    });

    // Drag and drop. The intents the browser would raise for these
    // (insertFromDrop, deleteByDrag) are blocked with everything else in
    // beforeinput, so the gesture is taken over here: text dropped from
    // outside is inserted at the point, a selection dragged within the surface
    // is moved there (copied with ⌥ held), and files go to the host.
    on(ce, "dragstart", (e) => {
      const text = this.doc.selected_text();
      if (text == null) return;
      // What is dragged is the selected *source*, not the projection the
      // browser would serialise, so a drop within the surface moves the markup
      // as it stands and a drop elsewhere gets the same two flavours a copy
      // writes.
      e.dataTransfer.setData("text/plain", text);
      const html = this.doc.selection_html();
      if (html != null) e.dataTransfer.setData("text/html", html);
      e.dataTransfer.effectAllowed = "copyMove";
      this._dragging = { start: this.doc.anchor_offset(), end: this.doc.caret_offset() };
    });
    on(ce, "dragend", () => {
      this._dragging = null;
    });
    on(ce, "dragover", (e) => {
      e.preventDefault(); // "yes, a drop is welcome here"
      e.dataTransfer.dropEffect = this._dragging && !e.altKey ? "move" : "copy";
    });
    on(ce, "drop", (e) => this._onDrop(e));

    // Reflow on viewport change.
    if (typeof ResizeObserver !== "undefined") {
      this._resizeObs = new ResizeObserver(() => this._scheduleRefit());
      this._resizeObs.observe(this.container);
    } else {
      on(window, "resize", () => this._scheduleRefit());
    }
  }

  /**
   * A click on a rendered link or task box.
   *
   * Both act on the run's own `src` rather than on the caret: a click knows an
   * offset and not a caret position, and asking core about the *caret* would
   * answer about wherever the last selection change put it, which on a click is
   * a race. Neither gesture moves the caret, so a plain click still places one.
   *
   * A link is followed only with the platform's modifier held (⌘ on a Mac, Ctrl
   * elsewhere). This is an editor: clicking a link in your own prose should put
   * the caret in it, not navigate away from the document.
   */
  _onClick(e) {
    const runEl = e.target.closest?.(".leaf-run");
    if (!runEl) return;
    const src = runEl._src;
    if (!Number.isFinite(src)) return;

    // A highlight is the host's; say which one was hit and let the click go
    // on to place the caret as it would anywhere else.
    if (runEl.classList.contains("leaf-hl") && this._onActivateHighlight) {
      this._onActivateHighlight(runEl.dataset.hl);
    }

    // A task marker is core's `☐ `/`☑ ` drawn in the list marker's place.
    if (runEl.classList.contains("leaf-r-list") && /^[☐☑]/.test(runEl.textContent)) {
      e.preventDefault();
      this.render(this.doc.toggle_task_at(src));
      return;
    }

    if (!primaryModifier(e)) return;

    // A footnote reference before the link question: it is drawn with the
    // link role too (that is how it gets its colour), so asking about the link
    // first would answer for one and send the reader out of the document.
    if (runEl.classList.contains("leaf-sup")) {
      const note = this.doc.footnote_at(src);
      if (note) {
        e.preventDefault();
        if (note.offset != null) this.reveal(note.offset, note.end ?? null);
        return;
      }
    }

    if (runEl.classList.contains("leaf-r-link")) {
      const dest = this.doc.link_destination_at(src);
      if (!dest) return;
      e.preventDefault();
      // A fragment names a place in this document; land there rather than
      // hand the host an address it would only send back.
      if (dest.startsWith("#") && this.goTo(dest.slice(1))) return;
      this._onFollowLink ? this._onFollowLink(dest) : window.open(dest, "_blank", "noopener");
      return;
    }

    // In a note, the return leg: back up to what cites it. The mousedown has
    // already put the browser's caret where the click fell, so the question
    // is asked of the caret rather than of the run.
    this._syncFromDom();
    const def = this.doc.footnote_definition_at_caret();
    if (def && def.offset != null) {
      e.preventDefault();
      this.reveal(def.offset);
    }
  }

  /**
   * A drop on the surface — see the drag listeners in `_bindEvents`.
   *
   * A move is a delete and an insert, in that order, with the target adjusted
   * by the bytes that left in front of it. Dropping a selection onto itself is
   * nothing at all. The point under the pointer is hit-tested the way a click
   * is; a drop into a drawn grid has no row to name and is declined, which
   * leaves the document as it was rather than guessing a cell.
   */
  _onDrop(e) {
    const drag = this._dragging;
    this._dragging = null;
    const dt = e.dataTransfer;
    if (!dt) return;
    e.preventDefault();
    const hit = this._hitTest(e.clientX, e.clientY);
    const at = hit ? this.doc.offset_for_pos(hit.row, hit.ch) : null;

    if (dt.files && dt.files.length) {
      if (this._onDropFiles) this._onDropFiles([...dt.files], at ?? this.doc.caret_offset());
      return;
    }
    if (at == null) return;
    const html = dt.getData("text/html");
    const text = dt.getData("text/plain");
    if (!html && !text) return;

    let target = at;
    if (drag && !e.altKey) {
      const s = Math.min(drag.start, drag.end);
      const en = Math.max(drag.start, drag.end);
      if (target >= s && target <= en) return; // onto itself
      this.doc.select_range(s, en);
      this.doc.backspace();
      if (target > en) target -= en - s;
    }
    this.doc.set_selection_offsets(target, target);
    // Within the surface the plain flavour *is* the source, and goes back in
    // verbatim; from outside, the rich flavour is preferred as a paste does.
    this.render(drag ? this.doc.paste(text) : this.doc.paste_rich(html || undefined, text || ""));
  }

  /**
   * What to do with files dropped on the surface — upload them and insert the
   * media, say. Called with the `File`s and the source offset under the
   * pointer. Without a handler, dropped files are ignored.
   */
  onDropFiles(cb) {
    this._onDropFiles = cb;
    return this;
  }

  /**
   * What to do when a link is followed, instead of opening a new tab — for a
   * host that resolves a relative path against its own document store, or wants
   * an in-app preview. Called with the destination as core parsed it.
   */
  onFollowLink(cb) {
    this._onFollowLink = cb;
    return this;
  }

  /**
   * Translate a `beforeinput` intent into a core operation. Everything is
   * prevented so the browser never edits the projected DOM; core edits, then we
   * repaint and restore the selection. Composition is handled separately.
   */
  _onBeforeInput(e) {
    if (this._composing || e.inputType === "insertCompositionText") return;
    const d = this.doc;
    // Act where the user is: sync core's selection from the DOM first.
    this._syncFromDom();

    let view;
    switch (e.inputType) {
      case "insertText":
        this._selectTargetRange(e);
        view = d.insert(e.data ?? "");
        break;
      case "insertReplacementText": {
        // Autocorrect / dictation replacement — the text rides the dataTransfer.
        // What it replaces is not the selection: the caret sits after the word
        // the browser is correcting, and the word itself is named only by the
        // event's target range. Inserting at the caret alone left the misspelt
        // word in place with the correction appended to it.
        this._selectTargetRange(e);
        const rep = (e.dataTransfer && e.dataTransfer.getData("text/plain")) || e.data || "";
        view = d.insert(rep);
        break;
      }
      case "insertParagraph":
        // Inside a grid, Return means the cell below — adding a row at the last
        // one — not a new paragraph in the middle of a table. Core answers
        // `undefined` when the caret isn't in one, which is the signal to fall
        // through to the ordinary meaning.
        view = d.cell_return() ?? d.newline();
        break;
      case "insertLineBreak":
        // Shift+Return: a break *within* the cell rather than a new row.
        view = d.cell_line_break() ?? d.newline();
        break;
      case "deleteContentBackward":
        view = d.backspace();
        break;
      case "deleteContentForward":
        view = d.delete_forward();
        break;
      case "deleteWordBackward":
        view = d.delete_word_back();
        break;
      case "deleteWordForward":
        view = d.delete_word_forward();
        break;
      case "deleteSoftLineBackward":
      case "deleteHardLineBackward":
        // ⌘⌫: everything back to the start of the line. Core has no single
        // verb for it, but it has the two halves — and at a line's start the
        // selection is empty and Backspace joins the lines, which is what the
        // key does in a native field too.
        d.move_home(true);
        view = d.backspace();
        break;
      case "deleteSoftLineForward":
      case "deleteHardLineForward":
        d.move_end(true);
        view = d.delete_forward();
        break;
      case "historyUndo":
        view = d.undo();
        break;
      case "historyRedo":
        view = d.redo();
        break;
      case "formatBold":
        view = d.toggle_bold();
        break;
      case "formatItalic":
        view = d.toggle_italic();
        break;
      case "formatUnderline":
        view = d.toggle_underline();
        break;
      case "formatStrikeThrough":
        view = d.toggle_strike();
        break;
      // Clipboard and drag-drop have dedicated handlers; just block the default.
      case "insertFromPaste":
      case "deleteByCut":
      case "insertFromDrop":
      case "deleteByDrag":
        e.preventDefault();
        return;
      default:
        // Anything unrecognised is still prevented, to keep the DOM ≡ the model.
        e.preventDefault();
        return;
    }
    e.preventDefault();
    if (view) this.render(view);
  }

  /**
   * Put core's selection over the range a `beforeinput` says it is about to
   * replace, when that is more than the caret. The browser hands one over for
   * an autocorrect, a dictation edit, or a keyboard's word suggestion — the
   * cases where what is replaced is a word behind the caret, not the selection.
   */
  _selectTargetRange(e) {
    const range = e.getTargetRanges?.()[0];
    if (!range || range.collapsed) return;
    if (!this.contentEl.contains(range.startContainer)) return;
    const view = this._pointsToCore(
      range.startContainer, range.startOffset, range.endContainer, range.endOffset
    );
    if (view) this._lastView = this._whole(view);
  }

  /**
   * Keyboard shortcuts not covered by a `beforeinput` intent — view toggle and
   * Tab always, plus the formatting/history shortcuts (routed here so they work
   * uniformly). Caret motion and selection are left to the browser (→ native
   * selection → `selectionchange`).
   */
  _onKeyDown(e) {
    if (this._composing || e.isComposing || e.keyCode === 229) return;
    const d = this.doc;

    // ⌘⇧V: the paste that follows takes the plain flavour. Remembered here and
    // consumed by the paste handler, since the paste itself is the browser's
    // to deliver; any other key retires it, so a ⌘⇧V with nothing to paste
    // can't turn the *next* paste plain.
    this._plainPaste = primaryModifier(e) && e.shiftKey && e.key.toLowerCase() === "v";
    if (this._plainPaste) return;

    if (primaryModifier(e)) {
      const op = this._shortcut(e);
      if (!op) return; // copy/cut/paste, select-all, ⌘←/→, … stay the browser's
      e.preventDefault();
      this._syncFromDom();
      this.render(op());
      return;
    }

    if (e.key === "Tab") {
      // Tab / Shift+Tab are structural: they indent / outdent the caret's line,
      // nesting or unnesting a list item (not typing spaces). The core decides
      // the step — a list item moves by its marker width, prose by one level.
      //
      // In a table it means the next (or previous) cell instead, adding a row
      // when it steps off the end. Core answers `undefined` when the caret isn't
      // in one, so the ordinary meaning is the fallback rather than a separate
      // branch that has to ask first.
      e.preventDefault();
      this._syncFromDom();
      const forward = !e.shiftKey;
      this.render(d.cell_tab(forward) ?? (forward ? d.indent() : d.outdent()));
    }
    // Enter, Backspace, Delete, arrows: handled via beforeinput / native motion.
  }

  /**
   * The model operation a modified key names, or null — the set leaf-gpui
   * binds, spelled for a browser. Headings and lists go by `code` (the
   * physical key) rather than `key`: with ⌥ held a Mac types a symbol for a
   * digit, and with ⇧ held every keyboard does.
   */
  _shortcut(e) {
    const d = this.doc;
    const key = e.key.toLowerCase();
    if (e.altKey && !e.shiftKey) {
      // ⌘⌥C: a code block — the block peer of ⌘⇧C's inline code.
      if (e.code === "KeyC") return () => d.toggle_code_block();
      // ⌘⌥0–6: paragraph, then heading levels — the web-editor convention.
      const m = /^Digit([0-6])$/.exec(e.code);
      if (!m) return null;
      const level = Number(m[1]);
      return level === 0 ? () => d.set_paragraph() : () => d.set_heading(level);
    }
    if (e.altKey) return null;
    if (e.shiftKey) {
      switch (e.code === "Digit7" || e.code === "Digit8" ? e.code : key) {
        case "c": return () => d.toggle_code();
        case "m": return () => d.toggle_mark();
        case "x": return () => d.toggle_strike();
        case "z": return () => d.redo();
        case "Digit7": return () => d.toggle_list(true);
        case "Digit8": return () => d.toggle_list(false);
        default: return null;
      }
    }
    switch (key) {
      case "b": return () => d.toggle_bold();
      case "i": return () => d.toggle_italic();
      case "u": return () => d.toggle_underline();
      case "e": return () => d.toggle_view();
      case "z": return () => d.undo();
      case "y": return () => d.redo();
      case "[": return () => d.outdent();
      case "]": return () => d.indent();
      default: return null;
    }
  }

  _emitChange(view) {
    if (!this._onChange) return;
    this._onChange({
      view: view.view,
      dirty: view.dirty,
      // What the history buttons enable by; both false on a read-only document.
      canUndo: view.can_undo,
      canRedo: view.can_redo,
      readOnly: this.doc.read_only(),
      heading: view.heading ?? null,
      // Rides the frame for `heading`'s reason: walking the caret into a fence
      // changes no mark, so a button asking for itself would never be told.
      codeBlock: view.code_block,
      active: view.active,
      // Rides the frame rather than being a query the host makes for itself: a
      // toolbar redraws on state change, and walking the caret out of a link
      // changes no mark, no heading and no dirty flag, so a Link affordance
      // asking on its own would keep a stale answer.
      link: view.link ?? null,
      // The caret highlight's colour, riding the frame for the same reason and
      // more sharply: walking from a red highlight into a blue one changes no
      // mark, no heading and no dirty flag, so a picker asking on its own would
      // never be told to move.
      markColor: view.mark_color ?? null,
      // Whether a colour would *make* a highlight rather than recolour one —
      // the other half of a picker's enabled state.
      hasSelection: view.has_selection,
      caretSrc: view.caret_src,
    });
  }
}

/**
 * @typedef {Object} EditorState
 * @property {string} view      `"wysiwyg"` | `"source"`
 * @property {boolean} dirty    buffer differs from last saved
 * @property {boolean} canUndo  there is an edit to undo
 * @property {boolean} canRedo  there is an undone edit to redo
 * @property {boolean} readOnly the document refuses edits — see `setReadOnly`
 * @property {number | null} heading  heading level at the caret, or null
 * @property {boolean} codeBlock  the caret stands in a code block
 * @property {string[]} active  inline marks active at the caret
 * @property {string | null} link  destination of the link at the caret, or null
 * @property {string | null} markColor  colour of the highlight at the caret
 *   (one of `MARK_COLORS`), or null — both outside a highlight and inside an
 *   uncoloured one
 * @property {boolean} hasSelection  a non-empty selection is live
 * @property {number} caretSrc  the caret's source byte offset
 */

// ── module-private helpers ────────────────────────────────────────────────────

/** Whether this is a Mac, where the command key is the shortcut modifier and
 *  Control belongs to the system's own text bindings. */
const IS_MAC =
  typeof navigator !== "undefined" &&
  /mac|iphone|ipad|ipod/i.test(navigator.userAgentData?.platform || navigator.platform || "");

/**
 * Whether the platform's shortcut modifier is held: ⌘ on a Mac, Ctrl elsewhere.
 *
 * Not "either". On a Mac, Control is the system's: Ctrl+B/F step the caret,
 * Ctrl+E/A go to the line's ends, Ctrl+Y yanks — every native text field
 * honours them, and a contenteditable does too. Taking them for bold, toggle
 * view and redo broke the editor for anyone who types that way.
 */
function primaryModifier(e) {
  return IS_MAC ? e.metaKey : e.ctrlKey;
}

function el(tag, cls) {
  const e = document.createElement(tag);
  e.className = cls;
  return e;
}

/**
 * A family the author named, as a `font-family` declaration wants it: quoted.
 *
 * `Garamond` would stand bare and `Times New Roman` would happen to mean the
 * same thing unquoted, but a name beginning with a digit, holding a comma, or
 * spelling a CSS-wide keyword (`inherit`) would not — and a quoted string is
 * the canonical form for every family that is not a generic, which these never
 * are (see `isFontGeneric`). Quotes and backslashes inside the name are
 * escaped rather than dropped, so a name cannot close the string and spill
 * into the declaration.
 */
function cssFamily(name) {
  return '"' + name.replace(/["\\]/g, "\\$&") + '"';
}

/**
 * Whether `row` is the blank row core spells a block boundary with: no caret
 * home, drawn short so a boundary reads as spacing rather than as an empty line
 * the author never typed.
 *
 * Core says so outright. This used to sniff — decoration, not code, no visible
 * glyphs — which is re-deriving structure core had already worked out while
 * walking the AST to emit the row, and got a table's rule row wrong for exactly
 * as long as the sniff described one.
 */
function isBlockGap(row) {
  return row.boundary != null;
}

/**
 * How tall to draw a boundary, as a multiple of the theme's gap.
 *
 * A boundary's height is a frontend decision, but what it separates is not, and
 * typography spaces a gap by what is on either side of it: the margin above a
 * heading is wider than the one between two paragraphs, so the heading groups
 * with the text it introduces rather than floating between two blocks. Core
 * publishes the pair; this is leaf-web's opinion about it.
 */
function gapScale(row) {
  const b = row.boundary;
  if (!b) return 1;
  if (b.below === "heading") return 1.6;
  if (b.above === "heading") return 0.7;
  return 1;
}

/**
 * A row's runs with the splits that are not style merged back together.
 *
 * Core splits a run where the selection or a highlight begins and ends, so a
 * renderer painting its own selection can. This one lets the browser paint it,
 * so to the DOM those are one span — and if they were built as two, moving the
 * selection would change every row it crossed and cost each its element. Two
 * neighbours merge when they look the same and are contiguous in the source
 * (the second starts where the first's bytes end). The source view's runs all
 * report offset 0 and merge on looks alone.
 */
function canonicalRuns(runs) {
  const out = [];
  for (const run of runs) {
    const prev = out[out.length - 1];
    if (
      prev &&
      sameStyle(prev, run) &&
      (run.src === prev.src + utf8Length(prev.text) || (prev.src === 0 && run.src === 0))
    ) {
      out[out.length - 1] = { ...prev, text: prev.text + run.text };
    } else {
      out.push(run);
    }
  }
  return out;
}

function sameStyle(a, b) {
  return (
    a.role === b.role &&
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.underline === b.underline &&
    a.strike === b.strike &&
    a.sup === b.sup &&
    a.sub === b.sub &&
    a.hl === b.hl &&
    a.hl_color === b.hl_color &&
    a.mark_color === b.mark_color &&
    a.token === b.token &&
    a.size === b.size &&
    a.font === b.font &&
    a.text_color === b.text_color
  );
}

/** What a run's DOM is made from, for a key. Not `src` — see `_runEl`. */
function runKey(run) {
  return (
    run.role +
    (run.bold ? "b" : "") +
    (run.italic ? "i" : "") +
    (run.underline ? "u" : "") +
    (run.strike ? "s" : "") +
    (run.sup ? "^" : "") +
    (run.sub ? "_" : "") +
    (run.hl ? "#" + run.hl + ":" + (run.hl_color || "") : "") +
    (run.mark_color ? "=" + run.mark_color : "") +
    (run.token ? "@" + run.token : "") +
    (run.size ? "~" + run.size : "") +
    (run.font ? "$" + run.font : "") +
    (run.text_color ? "%" + run.text_color : "") +
    "\u0001" +
    run.text +
    "\u0002"
  );
}

/**
 * Everything `_rowEl` reads when it builds a row — so two rows with the same
 * key produce the same element, and a row can be reused under it.
 */
function rowKey(row, runs, i, rows) {
  const b = row.boundary;
  let key =
    "r" +
    (row.heading || 0) +
    (row.code ? "c" : "") +
    (row.code && i > 0 && rows[i - 1].code ? "" : "F") +
    (row.code && i < rows.length - 1 && rows[i + 1].code ? "" : "L") +
    "|" +
    (row.code_lang || "") +
    "|" +
    (b ? (b.above || "") + "/" + (b.below || "") : "") +
    "|" +
    (row.align || "") +
    "|" +
    (row.line_height || "") +
    "|";
  for (const run of runs) key += runKey(run);
  return key;
}

/** `rowKey` for a media row: the element built and the row core addresses. */
function mediaKey(media, row) {
  const len = (row?.runs || []).reduce((n, r) => n + r.text.length, 0);
  return "m" + len + "|" + JSON.stringify([media.kind, media.src, media.poster, media.alt, media.sources]);
}

/** `rowKey` for a display formula's row: the picture drawn and the row core
 *  addresses. The ink is in it because the picture is. */
function mathBlockKey(m, row, ink) {
  const len = (row?.runs || []).reduce((n, r) => n + r.text.length, 0);
  return "M" + len + "|" + ink + "|" + (m.display ? "D" : "T") + m.tex;
}

/** What an ordinary row's key leaves out when it holds a formula: the TeX
 *  and the ink of each inline one, neither of which the `∑` run itself
 *  carries, and — for any row a formula stands on, `hasMath`, a block's
 *  placeholder row included — whether the typesetter was there to draw it. */
function mathRowKey(runs, mathInlineAt, ink, hasMath) {
  if (!hasMath) return "";
  let key = "";
  for (const run of runs) {
    if (run.role !== "math") continue;
    const m = mathInlineAt.get(run.src);
    if (m) key += "\u0004" + (m.display ? "D" : "T") + m.tex;
  }
  return key + "|" + ink + "|" + mathState();
}

/** `rowKey` for a page break: a rule is a rule, and the only thing that can
 *  change under it is what core counts the placeholder row as. */
function pageBreakKey(row) {
  const len = (row?.runs || []).reduce((n, r) => n + r.text.length, 0);
  return "pb" + len;
}

/** `rowKey` for a drawn grid: its shape, alignment, and every cell's runs. */
function tableKey(t) {
  let key = "t";
  for (const row of t.grid) {
    key += row.head ? "H" : "R";
    for (const cell of row.cells) {
      key += "|" + (cell.align || "") + "[";
      for (const line of cell.lines) {
        key += "\u0003";
        for (const run of canonicalRuns(line.runs)) key += runKey(run);
      }
      key += "]";
    }
    key += "\n";
  }
  return key;
}

/**
 * A TreeWalker over a row's editable text nodes — everything except the code
 * block's language label, which is chrome, not document text, and must never
 * count toward an offset.
 */
function textWalker(rowEl) {
  return document.createTreeWalker(rowEl, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) =>
      n.parentElement && n.parentElement.closest(".leaf-code-lang, .leaf-media")
        ? NodeFilter.FILTER_REJECT
        : NodeFilter.FILTER_ACCEPT,
  });
}

/** The row's editable text length in UTF-16 units (label excluded). */
function rowTextLength(rowEl) {
  const w = textWalker(rowEl);
  let acc = 0,
    n;
  while ((n = w.nextNode())) acc += n.length;
  return acc;
}

/**
 * How long core believes a *drawn* row is, or null for an ordinary one.
 *
 * A row the renderer draws its own element on — block media, a page break — is
 * where the text core addresses and the text the browser renders are unrelated.
 * Core lays out `🖼 alt` — nine columns for a picture with a short caption — and
 * publishes two caret stops on it, one in front of the media and one past it.
 * The renderer draws the real element instead, whose only editable text is a
 * zero-width space on each side. Mapping a column through *that* text put both
 * stops at the same place, so the caret could never be seen after a picture and
 * a step down through a document needed two presses per image to get by.
 *
 * The row's two ends are the only positions either description agrees on, so
 * that is what the two are mapped through.
 */
function atomCoreLen(rowEl) {
  const raw = rowEl?.dataset?.leafCoreLen;
  return raw == null ? null : Number(raw);
}

/** A collapsed `Range` `off` UTF-16 units into a row's editable text. */
function rangeAtOffset(rowEl, off) {
  const coreLen = atomCoreLen(rowEl);
  // Core's column, translated to the near or far side of the drawn element —
  // the row's start, or past everything on it.
  if (coreLen != null) off = off <= 0 ? 0 : rowTextLength(rowEl);
  const walker = textWalker(rowEl);
  let node,
    acc = 0,
    last = null;
  while ((node = walker.nextNode())) {
    last = node;
    if (acc + node.length >= off) {
      const r = document.createRange();
      r.setStart(node, off - acc);
      r.collapse(true);
      return r;
    }
    acc += node.length;
  }
  const r = document.createRange();
  if (last) {
    r.setStart(last, last.length);
    r.collapse(true);
  } else {
    // Empty row (only a <br>): collapse to its start so the caret sits on the line.
    r.setStart(rowEl, 0);
    r.collapse(true);
  }
  return r;
}

/**
 * The UTF-16 text offset of a DOM point within a row: the editable text length
 * from the row's start up to `(node, offset)` (the code-lang label excluded).
 * `doc.set_selection` / `click_ch` map it to core's display column, so wide
 * glyphs stay correct.
 */
function offsetTo(rowEl, node, offset) {
  const coreLen = atomCoreLen(rowEl);
  if (coreLen != null) {
    // Anywhere but hard against the row's start counts as past the media, so a
    // single ArrowRight steps over a picture instead of landing in the gap
    // between it and the zero-width space in front of it.
    return domOffsetIn(rowEl, node, offset) <= 0 ? 0 : coreLen;
  }
  return domOffsetIn(rowEl, node, offset);
}

/**
 * The UTF-16 offset into a cell line's text at source offset `src` — the inverse
 * of `srcInLine`, for placing a `Range` where core says the caret is.
 *
 * Walks run by run, because a run is the largest unit whose glyphs are known to
 * be contiguous in the source: within one, advancing a character advances the
 * offset by that character's UTF-8 length, and the run's own offset says where
 * it started. `lineStart` is the fallback for a line with no runs at all.
 */
function utf16InLine(lineEl, src, lineStart) {
  let acc = 0;
  for (const runEl of lineEl._leafRuns || []) {
    const base = runEl._src;
    const text = runEl.textContent;
    const bytes = utf8Length(text);
    if (src < base) return acc;
    if (src <= base + bytes) {
      let b = base;
      let u = 0;
      for (const ch of text) {
        if (b >= src) break;
        b += utf8Length(ch);
        u += ch.length;
      }
      return acc + u;
    }
    acc += text.length;
  }
  return src <= lineStart ? 0 : acc;
}

/**
 * The source offset of a DOM point inside a cell line — the inverse of
 * `utf16InLine`. Clamped to the line's own range, since a point on the line
 * element itself (rather than in a run) addresses one of its ends.
 */
function srcInLine(lineEl, node, offset, lineStart, lineEnd) {
  const clamp = (v) => Math.min(Math.max(v, lineStart), lineEnd);
  const runEl = (node.nodeType === 1 ? node : node.parentElement)?.closest(".leaf-run");
  if (!runEl) {
    // An element endpoint: a child index into the line, so sum what precedes it.
    if (node === lineEl) {
      let acc = 0;
      for (let i = 0; i < offset && i < node.childNodes.length; i++) {
        acc += node.childNodes[i].textContent.length;
      }
      return clamp(acc === 0 ? lineStart : lineEnd);
    }
    return clamp(lineStart);
  }
  const base = runEl._src;
  const text = node.nodeType === 3 ? node.textContent : runEl.textContent;
  let b = base;
  let u = 0;
  for (const ch of text) {
    if (u >= offset) break;
    u += ch.length;
    b += utf8Length(ch);
  }
  return clamp(b);
}

/** A string's length in UTF-8 bytes — core's offsets are byte offsets, and JS
 *  strings are counted in UTF-16 units. */
function utf8Length(text) {
  let n = 0;
  for (const ch of text) {
    const c = ch.codePointAt(0);
    n += c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
  }
  return n;
}

/**
 * The UTF-16 offset of a DOM point within a row's editable text, without the
 * media translation `offsetTo` applies on top of it.
 */
function domOffsetIn(rowEl, node, offset) {
  if (node.nodeType !== 3) {
    let acc = 0;
    for (let i = 0; i < offset && i < node.childNodes.length; i++) {
      const c = node.childNodes[i];
      if (
        c.nodeType === 1 &&
        (c.classList.contains("leaf-code-lang") || c.classList.contains("leaf-media"))
      ) {
        continue;
      }
      acc += c.textContent.length;
    }
    return acc;
  }
  const walker = textWalker(rowEl);
  let acc = 0;
  let n;
  while ((n = walker.nextNode())) {
    if (n === node) return acc + offset;
    acc += n.length;
  }
  return acc;
}

/** Cross-browser `caret{Range,Position}FromPoint` → `{node, offset}` or null. */
function caretFromPoint(x, y) {
  if (document.caretRangeFromPoint) {
    const r = document.caretRangeFromPoint(x, y);
    return r ? { node: r.startContainer, offset: r.startOffset } : null;
  }
  if (document.caretPositionFromPoint) {
    const p = document.caretPositionFromPoint(x, y);
    return p ? { node: p.offsetNode, offset: p.offset } : null;
  }
  return null;
}

/** Inject the shared stylesheet once. Colours are light/dark aware and can be
 *  overridden per-editor via the `--leaf-*` custom properties. */
function ensureStylesheet() {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement("style");
  style.id = STYLE_ID;
  // The presentation vocabulary is the same text `presentation.css` ships —
  // scoped to the editor's own surface, because a host page has its own
  // `.center` and an editor has no business restyling it. The editor's
  // corrections to those rules follow, and win by coming last.
  style.textContent = EDITOR_CSS + scopeCss(PRESENTATION_CSS, ".leaf-editor") + EDITOR_PRESENTATION_CSS;
  document.head.appendChild(style);
}

/**
 * The same rules, confined to a prefix: every selector in `css` gets `prefix `
 * in front of it, so `.center` becomes `.leaf-editor .center`.
 *
 * Deliberately simple, and safe only for the shape `PRESENTATION_CSS` is in —
 * flat rules, one selector list each, no at-rules and no nesting. Comments are
 * dropped first so a brace inside one cannot be read as a rule.
 */
function scopeCss(css, prefix) {
  return css
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/([^{}]+)\{/g, (_, selectors) =>
      selectors
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean)
        .map((s) => prefix + " " + s)
        .join(", ") + " {"
    );
}

const EDITOR_CSS = `
.leaf-editor {
  /* Colour defaults (light). A host can override any of these on the element. */
  --leaf-text: #23262c;
  --leaf-caret: #1e8f7e;
  --leaf-sel: #bcdcf5;
  --leaf-muted: #6a7280;
  --leaf-link: #1d68c7;
  --leaf-mark-bg: #f4e59a;
  --leaf-mark-fg: #23262c;
  /* The seven washes a coloured highlight can name. Pale enough that
     --leaf-mark-fg still reads over them, so the colour is the pen and the ink
     is unchanged. Yellow is --leaf-mark-bg itself: a document that says yellow
     and one that says nothing both mean a yellow highlighter. */
  --leaf-mk-red: #f8c9c9;
  --leaf-mk-orange: #fbdcb4;
  --leaf-mk-yellow: #f4e59a;
  --leaf-mk-green: #c6ecc6;
  --leaf-mk-blue: #c4dcf7;
  --leaf-mk-purple: #e0cdf7;
  --leaf-mk-brown: #e4d2b2;
  /* The same seven names as *ink* — what a run whose data-color says so is
     painted. The wash above is a background and pale by design; these are
     letters, so they are the deeper version of each hue, the one the syntax
     palette below already sets code in. presentation.css names the same
     properties with a mid-tone fallback, for a page that defines neither. */
  --leaf-red: #c02617;
  --leaf-orange: #b45309;
  --leaf-yellow: #8f7000;
  --leaf-green: #0a7040;
  --leaf-blue: #1d68c7;
  --leaf-purple: #9526a0;
  --leaf-brown: #8a5a2b;
  --leaf-code-fg: #b5305f;
  --leaf-code-bg: #f1f2f4;
  --leaf-code-border: #dfe2e8;
  --leaf-hl-bg: #ffe08a;
  /* The syntax-highlighting palette inside a fenced block, one per token
     class core emits — the palette plates publishes a page with, so a block
     reads the same in the editor and on the site. Punctuation and comments
     share the quiet grey. A run with no token keeps the row's own ink. */
  --leaf-syn-punctuation: #6a7580;
  --leaf-syn-keyword: #9526a0;
  --leaf-syn-entity: #2b62d9;
  --leaf-syn-support: #016a99;
  --leaf-syn-constant: #98590a;
  --leaf-syn-string: #0a7040;
  --leaf-syn-comment: #6a7580;
  --leaf-syn-invalid: #c02617;

  position: relative;
  overflow: auto;
  cursor: text;
  color: var(--leaf-text);
  font-family: var(--leaf-font);
  font-size: var(--leaf-size);
  line-height: var(--leaf-line);
}
@media (prefers-color-scheme: dark) {
  .leaf-editor {
    --leaf-text: #d7dce5;
    --leaf-caret: #7fd1c1;
    --leaf-sel: #34506b;
    --leaf-muted: #7a8394;
    --leaf-link: #6fb3ff;
    --leaf-mark-bg: #d8c56a;
    --leaf-mark-fg: #1c1f26;
    /* Deeper on a dark page, for the reason --leaf-mark-bg is: the ink stays
       dark, so a wash that reads as pale against white reads as a glare
       against near-black. */
    --leaf-mk-red: #d89b9b;
    --leaf-mk-orange: #d9b384;
    --leaf-mk-yellow: #d8c56a;
    --leaf-mk-green: #9cd09c;
    --leaf-mk-blue: #93b8dd;
    --leaf-mk-purple: #bda3dc;
    --leaf-mk-brown: #c0a87e;
    /* The inks lighten as the page darkens, for the reason the washes deepen:
       the same seven names, legible against near-black. */
    --leaf-red: #f7768e;
    --leaf-orange: #e09c50;
    --leaf-yellow: #e0af68;
    --leaf-green: #9ece6a;
    --leaf-blue: #7aa2f7;
    --leaf-purple: #c678dd;
    --leaf-brown: #c0a87e;
    --leaf-code-fg: #e59ac0;
    --leaf-code-bg: #2a2f3a;
    --leaf-code-border: #3a4150;
    --leaf-hl-bg: #6b5a1e;
    --leaf-syn-punctuation: #7f8b99;
    --leaf-syn-keyword: #c678dd;
    --leaf-syn-entity: #7aa2f7;
    --leaf-syn-support: #56b6c2;
    --leaf-syn-constant: #e0af68;
    --leaf-syn-string: #9ece6a;
    --leaf-syn-comment: #7f8b99;
    --leaf-syn-invalid: #f7768e;
  }
}
/* The editable surface: the browser draws the caret (themed) and selection. */
.leaf-content {
  position: relative; padding: 16px 20px; min-height: 100%;
  outline: none; caret-color: var(--leaf-caret);
}
.leaf-editor ::selection { background: var(--leaf-sel); }
.leaf-editor ::-moz-selection { background: var(--leaf-sel); }

/* One core row is exactly one visual line: core owns wrapping, so the browser
   must not re-wrap (that would desync vertical motion from what's drawn). A line
   wider than the viewport scrolls horizontally rather than folding. */
.leaf-row { white-space: pre; position: relative; }
.leaf-row.h { font-weight: 700; }

/* Role → presentation. Headings carry no colour of their own — size and weight
   (set on the row) do the distinguishing, as in leaf-gpui. */
.leaf-r-link { color: var(--leaf-link); text-decoration: underline; }
.leaf-r-mark { background: var(--leaf-mark-bg); color: var(--leaf-mark-fg); border-radius: 2px; }
/* A highlight that named its colour swaps the wash and keeps the ink. */
.leaf-r-mark.leaf-mk-red { background: var(--leaf-mk-red); }
.leaf-r-mark.leaf-mk-orange { background: var(--leaf-mk-orange); }
.leaf-r-mark.leaf-mk-yellow { background: var(--leaf-mk-yellow); }
.leaf-r-mark.leaf-mk-green { background: var(--leaf-mk-green); }
.leaf-r-mark.leaf-mk-blue { background: var(--leaf-mk-blue); }
.leaf-r-mark.leaf-mk-purple { background: var(--leaf-mk-purple); }
.leaf-r-mark.leaf-mk-brown { background: var(--leaf-mk-brown); }
.leaf-r-list { color: var(--leaf-muted); }
.leaf-r-quote { color: var(--leaf-muted); }
.leaf-r-rule { color: var(--leaf-muted); }
/* Raw markup revealed on the caret's line under the "full" markdown mode: the
   asterisks around an emphasis, a heading's "# ", a link's "](dest)". Muted like
   the other scaffolding roles so the line still reads as prose, not as source.
   No backticks in here: this comment is inside EDITOR_CSS, a template literal,
   and one would close it. */
.leaf-r-delimiter { color: var(--leaf-muted); }

/* Where a reveal landed the reader: the block flashes once and fades. */
.leaf-landed { animation: leaf-land 1.2s ease-out; }
@keyframes leaf-land {
  from { background: var(--leaf-hl-bg); }
  to { background: transparent; }
}

/* A host-painted highlight. The wash is the theme's unless the highlight
   brought a colour, which is mixed down so the text stays readable over it. */
.leaf-hl {
  background: var(--leaf-hl-bg);
  border-radius: 2px;
  cursor: pointer;
}
.leaf-hl[style*="--leaf-hl"] {
  background: color-mix(in srgb, var(--leaf-hl) 35%, transparent);
}

/* Inline code (a run outside a fenced block): a monospace pill. */
.leaf-row:not(.code) .leaf-r-code {
  font-family: var(--leaf-mono);
  font-size: 0.92em;
  color: var(--leaf-code-fg);
  background: var(--leaf-code-bg);
  border-radius: 4px;
  padding: 0.05em 0.3em;
}

/* Author emphasis, orthogonal to role. */
.leaf-b { font-weight: 700; }
.leaf-i { font-style: italic; }
.leaf-u { text-decoration: underline; }
.leaf-s { text-decoration: line-through; }

/* Fenced/indented code block: a tinted, bordered panel in the mono family. */
.leaf-row.code {
  font-family: var(--leaf-mono);
  font-size: 0.92em;
  background: var(--leaf-code-bg);
  box-shadow: -20px 0 0 var(--leaf-code-bg), 20px 0 0 var(--leaf-code-bg);
}
.leaf-row.code-first {
  border-top: 1px solid var(--leaf-code-border);
  border-top-left-radius: 6px; border-top-right-radius: 6px; margin-top: 4px;
}
.leaf-row.code-last {
  border-bottom: 1px solid var(--leaf-code-border);
  border-bottom-left-radius: 6px; border-bottom-right-radius: 6px; margin-bottom: 4px;
}
/* Syntax highlighting: a code run's token picks its ink, inside a fenced block
   — or anywhere on the source surface, where a fence's body is the same code
   with its fences showing. Core puts a token on nothing else, and scoping it
   keeps that so. A comment is italic on top. */
:is(.leaf-row.code, .leaf-source) .leaf-t-punctuation { color: var(--leaf-syn-punctuation); }
:is(.leaf-row.code, .leaf-source) .leaf-t-keyword { color: var(--leaf-syn-keyword); }
:is(.leaf-row.code, .leaf-source) .leaf-t-entity { color: var(--leaf-syn-entity); }
:is(.leaf-row.code, .leaf-source) .leaf-t-support { color: var(--leaf-syn-support); }
:is(.leaf-row.code, .leaf-source) .leaf-t-constant { color: var(--leaf-syn-constant); }
:is(.leaf-row.code, .leaf-source) .leaf-t-string { color: var(--leaf-syn-string); }
:is(.leaf-row.code, .leaf-source) .leaf-t-comment { color: var(--leaf-syn-comment); font-style: italic; }
:is(.leaf-row.code, .leaf-source) .leaf-t-invalid { color: var(--leaf-syn-invalid); }

/* The source view: the document as written, set in the mono family at one
   size so its columns line up — a code editor's surface, with the markup
   coloured by what it is. A heading's text is bold, as its row is in the
   rendered view; nothing is sized, because a source line is a line. The
   inline-code pill comes off: a fence's body arrives as a code run per line
   here, and a pill per line is a ladder, not a block. The ink stays. */
.leaf-editor.leaf-source { font-family: var(--leaf-mono); }
.leaf-source :is(.leaf-r-h1, .leaf-r-h2, .leaf-r-h3, .leaf-r-h4, .leaf-r-h5, .leaf-r-h6) {
  font-weight: 700;
}
.leaf-source .leaf-row:not(.code) .leaf-r-code {
  font-size: inherit; background: none; border-radius: 0; padding: 0;
}
.leaf-code-lang {
  position: absolute; right: 6px; top: 1px;
  font-size: 11px; color: var(--leaf-muted); font-family: var(--leaf-font);
  -webkit-user-select: none; user-select: none; /* chrome, not document text */
}

/* A block image / video / audio, in place of core's placeholder glyphs. The row
   drops the surface's white-space:pre (its only text is the two zero-width
   spaces that give the caret a home either side) and grows to fit — the
   web equivalent of leaf-gpui laying images out in pixels rather than reserving
   character rows. */
.leaf-media-row {
  white-space: normal;
  padding: 4px 0;
  line-height: 0; /* no half-line of leading under the element from the ZWSPs */
}
.leaf-media {
  display: block;
  max-width: 100%;
  border-radius: 6px;
  -webkit-user-select: none;
  user-select: none; /* an atom: selected as a unit by the row, never internally */
}
/* Audio has no picture, so it is a transport at a natural control height rather
   than something to fit a box. */
audio.leaf-media { width: 100%; max-width: 420px; border-radius: 999px; }
img.leaf-media, video.leaf-media { max-height: 60vh; }

/* A display formula: its picture centred on the measure, at its own size. */
.leaf-math-row { text-align: center; }
.leaf-math-block { display: inline-block; max-width: 100%; border-radius: 0; }

/* An inline formula: core's one-character atom, drawn as the picture. The
   character stays for the caret to count by and is stretched to the picture's
   width (see _mathRunEl); the picture is the background. The baseline of an
   inline-block with hidden overflow is its bottom edge, which vertical-align
   then drops by the formula's depth. The caret keeps the text colour: only the
   glyph's fill goes clear. */
.leaf-r-math.leaf-math {
  display: inline-block;
  overflow: hidden;
  -webkit-text-fill-color: transparent;
  background-repeat: no-repeat;
  background-position: left top;
  background-size: 100% 100%;
  white-space: pre;
}
/* TeX the typesetter could not read: the atom, tinted, with the fault in its
   title. Also every math run before the surface has said it paints pictures. */
.leaf-r-math { color: var(--leaf-muted); }
.leaf-r-math.leaf-math-error { color: #c0392b; text-decoration: underline wavy; }

/* Didn't load: a missing file would otherwise leave the row blank, with nothing
   to say what was meant to be there. */
.leaf-media-broken .leaf-media {
  min-height: var(--leaf-line);
  min-width: 8em;
  outline: 1px dashed var(--leaf-muted);
  outline-offset: 2px;
}

/* A real grid in place of core's box-glyph picture. Collapsed borders so one
   rule sits between two cells rather than two abutting; the widths are the
   browser's to work out from the content, which is the whole reason a
   proportional surface draws its own instead of painting fixed cells.
   No backticks in here: this is inside EDITOR_CSS, a template literal. */
.leaf-table {
  border-collapse: collapse;
  margin: 6px 0;
  /* The grid is chrome, so it does not inherit the row's pre wrapping —
     a cell wraps like ordinary prose. */
  white-space: normal;
}
.leaf-table th,
.leaf-table td {
  border: 1px solid var(--leaf-code-border);
  padding: 3px 8px;
  vertical-align: top;
  text-align: left;
  outline: none;
}
.leaf-table th { font-weight: 700; background: var(--leaf-code-bg); }
.leaf-cell-line { min-height: var(--leaf-line); }

/* Raised and lowered text — a footnote's marker, an author's superscript.
   Styled with vertical-align rather than real sup/sub elements, so the glyphs
   stay in the same text node the offset mapping counts through. */
.leaf-sup { vertical-align: super; font-size: 0.75em; }
.leaf-sub { vertical-align: sub; font-size: 0.75em; }

.leaf-measure {
  position: absolute; visibility: hidden; white-space: pre; top: -9999px; left: 0;
  font-family: var(--leaf-font); font-size: var(--leaf-size);
}
`;

/**
 * leaf's presentation vocabulary as CSS — the same rules as
 * [`presentation.css`](./presentation.css), which is the file a page built from
 * a leaf document links to, and which the test page holds level with this
 * constant so the two cannot drift.
 *
 * Exported because a host that renders leaf documents *outside* the editor (a
 * preview pane, a print sheet) needs these rules and should not have to fetch a
 * file to get them. The editor injects them scoped to its own surface; on a
 * published page they are unscoped, which is the whole point of them.
 *
 * One rule per token, and only the tokens that are *names*: a stylesheet can
 * enumerate `large` and `red` and can never enumerate `14pt` or `#c03030`. A
 * host that renders runs itself draws those the way `_runEl` does — the value
 * inline, beside the attribute — and a page that has only this file draws the
 * names and leaves the values at its own defaults.
 */
export const PRESENTATION_CSS = `
.center  { text-align: center }
.right   { text-align: right }
.justify { text-align: justify }

[data-line-height="1.15"] { line-height: 1.15 }
[data-line-height="1.5"]  { line-height: 1.5 }
[data-line-height="2"]    { line-height: 2 }

[data-size="xx-small"]  { font-size: xx-small }
[data-size="x-small"]   { font-size: x-small }
[data-size="small"]     { font-size: small }
[data-size="large"]     { font-size: large }
[data-size="x-large"]   { font-size: x-large }
[data-size="xx-large"]  { font-size: xx-large }
[data-size="xxx-large"] { font-size: xxx-large }

[data-font="serif"]      { font-family: serif }
[data-font="sans-serif"] { font-family: sans-serif }
[data-font="monospace"]  { font-family: monospace }
[data-font="cursive"]    { font-family: cursive }

[data-color="red"]    { color: var(--leaf-red, #d24b3f) }
[data-color="orange"] { color: var(--leaf-orange, #b96c12) }
[data-color="yellow"] { color: var(--leaf-yellow, #9a7b12) }
[data-color="green"]  { color: var(--leaf-green, #2e8b57) }
[data-color="blue"]   { color: var(--leaf-blue, #3d7fd6) }
[data-color="purple"] { color: var(--leaf-purple, #a855c9) }
[data-color="brown"]  { color: var(--leaf-brown, #a9713c) }
`;

/**
 * What the editor says about the vocabulary that a published page does not.
 * Injected after the scoped copy of `PRESENTATION_CSS`, so these win.
 *
 * Two corrections and one drawing. The size keywords are *absolute*: on a page
 * whose headings are the browser's own that is exactly right, but here a
 * heading's size is set on the row, and a large keyword on a run inside it would
 * drop the run to 18px instead of making it a step larger than the heading. So
 * the same ramp is restated as multiples of the text around it, in the ratios
 * CSS's own user-agent stylesheet gives those very words — 13/16 for `small`,
 * 18/16 for `large`, and so on, which is what makes the two agree on a page
 * whose body is 16px. Line spacing is the other: the tokens are multiples of the *theme's*
 * line height, a number only this editor knows, so the row's line box is
 * computed in `_rowEl` and arrives as an inline style; the unitless rule
 * underneath is a published page's answer and is overridden by that. And a page
 * break is a rule across the measure, which only a surface that is not itself
 * paginated has to draw.
 */
const EDITOR_PRESENTATION_CSS = `
.leaf-editor [data-size="xx-small"]  { font-size: 0.5625em }
.leaf-editor [data-size="x-small"]   { font-size: 0.625em }
.leaf-editor [data-size="small"]     { font-size: 0.8125em }
.leaf-editor [data-size="large"]     { font-size: 1.125em }
.leaf-editor [data-size="x-large"]   { font-size: 1.5em }
.leaf-editor [data-size="xx-large"]  { font-size: 2em }
.leaf-editor [data-size="xxx-large"] { font-size: 3em }

/* A page break: the seam between two pages, on a surface that has only one. The
   row is an atom holding no caret (see _pageBreakRowEl), so it is drawn rather
   than written — a dashed hairline across the measure, which is what
   leaf-swift's continuous view draws in the same place. */
.leaf-page-break {
  white-space: normal;
  padding: 4px 0;
  line-height: 0;
  -webkit-user-select: none;
  user-select: none;
  cursor: default;
}
.leaf-page-break-rule {
  border-top: 1px dashed var(--leaf-muted);
  height: 0;
}
/* Printed, it is the thing it stands for. */
@media print {
  .leaf-page-break { break-after: page; page-break-after: always; }
  .leaf-page-break-rule { border-top: 0; }
}
`;
