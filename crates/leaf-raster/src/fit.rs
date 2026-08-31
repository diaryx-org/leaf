//! Aspect-preserving containment, in pixels.

/// The pixel box an image fits into: as wide as `max_w` allows but never
/// upscaled past the source's own pixels, and as tall as that width makes it,
/// capped at `max_h` (shrinking the width back to keep the aspect ratio).
///
/// In pixels on purpose: the terminal's non-square cells and a GUI's points are
/// both a rounding step *after* this policy, so the frontends can share the
/// sizing rule and differ only in the rounding.
pub fn fit_within(intrinsic: (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    let (iw, ih) = (intrinsic.0.max(1) as u64, intrinsic.1.max(1) as u64);

    // Fit the width, never upscaling; the height follows from the aspect ratio.
    let mut w = iw.min(max_w.max(1) as u64);
    let mut h = w * ih / iw;
    let max_h = max_h.max(1) as u64;
    if h > max_h {
        h = max_h;
        w = h * iw / ih;
    }
    (w.max(1) as u32, h.max(1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_image_is_never_upscaled() {
        assert_eq!(fit_within((200, 100), 400, 600), (200, 100));
    }

    #[test]
    fn a_wide_image_scales_down_to_the_width() {
        assert_eq!(fit_within((800, 400), 400, 600), (400, 200));
    }

    #[test]
    fn the_height_cap_shrinks_the_width_with_it() {
        let (w, h) = fit_within((100, 4000), 400, 160);
        assert_eq!(h, 160, "height is held to the cap");
        assert_eq!(w, 4, "width shrinks to keep the aspect ratio");
    }

    #[test]
    fn degenerate_inputs_still_yield_a_paintable_box() {
        assert_eq!(fit_within((0, 0), 0, 0), (1, 1));
        // An extreme aspect ratio can't round a side down to nothing.
        let (w, h) = fit_within((10_000, 1), 100, 100);
        assert!(w >= 1 && h >= 1);
    }
}
