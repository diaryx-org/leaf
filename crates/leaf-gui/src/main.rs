//! The `leaf-gui` binary — the standalone application. All the editor and app
//! logic lives in the reusable `leaf-gpui` crate; this is just the entry point,
//! so the same code powers both the shipped app and an embedded editor widget.

fn main() {
    leaf_gpui::run();
}
