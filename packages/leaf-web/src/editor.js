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

let wasmReady = null;

export class LeafEditor {
  /**
   * Load and instantiate the wasm module. Call once before constructing any
   * editor; repeated calls share one instantiation. `wasmUrl` overrides where
   * the `.wasm` is fetched from (defaults to the file next to the JS glue).
   * @param {string | URL} [wasmUrl]
   * @returns {Promise<void>}
   */
  static init(wasmUrl) {
    if (!wasmReady) wasmReady = init(wasmUrl ? { module_or_path: wasmUrl } : undefined);
    return wasmReady.then(() => undefined);
  }

  /**
   * @param {HTMLElement} container  the element to mount into (becomes the
   *   scroll viewport; its contents are replaced).
   * @param {{ source?: string, format?: string, theme?: Partial<typeof DEFAULT_THEME>,
   *           onChange?: (state: EditorState) => void }} [opts]
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
    this._composing = false;
    /** Guard so our own selection restores don't echo back through selectionchange. */
    this._settingSelection = false;

    this.doc = new LeafDoc(opts.source ?? "", opts.format ?? "markdown");

    ensureStylesheet();
    this._buildDom();
    this._applyTheme();
    this._bindEvents();

    // First paint at the wrap width the viewport implies.
    this.render(this.doc.set_width(this._cols()));
    this.focus();
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
    this.doc.free?.();
    this.container.innerHTML = "";
    this.container.classList.remove("leaf-editor");
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

  /**
   * Recompute the wrap width from the current viewport and repaint. Called
   * automatically on container resize; expose it for hosts that resize the
   * editor programmatically.
   */
  refit() {
    this.render(this.doc.set_width(this._cols()));
  }

  // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────
  // Each syncs core's selection from the browser first, so the command acts on
  // exactly what the user has selected, then repaints.

  toggleBold() { this._command((d) => d.toggle_bold()); }
  toggleItalic() { this._command((d) => d.toggle_italic()); }
  toggleCode() { this._command((d) => d.toggle_code()); }
  toggleMark() { this._command((d) => d.toggle_mark()); }
  toggleUnderline() { this._command((d) => d.toggle_underline()); }
  toggleStrike() { this._command((d) => d.toggle_strike()); }
  setParagraph() { this._command((d) => d.set_paragraph()); }
  /** Toggle the block to a heading of `level` (1–6); the active level toggles off to a paragraph. */
  setHeading(level) { this._command((d) => d.set_heading(level)); }
  toggleBlockquote() { this._command((d) => d.toggle_blockquote()); }
  toggleList(ordered) { this._command((d) => d.toggle_list(!!ordered)); }
  insertLink(dest) { this._command((d) => d.insert_link(dest)); }
  undo() { this._command((d) => d.undo()); }
  redo() { this._command((d) => d.redo()); }
  selectAll() { this._command((d) => d.select_all()); }
  /** Switch between the rendered WYSIWYG surface and the raw source. */
  toggleView() { this._command((d) => d.toggle_view()); }

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
    this._lastView = view;

    for (const rEl of this.rowEls) rEl.remove();
    this.rowEls = [];
    const frag = document.createDocumentFragment();
    for (let i = 0; i < view.rows.length; i++) {
      frag.appendChild(this._rowEl(view.rows[i], i, view.rows));
    }
    this.contentEl.appendChild(frag);

