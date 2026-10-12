// Renders Mermaid off the page's thread for mermaid-editor.js: a large
// flowchart takes a quarter of a second to lay out, which on the page would
// stall the typing that asked for it. Labels are measured on an offscreen
// canvas, in the faces the page will draw the picture's text in.
import init, { render_mermaid } from "./pkg-mermaid/leaf_mermaid_wasm.js";

const ready = init();
const ctx = new OffscreenCanvas(1, 1).getContext("2d");

function measure(text, size, family) {
  ctx.font = `${size}px ${family}`;
  return ctx.measureText(text).width;
}

onmessage = async ({ data: { id, source, dark } }) => {
  await ready;
  try {
    const p = render_mermaid(source, dark, measure);
    postMessage({ id, svg: p.svg, width: p.width, height: p.height });
    p.free?.();
  } catch (e) {
    postMessage({ id, fault: String(e?.message ?? e) });
  }
};
