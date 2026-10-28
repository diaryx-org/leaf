//! `cargo xtask package-isolation` — check each crate alone, in each feature
//! shape a host builds. CI runs it as the `package-isolation` job in
//! `.config/ship.figl`.
//!
//! The tests at the bottom are workspace checks that `cargo test --workspace`
//! runs: that every member is in [`ISOLATED`].

use crate::util::{cargo, run};
#[cfg(test)]
use crate::util::{read, root};
#[cfg(test)]
use anyhow::Context;
use anyhow::Result;
#[cfg(test)]
use anyhow::bail;

/// Workspace feature unification means `cargo check --workspace` can pass even
/// when a crate cannot compile on its own — some other member's feature
/// selection quietly fills the gap. Each row below checks one crate the way some
/// host actually builds it.
///
/// The `--no-default-features` rows are not thoroughness for its own sake:
/// `leaf-core` without `fs` is what a host with no files to read links, and
/// what the Dioxus widget takes, turning on only the features it uses.
///
/// The Dioxus widget's second shape is a host's that brings its own
/// clipboard: without `system-clipboard`, its renderer, `vello`, turned back
/// on, since dioxus-native needs one. Its shape in a browser, and the web
/// app's, are for `wasm32-unknown-unknown`: `.config/ship.figl`'s `web` job.
///
/// A new workspace member belongs in this list; the test below is what says so.
const ISOLATED: &[&[&str]] = &[
    &["-p", "leaf-core"],
    &["-p", "leaf-core", "--no-default-features"],
    &["-p", "leaf-raster"],
    &["-p", "leaf-math"],
    &["-p", "leaf-math-edit"],
    &["-p", "leaf-math-edit", "--features", "wasm"],
    &["-p", "leaf-session"],
    &["-p", "leaf-mermaid"],
    &["-p", "leaf-dioxus"],
    &[
        "-p",
        "leaf-dioxus",
        "--no-default-features",
        "--features",
        "vello",
    ],
    &["-p", "leaf-pages"],
    &["-p", "leaf-dioxus-app"],
    &["-p", "leaf-dioxus-tui"],
    &["-p", "leaf-dioxus-web"],
    &["-p", "xtask"],
];

pub fn run_task() -> Result<()> {
    for spec in ISOLATED {
        let mut args = vec!["check"];
        args.extend_from_slice(spec);
        run(cargo().args(&args))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------
//
// This came across from `release.rs` when releasing moved out of the xtask.
// Only the isolation test below still asks the question, and it is a CI
// question: a crate added to the workspace and left out of
// `ISOLATED` is checked by `--workspace` and by nothing alone.
//
// It asks for names rather than directories, because a crate's name is what
// `-p` takes.

/// Every workspace member's package name, with `crates/*` and `apps/*`
/// expanded, less any `exclude`d: a glob is a directory listing, so an
/// excluded crate is on disk and would otherwise be demanded of `ISOLATED`,
/// for a crate `cargo check -p` in this workspace cannot reach.
#[cfg(test)]
fn member_names() -> Result<Vec<String>> {
    let manifest = read("Cargo.toml")?;
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("members"))
        .context("no `members` in [workspace]")?;
    let excluded: Vec<&str> = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("exclude"))
        .map(|line| line.split('"').skip(1).step_by(2).collect())
        .unwrap_or_default();

    let mut dirs = Vec::new();
    for entry in line.split('"').skip(1).step_by(2) {
        match entry.strip_suffix("/*") {
            None => dirs.push(entry.to_string()),
            Some(parent) => {
                let mut expanded = Vec::new();
                let listing = std::fs::read_dir(root().join(parent))
                    .with_context(|| format!("could not list workspace glob `{entry}`"))?;
                for child in listing {
                    let child = child?.file_name().to_string_lossy().into_owned();
                    let dir = format!("{parent}/{child}");
                    if root().join(&dir).join("Cargo.toml").is_file() {
                        expanded.push(dir);
                    }
                }
                expanded.sort();
                dirs.extend(expanded);
            }
        }
    }

    dirs.retain(|dir| !excluded.contains(&dir.as_str()));
    dirs.iter().map(|dir| package_name(dir)).collect()
}

/// `[package] name` — read from that table specifically, because a manifest with
/// a `[[bin]]` section has a second `name = "…"` in it.
#[cfg(test)]
fn package_name(dir: &str) -> Result<String> {
    let manifest = read(format!("{dir}/Cargo.toml"))?;
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(name) = rest.split('"').nth(1)
        {
            return Ok(name.to_string());
        }
    }
    bail!("no `[package] name` in {dir}/Cargo.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every member of the workspace should be checked alone; that is the whole
    /// point of the job. A crate added to the workspace and left out of
    /// `ISOLATED` is exactly the case this catches — nothing else would, since
    /// `--workspace` would go on passing.
    #[test]
    fn package_isolation_covers_every_member() {
        for name in member_names().unwrap() {
            assert!(
                ISOLATED.iter().any(|spec| spec == &["-p", &name]),
                "workspace member `{name}` is not checked in isolation by \
                 `cargo xtask package-isolation`",
            );
        }
    }
}
