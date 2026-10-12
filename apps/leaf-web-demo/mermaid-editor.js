// A Mermaid renderer and editor for leaf's `regionRenderers` and
// `regionEditors` — this demo page's, not leaf's.
//
// leaf hands every ```mermaid fence to them as a region. Closed, the fence is
// its diagram: `renderMermaid` answers with the SVG, which leaf lays out and
// draws as a picture, as it does a formula, so it prints and pages with the
// text around it. Opened by → in front of it, ← behind it or a click, it is
// the fence's source in a textarea beside a live preview (`mermaidEditor`).
// What the textarea holds goes back through `host.replace`, which leaf turns
// into one splice on the fence's content and one undo step.
//
// The diagrams are drawn by leaf-mermaid (mmdr, in Rust) from a module of its
// own beside this page, run in a worker the first time a diagram is drawn;
// leaf-web draws none, and its npm package does not carry the module. See
// docs/proposals/embedded-editors.md.

// One worker draws every diagram, in the order asked (mermaid-worker.js).
let worker = null;
let nextId = 0;
const waiting = new Map();

function render(source, dark) {
  if (!worker) {
    worker = new Worker(new URL("./mermaid-worker.js", import.meta.url), { type: "module" });
    worker.onmessage = ({ data }) => {
      waiting.get(data.id)?.(data);
      waiting.delete(data.id);
    };
  }
  const id = ++nextId;
  return new Promise((resolve) => {
    waiting.set(id, resolve);
    worker.postMessage({ id, source, dark });
  });
}

/** Whether the page is dark: what the last picture leaf asked for said, or
 *  the page's scheme before it has asked. The preview follows it. */
let dark = matchMedia("(prefers-color-scheme: dark)").matches;

/** A note in place of a diagram — an empty one, or one that will not draw. */
function note(text, fault) {
  const fig = document.createElement("figure");
  fig.className = "mermaid-drawing" + (fault ? " fault" : "");
  fig.textContent = text;
  return fig;
}

/**
 * The renderer for `fence:mermaid`: the diagram as `{svg, width, height}`,
 * drawn in the worker, so leaf shows the fence's placeholder until it
 * arrives. A fence that is empty, or that Mermaid cannot read, is a note
 * instead, as an element.
 */
export async function renderMermaid(region, ctx) {
  dark = ctx.dark;
  if (!region.content.trim()) return { element: note("(empty diagram)", false) };
  const p = await render(region.content, ctx.dark);
  if (p.fault != null) return { element: note(p.fault, true) };
  return { svg: p.svg, width: p.width, height: p.height };
}

/** Draw `source` into the preview `target`, or the fault in its place. A
 *  later call on the same target wins over an earlier one still rendering. */
async function draw(target, source) {
  const ticket = (target._ticket = (target._ticket ?? 0) + 1);
  let img = null;
  let fault = null;
  if (source.trim()) {
    const p = await render(source, dark);
    if (p.fault != null) {
      fault = p.fault;
    } else {
      img = document.createElement("img");
      img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(p.svg);
      img.width = Math.round(p.width);
      img.height = Math.round(p.height);
      img.alt = "Mermaid diagram";
    }
  }
  if (target._ticket !== ticket) return;
  target.classList.toggle("fault", fault !== null);
  target.replaceChildren(img ?? document.createTextNode(fault ?? "(empty diagram)"));
}

// A fence's content is whole lines, each with its newline; the textarea shows
// it without the last one, and an empty textarea is an empty fence.
const toText = (content) => (content.endsWith("\n") ? content.slice(0, -1) : content);
const toContent = (text) => (text === "" ? "" : text + "\n");

/** The editor for `fence:mermaid`: the source beside a live preview. */
export const mermaidEditor = {
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
      timer = setTimeout(() => draw(preview, toContent(source.value)), 60);
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