    this._restoreSelection(view);
    this._scrollCaretIntoView(view.caret_row);
    this._emitChange(view);
  }

  /** Build one row element from a `Row`. */
  _rowEl(row, i, rows) {
    const div = el("div", "leaf-row");
    // Sizing the *whole* row from its heading level (not per run) mirrors gpui
    // shaping a heading's line at one size: an inline `code` run inside a
    // heading still reads at the heading's size.
    if (row.heading) {
      const size = this.theme.fontSize * this.theme.headingScale[Math.min(row.heading, 6) - 1];
      div.classList.add("h");
      div.style.fontSize = size + "px";
      div.style.lineHeight = size * this._ratio + "px";
    }
    // Keep empty rows occupying their line so the caret has somewhere to sit —
    // except a block-boundary gap row (empty, holds no caret), drawn short so a
    // paragraph break reads as spacing rather than a blank line.
    div.style.minHeight = (row.heading
      ? this.theme.fontSize * this.theme.headingScale[Math.min(row.heading, 6) - 1] * this._ratio
      : isBlockGap(row)
        ? this.theme.lineHeight * this.theme.blockGapScale
        : this.theme.lineHeight) + "px";

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

    for (const run of row.runs) {
      const span = document.createElement("span");
      let cls = "leaf-r-" + run.role;
      if (run.bold) cls += " leaf-b";
      if (run.italic) cls += " leaf-i";
      if (run.underline) cls += " leaf-u";
      if (run.strike) cls += " leaf-s";
      span.className = cls;
      span.textContent = run.text;
      div.appendChild(span);
    }

    // A contenteditable block needs a placeholder to hold a caret when it has no
    // text of its own (an empty paragraph).
    if (row.runs.length === 0) div.appendChild(document.createElement("br"));

    this.rowEls.push(div);
    return div;
  }

  // ── native selection (model ⇄ browser) ────────────────────────────────────

  /** Paint the model's caret/selection onto the browser's native selection. */
  _restoreSelection(view) {
    const focusEl = this.rowEls[view.caret_row];
    const anchorEl = this.rowEls[view.anchor_row];
    if (!focusEl || !anchorEl) return;
    const f = rangeAtOffset(focusEl, view.caret_ch);
    const a = view.has_selection ? rangeAtOffset(anchorEl, view.anchor_ch) : f;
    const sel = window.getSelection();
    this._settingSelection = true;
    try {
      // base/extent (not start/end) keeps the model's selection direction.
      sel.setBaseAndExtent(a.startContainer, a.startOffset, f.startContainer, f.startOffset);
    } catch {
      /* endpoints in an edge/detached node — leave the selection as the browser has it */
    }
    this._settingSelection = false;
  }

  /** Read the browser's selection into core (no repaint). Returns whether it mapped. */
  _syncFromDom() {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) return false;
    const r = sel.getRangeAt(0);
    if (!this.contentEl.contains(r.commonAncestorContainer)) return false;
    const a = this._domPoint(sel.anchorNode, sel.anchorOffset);
    const f = this._domPoint(sel.focusNode, sel.focusOffset);
    if (!a || !f) return false;
    this._lastView = this.doc.set_selection(a.row, a.ch, f.row, f.ch);
    return true;
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

  _scrollCaretIntoView(row) {
    const rowEl = this.rowEls[row];
    if (!rowEl) return;
    const c = this.container;
    const top = rowEl.offsetTop;
    const bottom = top + rowEl.offsetHeight;
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

  // ── wrap width ────────────────────────────────────────────────────────────

  /**
   * The column budget the viewport implies, from the body font's average glyph
   * width. Proportional text means this is a good average rather than exact —
   * core wraps to it, and unusually wide/narrow lines vary from the edge — but
   * ordinary prose fills the measure.
   */
  _cols() {
    const avg = this.measureEl.getBoundingClientRect().width / WIDTH_SAMPLE.length;
    if (!(avg > 0)) return 80;
    const cs = getComputedStyle(this.contentEl);
    const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
    const avail = this.container.clientWidth - padX;
    return Math.max(1, Math.floor(avail / avg));
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
      // the browser left in the DOM during composition.
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
      const a = this._domPoint(sel.anchorNode, sel.anchorOffset);
      const f = this._domPoint(sel.focusNode, sel.focusOffset);
      if (a && f) this._emitChange((this._lastView = this.doc.set_selection(a.row, a.ch, f.row, f.ch)));
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

    on(ce, "focus", () => this.container.classList.add("leaf-focus"));
    on(ce, "blur", () => this.container.classList.remove("leaf-focus"));

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
      const html = e.clipboardData.getData("text/html");
      const text = e.clipboardData.getData("text/plain");
      if (!html && !text) return;
      this._syncFromDom();
      this.render(this.doc.paste_rich(html || undefined, text || ""));
      e.preventDefault();
    });

    // Reflow on viewport change.
    if (typeof ResizeObserver !== "undefined") {
      this._resizeObs = new ResizeObserver(() => this.refit());
      this._resizeObs.observe(this.container);
    } else {
      on(window, "resize", () => this.refit());
    }
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
        view = d.insert(e.data ?? "");
        break;
      case "insertReplacementText": {
        // Autocorrect / dictation replacement — the text rides the dataTransfer.
        const rep = (e.dataTransfer && e.dataTransfer.getData("text/plain")) || e.data || "";
        view = d.insert(rep);
        break;
      }
      case "insertParagraph":
      case "insertLineBreak":
        view = d.newline();
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
        view = d.delete_word_back();
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
   * Keyboard shortcuts not covered by a `beforeinput` intent — view toggle and
   * Tab always, plus the formatting/history shortcuts (routed here so they work
   * uniformly). Caret motion and selection are left to the browser (→ native
   * selection → `selectionchange`).
   */
  _onKeyDown(e) {
    if (this._composing || e.isComposing || e.keyCode === 229) return;
    const mod = e.metaKey || e.ctrlKey;
    const d = this.doc;

    if (mod) {
      let op;
      switch (e.key.toLowerCase()) {
        case "b": op = () => d.toggle_bold(); break;
        case "i": op = () => d.toggle_italic(); break;
        case "u": op = () => d.toggle_underline(); break;
        case "e": op = () => d.toggle_view(); break;
        case "z": op = e.shiftKey ? () => d.redo() : () => d.undo(); break;
        case "y": op = () => d.redo(); break;
        default: return; // copy/cut/paste, select-all, ⌘←/→, … stay the browser's
      }
      e.preventDefault();
      this._syncFromDom();
      this.render(op());
      return;
    }

    if (e.key === "Tab") {
      e.preventDefault();
      this._syncFromDom();
      this.render(d.insert("  "));
    }
    // Enter, Backspace, Delete, arrows: handled via beforeinput / native motion.
  }

  _emitChange(view) {
    if (!this._onChange) return;
    this._onChange({
      view: view.view,
      dirty: view.dirty,
      heading: view.heading ?? null,
      active: view.active,
    });
  }
}

