// A date chip for leaf's `regionRenderers` and `regionEditors` — this demo
// page's, not leaf's: the second client of leaf's picture session, as the
// Mermaid editor is of a host's view.
//
// `:date[2026-10-12]` is an inline directive, which leaf hands over as a
// region of kind `directive:date`. Closed, `renderDate` draws it as a pill in
// the line — red, with the text as it stands, for a label that is not a date.
// Opened by → in front of it, ← behind it or a click, `dateEditor` answers
// with a picture session: leaf keeps the focus and the keys, asks the session
// for its picture on every key, and draws the caret and the selection the
// picture says, over it.
//
// - ← and → move between the year, the month and the day, and leave the chip
//   at either end.
// - ↑ and ↓ add or take one from the field under the caret: a day stays in its
//   month, a month wraps round the year.
// - Digits overwrite the field: the first replaces it, the next fill it, and a
//   full field moves on to the next. `-`, `/` and `.` move on too.
// - Escape, Enter and Tab leave it.
// - A calendar opens under the caret (`host.caretRect()`) while the chip is
//   open, follows the date, and a click on a day writes it and leaves.
//
// Everything is written back as one splice on the label's bytes inside the
// directive's source. See docs/proposals/embedded-editors.md.

const FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif';
const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];
const FIELDS = ["year", "month", "day"];
/** How many digits fill each field. */
const WIDTHS = [4, 2, 2];

const pad = (n, w) => String(n).padStart(w, "0");
const daysIn = (y, m) => new Date(Date.UTC(y, m, 0)).getUTCDate();
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
const wrap = (v, lo, hi) => ((((v - lo) % (hi - lo + 1)) + (hi - lo + 1)) % (hi - lo + 1)) + lo;
const format = ({ y, m, d }) => `${pad(y, 4)}-${pad(m, 2)}-${pad(d, 2)}`;
const fieldsOf = (date) => [pad(date.y, 4), pad(date.m, 2), pad(date.d, 2)];
const bytes = (s) => new TextEncoder().encode(s).length;
const escape = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** `label` as a date, or null when it is not `YYYY-MM-DD` naming a real day. */
export function parseDate(label) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(label.trim());
  if (!m) return null;
  const date = { y: +m[1], m: +m[2], d: +m[3] };
  if (date.y < 1 || date.m < 1 || date.m > 12 || date.d < 1 || date.d > daysIn(date.y, date.m)) return null;
  return date;
}

function today() {
  const t = new Date();
  return { y: t.getFullYear(), m: t.getMonth() + 1, d: t.getDate() };
}

let measureCtx = null;
/** The advance of `text` at `px` in the chip's face. */
function measure(text, px) {
  measureCtx ??= document.createElement("canvas").getContext("2d");
  measureCtx.font = `${px}px ${FONT}`;
  return measureCtx.measureText(text).width;
}

/**
 * The pill, at `size` CSS pixels per em in `ink`: a small calendar mark and
 * `parts` — the three fields with their dashes, or a label that is not a
 * date — laid out left to right. Answers the `{svg}` picture, the text's
 * top and height (where a caret stands), and each part's box.
 */
function pill(parts, size, ink, fault) {
  const fs = size * 0.86;
  const h = Math.round(size * 1.32);
  const baseline = h / 2 + fs * 0.34;
  const padX = size * 0.5;
  const icon = fs * 0.8;
  let x = padX + icon + size * 0.3;
  const boxes = parts.map((text) => {
    const w = measure(text, fs);
    const box = { text, x, w };
    x += w;
    return box;
  });
  const width = Math.ceil(x + padX);
  const colour = fault ? "#c0392b" : ink;
  const iy = (h - icon) / 2;
  const ix = padX;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${h}" viewBox="0 0 ${width} ${h}">` +
    `<rect x="0.5" y="0.5" width="${width - 1}" height="${h - 1}" rx="${(h - 1) / 2}" ` +
    `fill="${colour}" fill-opacity="${fault ? 0.12 : 0.08}" stroke="${colour}" stroke-opacity="${fault ? 0.7 : 0.28}"/>` +
    // The calendar mark: a page with a binding along its top.
    `<rect x="${ix}" y="${iy + icon * 0.12}" width="${icon}" height="${icon * 0.88}" rx="${icon * 0.15}" ` +
    `fill="none" stroke="${colour}" stroke-opacity="0.7" stroke-width="${Math.max(1, size / 14)}"/>` +
    `<rect x="${ix}" y="${iy + icon * 0.12}" width="${icon}" height="${icon * 0.28}" fill="${colour}" fill-opacity="0.7"/>` +
    boxes
      .map(
        (b) =>
          `<text x="${b.x}" y="${baseline}" font-family='${FONT}' font-size="${fs}" fill="${colour}">${escape(b.text)}</text>`,
      )
      .join("") +
    `</svg>`;
  return {
    picture: { svg, width, height: h, baseline },
    textTop: baseline - fs * 0.92,
    textHeight: fs * 1.2,
    boxes,
  };
}

