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

use crate::util::{cmd, require_tool, run, run_ignoring_failure, stdout};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;

/// Matches `PRODUCT_BUNDLE_IDENTIFIER` in `apps/leaf-editor/project.yml`; the
/// simulator addresses an installed app by id, not by path.
const BUNDLE_ID: &str = "dev.leaf.editor.ui";
const SCHEME: &str = "LeafEditorApp";

/// The simulator `--ios` runs on when `--device` names none: leaf's own,
/// created from [`DEVICE_TYPE`] on first use. A stock `iPhone 17` is shared
/// with every other project on the Mac, and installing onto a shared device
/// brings whatever was last installed there to the front — an editor under
/// test disappears behind another app mid-session.
const DEVICE: &str = "iPhone 17 (leaf)";
/// A name from `xcrun simctl list devicetypes`; the runtime is the newest
/// installed, which is what `simctl create` picks when none is given.
const DEVICE_TYPE: &str = "iPhone 17";

/// Where the Simulator's front end lives, by bundle id: `Simulator.app` up to
/// Xcode 26, `DeviceHub.app` from Xcode 27, which folded it in. `open -a
/// Simulator` on the latter fails with "Unable to find application".
const SIMULATOR_APPS: [&str; 2] = ["com.apple.iphonesimulator", "com.apple.dt.Devices"];

#[derive(clap::Args)]
pub struct Args {
    /// Run in the iOS Simulator instead of on macOS.
    #[arg(long)]
    ios: bool,

    /// An existing simulator to run on, by name (implies --ios). Without it
    /// the app runs on `iPhone 17 (leaf)`, created if need be.
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
    let device = if ios {
        Some(simulator(args.device.as_deref())?)
    } else {
        None
    };
    let config = if args.release { "Release" } else { "Debug" };

    // A macOS build and a simulator build write incompatible products under the
    // same names, so they get their own derived-data trees and neither
    // invalidates the other's incremental state.
    let derived = app_dir.join(if ios { "build/DD-iOS" } else { "build/DD" });
    let destination = match &device {
        // By id rather than name: two runtimes can each hold an `iPhone 17`,
        // and a name then builds for whichever xcodebuild reaches first.
        Some(device) => format!("platform=iOS Simulator,id={}", device.udid),
        None => "platform=macOS".to_string(),
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

    match device {
        Some(device) => launch_simulator(&device, &product),
        None => launch_macos(&product),
    }
}

/// A simulator as `simctl` knows it. Every `simctl` verb and xcodebuild's
/// destination take the udid, which unlike the name is unique.
struct Simulator {
    name: String,
    udid: String,
}

/// The simulator to run on: the one named by `--device`, which must exist, or
/// [`DEVICE`], which is made if it doesn't.
fn simulator(requested: Option<&str>) -> Result<Simulator> {
    let name = requested.unwrap_or(DEVICE);
    if let Some(udid) = find_simulator(name)? {
        return Ok(Simulator {
            name: name.to_string(),
            udid,
        });
    }
    if requested.is_some() {
        bail!(
            "no simulator named `{name}` — `xcrun simctl list devices available` \
             has the ones there are"
        );
    }
    let udid = stdout(cmd("xcrun").args(["simctl", "create", name, DEVICE_TYPE]))
        .with_context(|| format!("could not create the `{name}` simulator"))?;
    println!("✓ Created the `{name}` simulator from `{DEVICE_TYPE}`");
    Ok(Simulator {
        name: name.to_string(),
        udid: udid.trim().to_string(),
    })
}

/// The udid of the available simulator called `name`, if there is one.
fn find_simulator(name: &str) -> Result<Option<String>> {
    let listing = stdout(cmd("xcrun").args(["simctl", "list", "devices", "available"]))?;
    Ok(listing.lines().find_map(|line| {
        // `    <name> (<udid>) (<state>)`, and the name may hold parentheses of
        // its own — `iPad mini (A17 Pro)` — so it is read from the right.
        let (rest, _state) = line.trim().rsplit_once(" (")?;
        let (candidate, udid) = rest.rsplit_once(" (")?;
        (candidate == name).then(|| udid.trim_end_matches(')').to_string())
    }))
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

fn launch_simulator(device: &Simulator, product: &Path) -> Result<()> {
    let Simulator { name, udid } = device;
    // Already-booted is the common case and reports as a failure; nothing else
    // here can succeed if the boot genuinely failed, so let install say so.
    run_ignoring_failure(cmd("xcrun").args(["simctl", "boot", udid]));
    // Without the Simulator's front end open, the booted device runs headless.
    // Whichever of the two this Xcode ships is the one that opens; the other's
    // "Unable to find application" is expected and kept quiet.
    let opened = SIMULATOR_APPS.iter().any(|id| {
        let mut open = cmd("open");
        open.args(["-b", id]).stderr(Stdio::null());
        println!("▸ open -b {id}");
        open.status().is_ok_and(|s| s.success())
    });
    if !opened {
        bail!(
            "could not open the Simulator: no app with the bundle id {} or {}",
            SIMULATOR_APPS[0],
            SIMULATOR_APPS[1]
        );
    }
    run(cmd("xcrun").args(["simctl", "install", udid]).arg(product))
        .with_context(|| format!("could not install onto the `{name}` simulator"))?;
    run(cmd("xcrun").args([
        "simctl",
        "launch",
        "--terminate-running-process",
        udid,
        BUNDLE_ID,
    ]))?;
    println!("✓ Running {SCHEME} on the `{name}` simulator");
    Ok(())
}
