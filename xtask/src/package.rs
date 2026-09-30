//! `cargo xtask package` — Leaf for Macs other than the one that built it: a
//! Developer ID–signed, notarised and stapled `Leaf.app` inside a signed,
//! notarised and stapled `.dmg`, written to `target/package/`.
//!
//! `project.yml` leaves the Mac build unsigned so that `cargo xtask swift`
//! works on a machine with no Apple team. Signing is switched on here, from
//! the command line, and nowhere else: xcodebuild's command-line settings
//! outrank the project's, SDK-conditional ones included.
//!
//! Two notarisations, not one. A ticket is looked up by the hash of what it
//! covers, so the app is notarised and stapled before it goes into the image —
//! a copy dragged out of the `.dmg` then opens offline — and the image, whose
//! hash stapling the app has just changed, is notarised in its own right.
//!
//! What this needs from the machine:
//!
//! - a `Developer ID Application` identity for the team in the keychain. The
//!   team is the project's own unless `DEVELOPMENT_TEAM` names another.
//! - notary credentials, either `NOTARY_KEYCHAIN_PROFILE` (a profile stored
//!   with `xcrun notarytool store-credentials`) or an App Store Connect API
//!   key as `APPLE_API_KEY_PATH`, `APPLE_API_KEY_ID` and `APPLE_API_ISSUER_ID`
//!   — the names diaryx's release workflow already gives its secrets.
//!   `--no-notarize` skips both notarisations and needs neither.
//!
//! `.github/workflows/mac-app.yml` runs this on a release tag, from secrets.
//!
//! Apple Silicon only, for now: the project's pre-build script builds the Rust
//! staticlib for `aarch64-apple-darwin` alone, and the file name says so.

use crate::swift::SCHEME;
use crate::util::{cmd, require_tool, root, run, stdout};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The team `DEVELOPMENT_TEAM` in `apps/leaf-editor/project.yml` names.
const TEAM: &str = "V4322HH5HU";

#[derive(clap::Args)]
pub struct Args {
    /// Sign, but do not notarise: an image for checking the build, which
    /// Gatekeeper on another Mac will still refuse.
    #[arg(long)]
    no_notarize: bool,

    /// Let xcodebuild log at full volume.
    #[arg(long)]
    verbose: bool,
}

