//! Shared plumbing for the tasks: where the repo is, how a subprocess is run,
//! and how a missing external toolchain is reported.

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repo root. xtask lives at `<root>/xtask`, so every path a task builds is
/// anchored here rather than at the caller's cwd — which is why `cargo xtask`
/// behaves the same from any subdirectory.
///
/// Asked at *runtime*, not baked in with `env!`. The compile-time answer is a
/// string literal in the binary, and cargo does not rebuild on a path change
/// alone: moving the checkout (`~/Code/leaf` → `~/diaryx/leaf`) carries `target/`
/// along with it, the stale `xtask` is reused, and every task then reads and
/// writes a directory that no longer exists. `cargo xtask web` failed that way
/// with `could not read /Users/adamharris/Code/leaf/packages/leaf-web/src` —
/// naming a path nothing in the checkout mentions, from a tree that was moved
/// weeks earlier.
///
/// Cargo sets `CARGO_MANIFEST_DIR` in the environment of what it runs, so the
/// alias in `.cargo/config.toml` hands us the truth for free. The walk up from
/// the executable covers a binary invoked directly out of `target/`, and the
/// compile-time value is the last resort — right whenever the tree hasn't moved,
/// which is the ordinary case.
pub fn root() -> PathBuf {
    from_manifest_dir(std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from))
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| ancestor_root(&exe))
        })
        .or_else(|| from_manifest_dir(Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))))
        .unwrap_or_else(|| {
            panic!(
                "could not locate the repo root: no CARGO_MANIFEST_DIR, nothing \
                 named xtask/Cargo.toml above the executable, and the compile-time \
                 root ({}) is gone — run this through `cargo xtask`",
                env!("CARGO_MANIFEST_DIR")
            )
        })
}

/// `<dir>/..`, if `dir` really is this repo's `xtask/`.
fn from_manifest_dir(dir: Option<PathBuf>) -> Option<PathBuf> {
    let root = dir?.parent()?.to_path_buf();
    is_root(&root).then_some(root)
}

/// The nearest ancestor of `from` that looks like the repo root — for a binary
/// run straight out of `target/debug/`, where the manifest dir isn't in the
/// environment but the checkout is still overhead.
fn ancestor_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|dir| is_root(dir))
        .map(Path::to_path_buf)
}

/// Whether `dir` is a checkout of this repo, asked by the one file that is
/// always there and is what `root()` is *for*: the task runner's own manifest.
fn is_root(dir: &Path) -> bool {
    dir.join("xtask/Cargo.toml").is_file()
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

/// Read a repo file, named by its path from the root. Test-only since releasing
/// moved out: the isolation test reads the workspace manifest, and nothing the
/// binary does touches a file directly.
#[cfg(test)]
pub fn read(path: impl AsRef<Path>) -> Result<String> {
    let path = root().join(path);
    std::fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The root is the checkout the test binary was built from — and, crucially,
    /// a directory that exists. A stale compile-time answer passes the first
    /// assertion and fails the second, which is the bug this function was
    /// rewritten to stop.
    #[test]
    fn root_is_a_real_checkout() {
        let root = root();
        assert!(root.is_dir(), "{} is not a directory", root.display());
        assert!(root.join("xtask/Cargo.toml").is_file());
        assert!(root.join("crates/leaf-core/Cargo.toml").is_file());
    }
}
