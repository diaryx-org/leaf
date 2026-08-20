//! `cargo xtask ci` — the checks a release has to pass, as one program.
//!
//! leaf has no CI workflow: the checks live here, and a release runs them
//! ([`crate::release::release`] calls [`run_all`] before it writes anything). So
//! this table is not a mirror of a YAML file that could drift from it — it is
//! the only statement of what "green" means, and `cargo xtask ci` is the only
//! way to ask.
//!
//! Ordered cheapest-first, so the run that is going to fail fails in seconds
//! rather than after gpui compiles.

use crate::util::{cargo, run};
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
/// `leaf-ratatui` without `images` is the terminal with no graphics protocol,
/// and `leaf-gpui` without `desktop` is `apps/leaf-ios` on gpui-mobile — the one
/// build in the workspace that no `cargo check` here ever reaches, since
/// leaf-ios is a standalone workspace.
///
/// A new workspace member belongs in this list; the test below is what says so.
const ISOLATED: &[&[&str]] = &[
    &["-p", "leaf-core"],
    &["-p", "leaf-core", "--no-default-features"],
    &["-p", "leaf-ffi"],
    &["-p", "leaf-ratatui"],
    &["-p", "leaf-ratatui", "--no-default-features"],
    &["-p", "leaf-wasm"],
    &["-p", "leaf-gpui"],
    &["-p", "leaf-gpui", "--no-default-features"],
    &["-p", "leaf-tui"],
    &["-p", "leaf"],
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
        for member in crate::release::members().unwrap() {
            assert!(
                ISOLATED.iter().any(|spec| spec == &["-p", &member.name]),
                "workspace member `{}` is not checked in isolation by \
                 `cargo xtask ci package-isolation`",
                member.name,
            );
        }
    }
}
