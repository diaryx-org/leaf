//! The workspace's task runner — the [cargo-xtask] pattern: a plain Rust binary,
//! invoked as `cargo xtask <task>` through the alias in `.cargo/config.toml`, so
//! every developer already has the only toolchain it needs.
//!
//! What earns a task here is work that leaves Rust, or work that must be done
//! the same way every time. `cargo run` already opens the TUI and `cargo run -p
//! leaf` the GUI; the Apple app, the Android app and the web demo each need three or four tools
//! driven in the right order.
//!
//! CI and releasing are not here. Both are `ship`, configured by
//! `.config/ship.figl` — the same tool the org's other repositories cut
//! releases with, because five copies of one program is five places for it to
//! drift. `ship ci` runs the checks (some of them call tasks here), and `ship`
//! cuts a release, running `sync-versions` after the bump.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod android;
mod isolation;
mod package;
mod swift;
mod util;
mod versions;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Build leaf's non-Rust frontends, and the checks too big for one command",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build and launch the Apple app (apps/leaf-editor) over packages/leaf-swift.
    Swift(swift::Args),
    /// Build the wasm binding and serve the web demo (apps/leaf-web-demo).
    Web(web::Args),
    /// Build and launch the Android app (apps/leaf-android) over packages/leaf-android.
    Android(android::Args),
    /// Build a signed, notarised Leaf.app in a .dmg for other Macs (target/package/).
    Package(package::Args),

    /// Check each crate alone, in each feature shape a host builds.
    PackageIsolation,

    /// Write the workspace version into the files no manifest parser reaches
    /// (packages/leaf-web/package.json). `ship`'s bump runs this.
    SyncVersions {
        /// Write nothing; fail unless every file already states the version.
        #[arg(long)]
        check: bool,
    },

    /// Moved: CI is `ship ci` now, configured by `.config/ship.figl`.
    #[command(hide = true)]
    Ci {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Swift(args) => swift::run_task(args),
        Task::Web(args) => web::run_task(args),
        Task::Android(args) => android::run_task(args),
        Task::Package(args) => package::run_task(args),
        Task::PackageIsolation => isolation::run_task(),
        Task::SyncVersions { check: false } => versions::run_task(),
        Task::SyncVersions { check: true } => versions::check(),
        Task::Ci { .. } => anyhow::bail!(
            "`cargo xtask ci` is `ship ci` now (`ship ci <job>` for one job, `ship ci --list` for the jobs)"
        ),
    }
}
