//! `cargo xtask ci` — the checks a release has to pass, as one program.
//!
//! leaf has no CI workflow: the checks live here, and a release runs them
//! (the shared `release release` runs `cargo xtask ci` before it writes
//! anything, as `.config/release.toml` names it). So
//! this table is not a mirror of a YAML file that could drift from it — it is
//! the only statement of what "green" means, and `cargo xtask ci` is the only
//! way to ask.
//!
//! Ordered cheapest-first, so the run that is going to fail fails in seconds
//! rather than after gpui compiles.

use crate::util::{cargo, run};
#[cfg(test)]
use crate::util::{read, root};
#[cfg(test)]
use anyhow::Context;
use anyhow::{Result, bail};

/// One check: what to call it, and the work itself.
pub struct Job {
    /// `cargo xtask ci <id>`.
    pub id: &'static str,
    /// The heading printed above it in a full run.
    pub name: &'static str,
    /// One line, for `cargo xtask ci --list`.
    pub about: &'static str,
    run: fn() -> Result<()>,
}

/// The whole of CI, in the order [`run_all`] runs it.
pub const JOBS: &[Job] = &[
    Job {
        id: "fmt",
        name: "Format",
        about: "rustfmt, in check mode",
        run: fmt,
    },
    Job {
        id: "clippy",
        name: "Clippy",
        about: "clippy over every target, warnings denied",
        run: clippy,
    },
    Job {
        id: "test",
        name: "Test",
        about: "the workspace test suite",
        run: test,
    },
    Job {
        id: "package-isolation",
        name: "Package isolation",
        about: "check each crate alone, and each feature-off shape a host builds",
        run: package_isolation,
    },
];

fn fmt() -> Result<()> {
    run(cargo().args(["fmt", "--all", "--check"]))
}

/// Warnings are errors here because they are errors in review — a lint that only
/// fires on someone else's machine is a lint found too late.
fn clippy() -> Result<()> {
    run(cargo().args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))
}

fn test() -> Result<()> {
    run(cargo().args(["test", "--workspace"]))
}

/// Workspace feature unification means `cargo check --workspace` can pass even
/// when a crate cannot compile on its own — some other member's feature
/// selection quietly fills the gap. Each row below checks one crate the way some
/// host actually builds it.
///
/// The `--no-default-features` rows are not thoroughness for its own sake: each
/// is a shape that ships. `leaf-core` without `fs` is what `leaf-wasm` and
/// `leaf-ffi` link (a browser and a sandboxed embed have no path to read),
/// `leaf-ratatui` bare is the terminal with no graphics protocol and no
/// theme query, and `leaf-ratatui` with only `images` is a host that paints
/// pictures but themes the surface itself rather than letting it ask the
/// terminal.
///
/// The gpui crates are not here because they are not in this workspace:
/// `crates/leaf-gpui`, `apps/leaf`, and `apps/leaf-ios` are `exclude`d, each
/// with its own lockfile, and nothing in this file reaches them. That is
/// deliberate — see the `exclude` note in the root Cargo.toml — and it means the
/// gpui side is checked by building it, not by CI.
///
/// A new workspace member belongs in this list; the test below is what says so.
const ISOLATED: &[&[&str]] = &[
    &["-p", "leaf-core"],
    &["-p", "leaf-core", "--no-default-features"],
    &["-p", "leaf-ffi"],
    &["-p", "leaf-ratatui"],
    &["-p", "leaf-ratatui", "--no-default-features"],
    &[
        "-p",
        "leaf-ratatui",
        "--no-default-features",
        "--features",
        "images",
    ],
    &["-p", "leaf-wasm"],
    &["-p", "leaf-tui"],
    &["-p", "xtask"],
];

fn package_isolation() -> Result<()> {
    for spec in ISOLATED {
        let mut args = vec!["check"];
        args.extend_from_slice(spec);
        run(cargo().args(&args))?;
    }
    Ok(())
}

#[derive(clap::Args)]
pub struct Args {
    /// Run one job by id instead of every job.
    job: Option<String>,

    /// List the jobs and exit.
    #[arg(long)]
    list: bool,
}

pub fn run_task(args: Args) -> Result<()> {
    if args.list {
        for job in JOBS {
            println!("  {:<18}{}", job.id, job.about);
        }
        return Ok(());
    }
    match args.job {
        None => run_all(),
        Some(id) => match JOBS.iter().find(|job| job.id == id) {
            Some(job) => (job.run)(),
            None => bail!(
                "unknown job `{id}` — try one of: {}",
                JOBS.iter().map(|job| job.id).collect::<Vec<_>>().join(", ")
            ),
        },
    }
}

