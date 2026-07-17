//! The UniFFI bindings generator, in-tree so its version tracks the `uniffi`
//! runtime this crate links (a mismatch between generator and runtime is the
//! classic UniFFI footgun). Invoked by `scripts/build-xcframework.sh`:
//!
//! ```sh
//! cargo run -p leaf-ffi --bin uniffi-bindgen -- \
//!   generate --library <libleaf_ffi.dylib> --language swift --out-dir <dir>
//! ```
fn main() {
    uniffi::uniffi_bindgen_main()
}
