//! `cargo xtask android` — build and launch Leaf for Android, the Compose app
//! in `apps/leaf-android`, the host for the `packages/leaf-android` editor.
//!
//! Three toolchains deep, like `swift`: cargo-ndk cross-compiles
//! `crates/leaf-ffi` into a `libleaf_ffi.so` per ABI, uniffi-bindgen reads the
//! library back into Kotlin, and Gradle builds the app over both. The first two
//! write under `packages/leaf-android/leaf-ffi/build/rust/`, which that module's
//! build script reads and nothing tracks — unlike the Swift binding there is no
//! package manager consuming this one from a bare checkout, so it is generated
//! rather than committed.

use crate::util::{cargo, cmd, require_tool, run, stdout};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// `applicationId` in `apps/leaf-android/app/build.gradle.kts` — the same as the
/// Apple app's bundle id, since they are the same app.
const APPLICATION_ID: &str = "org.diaryx.leaf";
const ACTIVITY: &str = "org.diaryx.leaf.app.MainActivity";

/// The lowest API the library loads on: bionic has ELF thread-locals from 29,
/// and the Zig static library inside the .so (twig) uses them.
const MIN_API: &str = "29";

#[derive(clap::Args)]
pub struct Args {
    /// Build the release profile of the Rust library and the release variant
    /// of the app.
    #[arg(long)]
    release: bool,

    /// Build for every ABI Android still ships (arm64-v8a and x86_64) rather
    /// than arm64-v8a alone, which is a phone and an emulator on Apple silicon.
    #[arg(long)]
    all_abis: bool,

    /// Build the Rust library and its Kotlin binding, and stop — what Android
    /// Studio needs before its first sync.
    #[arg(long)]
    ffi_only: bool,

    /// Build without installing or launching.
    #[arg(long)]
    build_only: bool,

    /// The device to install on, by its `adb devices` serial. Without it, the
    /// one attached device — and an error naming them when there are several.
    #[arg(long, value_name = "SERIAL")]
    device: Option<String>,

    /// Extra arguments for the cargo build and the bindgen run, in the
    /// `LEAF_ANDROID_CARGO_ARGS` spelling — a `--config
    /// 'patch.crates-io.twig-sys.path="…"'` while a dependency's Android
    /// support is not yet released, say. Both commands take the same ones, or a
    /// patch the build wrote into the lockfile re-resolves differently for the
    /// host.
    #[arg(long, value_name = "ARGS", allow_hyphen_values = true)]
    cargo_args: Option<String>,
}

pub fn run_task(args: Args) -> Result<()> {
    require_tool(
        "cargo-ndk",
        "cargo install cargo-ndk, and rustup target add aarch64-linux-android x86_64-linux-android",
    )?;
    let sdk = android_sdk()?;
    let ndk = android_ndk(&sdk)?;

    let root = crate::util::root();
    let rust_out = root.join("packages/leaf-android/leaf-ffi/build/rust");
    let cargo_args = args
        .cargo_args
        .clone()
        .or_else(|| std::env::var("LEAF_ANDROID_CARGO_ARGS").ok())
        .map(|a| split_args(&a))
        .unwrap_or_default();

    build_library(&rust_out, &ndk, &args, &cargo_args)?;
    generate_binding(&rust_out, &cargo_args)?;
    if args.ffi_only {
        println!(
            "✓ Built libleaf_ffi.so and its Kotlin binding under {}",
            rust_out.display()
        );
        return Ok(());
    }

    let app_dir = root.join("apps/leaf-android");
    let variant = if args.release { "Release" } else { "Debug" };
    let mut gradle = cmd(app_dir.join("gradlew"));
    gradle
        .current_dir(&app_dir)
        .env("ANDROID_HOME", &sdk)
        .arg(format!(":app:assemble{variant}"));
    run(&mut gradle)?;

    // Both variants are signed (release with the debug key — see the app's
    // build script), so both install.
    let apk = app_dir.join(format!(
        "app/build/outputs/apk/{0}/app-{0}.apk",
        variant.to_lowercase()
    ));
    if !apk.is_file() {
        bail!("Gradle reported success but {} is missing", apk.display());
    }
    if args.build_only {
        println!("✓ Built {}", apk.display());
        return Ok(());
    }

    let adb = sdk.join("platform-tools/adb");
    let serial = device(&adb, args.device.as_deref())?;
    run(cmd(&adb).args(["-s", &serial, "install", "-r"]).arg(&apk))
        .with_context(|| format!("could not install onto `{serial}`"))?;
    // `-S` stops a running instance first, so "run" means the thing just built.
    run(cmd(&adb)
        .args(["-s", &serial, "shell", "am", "start", "-S", "-n"])
        .arg(format!("{APPLICATION_ID}/{ACTIVITY}")))?;
    println!("✓ Running Leaf on `{serial}`");
    Ok(())
}