/// Every job, in order. Stops at the first failure, on the theory that a red
/// build is worth reading before the next one buries it.
pub fn run_all() -> Result<()> {
    for job in JOBS {
        println!("\n\x1b[1m━━ {} ━━\x1b[0m", job.name);
        (job.run)()?;
    }
    println!("\n\x1b[32mall {} jobs passed\x1b[0m", JOBS.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------
//
// This came across from `release.rs` when releasing moved to the shared tooling
// (diaryx-org/devtools). Only the isolation test below still asks the question,
// and it is a CI question: a crate added to the workspace and left out of
// `ISOLATED` is checked by `--workspace` and by nothing alone.
//
// It asks for names rather than directories, which the release code needed and
// this does not — `apps/leaf-tui` is the crate `leaf-tui` and `apps/leaf` is the
// crate `leaf`, so the two are not interchangeable and only one is what `-p`
// takes.

/// Every workspace member's package name, with `crates/*` expanded.
///
/// `exclude`d members are deliberately not here, and are subtracted explicitly
/// rather than left to the glob: `crates/*` is a directory listing, so
/// `crates/leaf-gpui` is still on disk and would otherwise be demanded of
/// `ISOLATED` — for a crate `cargo check -p` in this workspace can no longer
/// reach. The three excluded are the gpui side, each a standalone workspace with
/// its own lockfile.
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
/// a `[[bin]]` section has a second `name = "…"` in it, and `apps/leaf-tui`'s
/// says `leaf`.
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

/// The methods `leaf-ffi` and `leaf-wasm` are allowed to disagree about, and why.
///
/// Everything else has to match. The two crates are thin projections of one
/// `leaf_core::Doc` for two hosts that want the same facts, and the way they
/// come apart is not a decision anybody makes — it is a feature landing in the
/// binding whose frontend is being worked on and not in the other. That is how
/// the browser ended up without tables, footnotes, or a link it could follow,
/// months after the Apple app had all three.
///
/// A name belongs here only when the *concept* is host-specific. A name that is
/// merely not written yet does not: the point of the test is to fail then.
#[cfg(test)]
const BINDING_DIVERGENCE: &[(&str, &str)] = &[
    (
        "set_dark_appearance",
        "leaf-wasm spells the same thing `set_color_scheme`, taking the CSS \
         media-query name a browser already has rather than the Bool an \
         NSAppearance hands Swift",
    ),
    (
        "set_color_scheme",
        "the browser half of the `set_dark_appearance` pair above",
    ),
];

/// The method names a binding crate exports, read off its source: the `pub fn`s
/// at one level of indentation, which is where an `impl` block's methods sit and
/// where a free function does not.
#[cfg(test)]
fn exported_methods(path: &str) -> Result<Vec<String>> {
    let src = read(path)?;
    let mut names: Vec<String> = src
        .lines()
        .filter_map(|line| line.strip_prefix("    pub fn "))
        .filter_map(|rest| rest.split(['(', '<', ' ']).next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        bail!("no exported methods found in {path} — has the file moved?");
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_are_distinct() {
        let mut ids: Vec<&str> = JOBS.iter().map(|job| job.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate job id");
    }

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
                 `cargo xtask ci package-isolation`",
            );
        }
    }

    /// Every method one binding exports, the other exports too.
    ///
    /// Read off the source rather than asked of the type system, because there
    /// is no shared trait to ask: `leaf-ffi`'s methods hang off a
    /// `#[uniffi::export]` impl and `leaf-wasm`'s off a `#[wasm_bindgen]` one,
    /// and neither generator produces anything the other can be compared to. A
    /// text scan is coarse — it counts a name, not a signature — but it catches
    /// the failure that actually happens, which is a whole method existing on
    /// one side and not the other.
    #[test]
    fn the_two_bindings_export_the_same_methods() {
        let ffi = exported_methods("crates/leaf-ffi/src/lib.rs").unwrap();
        let wasm = exported_methods("crates/leaf-wasm/src/lib.rs").unwrap();
        let allowed: Vec<&str> = BINDING_DIVERGENCE.iter().map(|(name, _)| *name).collect();

        let missing = |from: &[String], present_in: &[String]| -> Vec<String> {
            from.iter()
                .filter(|m| !present_in.contains(m) && !allowed.contains(&m.as_str()))
                .cloned()
                .collect()
        };

        let absent_from_wasm = missing(&ffi, &wasm);
        let absent_from_ffi = missing(&wasm, &ffi);
        assert!(
            absent_from_wasm.is_empty() && absent_from_ffi.is_empty(),
            "the two frontend bindings have drifted.\n  \
             in leaf-ffi, missing from leaf-wasm: {absent_from_wasm:?}\n  \
             in leaf-wasm, missing from leaf-ffi: {absent_from_ffi:?}\n\
             Add the method to the other binding, or — if the concept really is \
             specific to one host — name it in BINDING_DIVERGENCE with the reason.",
        );
    }

    /// An entry that no longer describes a real divergence is worse than no
    /// entry: it silently exempts a name the test would otherwise check.
    #[test]
    fn every_divergence_is_still_a_real_one() {
        let ffi = exported_methods("crates/leaf-ffi/src/lib.rs").unwrap();
        let wasm = exported_methods("crates/leaf-wasm/src/lib.rs").unwrap();
        for (name, why) in BINDING_DIVERGENCE {
            let in_ffi = ffi.iter().any(|m| m == name);
            let in_wasm = wasm.iter().any(|m| m == name);
            assert!(
                in_ffi || in_wasm,
                "`{name}` is exempted but exists in neither binding — delete the entry",
            );
            assert!(
                !(in_ffi && in_wasm),
                "`{name}` is exempted but both bindings export it — delete the entry ({why})",
            );
        }
    }
}
