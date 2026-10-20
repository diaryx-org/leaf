//! On a Mac, `Info.plist` embedded in the binary's `__TEXT,__info_plist`
//! section: AppKit reads an unbundled executable's there, so `cargo run`
//! knows the document types and their class as `Leaf.app` does.
//!
//! `LEAF_BUNDLE_ID` replaces the bundle identifier, so a build under test
//! keeps its own preferences, recents and autosaves, apart from the
//! installed app's.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=Info.plist");
    println!("cargo:rerun-if-env-changed=LEAF_BUNDLE_ID");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let mut plist = std::fs::read_to_string("Info.plist").expect("Info.plist");
    if let Ok(id) = std::env::var("LEAF_BUNDLE_ID") {
        plist = plist.replace(
            "<string>org.diaryx.leaf</string>",
            &format!("<string>{id}</string>"),
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("Info.plist");
    std::fs::write(&out, plist).expect("the embedded Info.plist");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        out.display()
    );
}