/** The parts the open chip is drawn as: year, dash, month, dash, day. The
 *  fields are parts 0, 2 and 4. */
const partsOf = (date) => {
  const [y, m, d] = fieldsOf(date);
  return [y, "-", m, "-", d];
};

/**
 * The renderer for `directive:date`: the pill, or — for a label that is not
 * a date — a red one with the label as written.
 */
export function renderDate(region, ctx) {
  const date = parseDate(region.label);
  const parts = date ? partsOf(date) : [region.label.trim() || "no date"];
  return pill(parts, ctx.size, ctx.ink, !date).picture;
}

/** The calendar under the open chip: a month grid, built afresh each time. */
function calendar() {
  const box = document.createElement("div");
  box.className = "date-popover";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", "Choose a date");
  // The editor keeps the focus — and the chip stays open — through a click
  // here.
  box.addEventListener("mousedown", (e) => e.preventDefault());
  return box;
}

/** The editor for `directive:date`: a picture session leaf runs. */
export const dateEditor = {
  open(region, entry, host) {
    let content = region.content;
    let date = parseDate(region.label) ?? today();
    let field = entry.at === "home" ? 0 : 2;
    /** The digits typed into the field so far, since the caret came to it. */
    let typed = "";
    if (entry.at === "point") {
      const { boxes } = pill(partsOf(date), entry.size, "#000000", false);
      const fields = [boxes[0], boxes[2], boxes[4]];
      let best = Infinity;
      fields.forEach((b, i) => {
        const dist = Math.abs(entry.x - (b.x + b.w / 2));
        if (dist < best) [best, field] = [dist, i];
      });
    }

    // The month the calendar shows: the date's, or one the reader paged to.
    let shown = { y: date.y, m: date.m };
    /** The date the calendar was last drawn for. */
    let shownFor = null;
    const popover = calendar();
    document.body.appendChild(popover);
    let frame = 0;

    /** The label rewritten as `date`, as the one splice that does it. */
    const write = () => {
      const label = format(date);
      const open = content.indexOf("[");
      if (open < 0) {
        // `:date` with no label yet: one goes in after the name.
        const at = 1 + region.name.length;
        content = content.slice(0, at) + `[${label}]` + content.slice(at);
        return { edit: [{ from: bytes(content.slice(0, at)), to: bytes(content.slice(0, at)), text: `[${label}]` }] };
      }
      const close = content.indexOf("]", open);
      const from = bytes(content.slice(0, open + 1));
      const to = bytes(content.slice(0, close));
      content = content.slice(0, open + 1) + label + content.slice(close);
      return { edit: [{ from, to, text: label }] };
    };

    const step = (delta) => {
      if (field === 0) date.y = clamp(date.y + delta, 1, 9999);
      else if (field === 1) date.m = wrap(date.m + delta, 1, 12);
      else date.d = wrap(date.d + delta, 1, daysIn(date.y, date.m));
      date.d = Math.min(date.d, daysIn(date.y, date.m));
    };

    const move = (to) => {
      field = to;
      typed = "";
      return { moved: true };
    };

    const drawCalendar = () => {
      const { y, m } = shown;
      const first = (new Date(Date.UTC(y, m - 1, 1)).getUTCDay() + 6) % 7; // Monday first
      const head = document.createElement("div");
      head.className = "date-popover-head";
      const nav = (label, delta) => {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = label;
        b.setAttribute("aria-label", delta < 0 ? "previous month" : "next month");
        b.addEventListener("click", () => {
          const n = shown.y * 12 + (shown.m - 1) + delta;
          shown = { y: Math.floor(n / 12), m: (n % 12) + 1 };
          drawCalendar();
        });
        return b;
      };
      const title = document.createElement("span");
      title.textContent = `${MONTHS[m - 1]} ${y}`;
      head.append(nav("‹", -1), title, nav("›", 1));
      const grid = document.createElement("div");
      grid.className = "date-popover-grid";
      for (const w of ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]) {
        const c = document.createElement("span");
        c.className = "dow";
        c.textContent = w;
        grid.appendChild(c);
      }
      for (let i = 0; i < first; i++) grid.appendChild(document.createElement("span"));
      for (let d = 1; d <= daysIn(y, m); d++) {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = String(d);
        b.dataset.day = format({ y, m, d });
        if (y === date.y && m === date.m && d === date.d) b.className = "on";
        b.addEventListener("click", () => {
          date = { y, m, d };
          host.edit(write().edit);
          host.leave("after");
        });
        grid.appendChild(b);
      }
      popover.replaceChildren(head, grid);
    };

    /** Put the calendar under the caret, once leaf has laid the picture out. */
    const follow = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const at = host.caretRect() ?? host.regionRect();
        if (!at) {
          popover.hidden = true;
          return;
        }
        popover.hidden = false;
        popover.style.left = `${Math.round(at.left - 12)}px`;
        popover.style.top = `${Math.round(at.bottom + 6)}px`;
      });
    };

    return {
      picture(size, ink) {
        const laid = pill(partsOf(date), size, ink, false);
        const box = laid.boxes[field * 2];
        // The calendar follows the date, and stays on a month the reader
        // paged to until the date moves.
        if (shownFor !== format(date)) {
          shownFor = format(date);
          shown = { y: date.y, m: date.m };
          drawCalendar();
        }
        follow();
        return {
          ...laid.picture,
          caret: { x: box.x + box.w, y: laid.textTop, width: 0, height: laid.textHeight },
          selection: [{ x: box.x, y: laid.textTop, width: box.w, height: laid.textHeight }],
        };
      },
      key(name) {
        switch (name) {
          case "ArrowLeft":
            return field > 0 ? move(field - 1) : { leave: "before" };
          case "ArrowRight":
            return field < 2 ? move(field + 1) : { leave: "after" };
          case "ArrowUp":
          case "ArrowDown":
            typed = "";
            step(name === "ArrowUp" ? 1 : -1);
            return write();
          case "LineStart":
          case "Home":
            return move(0);
          case "LineEnd":
          case "End":
            return move(2);
          case "Backspace":
          case "Delete":
            typed = "";
            return { moved: true };
          case "Escape":
          case "Enter":
          case "Tab":
            return { leave: "after" };
          default:
            // A name this chip does not know — leaf closes it and runs the key.
            return null;
        }
      },
      type(text) {
        let wrote = false;
        for (const ch of text) {
          if (/[-/.]/.test(ch)) {
            if (field < 2) move(field + 1);
            continue;
          }
          if (!/[0-9]/.test(ch)) continue;
          typed += ch;
          const v = parseInt(typed, 10);
          if (field === 0) date.y = Math.max(1, v);
          else if (field === 1) date.m = clamp(v || 1, 1, 12);
          else date.d = clamp(v || 1, 1, daysIn(date.y, date.m));
          date.d = Math.min(date.d, daysIn(date.y, date.m));
          wrote = true;
          // A full field moves on: all its digits typed, or a first digit no
          // second one could follow (a month past 1, a day past 3).
          const full = typed.length >= WIDTHS[field] || (field === 1 && v > 1) || (field === 2 && v > 3);
          if (full) {
            if (field < 2) move(field + 1);
            else typed = "";
          }
        }
        return wrote ? write() : { moved: true };
      },
      click(x, y, size) {
        const { boxes } = pill(partsOf(date), size, "#000000", false);
        let best = Infinity;
        let to = field;
        [boxes[0], boxes[2], boxes[4]].forEach((b, i) => {
          const dist = Math.abs(x - (b.x + b.w / 2));
          if (dist < best) [best, to] = [dist, i];
        });
        return move(to);
      },
      copy: () => format(date),
      reload(next) {
        content = next;
        const open = next.indexOf("[");
        const close = next.indexOf("]", open);
        const parsed = open < 0 ? null : parseDate(next.slice(open + 1, close));
        if (parsed) date = parsed;
        return true;
      },
      accessibilityValue: () =>
        `Date, ${date.d} ${MONTHS[date.m - 1]} ${date.y}, ${FIELDS[field]} selected`,
      close() {
        cancelAnimationFrame(frame);
        popover.remove();
      },
    };
  },
};
