//! `cargo xtask swift` — build and launch Leaf: on macOS the Rust app,
//! `apps/leaf-dioxus` (which is `cargo xtask mac`'s, kept under this name for
//! the hands that already type it); with `--ios`, `apps/leaf-editor`, over the
//! `packages/leaf-swift` editor.
//!
//! The iOS app is four toolchains deep: cargo builds `crates/leaf-ffi`,
//! uniffi-bindgen turns it into Swift — `apps/leaf-editor/bootstrap.sh`'s
//! job — then xcodegen turns `project.yml` into an Xcode project and
//! xcodebuild builds it. This task decides *when* the project needs
//! generating, then builds and launches.

use crate::util::{cmd, require_tool, run, run_ignoring_failure, stdout};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;

/// Matches `PRODUCT_BUNDLE_IDENTIFIER` in `apps/leaf-editor/project.yml`; the
/// simulator addresses an installed app by id, not by path.
const BUNDLE_ID: &str = "org.diaryx.leaf";
pub const SCHEME: &str = "Leaf";

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

    /// A document to open in the app once it is running (macOS only). Without
    /// one the app opens a copy of the bundled sample, from `target/`.
    #[arg(value_name = "FILE")]
    file: Option<std::path::PathBuf>,
}

/// The iOS app's directory.
pub fn app_dir() -> std::path::PathBuf {
    crate::util::root().join("apps/leaf-editor")
}

/// The Xcode project, generated first if it is missing, older than what it is
/// generated from, or `regen` asks for it.
pub fn project(regen: bool) -> Result<std::path::PathBuf> {
    require_tool("xcodebuild", "install Xcode and its command-line tools")?;
    require_tool("xcodegen", "brew install xcodegen")?;

    let root = crate::util::root();
    let app_dir = app_dir();
    let project = app_dir.join(format!("{SCHEME}.xcodeproj"));
    let binding = root.join("packages/leaf-swift/uniffi-generated/Sources/LeafFFI/leaf_ffi.swift");

    // The project is git-ignored and regenerable (the binding is committed, but
    // guard its absence too — e.g. a checkout mid-rebase), so a fresh checkout
    // lands here on the first run rather than in an xcodebuild error about a
    // missing package.
    if regen || !project.exists() || !binding.exists() || stale(&app_dir, &project) {
        run(cmd("bash").arg(app_dir.join("bootstrap.sh")))?;
    }
    Ok(project)
}

/// Whether the generated project is older than anything it is generated from:
/// `project.yml`, a file in `App/` (xcodegen lists each source by name, so a
/// file a merge or a checkout added is missing from a project generated before
/// it), or the app's own packages' manifests. A checkout writes what it
/// changes with the time it ran, so this catches every branch switch that
/// adds or removes a source.
fn stale(app_dir: &Path, project: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(generated) = modified(&project.join("project.pbxproj")) else {
        return true;
    };
    let mut inputs = vec![
        app_dir.join("project.yml"),
        app_dir.join("LeafMermaid/Package.swift"),
    ];
    if let Ok(entries) = std::fs::read_dir(app_dir.join("App")) {
        inputs.extend(entries.filter_map(|e| e.ok()).map(|e| e.path()));
    }
    // A directory's own time moves when a file in it is added or removed.
    inputs.push(app_dir.join("App"));
    inputs
        .iter()
        .filter_map(|p| modified(p))
        .any(|t| t > generated)
}

pub fn run_task(args: Args) -> Result<()> {
    let ios = args.ios || args.device.is_some();
    if !ios {
        return crate::mac::run_task(crate::mac::Args {
            release: args.release,
            build_only: args.build_only,
            file: args.file,
        });
    }
    let project = project(args.regen)?;
    let app_dir = app_dir();

    // A build alone needs no simulator: it is for a generic one, and makes
    // or boots none.
    let device = if !args.build_only {
        Some(simulator(args.device.as_deref())?)
    } else {
        None
    };
    let config = if args.release { "Release" } else { "Debug" };

    let derived = app_dir.join("build/DD-iOS");
    let destination = match &device {
        // By id rather than name: two runtimes can each hold an `iPhone 17`,
        // and a name then builds for whichever xcodebuild reaches first.
        Some(device) => format!("platform=iOS Simulator,id={}", device.udid),
        None => "generic/platform=iOS Simulator".to_string(),
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
    if device.is_none() {
        // A generic simulator is every architecture; the Rust staticlib is
        // Apple Silicon's alone.
        build.arg("ARCHS=arm64");
    }
    if !args.verbose {
        build.arg("-quiet");
    }
    run(&mut build)?;

    let product = derived
        .join("Build/Products")
        .join(format!("{config}-iphonesimulator"))
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
        None => Ok(()),
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
    // `--sample` reaches the app as a launch argument: the browser's Create
    // then makes the sample rather than an empty document.
    run(cmd("xcrun").args([
        "simctl",
        "launch",
        "--terminate-running-process",
        udid,
        BUNDLE_ID,
        "--sample",
    ]))?;
    println!("✓ Running {SCHEME} on the `{name}` simulator");
    Ok(())
}
