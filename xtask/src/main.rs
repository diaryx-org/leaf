//! The workspace's task runner — the [cargo-xtask] pattern: a plain Rust binary,
//! invoked as `cargo xtask <task>` through the alias in `.cargo/config.toml`, so
//! every developer already has the only toolchain it needs.
//!
//! What earns a task here is work that leaves Rust, or work that must be done
//! the same way every time. `cargo run` already opens the TUI and `cargo run -p
//! leaf` the GUI; the Apple app and the web demo each need three or four tools
//! driven in the right order.
//!
//! Releasing is not here. It is `release <command>`, from diaryx-org/devtools,
//! configured by `.config/release.toml` — the same tool leaf, prov, twig,
//! flower, and the historica repos all cut releases with, because five copies
//! of one program is five places for it to drift.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod ci;
mod swift;
mod util;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Build leaf's non-Rust frontends and check the workspace",
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
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Swift(args) => swift::run_task(args),
        Task::Web(args) => web::run_task(args),
        Task::Ci(args) => ci::run_task(args),
    }
}
