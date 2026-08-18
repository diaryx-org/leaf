//! The workspace's task runner — the [cargo-xtask] pattern: a plain Rust binary,
//! invoked as `cargo xtask <task>` through the alias in `.cargo/config.toml`, so
//! every developer already has the only toolchain it needs.
//!
//! What earns a task here is a build that leaves Rust. `cargo run` already opens
//! the TUI and `cargo run -p leaf` the GUI; the Apple app and the web demo each
//! need three or four tools driven in the right order, and that order deserves to
//! live in the repo rather than in a paragraph of a README.
//!
//! [cargo-xtask]: https://github.com/matklad/cargo-xtask

mod swift;
mod util;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Build and run leaf's non-Rust frontends",
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
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Swift(args) => swift::run_task(args),
        Task::Web(args) => web::run_task(args),
    }
}
