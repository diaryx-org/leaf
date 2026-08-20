//! Shared plumbing for the tasks: where the repo is, how a subprocess is run,
//! and how a missing external toolchain is reported.

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repo root. xtask lives at `<root>/xtask`, so the root is fixed at compile
/// time — every path a task builds is anchored here rather than at the caller's
/// cwd, which is why `cargo xtask` behaves the same from any subdirectory.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

/// A command whose working directory is the repo root.
pub fn cmd(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.current_dir(root());
    c
}

/// `cargo`, rooted at the workspace. Cargo tells its subprocesses which cargo it
/// is; prefer that over whichever one happens to be first on `PATH`, so a task
/// invoked through `cargo +nightly xtask` doesn't quietly switch toolchains
/// halfway.
pub fn cargo() -> Command {
    cmd(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

/// Run a command to completion, echoing it first in the same `▸` voice the shell
/// scripts under `scripts/` use, and failing loudly on a non-zero exit.
pub fn run(cmd: &mut Command) -> Result<()> {
    println!("▸ {}", render(cmd));
    let status = cmd
        .status()
        .with_context(|| format!("could not spawn `{}`", name(cmd)))?;
    if !status.success() {
        bail!("`{}` failed ({status})", name(cmd));
    }
    Ok(())
}

/// Run a command whose failure is not interesting — `simctl boot` on a device
/// that is already booted, `pkill` with nothing to kill.
pub fn run_ignoring_failure(cmd: &mut Command) {
    println!("▸ {}", render(cmd));
    let _ = cmd.status();
}

/// Run a command and hand back its stdout, for the answers a task acts on rather
/// than shows — an HTTP status, a branch name, a tag list. Deliberately not
/// echoed: these are questions, and a log of them reads as noise between the
/// commands that actually did something.
pub fn capture(cmd: &mut Command) -> Result<String> {
    let output = cmd
        .output()
        .with_context(|| format!("could not spawn `{}`", name(cmd)))?;
    if !output.status.success() {
        bail!(
            "`{}` failed ({})\n{}",
            render(cmd),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Read a repo file, named by its path from the root.
pub fn read(path: impl AsRef<Path>) -> Result<String> {
    let path = root().join(path);
    std::fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))
}

/// Write a repo file, named by its path from the root.
pub fn write(path: impl AsRef<Path>, contents: &str) -> Result<()> {
    let path = root().join(path);
    std::fs::write(&path, contents).with_context(|| format!("could not write {}", path.display()))
}

/// Assert an external tool is on `PATH`, naming how to get it when it isn't.
/// The alternative is a bare `No such file or directory` from `spawn` several
/// steps later, which says nothing about what to install.
pub fn require_tool(bin: &str, how_to_get_it: &str) -> Result<()> {
    let found = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(bin);
                candidate.is_file() && is_executable(&candidate)
            })
        })
        .unwrap_or(false);
    if !found {
        bail!("`{bin}` is not on PATH — {how_to_get_it}");
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

fn name(cmd: &Command) -> String {
    cmd.get_program().to_string_lossy().into_owned()
}

/// The command as a reader could retype it. Arguments carrying spaces (an
/// `xcodebuild -destination`, most often) are quoted so the echo stays copyable.
fn render(cmd: &Command) -> String {
    let mut out = name(cmd);
    for arg in cmd.get_args() {
        let arg = arg.to_string_lossy();
        out.push(' ');
        if arg.contains(' ') {
            out.push_str(&format!("'{arg}'"));
        } else {
            out.push_str(&arg);
        }
    }
    out
}
