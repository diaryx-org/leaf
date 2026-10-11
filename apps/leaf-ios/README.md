# leaf-ios — archived

The gpui iOS host, archived on 2026-10-10. It still names the Zed commit
`bd72919` and Adam's gpui-mobile fork, while leaf-gpui has moved to gpui-pre
0.3.8, so it no longer builds against the widget beside it. gpui on iOS is not
mature enough yet and has little upstream interest. leaf on iOS is
[`leaf-swift`](../../packages/leaf-swift). The gpui desktop app,
[`apps/leaf`](../leaf), is kept for Linux and Windows.

## If it is revived

What a port to gpui-pre found on 2026-10-09:

- **The platform layer.** Use Longbridge's `gpui-pre-mobile`. The crates.io
  0.1.0 pins gpui-pre `=0.3.5`, and the gpui-pre crates pin each other with
  `=`, so it must come from git `main` or a later release to match leaf-gpui's
  version.
- **The link.** Any iOS link fails with `ld: framework 'OpenGL' not found`.
  The chain is gpui → core-video 0.5 → io-surface 0.16 → cgl, which links
  OpenGL.framework, and iOS doesn't have it.
  - A `staticlib` defers the error to Xcode.
  - A `cdylib` hits it in cargo. That includes gpui-pre-mobile's own
    `crate-type`, since cargo builds every crate-type of a dependency.
  - The real fix is core-video 0.6.1, which has no cgl. It needs version
    bumps in gpui, gpui_macos and gpui_apple, plus two call sites in
    `gpui_apple/src/metal_renderer.rs`: pass
    `&surface.image_buffer.as_image_buffer()`, with `TCVImageBuffer` imported,
    where it passed `as_concrete_TypeRef()`.
  - With that bump, macOS still renders and passes its tests.
- **APIs only the fork had.** `gpui_mobile::ios::ffi::with_app` can be replaced
  by a host-held `AsyncApp`, taken with `cx.to_async()` once the app runs.
  `set_keyboard_height_callback` can be replaced by a `cx.spawn` loop that
  polls `gpui_mobile::keyboard_height()`.
