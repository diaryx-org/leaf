//! `cargo xtask swift` — build and launch the AppKit/UIKit app in
//! `apps/leaf-editor`, the host for the `packages/leaf-swift` editor.
//!
//! The chain behind that one word is four toolchains deep: cargo builds
//! `crates/leaf-ffi`, uniffi-bindgen turns it into Swift, xcodegen turns
//! `project.yml` into an Xcode project, and xcodebuild builds the app (its own
//! pre-build script rebuilding the Rust staticlib for whichever destination is
//! selected). The first two and the third are `apps/leaf-editor/bootstrap.sh`'s
//! job and stay there — this task decides *when* they need to run, then builds
//! and launches.

use crate::util::{cmd, require_tool, run, run_ignoring_failure};
use anyhow::{Context, Result, bail};
use std::path::Path;

/// Matches `PRODUCT_BUNDLE_IDENTIFIER` in `apps/leaf-editor/project.yml`; the
/// simulator addresses an installed app by id, not by path.
const BUNDLE_ID: &str = "dev.leaf.editor.ui";
const SCHEME: &str = "LeafEditorApp";

#[derive(clap::Args)]
pub struct Args {
    /// Run in the iOS Simulator instead of on macOS.
    #[arg(long)]
    ios: bool,

    /// The simulator device to run on (implies --ios).
    #[arg(long, value_name = "NAME")]
    device: Option<String>,

    /// Build the Release configuration.
    #[arg(long)]
    release: bool,

    /// Regenerate the UniFFI binding and the Xcode project first. Needed after
    /// the Rust *API* surface changes; ordinary Rust edits are picked up by the
    /// project's own pre-build script.
    #[arg(long)]
    regen: bool,

    /// Build without launching.
    #[arg(long)]
    build_only: bool,

    /// Let xcodebuild log at full volume.
    #[arg(long)]
    verbose: bool,
}

pub fn run_task(args: Args) -> Result<()> {
    require_tool("xcodebuild", "install Xcode and its command-line tools")?;
    require_tool("xcodegen", "brew install xcodegen")?;

    let root = crate::util::root();
    let app_dir = root.join("apps/leaf-editor");
    let project = app_dir.join(format!("{SCHEME}.xcodeproj"));
    let binding = root.join("packages/leaf-swift/uniffi-generated/Sources/LeafFFI/leaf_ffi.swift");

    // The project is git-ignored and regenerable (the binding is committed, but
    // guard its absence too — e.g. a checkout mid-rebase), so a fresh checkout
    // lands here on the first run rather than in an xcodebuild error about a
    // missing package.
    if args.regen || !project.exists() || !binding.exists() {
        run(cmd("bash").arg(app_dir.join("bootstrap.sh")))?;
    }

    let ios = args.ios || args.device.is_some();
    let device = args.device.as_deref().unwrap_or("iPhone 17");
    let config = if args.release { "Release" } else { "Debug" };

    // A macOS build and a simulator build write incompatible products under the
    // same names, so they get their own derived-data trees and neither
    // invalidates the other's incremental state.
    let derived = app_dir.join(if ios { "build/DD-iOS" } else { "build/DD" });
    let destination = if ios {
        format!("platform=iOS Simulator,name={device}")
    } else {
        "platform=macOS".to_string()
    };

    let mut build = cmd("xcodebuild");
    build
        .arg("-project")
        .arg(&project)
        .args(["-scheme", SCHEME])
        .args(["-configuration", config])
        .arg("-destination")
        .arg(&destination)
        .arg("-derivedDataPath")
        .arg(&derived)
        .arg("build");
    if !args.verbose {
        build.arg("-quiet");
    }
    run(&mut build)?;

    let product = derived
        .join("Build/Products")
        .join(if ios {
            format!("{config}-iphonesimulator")
        } else {
            config.to_string()
        })
        .join(format!("{SCHEME}.app"));
    if !product.is_dir() {
        bail!(
            "xcodebuild reported success but {} is missing",
            product.display()
        );
    }

    if args.build_only {
        println!("✓ Built {}", product.display());
        return Ok(());
    }

    if ios {
        launch_simulator(device, &product)
    } else {
        launch_macos(&product)
    }
}

fn launch_macos(product: &Path) -> Result<()> {
    // `open` on a bundle that is already running only raises its window, which
    // would silently show the *previous* build. Retiring the old instance first
    // makes "run" mean the thing that was just built.
    run_ignoring_failure(cmd("pkill").args(["-x", SCHEME]));
    run(cmd("open").arg(product))?;
    println!("✓ Running {SCHEME} on macOS");
    Ok(())
}

fn launch_simulator(device: &str, product: &Path) -> Result<()> {
    // Already-booted is the common case and reports as a failure; nothing else
    // here can succeed if the boot genuinely failed, so let install say so.
    run_ignoring_failure(cmd("xcrun").args(["simctl", "boot", device]));
    // Without the Simulator app in front, the booted device runs headless.
    run(cmd("open").args(["-a", "Simulator"]))?;
    run(cmd("xcrun")
        .args(["simctl", "install", device])
        .arg(product))
    .with_context(|| format!("could not install onto the `{device}` simulator"))?;
    run(cmd("xcrun").args([
        "simctl",
        "launch",
        "--terminate-running-process",
        device,
        BUNDLE_ID,
    ]))?;
    println!("✓ Running {SCHEME} on the `{device}` simulator");
    Ok(())
}
