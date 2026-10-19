# winit-uikit 0.31.0-beta.3, patched

The crate dioxus-native pins, vendored unmodified in the commit before this
file's and patched after it, so `git log -p -- .` is the patch.

Consumers take it with `[patch.crates-io]` in their root manifest, as they
take `../winit-appkit`: inside leaf by path, anything outside leaf by git,
`{ git = "https://github.com/diaryx-org/leaf", branch = "main" }`.

## What was wrong

An app built with the iOS 27 SDK must adopt the UIScene lifecycle, declaring
a scene configuration in its `Info.plist`; iOS 27 refuses to launch one that
does not. winit-uikit 0.31.0-beta.3 knows nothing of scenes: it makes its
window from the application's lifecycle, which a scene app no longer has.

## What the patch does

rust-windowing/winit#4723, "ios: support the UIScene lifecycle", applied
verbatim to `src/` (`app_state.rs`, `event_loop.rs`, `lib.rs`, `window.rs`),
and the two UIKit features it needs added to `Cargo.toml` (`UIScene`,
`UIWindowScene`) by hand, as the PR's manifest hunk does not apply to the
published crate. No `leaf patch` marks: the diff is the PR's.

The app declares the scene configuration (`lib.rs`'s "Scenes" says how); no
scene delegate is needed. `spikes/blitz-ios-a11y/ios/Info.plist` has one.

## When it goes

When #4723 is in a winit release that dioxus-native takes. Until then the
proposal "A shared, patched winit for the Dioxus work" says to support the PR
upstream rather than open another.
