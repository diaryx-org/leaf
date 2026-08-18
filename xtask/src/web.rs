//! `cargo xtask web` — build the wasm binding and serve the demo page in
//! `apps/leaf-web-demo`.
//!
//! The page imports `packages/leaf-web/src/index.js`, which imports the
//! wasm-pack output in `packages/leaf-web/pkg/` — both by relative path, and
//! both outside the demo directory. So the server's document root is the *repo
//! root* and the page is reached at `/apps/leaf-web-demo/`; rooting it at the
//! demo directory instead serves an index.html whose every import 404s.

use crate::util::{cmd, require_tool, root, run};
use anyhow::{Context, Result, bail};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The port `.claude/launch.json` already uses for this page.
const DEFAULT_PORT: u16 = 8137;

#[derive(clap::Args)]
pub struct Args {
    /// Port for the static server.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Build the wasm unoptimized — much faster to compile, much slower to run.
    #[arg(long)]
    dev: bool,

    /// Serve the `pkg/` that is already there instead of rebuilding it.
    #[arg(long)]
    no_build: bool,

    /// Build the wasm and exit without serving.
    #[arg(long)]
    build_only: bool,

    /// Don't open a browser.
    #[arg(long)]
    no_open: bool,
}

pub fn run_task(args: Args) -> Result<()> {
    let root = root();
    let pkg = root.join("packages/leaf-web/pkg");

    check_js_syntax(&root)?;

    if args.no_build {
        if !pkg.join("leaf_wasm.js").is_file() {
            bail!(
                "--no-build was passed but {} has no wasm-pack output to serve",
                pkg.display()
            );
        }
    } else {
        require_tool("wasm-pack", "cargo install wasm-pack")?;
        let mut build = cmd("wasm-pack");
        build.arg("build").arg(root.join("crates/leaf-wasm"));
        if args.dev {
            build.arg("--dev");
        }
        // `--target web` emits an ES module with an explicit `init()`, which is
        // what packages/leaf-web/src/index.js expects; the out-dir is the `pkg/`
        // the package's `files` field ships. Both match the `build:wasm` script
        // in packages/leaf-web/package.json — this task is that script plus a
        // server, not a second opinion about how the binding is built.
        build.args(["--target", "web"]).arg("--out-dir").arg(&pkg);
        run(&mut build)?;
    }

    if args.build_only {
        println!("✓ Built {}", pkg.display());
        return Ok(());
    }

    require_tool("python3", "install Python 3.7+ (xtask/serve.py runs on it)")?;
    let url = format!("http://127.0.0.1:{}/apps/leaf-web-demo/", args.port);

    // xtask/serve.py rather than `python3 -m http.server`: the same handler, with
    // caching turned off. See the docstring there — everything under this root is
    // a build output, and a browser quietly reusing a stale one costs an
    // afternoon before you think to suspect the cache.
    let mut server = cmd("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("serve.py"))
        .arg(args.port.to_string())
        .arg(&root)
        .spawn()
        .context("could not start xtask/serve.py")?;

    if !args.no_open {
        // Opening before the socket is listening races the browser to a
        // connection-refused page, so wait for the port to actually answer.
        if wait_for_port(args.port) {
            let _ = cmd("open").arg(&url).status();
        } else {
            eprintln!(
                "! the server never came up on port {} — is it in use?",
                args.port
            );
        }
    }

    println!("▸ serving {} at {url}", root.display());
    println!("  Ctrl-C to stop.");
    let status = server
        .wait()
        .context("the static server exited abnormally")?;
    // Ctrl-C reaches the child as a signal, which is an ordinary stop here, not
    // a failure to report.
    if !status.success() && signal(&status).is_none() {
        bail!("the static server exited with {status}");
    }
    Ok(())
}

/// Parse-check the hand-written JS before serving it.
///
/// The package is plain ES modules with no build step, so a syntax error is
/// invisible until a browser refuses the module — and then the *whole* editor is
/// simply absent, with one console line to explain it. That is how a stray
/// backtick inside `EDITOR_CSS` (a template literal) went unnoticed for a week.
/// `node --check` reports the identical error, here, in a second.
///
/// Best-effort: node isn't otherwise needed to build or run the web demo, so a
/// machine without it still gets the server rather than a hard stop.
fn check_js_syntax(root: &Path) -> Result<()> {
    if require_tool("node", "install Node for the parse check").is_err() {
        return Ok(());
    }
    let src = root.join("packages/leaf-web/src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .with_context(|| format!("could not read {}", src.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "js"))
        .collect();
    files.sort();
    for file in files {
        run(cmd("node").arg("--check").arg(&file))
            .with_context(|| format!("{} does not parse", file.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// Poll the loopback port until it accepts a connection, for up to ~5s.
fn wait_for_port(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    for _ in 0..50 {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