pub fn run_task(args: Args) -> Result<()> {
    let team = std::env::var("DEVELOPMENT_TEAM").unwrap_or_else(|_| TEAM.to_string());
    let identity = identity(&team)?;
    // Asked before a build that takes minutes, not after it.
    let notary = if args.no_notarize {
        None
    } else {
        require_tool("xcrun", "install Xcode and its command-line tools")?;
        Some(Notary::from_env()?)
    };

    // Always regenerated: the project is git-ignored, and one made before a
    // source file was added builds without it — or, from an older
    // `project.yml`, for the wrong deployment target — and says nothing.
    let project = crate::swift::project(true)?;
    let root = root();
    // Its own derived data: a signed and an unsigned build of the same
    // configuration would otherwise take turns invalidating each other.
    let derived = root.join("apps/leaf-editor/build/DD-package");

    let mut build = cmd("xcodebuild");
    build
        .arg("-project")
        .arg(&project)
        .args(["-scheme", SCHEME, "-configuration", "Release"])
        .args(["-destination", "platform=macOS"])
        .arg("-derivedDataPath")
        .arg(&derived)
        .arg("clean")
        .arg("build")
        .args([
            "CODE_SIGNING_ALLOWED=YES",
            "CODE_SIGNING_REQUIRED=YES",
            "CODE_SIGN_STYLE=Manual",
            "PROVISIONING_PROFILE_SPECIFIER=",
            // Notarisation refuses code without the hardened runtime, and
            // refuses `get-task-allow`, which Xcode adds for debugging.
            "ENABLE_HARDENED_RUNTIME=YES",
            "CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO",
            "OTHER_CODE_SIGN_FLAGS=--timestamp",
        ])
        .arg(format!("CODE_SIGN_IDENTITY={}", identity.hash))
        .arg(format!("DEVELOPMENT_TEAM={team}"));
    if !args.verbose {
        build.arg("-quiet");
    }
    run(&mut build)?;

    let built = derived.join(format!("Build/Products/Release/{SCHEME}.app"));
    if !built.is_dir() {
        bail!(
            "xcodebuild reported success but {} is missing",
            built.display()
        );
    }

    let out = root.join("target/package");
    if out.exists() {
        std::fs::remove_dir_all(&out)?;
    }
    std::fs::create_dir_all(&out)?;
    let app = out.join(format!("{SCHEME}.app"));
    // `ditto`, not a plain copy: it keeps the bundle's symlinks, extended
    // attributes and signature intact.
    run(cmd("ditto").arg(&built).arg(&app))?;
    verify_signature(&app)?;

    if let Some(notary) = &notary {
        let zip = out.join(format!("{SCHEME}.zip"));
        run(cmd("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&app)
            .arg(&zip))?;
        notary.submit(&zip)?;
        std::fs::remove_file(&zip)?;
        run(cmd("xcrun").args(["stapler", "staple"]).arg(&app))?;
        run(cmd("spctl")
            .args(["--assess", "--type", "execute", "-vv"])
            .arg(&app))?;
    }

    let version = stdout(
        cmd("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(app.join("Contents/Info.plist")),
    )?;
    let dmg = out.join(format!("{SCHEME}-{}-aarch64.dmg", version.trim()));
    image(&app, &dmg)?;
    run(cmd("codesign")
        .args(["--sign", &identity.hash, "--timestamp"])
        .arg(&dmg))?;

    match &notary {
        Some(notary) => {
            notary.submit(&dmg)?;
            run(cmd("xcrun").args(["stapler", "staple"]).arg(&dmg))?;
            run(cmd("spctl")
                .args(["--assess", "--type", "open"])
                .args(["--context", "context:primary-signature", "-vv"])
                .arg(&dmg))?;
            println!("✓ Signed, notarised and stapled {}", dmg.display());
        }
        None => println!(
            "✓ Signed {} (not notarised: another Mac will refuse it)",
            dmg.display()
        ),
    }
    Ok(())
}

/// A signing identity as the keychain lists it. The SHA-1 is what gets passed
/// on, not the name: a certificate imported into more than one keychain lists
/// once per keychain, and codesign calls a name that matches twice ambiguous.
struct Identity {
    hash: String,
}

fn identity(team: &str) -> Result<Identity> {
    let listing = stdout(cmd("security").args(["find-identity", "-v", "-p", "codesigning"]))?;
    // `  1) <SHA-1> "Developer ID Application: <name> (<team>)"`
    let wanted = format!("({team})\"");
    let hash = listing.lines().find_map(|line| {
        let (_, rest) = line.trim().split_once(") ")?;
        let (hash, name) = rest.split_once(' ')?;
        (name.starts_with("\"Developer ID Application:") && name.ends_with(&wanted))
            .then(|| hash.to_string())
    });
    match hash {
        Some(hash) => Ok(Identity { hash }),
        None => bail!(
            "no `Developer ID Application` identity for team {team} in the keychain — \
             create one at developer.apple.com (Certificates, Identifiers & Profiles) \
             or set DEVELOPMENT_TEAM to the team that has one"
        ),
    }
}

/// A strict check of the signature, and of the one flag notarisation will
/// otherwise reject after a round trip of minutes: the hardened runtime.
fn verify_signature(app: &Path) -> Result<()> {
    run(cmd("codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(app))?;
    // `codesign --display` writes its report to stderr.
    let report = cmd("codesign")
        .args(["--display", "--verbose=2"])
        .arg(app)
        .output()
        .context("could not spawn `codesign`")?;
    let report = String::from_utf8_lossy(&report.stderr);
    if !report
        .lines()
        .any(|l| l.contains("flags=") && l.contains("runtime"))
    {
        bail!(
            "{} is signed without the hardened runtime, which notarisation refuses:\n{report}",
            app.display()
        );
    }
    Ok(())
}

/// The disk image: the app and a link to `/Applications` to drag it onto.
fn image(app: &Path, dmg: &Path) -> Result<()> {
    let staging = dmg.with_extension("staging");
    std::fs::create_dir_all(&staging)?;
    run(cmd("ditto")
        .arg(app)
        .arg(staging.join(app.file_name().unwrap())))?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    run(cmd("hdiutil")
        .args([
            "create", "-volname", SCHEME, "-fs", "HFS+", "-format", "UDZO", "-ov",
        ])
        .arg("-srcfolder")
        .arg(&staging)
        .arg(dmg))?;
    std::fs::remove_dir_all(&staging)?;
    Ok(())
}

/// How `notarytool` is to authenticate.
enum Notary {
    Profile(String),
    ApiKey {
        path: PathBuf,
        id: String,
        issuer: String,
    },
}

impl Notary {
    fn from_env() -> Result<Self> {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        if let Some(profile) = var("NOTARY_KEYCHAIN_PROFILE") {
            return Ok(Notary::Profile(profile));
        }
        match (
            var("APPLE_API_KEY_PATH"),
            var("APPLE_API_KEY_ID"),
            var("APPLE_API_ISSUER_ID"),
        ) {
            (Some(path), Some(id), Some(issuer)) => Ok(Notary::ApiKey {
                path: path.into(),
                id,
                issuer,
            }),
            _ => bail!(
                "no notary credentials: set NOTARY_KEYCHAIN_PROFILE to a profile made with \
                 `xcrun notarytool store-credentials`, or APPLE_API_KEY_PATH, \
                 APPLE_API_KEY_ID and APPLE_API_ISSUER_ID to an App Store Connect API key \
                 — or pass --no-notarize for a signed image only"
            ),
        }
    }

    fn auth(&self, command: &mut std::process::Command) {
        match self {
            Notary::Profile(profile) => {
                command.args(["--keychain-profile", profile]);
            }
            Notary::ApiKey { path, id, issuer } => {
                command
                    .arg("--key")
                    .arg(path)
                    .args(["--key-id", id, "--issuer", issuer]);
            }
        }
    }

    /// Upload `file` and wait for Apple's verdict. `--wait` returns once the
    /// submission settles whatever the verdict, so the status is read from the
    /// output, and a rejection fetches the log that says why.
    fn submit(&self, file: &Path) -> Result<()> {
        let mut submit = cmd("xcrun");
        submit.args(["notarytool", "submit"]).arg(file);
        self.auth(&mut submit);
        submit.args(["--wait", "--output-format", "json"]);
        let json = stdout(&mut submit)?;
        if json_field(&json, "status").as_deref() == Some("Accepted") {
            println!("✓ Notarised {}", file.display());
            return Ok(());
        }
        if let Some(id) = json_field(&json, "id") {
            let mut log = cmd("xcrun");
            log.args(["notarytool", "log", &id]);
            self.auth(&mut log);
            if let Ok(log) = stdout(&mut log) {
                eprintln!("{log}");
            }
        }
        bail!(
            "notarisation of {} was not accepted: {json}",
            file.display()
        )
    }
}

/// A top-level string field of notarytool's one-line JSON. Enough for the
/// three keys it prints — `id`, `message`, `status` — without a JSON crate
/// for a task runner that otherwise has no use for one.
fn json_field(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[start..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

#[cfg(test)]
mod tests {
    use super::json_field;

    #[test]
    fn reads_notarytool_output() {
        let json = r#"{"message":"Processing complete","id":"2efe2717-52ef-43a5-96dc-0797e4ca1041","status":"Invalid"}"#;
        assert_eq!(json_field(json, "status").as_deref(), Some("Invalid"));
        assert_eq!(
            json_field(json, "id").as_deref(),
            Some("2efe2717-52ef-43a5-96dc-0797e4ca1041")
        );
        assert_eq!(json_field(json, "missing"), None);
    }
}