/// `libleaf_ffi.so` for each ABI, where the `leaf-ffi` Gradle module reads it.
fn build_library(out: &Path, ndk: &Path, args: &Args, extra: &[String]) -> Result<()> {
    let jni = out.join("jniLibs");
    // A stale ABI directory from an `--all-abis` run would otherwise ship a
    // library older than the one beside it.
    let _ = std::fs::remove_dir_all(&jni);
    let mut build = cargo();
    build.arg("ndk").args(["-t", "arm64-v8a"]);
    if args.all_abis {
        build.args(["-t", "x86_64"]);
    }
    build
        .args(["-P", MIN_API, "-o"])
        .arg(&jni)
        .args(["build", "-p", "leaf-ffi"])
        .args(extra)
        .env("ANDROID_NDK_HOME", ndk);
    if args.release {
        build.arg("--release");
    }
    run(&mut build)?;
    // cargo-ndk copies every cdylib the build produced; the app loads one.
    for abi in std::fs::read_dir(&jni)? {
        for lib in std::fs::read_dir(abi?.path())? {
            let lib = lib?.path();
            if lib.file_name().is_some_and(|n| n != "libleaf_ffi.so") {
                std::fs::remove_file(lib)?;
            }
        }
    }
    Ok(())
}

/// The Kotlin binding, read out of the arm64 library (every ABI's metadata is
/// the same) in UniFFI's library mode.
fn generate_binding(out: &Path, extra: &[String]) -> Result<()> {
    let kotlin = out.join("kotlin");
    let _ = std::fs::remove_dir_all(&kotlin);
    run(cargo()
        .args(["run", "-q", "-p", "leaf-ffi"])
        .args(extra)
        .args(["--bin", "uniffi-bindgen", "--", "generate"])
        .arg(out.join("jniLibs/arm64-v8a/libleaf_ffi.so"))
        .args(["--language", "kotlin", "--no-format", "--out-dir"])
        .arg(&kotlin))?;
    if !kotlin.join("uniffi/leaf_ffi/leaf_ffi.kt").is_file() {
        bail!(
            "uniffi-bindgen wrote no uniffi/leaf_ffi/leaf_ffi.kt under {}",
            kotlin.display()
        );
    }
    Ok(())
}

/// The Android SDK: `ANDROID_HOME`, then the older `ANDROID_SDK_ROOT`, then the
/// two places an installer puts one on a Mac — Android Studio's, and Homebrew's
/// `android-commandlinetools`.
fn android_sdk() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .chain(home.map(|h| h.join("Library/Android/sdk")))
        .chain([
            PathBuf::from("/opt/homebrew/share/android-commandlinetools"),
            PathBuf::from("/usr/local/share/android-commandlinetools"),
        ]);
    for sdk in candidates {
        if sdk.join("platform-tools").is_dir() {
            return Ok(sdk);
        }
    }
    bail!(
        "no Android SDK found — set ANDROID_HOME, or install one \
         (brew install --cask android-commandlinetools, then sdkmanager \
         'platform-tools' 'platforms;android-37.0' 'ndk;29.0.14206865')"
    )
}

/// The NDK: `ANDROID_NDK_HOME`, else the newest one under the SDK.
fn android_ndk(sdk: &Path) -> Result<PathBuf> {
    if let Some(ndk) = std::env::var_os("ANDROID_NDK_HOME") {
        return Ok(PathBuf::from(ndk));
    }
    let mut versions: Vec<PathBuf> = std::fs::read_dir(sdk.join("ndk"))
        .map(|dir| dir.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    versions.sort_by_key(|p| version_key(p));
    versions.pop().with_context(|| {
        format!(
            "no NDK under {} — sdkmanager 'ndk;29.0.14206865', or set ANDROID_NDK_HOME",
            sdk.display()
        )
    })
}

/// `29.0.14206865` as numbers, so `29.0.9` sorts below `29.0.14206865`.
fn version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// The device to install on: the one named, which must be attached, or the one
/// attached device.
fn device(adb: &Path, requested: Option<&str>) -> Result<String> {
    let listing = stdout(cmd(adb).arg("devices"))?;
    let attached: Vec<String> = listing
        .lines()
        .skip(1)
        .filter_map(|line| {
            let (serial, state) = line.split_once('\t')?;
            (state.trim() == "device").then(|| serial.to_string())
        })
        .collect();
    match (requested, attached.as_slice()) {
        (Some(want), _) if attached.iter().any(|s| s == want) => Ok(want.to_string()),
        (Some(want), _) => bail!("`{want}` is not attached — `adb devices` lists: {attached:?}"),
        (None, [one]) => Ok(one.clone()),
        (None, []) => bail!(
            "no Android device attached — start an emulator (`emulator -list-avds` \
             names them) or plug one in, or pass --build-only"
        ),
        (None, many) => bail!("several devices are attached ({many:?}) — name one with --device"),
    }
}

/// Split `LEAF_ANDROID_CARGO_ARGS` the way a shell would for the one kind of
/// argument it carries: whitespace-separated, with single or double quotes
/// grouping (and removed), so `--config 'patch.crates-io.x.path="../x"'` is
/// two arguments.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_argument_splits_as_a_shell_would() {
        assert_eq!(
            split_args(r#"--config 'patch.crates-io.twig-sys.path="../twig/x"'"#),
            ["--config", r#"patch.crates-io.twig-sys.path="../twig/x""#]
        );
        assert!(split_args("   ").is_empty());
    }
}
