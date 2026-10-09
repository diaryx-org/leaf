# leaf-math-edit (spike)

A structural math editor whose file format stays TeX: you type `1/2`, `x^2`,
`sqrt`, and the formula in the document becomes `\frac{1}{2}`, `x^{2}`,
`\sqrt{}`. You never write the TeX, and the TeX already there is never
rewritten.

## How it works

- **`tree`** reads a formula into rows and atoms, each with the byte span it
  came from: fractions, roots, scripts, `\left…\right`, one-argument wrappers
  such as `\hat`. Anything else is an opaque atom, kept whole: `\text{}`, font
  commands, environments, and groups that use `\choose`.
- **`edit`** turns each keystroke into a splice on the TeX, then rereads the
  formula. It never prints the tree, so bytes an edit does not name stay as
  written (`hand_written_tex_around_an_edit_survives`). It works like twig's
  `edit_range`, one level down.
- **`layout`** gets caret geometry out of RaTeX without patching it. It typesets
  the formula once with every atom wrapped in `\htmlStyle{color: #id}`, then
  reads the colours back off the display list as atom ids. RaTeX classifies
  `\htmlStyle` by what it wraps, so spacing and binary-operator cancellation
  are unchanged. `the_probe_does_not_move_anything` checks this on 28 formulas
  in both styles. The probe's own display list is what gets drawn, so what you
  see is exactly what was measured.
- **`doc`** finds `inline_math` and `display_math` through twig-doc and applies
  each splice with `Editor::edit_range` at the formula's offset in the
  document.

## Try it

```sh
cd crates/leaf-math-edit
wasm-pack build --dev --target web --out-dir <scratch>/pkg --no-typescript -- --features wasm
cp demo/index.html <scratch>/ && python3 -m http.server -d <scratch>
```

## What the spike found

- **Geometry without RaTeX changes holds up**, apart from three classes of
  construct the probe cannot wrap. Infix commands (`\over`, `\choose`) split
  their group. Font commands (`\mathbb`) don't reach a wrapped argument.
  Style switches (`\displaystyle`) would end at the wrapper. The first two are
  opaque here and switches are written bare; each was found by the probe test,
  not guessed.
- **TeX's own rules catch some keystrokes.** For example, `^` before a prime is
  a double superscript. The fuzz test (random typing, 200 rounds) found this
  one. Such a keystroke is now refused rather than written. A real editor
  would handle each case, such as stepping into the prime's superscript.
- **User colours are lost in the editing view**, since every atom is
  recoloured. The fix is to keep atom ids in a side channel, such as the low
  bits of alpha, rather than in the whole colour.
- **Edit-time placeholders (`\square`) change the layout** of a formula with an
  empty slot, which is intended. That is also why the fuzz test only compares
  probe and plain layouts when no slot is empty.

## In leaf

Two frontends use it, the web editor (`packages/leaf-web`) and the Swift views
(`packages/leaf-swift`). Each README has a section on formulas.

- `MathSession` is exported twice, over the editor here without its twig half:
  by `leaf-math-wasm` for the web, and by `leaf-ffi` for Swift. CI holds the two
  to the same methods.
- Each frontend applies the session's splices to the document through
  `LeafDoc::edit_source`.
- Core keeps a formula a picture on the caret's line once the frontend says it
  edits formulas itself (`Doc::set_edits_math`). Every `MathView` carries the
  formula's end and its TeX range.
- The Swift views draw the open formula without the layout knowing. The frame
  they lay out names it by a stand-in TeX for each revision, which `MathStore`
  answers with the session's picture.

The editor adds about 60 KB raw to the web's math module, measured against that
README's September figure.

## Not done

- Selection, copy/paste and delete-forward.
- Matrices and environments as grids.
- `\left(` auto-pairing.
- Removing an empty script when the caret leaves it.
- IME input.
- The terminal frontend.

The open formula is laid out again on every key; caching by source would be the
first optimisation.
