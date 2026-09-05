//! `cargo xtask sync-versions` — write the workspace version into the files no
//! manifest parser will ever find, and the CI check that they agree.
//!
//! The one such file today is `packages/leaf-web/package.json`. The release
//! tooling moves every Cargo manifest and the lockfile, and knows nothing about
//! an npm manifest — so the package sat at 0.1.2 through three releases of the
//! crates it wraps. `.config/release.toml` names this task as the bump's
//! `post_bump` and lists the file among `extra_version_files`, so a release
//! commit carries it.

use crate::util::{read, root};
use anyhow::{Context, Result, bail};

/// The files that follow the workspace version without a manifest parser
/// reaching them, as (path, the line prefix that finds the version).
const FOLLOWERS: &[(&str, &str)] = &[("packages/leaf-web/package.json", "  \"version\": \"")];

/// The version `[workspace.package]` states.
fn workspace_version() -> Result<String> {
    let manifest = read("Cargo.toml")?;
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
            continue;
        }
        if in_package && let Some(rest) = line.strip_prefix("version") {
            let value = rest.trim_start().trim_start_matches('=').trim();
            return Ok(value.trim_matches('"').to_string());
        }
    }
    bail!("no `version` under [workspace.package] in Cargo.toml")
}

/// The version a follower states, and the line it states it on.
fn follower_version(path: &str, prefix: &str) -> Result<(usize, String)> {
    let text = read(path)?;
    for (i, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix(prefix) {
            let version = rest.split('"').next().unwrap_or_default().to_string();
            return Ok((i, version));
        }
    }
    bail!("no version line in {path} (looked for a line starting {prefix:?})")
}

/// Write the workspace version into every follower. Idempotent.
pub fn run_task() -> Result<()> {
    let version = workspace_version()?;
    for (path, prefix) in FOLLOWERS {
        let (line_no, current) = follower_version(path, prefix)?;
        if current == version {
            println!("  {path} already at {version}");
            continue;
        }
        let full = root().join(path);
        let text = std::fs::read_to_string(&full)?;
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let rest = lines[line_no][prefix.len()..].to_string();
        let tail = &rest[current.len()..];
        lines[line_no] = format!("{prefix}{version}{tail}");
        let mut out = lines.join("\n");
        if text.ends_with('\n') {
            out.push('\n');
        }
        std::fs::write(&full, out).with_context(|| format!("could not write {path}"))?;
        println!("  {path}: {current} → {version}");
    }
    Ok(())
}

/// The CI job: every follower states the workspace version.
pub fn check() -> Result<()> {
    let version = workspace_version()?;
    for (path, prefix) in FOLLOWERS {
        let (_, current) = follower_version(path, prefix)?;
        if current != version {
            bail!(
                "{path} says {current} but the workspace is at {version} — run `cargo xtask sync-versions`"
            );
        }
        println!("  {path} at {version}");
    }
    Ok(())
}
