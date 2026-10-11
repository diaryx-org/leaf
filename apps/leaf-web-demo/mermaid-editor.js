// A Mermaid editor for leaf's `regionEditors` — this demo page's, not leaf's.
//
// leaf hands every ```mermaid fence to it as a region: closed, the fence is
// drawn as its diagram (`view`); opened by → in front of it, ← behind it or a
// click, it is the fence's source in a textarea beside a live preview
// (`open`). What the textarea holds goes back through `host.replace`, which
// leaf turns into one splice on the fence's content and one undo step. leaf
// knows nothing of Mermaid: mermaid.js is fetched from a CDN by this file the
// first time a diagram is drawn. See docs/proposals/embedded-editors.md.

const MERMAID_URL = "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";

let loading = null;
function mermaid() {
  loading ??= import(MERMAID_URL).then(({ default: m }) => {
    const dark = matchMedia("(prefers-color-scheme: dark)").matches;
    m.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "default" });
    return m;
  });
  return loading;
}

let ids = 0;

/** Draw `source` into `target`, or the fault in its place. A later call on
 *  the same target wins over an earlier one still rendering. */
async function draw(target, source) {
  const ticket = (target._ticket = (target._ticket ?? 0) + 1);
  let svg = null;
  let fault = null;
  try {
    const m = await mermaid();
    svg = source.trim() ? (await m.render(`leaf-mermaid-${++ids}`, source)).svg : null;
  } catch (e) {
    fault = String(e?.message ?? e);
  }
  if (target._ticket !== ticket) return;
  target.classList.toggle("fault", fault !== null);
  if (svg) target.innerHTML = svg;
  else target.textContent = fault ?? "(empty diagram)";
}

// A fence's content is whole lines, each with its newline; the textarea shows
// it without the last one, and an empty textarea is an empty fence.
const toText = (content) => (content.endsWith("\n") ? content.slice(0, -1) : content);
const toContent = (text) => (text === "" ? "" : text + "\n");

export const mermaidEditor = {
  view(region) {
    const fig = document.createElement("figure");
    fig.className = "mermaid-drawing";
    fig.title = "Mermaid — click, or → in front of it, to edit";
    draw(fig, region.content);
    return fig;
  },

  open(region, entry, host) {
    const box = document.createElement("div");
    box.className = "mermaid-editor";
    const source = document.createElement("textarea");
    source.spellcheck = false;
    source.value = toText(region.content);
    const preview = document.createElement("figure");
    preview.className = "mermaid-drawing";
    box.append(source, preview);
    draw(preview, region.content);

    let timer = 0;
    const redraw = () => {
      clearTimeout(timer);
      timer = setTimeout(() => draw(preview, toContent(source.value)), 150);
    };
    source.addEventListener("input", () => {
      host.replace(toContent(source.value));
      redraw();
    });
    // The document's history, not the textarea's: an undo here is an undo of
    // the fence's edits in the document, and comes back as `reload`.
    source.addEventListener("beforeinput", (e) => {
      if (e.inputType === "historyUndo" || e.inputType === "historyRedo") {
        e.preventDefault();
        if (e.inputType === "historyUndo") host.undo();
        else host.redo();
      }
    });
    source.addEventListener("keydown", (e) => {
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key;
      const { selectionStart: a, selectionEnd: b, value } = source;
      if (k === "Escape") {
        e.preventDefault();
        host.leave("after");
      } else if (mod && k.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) host.redo();
        else host.undo();
      } else if (e.shiftKey || mod || e.altKey || a !== b) {
        // A selection, or a modified key, is the textarea's.
      } else if ((k === "ArrowUp" && !value.slice(0, a).includes("\n")) || (k === "ArrowLeft" && a === 0)) {
        e.preventDefault();
        host.leave("before");
      } else if ((k === "ArrowDown" && !value.slice(a).includes("\n")) || (k === "ArrowRight" && a === value.length)) {
        e.preventDefault();
        host.leave("after");
      }
    });

    return {
      element: box,
      focus() {
        source.focus();
        const at = entry.at === "home" ? 0 : source.value.length;
        source.setSelectionRange(at, at);
      },
      reload(content) {
        const at = source.selectionStart;
        source.value = toText(content);
        source.setSelectionRange(Math.min(at, source.value.length), Math.min(at, source.value.length));
        redraw();
      },
      close() {
        clearTimeout(timer);
      },
    };
  },
};
