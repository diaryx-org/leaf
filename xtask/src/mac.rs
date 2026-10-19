//! `cargo xtask mac` — build `Leaf.app` from `apps/leaf-dioxus` and launch it.
//!
//! The Mac app is a cargo binary, so its bundle is put together here rather
//! than by Xcode: the binary, `apps/leaf-dioxus/Info.plist` with the app's
//! version stamped in, the icon compiled from the Icon Composer source the iOS
//! app uses (`actool`, which reads a `.icon` with no Xcode project around it),
//! and the bundled sample with its pictures, side by side in `Resources` where
//! an untitled document resolves them. The bundle is signed ad hoc, which is
//! what lets a Mac run it; `cargo xtask package` signs it again for others.
//!
//! `LEAF_BUNDLE_ID` names another bundle identifier, so a build under test
//! keeps its own preferences, recents and autosaves, apart from the copy of
//! Leaf the Mac has installed.

use crate::util::{cargo, cmd, require_tool, root, run, run_ignoring_failure};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const APP: &str = "Leaf";
const MANIFEST: &str = "apps/leaf-dioxus/Cargo.toml";

#[derive(clap::Args)]
pub struct Args {
    /// Build the release profile.
    #[arg(long)]
    pub release: bool,

    /// Build without launching.
    #[arg(long)]
    pub build_only: bool,

    /// A document to open once the app is running. Without one the app
    /// opens a copy of the bundled sample, from `target/`.
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,
}

pub fn run_task(args: Args) -> Result<()> {
    let app = bundle(args.release)?;
    if args.build_only {
        println!("✓ Built {}", app.display());
        return Ok(());
    }
    let file = match args.file {
        Some(f) => f,
        None => sample_copy()?,
    };
    launch(&app, &file)
}

/// The app's version: its manifest's, which `cargo xtask sync-versions`
/// keeps at the workspace's.
pub fn version() -> Result<String> {
    let manifest = crate::util::read(MANIFEST)?;
    manifest
        .lines()
        .find_map(|l| l.strip_prefix("version = \""))
        .map(|v| v.trim_end_matches('"').to_string())
        .context("no version in apps/leaf-dioxus/Cargo.toml")
}

/// Build the binary and put `Leaf.app` together around it, in the app's
/// `target/<profile>/`. Answers the bundle's path.
pub fn bundle(release: bool) -> Result<PathBuf> {
    require_tool("xcrun", "install Xcode and its command-line tools")?;
    let root = root();
    let mut build = cargo();
    build.args(["build", "--manifest-path", MANIFEST]);
    if release {
        build.arg("--release");
    }
    run(&mut build)?;

    let profile = if release { "release" } else { "debug" };
    let out = root.join("apps/leaf-dioxus/target").join(profile);
    let app = out.join(format!("{APP}.app"));
    if app.exists() {
        std::fs::remove_dir_all(&app)?;
    }
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    std::fs::copy(out.join(APP), macos.join(APP)).context("the built binary")?;
    let plist = contents.join("Info.plist");
    std::fs::copy(root.join("apps/leaf-dioxus/Info.plist"), &plist)?;
    let buddy = |command: String| -> Result<()> {
        run(cmd("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg(command)
            .arg(&plist))
    };
    buddy(format!("Set :CFBundleShortVersionString {}", version()?))?;
    if let Ok(id) = std::env::var("LEAF_BUNDLE_ID") {
        buddy(format!("Set :CFBundleIdentifier {id}"))?;
    }
    std::fs::write(contents.join("PkgInfo"), "APPL????")?;

    // The icon: Assets.car for the system's own rendering, leaf.icns for
    // older readers, both named by Info.plist.
    let sources = root.join("apps/leaf-editor/App");
    let partial = out.join("leaf-icon.plist");
    run(cmd("xcrun")
        .arg("actool")
        .arg(sources.join("leaf.icon"))
        .arg("--compile")
        .arg(&resources)
        .args([
            "--platform",
            "macosx",
            "--minimum-deployment-target",
            "14.0",
        ])
        .args(["--app-icon", "leaf", "--output-partial-info-plist"])
        .arg(&partial)
        .args(["--output-format", "human-readable-text"]))?;
    let _ = std::fs::remove_file(&partial);

    // The sample and its pictures, side by side, as an untitled document
    // finds them (`platform::resources`).
    std::fs::copy(sources.join("Sample.md"), resources.join("Sample.md"))?;
    for entry in std::fs::read_dir(sources.join("Media"))? {
        let entry = entry?;
        std::fs::copy(entry.path(), resources.join(entry.file_name()))?;
    }

    run(cmd("codesign").args(["--force", "--sign", "-"]).arg(&app))?;
    Ok(app)
}

/// The bundled sample and its media, copied to `target/leaf-sample/` so the
/// app opens it as a file — one the developer can edit and save.
pub fn sample_copy() -> Result<PathBuf> {
    let dir = root().join("target/leaf-sample");
    std::fs::create_dir_all(&dir)?;
    let app = root().join("apps/leaf-editor/App");
    std::fs::copy(app.join("Sample.md"), dir.join("Sample.md"))?;
    for entry in std::fs::read_dir(app.join("Media"))? {
        let entry = entry?;
        std::fs::copy(entry.path(), dir.join(entry.file_name()))?;
    }
    Ok(dir.join("Sample.md"))
}

fn launch(app: &Path, file: &Path) -> Result<()> {
    // `open` on a bundle that is already running only raises its window,
    // which would silently show the *previous* build.
    let binary = app.join("Contents/MacOS").join(APP);
    run_ignoring_failure(cmd("pkill").arg("-f").arg(&binary));
    let file = file
        .canonicalize()
        .with_context(|| format!("no such document: {}", file.display()))?;
    // Through the document system, as a double-click in the Finder would.
    run(cmd("open").arg("-a").arg(app).arg(&file))?;
    println!("✓ Running {APP}, with {}", file.display());
    Ok(())
}
