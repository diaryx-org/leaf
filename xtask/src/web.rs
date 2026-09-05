//! `cargo xtask web` — build the wasm binding and serve the demo page in
//! `apps/leaf-web-demo`, or (with `--test`) the editor test page in
//! `packages/leaf-web/test`.
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

    /// Serve the editor test page instead of the demo.
    ///
    /// The tests live in a browser rather than in `cargo test` because what they
    /// cover is the half of leaf-web that Rust cannot reach: a `TreeWalker`, a
    /// `Range`, a native selection, and text laid out in a proportional font.
    /// A stub DOM would mostly be testing the stub. So this is a task a person
    /// runs, not a job `cargo xtask ci` can — the page reports into
    /// `document.title` (`PASS n` / `FAIL n/m`) and `window.__results` as well
    /// as on screen, so a driver that does have a browser can read the outcome.
    #[arg(long)]
    test: bool,

    /// Run the editor tests in a headless browser and report the outcome,
    /// instead of serving the page for a person.
    ///
    /// Implies `--test` and `--no-open`. The browser is Chrome or Chromium:
    /// `$LEAF_BROWSER` if set, else the macOS application bundle, else
    /// whichever of `google-chrome`, `chromium` and friends is on PATH. Exits
    /// non-zero when a test fails, or when the page never reports at all —
    /// which is what a wasm that failed to load looks like.
    #[arg(long)]
    headless: bool,
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
    // Resolved before the server is up, so a machine with no browser hears so
    // at once rather than after a spawn it then has to tear down.
    let browser = if args.headless {
        Some(find_browser()?)
    } else {
        None
    };
    let page = if args.test || args.headless {
        "packages/leaf-web/test/editor.test.html"
    } else {
        "apps/leaf-web-demo/"
    };
    let url = format!("http://127.0.0.1:{}/{page}", args.port);

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

    if let Some(browser) = browser {
        // The server lives exactly as long as the one page load it exists for.
        let outcome = if wait_for_port(args.port) {
            run_headless(&browser, &url)
        } else {
            Err(anyhow::anyhow!(
                "the server never came up on port {} — is it in use?",
                args.port
            ))
        };
        let _ = server.kill();
        let _ = server.wait();
        return outcome;
    }

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

/// The Chrome or Chromium binary to run the tests in, or an error naming how to
/// point one out. `$LEAF_BROWSER` wins, for a machine whose browser lives
/// somewhere this list does not look.
fn find_browser() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("LEAF_BROWSER") {
        let explicit = PathBuf::from(explicit);
        if explicit.is_file() {
            return Ok(explicit);
        }
        bail!("LEAF_BROWSER={} is not a file", explicit.display());
    }
    let bundles = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    if let Some(found) = bundles.iter().map(Path::new).find(|p| p.is_file()) {
        return Ok(found.to_path_buf());
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ];
    for name in names {
        if require_tool(name, "").is_ok() {
            return Ok(PathBuf::from(name));
        }
    }
    bail!(
        "no Chrome or Chromium found — install one, or set LEAF_BROWSER to the binary \
         (on macOS: \"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome\")"
    )
}

/// Load the test page in a headless browser and turn what it reported into an
/// exit status.
///
/// `--dump-dom` prints the document once it has loaded, and the tests run
/// under a top-level `await`, so the page is given a virtual-time budget in
/// which to finish — the wasm has to instantiate, then every test has to lay a
/// document out and measure it. The page reports into `document.title`
/// (`PASS n` / `FAIL k/n`) and each result into an `<li class="pass|fail">`,
/// which is all this reads; no scraping of anything the page didn't put there
/// for a driver to read.
fn run_headless(browser: &Path, url: &str) -> Result<()> {
    let mut chrome = cmd(browser);
    chrome.args([
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--window-size=1200,900",
        "--virtual-time-budget=60000",
        "--dump-dom",
        url,
    ]);
    println!("▸ {} --headless=new --dump-dom {url}", browser.display());
    let output = chrome
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("could not run {}", browser.display()))?;
    let dom = String::from_utf8_lossy(&output.stdout);

    let title = between(&dom, "<title>", "</title>").unwrap_or_default();
    // Every result the page listed, pass or fail, so the run reads like the
    // page does. A failure's reason follows it in a `.why` block.
    let mut rest = dom.as_ref();
    while let Some(start) = rest.find("<li class=\"") {
        let li = &rest[start..];
        let Some(end) = li.find("</li>") else { break };
        let class = between(li, "<li class=\"", "\"").unwrap_or_default();
        let name = li[..end].rsplit('>').next().unwrap_or_default();
        let mark = if class == "pass" { "ok  " } else { "FAIL" };
        println!("  {mark} {}", unescape(name));
        rest = &li[end..];
        if class != "pass"
            && let Some(why) = between(rest, "<div class=\"why\">", "</div>")
        {
            println!("       {}", unescape(why));
        }
    }

    if let Some(n) = title.strip_prefix("PASS ") {
        println!("✓ all {n} web editor tests passed");
        return Ok(());
    }
    if let Some(counts) = title.strip_prefix("FAIL ") {
        bail!("web editor tests: {counts} failed");
    }
    bail!(
        "the test page never reported (title was {title:?}) — the wasm may not have loaded; \
         run `cargo xtask web --test` and look at the browser console"
    )
}

/// The text between the first `open` and the `close` that follows it.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

/// Undo the entity escaping `--dump-dom` applies to text, enough to print a
/// test name or an assertion message as it was written.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
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
