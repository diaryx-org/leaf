//! Time, in a window or in a browser tab.
//!
//! On `wasm32-unknown-unknown`, `std::time::Instant::now` panics — there is no
//! clock under std there — and tokio's timers need tokio's runtime, which
//! dioxus-native starts only off the web. So the editor reads the time and
//! waits through here: `web_time`'s `Instant`, which is std's own off the web
//! and `performance.now()` on it, and a sleep that is tokio's off the web and
//! the page's `setTimeout` on it.

use std::time::Duration;

pub use web_time::Instant;

/// Wait `d`, without holding up the thread.
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

/// Wait `d`, without holding up the thread.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(d: Duration) {
    let ms = d.as_millis().min(i32::MAX as u128) as i32;
    let wake = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(wake).await;
}
