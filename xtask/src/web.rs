//! `cargo xtask web` — build Leaf for the browser (`apps/leaf-dioxus-web`)
//! into its `pkg/`, split: the Mermaid renderer in a wasm module of its own,
//! fetched the first time a diagram is drawn.
//!
//! The steps are `dx build --wasm-split`'s, without `dx`, whose web build is
//! made for dioxus-web rather than for Blitz in a canvas:
//!
//! 1. Cargo, for `wasm32-unknown-unknown`, with fat LTO, debug info and
//!    `--emit-relocs` (the manifest's release profile and the flag below):
//!    the splitter finds the call graph in the relocations and the names.
//! 2. `wasm-bindgen`, keeping those (`--keep-debug --keep-lld-exports`, and
//!    `--no-demangle`, or its names no longer match the symbols').
//! 3. `wasm-split-cli`, which moves what only a `#[wasm_split]` function
//!    reaches into that function's module, and what several share into a
//!    chunk.
//! 4. The custom sections — names, DWARF, relocations — stripped from every
//!    module, and the splitter's loader glue (`__wasm_split.js`) pointed at
//!    the modules beside it.
//!
//! The two tools are installed into the app's `target/tools` at the versions
//! the app's lockfile names, the first time; nothing is put on `PATH`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::util::{cargo, cmd, root, run};

const APP: &str = "apps/leaf-dioxus-web";
/// The binary's name, which names the wasm and wasm-bindgen's output.
const BIN: &str = "leaf_dioxus_web";

#[derive(clap::Args)]
pub struct Args {
    /// Serve the page on this port once it is built (Python's
    /// `http.server`, on 127.0.0.1).
    #[arg(long, value_name = "PORT")]
    pub serve: Option<u16>,
    /// One module, without fat LTO or the split: for trying a change, not
    /// for serving. Built with the app's `quick` profile.
    #[arg(long)]
    pub quick: bool,
}

pub fn run_task(args: Args) -> Result<()> {
    let app = root().join(APP);
    let tools = app.join("target/tools");
    let lock = crate::util::read(format!("{APP}/Cargo.lock"))?;
    let bindgen = tool(
        &tools,
        "wasm-bindgen-cli",
        "wasm-bindgen",
        &locked(&lock, "wasm-bindgen")?,
    )?;
    let splitter = tool(
        &tools,
        "wasm-split-cli",
        "wasm-split-cli",
        &locked(&lock, "wasm-splitter")?,
    )?;

    let pkg = app.join("pkg");
    let split = app.join("target/split");
    for dir in [&pkg, &split] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
    }
    if args.quick {
        quick(&app, &bindgen, &pkg)?;
    } else {
        split_build(&app, &bindgen, &splitter, &pkg, &split)?;
    }

    println!("✓ Built {}", pkg.display());
    match args.serve {
        Some(port) => run(cmd("python3")
            .args([
                "-I",
                "-m",
                "http.server",
                &port.to_string(),
                "--bind",
                "127.0.0.1",
                "-d",
            ])
            .arg(&app)),
        None => {
            println!("  Serve {} with any static server.", app.display());
            Ok(())
        }
    }
}

/// The steps the module's doc lists: the release build, split.
fn split_build(
    app: &Path,
    bindgen: &Path,
    splitter: &Path,
    pkg: &Path,
    split: &Path,
) -> Result<()> {
    run(cargo()
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
        ])
        .arg(app.join("Cargo.toml"))
        .args([
            "--config",
            r#"target.wasm32-unknown-unknown.rustflags=["-Clink-args=--emit-relocs"]"#,
        ]))?;
    let raw = app.join(format!("target/wasm32-unknown-unknown/release/{BIN}.wasm"));

    run(cmd(bindgen)
        .arg(&raw)
        .args([
            "--target",
            "web",
            "--no-typescript",
            "--keep-debug",
            "--keep-lld-exports",
        ])
        .args(["--no-demangle", "--out-dir"])
        .arg(pkg))?;
    let bindgened = pkg.join(format!("{BIN}_bg.wasm"));
    run(cmd(splitter)
        .arg("split")
        .arg(&raw)
        .arg(&bindgened)
        .arg(split))?;

    // The main module in wasm-bindgen's place, and the split modules beside
    // it, each without its custom sections.
    for entry in std::fs::read_dir(split)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let to = match name {
            "main.wasm" => bindgened.clone(),
            n if n.ends_with(".wasm") => pkg.join(n),
            _ => continue,
        };
        let bytes = std::fs::read(&path)?;
        std::fs::write(&to, strip_custom_sections(&bytes)?)?;
        println!(
            "  {} {:.1} MB",
            to.file_name().unwrap().to_string_lossy(),
            size(&to)?
        );
    }
    let glue = std::fs::read_to_string(split.join("__wasm_split.js"))?;
    std::fs::write(pkg.join("__wasm_split.js"), point_glue(&glue))?;

    Ok(())
}

