# wgpu-hal 30.0.1, patched

The crate anyrender_vello's wgpu 30 takes, vendored unmodified in the commit
before this file's and patched in this file's, so `git log -p -- .` is the
patch. Every change is marked `leaf patch` in the source, all of it in
`src/metal/`. The app takes it with `[patch.crates-io]` in
`apps/leaf-dioxus/Cargo.toml`, by path, under the exception Adam granted on
2026-10-19.

## What was wrong

`Surface::acquire_texture` on macOS returns `SurfaceError::Occluded` whenever
the window's `occlusionState` lacks `Visible`: the workaround for
gfx-rs/wgpu#8309 (wgpu PR #9141), where `-[CAMetalLayer nextDrawable]`
blocked for its one-second timeout after a covered window came back, because
the drawables presented while it was covered had not been released. Vello
skips a frame it gets no texture for, and says nothing.

So no frame reaches a window while it is covered, minimized or on another
Space. A document window shown already covered, which is how one opens or
is restored while another app is in front, starts with `occlusionState`
8192 (no `Visible` bit) and keeps it, and drew nothing at all: blank in
Mission Control, the window switcher, Stage Manager and any capture, until
it was uncovered. A window covered after it painted kept that frame,
whatever changed in it meanwhile.

## What the patch does

An occluded window still gets a drawable when one is free. The surface counts
the drawables it has handed out and not yet seen presented: each acquisition
adds one, and a presented handler on the drawable (macOS 10.15.4) takes it
off. `acquire_texture` refuses an occluded window's frame only when that count
has reached the layer's `maximumDrawableCount`, where `nextDrawable` would
wait. Before 10.15.4 it refuses every occluded frame, as upstream does. A
drawable whose handler never runs stays counted, which can only make an
occluded window refuse frames sooner.

The app asks for few such frames: it paints a window it believes covered only
when the page or its size changed, never for an animation such as the
caret's blink (`redraw` in `apps/leaf-dioxus/src/app.rs`).

Measured on macOS 27.0.1, a window opened under another app's: it painted its
page on opening, and every keystroke after it, 30 typed at 5 ms apart, at 3
to 17 ms a frame. Minimized, menu commands painted at 8 to 190 ms a frame. No
acquisition waited on `nextDrawable`.

## Upstream

Not sent. The change is wgpu's to take: counting drawables in flight instead
of refusing every occluded frame. Other projects have patched the same
refusal. When wgpu stops refusing frames that would not wait, and the wgpu
anyrender_vello takes has that change, this vendored copy and its `[patch]`
go.
