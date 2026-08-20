//! The workspace's task runner — the [cargo-xtask] pattern: a plain Rust binary,
//! invoked as `cargo xtask <task>` through the alias in `.cargo/config.toml`, so
//! every developer already has the only toolchain it needs.
//!
//! What earns a task here is work that leaves Rust, or work that must be done
//! the same way every time. `cargo run` already opens the TUI and `cargo run -p
//! leaf` the GUI; the Apple app and the web demo each need three or four tools
//! driven in the right order. And a release moves one version number through a
//! dozen manifests, cuts a changelog section, and uploads crates in an order
//! crates.io will accept — all of which is easy to get half-right by hand.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod ci;
mod release;
mod swift;
mod util;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Build leaf's non-Rust frontends, check the workspace, and cut releases",
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

    /// Run the checks a release has to pass — all of them, or one by id.
    Ci(ci::Args),

    /// Print the workspace version.
    Version,
    /// Move the workspace to a new version, and stop there.
    Bump {
        /// patch, minor, major, or a literal X.Y.Z to move to.
        spec: String,
    },
    /// Regenerate the changelog's unreleased region.
    Changelog(release::ChangelogArgs),
    /// Check, bump, cut the changelog, commit and tag — pushing only with --push.
    Release(release::ReleaseArgs),
    /// Publish every crate crates.io is missing, in dependency order.
    Publish(release::PublishArgs),
    /// One release's changelog section, for a GitHub release body.
    ReleaseNotes(release::NotesArgs),
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Swift(args) => swift::run_task(args),
        Task::Web(args) => web::run_task(args),
        Task::Ci(args) => ci::run_task(args),
        Task::Version => release::print_version(),
        Task::Bump { spec } => release::bump(&spec),
        Task::Changelog(args) => release::changelog(args),
        Task::Release(args) => release::release(args),
        Task::Publish(args) => release::publish(args),
        Task::ReleaseNotes(args) => release::release_notes(args),
    }
}