/// One module, unsplit, from the `quick` profile: the page loads it the same
/// way, and a diagram's renderer is in it from the start.
fn quick(app: &Path, bindgen: &Path, pkg: &Path) -> Result<()> {
    run(cargo()
        .args([
            "build",
            "--profile",
            "quick",
            "--no-default-features",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
        ])
        .arg(app.join("Cargo.toml")))?;
    let raw = app.join(format!("target/wasm32-unknown-unknown/quick/{BIN}.wasm"));
    run(cmd(bindgen)
        .arg(&raw)
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(pkg))?;
    let wasm = pkg.join(format!("{BIN}_bg.wasm"));
    println!("  {BIN}_bg.wasm {:.1} MB", size(&wasm)?);
    Ok(())
}

/// The version of `package` the lockfile names. One version only: a second
/// would leave the tool's version a guess.
fn locked(lock: &str, package: &str) -> Result<String> {
    let mut versions = lock
        .split("[[package]]")
        .filter(|p| p.lines().any(|l| l == format!(r#"name = "{package}""#)))
        .filter_map(|p| p.lines().find_map(|l| l.strip_prefix(r#"version = ""#)))
        .map(|v| v.trim_end_matches('"').to_string());
    let Some(version) = versions.next() else {
        bail!("{APP}/Cargo.lock has no {package}");
    };
    if versions.next().is_some() {
        bail!("{APP}/Cargo.lock has more than one {package}");
    }
    Ok(version)
}

/// The tool `bin` from `krate` at `version`, installed into `tools/<krate>-<version>`
/// unless it is there already.
fn tool(tools: &Path, krate: &str, bin: &str, version: &str) -> Result<PathBuf> {
    let at = tools.join(format!("{krate}-{version}"));
    let path = at.join("bin").join(bin);
    if !path.is_file() {
        run(cargo()
            .args(["install", "--locked", "--quiet", krate, "--version"])
            .arg(format!("={version}"))
            .arg("--root")
            .arg(&at))?;
    }
    Ok(path)
}

/// A wasm module without its custom sections: the name section, DWARF,
/// the linker's relocations and the producers, which the splitter needed
/// and the browser does not.
fn strip_custom_sections(wasm: &[u8]) -> Result<Vec<u8>> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        bail!("not a wasm module");
    }
    let mut out = wasm[..8].to_vec();
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        let (len, read) = leb128(&wasm[at + 1..])?;
        let end = at + 1 + read + len;
        if end > wasm.len() {
            bail!("a section runs past the end of the module");
        }
        if id != 0 {
            out.extend_from_slice(&wasm[at..end]);
        }
        at = end;
    }
    Ok(out)
}

/// An unsigned LEB128 number at the start of `bytes`: its value, and how many
/// bytes it took.
fn leb128(bytes: &[u8]) -> Result<(usize, usize)> {
    let mut value = 0usize;
    for (i, b) in bytes.iter().enumerate().take(5) {
        value |= ((b & 0x7f) as usize) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }
    bail!("a section's length is not a LEB128 number")
}

/// The splitter's loader glue, as `wasm-split-cli` writes it for Dioxus's own
/// test harness, pointed at this build: the main module's glue by its name,
/// and each split module beside the glue, wherever the page is served from.
fn point_glue(glue: &str) -> String {
    let glue = glue.replace(r#"from "./main.js""#, &format!(r#"from "./{BIN}.js""#));
    glue.replace(
        r#"makeLoad("/harness/split/"#,
        r#"makeLoad(new URL(import.meta.url).href.replace(/[^/]*$/, "") + ""#,
    )
}

fn size(path: &Path) -> Result<f64> {
    Ok(std::fs::metadata(path)?.len() as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_sections_go_and_the_rest_stays() {
        // The header, a custom section "x" of two bytes, and a type section.
        let wasm = [
            0, b'a', b's', b'm', 1, 0, 0, 0, //
            0, 3, 1, b'x', 9, //
            1, 1, 0,
        ];
        assert_eq!(
            strip_custom_sections(&wasm).unwrap(),
            [0, b'a', b's', b'm', 1, 0, 0, 0, 1, 1, 0]
        );
    }

    #[test]
    fn the_glue_loads_from_beside_itself() {
        let glue = r#"import { initSync } from "./main.js";
export const a = makeLoad("/harness/split/module_0_f.wasm", [], fusedImports, initSync);"#;
        let out = point_glue(glue);
        assert!(out.contains(r#"from "./leaf_dioxus_web.js""#));
        assert!(out.contains(
            r#"makeLoad(new URL(import.meta.url).href.replace(/[^/]*$/, "") + "module_0_f.wasm""#
        ));
    }

    #[test]
    fn a_locked_version_is_read_from_its_package() {
        let lock = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"b\"\nversion = \"2.1.0\"\n";
        assert_eq!(locked(lock, "b").unwrap(), "2.1.0");
        assert!(locked(lock, "c").is_err());
    }
}