/**
 * @typedef {Object} EditorState
 * @property {string} view      `"wysiwyg"` | `"source"`
 * @property {boolean} dirty    buffer differs from last saved
 * @property {number | null} heading  heading level at the caret, or null
 * @property {string[]} active  inline marks active at the caret
 */

// ── module-private helpers ────────────────────────────────────────────────────

function el(tag, cls) {
  const e = document.createElement(tag);
  e.className = cls;
  return e;
}

/**
 * Whether `row` is the blank decoration row core spells a block boundary with:
 * no caret home, and — unlike a table rule or a quote gutter — no visible
 * glyphs. These are the paragraph gaps drawn short so a boundary reads as
 * spacing rather than an empty line.
 */
function isBlockGap(row) {
  return (
    row.decoration &&
    !row.code &&
    row.runs.every((r) => r.text.trim() === "")
  );
}

/**
 * A TreeWalker over a row's editable text nodes — everything except the code
 * block's language label, which is chrome, not document text, and must never
 * count toward an offset.
 */
function textWalker(rowEl) {
  return document.createTreeWalker(rowEl, NodeFilter.SHOW_TEXT, {
    acceptNode: (n) =>
      n.parentElement && n.parentElement.closest(".leaf-code-lang")
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

/** A collapsed `Range` `off` UTF-16 units into a row's editable text. */
function rangeAtOffset(rowEl, off) {
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
  if (node.nodeType !== 3) {
    // An element endpoint (offset = child index): sum the text of the children
    // before it.
    let acc = 0;
    for (let i = 0; i < offset && i < node.childNodes.length; i++) {
      const c = node.childNodes[i];
      if (c.nodeType === 1 && c.classList.contains("leaf-code-lang")) continue;
      acc += c.textContent.length;
    }
    return acc;
  }
  const walker = textWalker(rowEl);
  let acc = 0,
    n;
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
  style.textContent = EDITOR_CSS;
  document.head.appendChild(style);
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
  --leaf-code-fg: #b5305f;
  --leaf-code-bg: #f1f2f4;
  --leaf-code-border: #dfe2e8;

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
    --leaf-code-fg: #e59ac0;
    --leaf-code-bg: #2a2f3a;
    --leaf-code-border: #3a4150;
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
.leaf-r-list { color: var(--leaf-muted); }
.leaf-r-quote { color: var(--leaf-muted); }
.leaf-r-rule { color: var(--leaf-muted); }

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
.leaf-code-lang {
  position: absolute; right: 6px; top: 1px;
  font-size: 11px; color: var(--leaf-muted); font-family: var(--leaf-font);
  -webkit-user-select: none; user-select: none; /* chrome, not document text */
}

.leaf-measure {
  position: absolute; visibility: hidden; white-space: pre; top: -9999px; left: 0;
  font-family: var(--leaf-font); font-size: var(--leaf-size);
}
`;
