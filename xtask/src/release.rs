//! Cutting a release, as one program.
//!
//! leaf has no release workflow — nothing publishes on a tag, and every upload
//! to crates.io is a command somebody types. That makes the mechanical half all
//! the easier to get half-right by hand: the workspace version lives in the root
//! manifest and again in every internal `{ path = …, version = … }` entry, the
//! lockfile has to follow it, the changelog's unreleased region has to be cut
//! into a released section, and the crates have to go up in an order crates.io
//! will accept. So it lives here instead:
//!
//!     cargo xtask version                 what the workspace calls itself
//!     cargo xtask bump <patch|minor|major|X.Y.Z>
//!     cargo xtask changelog [--write|--check]
//!     cargo xtask release <patch|minor|major|X.Y.Z> [--push] [--no-verify]
//!     cargo xtask publish [--list]
//!     cargo xtask release-notes [tag]
//!
//! Two steps are opt-in, and that asymmetry is the whole safety model.
//! `release` stops at the local tag unless it is given `--push`: everything
//! before the push is a commit that can be amended or thrown away. And the push
//! still publishes nothing — `publish` is its own command, run deliberately,
//! because a crates.io version number can be yanked but never reused.
//!
//! `publish` is idempotent per crate: a version already on the index is skipped
//! rather than attempted, so a run that died halfway is finished by running it
//! again.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result, bail};

use crate::ci;
use crate::util::{capture, cargo, cmd, read, require_tool, root, run, write};

/// The changelog, and the config that generates half of it.
const CHANGELOG: &str = "docs/CHANGELOG.md";
const CLIFF_CONFIG: &str = ".config/cliff.toml";

/// The generated region inside `## Unreleased`. Only the bytes between these two
/// lines are ever rewritten; a handwritten release intro lives below the end
/// marker, in the released section, where regeneration cannot reach it.
const BEGIN: &str = "<!-- git-cliff:begin — generated; edits here are overwritten -->";
const END: &str = "<!-- git-cliff:end -->";
/// What the region says when there is nothing unreleased — the normal state
/// immediately after a release.
const EMPTY_REGION: &str = "_No commits since the last tag._";

/// The branch a release is cut from.
const RELEASE_BRANCH: &str = "main";

const REPO: &str = "https://github.com/diaryx-org/leaf";

/// crates.io asks for a descriptive User-Agent and answers 403 without one.
const USER_AGENT: &str = "leaf-release (xtask; https://github.com/diaryx-org/leaf)";

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// A semver triple, which is all leaf has ever used. Pre-release and build
/// metadata are deliberately unparsed rather than silently dropped: a version
/// this cannot read is a version it must not rewrite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(text: &str) -> Result<Self> {
        let mut parts = text.trim().split('.');
        let mut next = || -> Result<u64> {
            parts
                .next()
                .and_then(|part| part.parse().ok())
                .with_context(|| format!("`{text}` is not an x.y.z version"))
        };
        let version = Version {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        match parts.next() {
            None => Ok(version),
            Some(_) => bail!("`{text}` is not an x.y.z version"),
        }
    }

    /// `patch`, `minor`, `major`, or a literal version to move to. A literal is
    /// checked against the current version rather than trusted: a release that
    /// goes backwards is a typo every time, and the version number it would
    /// spend cannot be taken back.
    fn bump(self, spec: &str) -> Result<Self> {
        match spec {
            "patch" => Ok(Version {
                patch: self.patch + 1,
                ..self
            }),
            "minor" => Ok(Version {
                minor: self.minor + 1,
                patch: 0,
                ..self
            }),
            "major" => Ok(Version {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            }),
            literal => {
                let next = Version::parse(literal)?;
                if next.ordered() <= self.ordered() {
                    bail!(
                        "{next} is not ahead of the current {self}\n\
                         hint: releases only move forward — a published version number can \
                         never be reused",
                    );
                }
                Ok(next)
            }
        }
    }

    fn ordered(self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `workspace.package.version` — the version every member inherits.
fn workspace_version() -> Result<Version> {
    let manifest = read("Cargo.toml")?;
    let line = manifest
        .lines()
        .find(|line| line.starts_with("version = \""))
        .context("no `version` in [workspace.package]")?;
    Version::parse(line.split('"').nth(1).unwrap_or_default())
}

pub fn print_version() -> Result<()> {
    println!("{}", workspace_version()?);
    Ok(())
}

// ---------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------

/// One workspace member: where its manifest lives, and what cargo calls it.
///
/// The two differ here in a way they don't in a flat workspace — `apps/leaf-tui`
/// is the crate `leaf-tui`, `apps/leaf` is the crate `leaf` — so anything that
/// reads a manifest wants the directory and anything that talks to cargo or
/// crates.io wants the name.
pub struct Member {
    pub dir: String,
    pub name: String,
}

/// The workspace members, in manifest order, with `crates/*` expanded.
///
/// `exclude`d members are deliberately not here: `apps/leaf-ios` is a standalone
/// workspace with its own lockfile and its own `[patch]` table, it is
/// `publish = false`, and its path dependencies on leaf carry no version — so a
/// release has nothing to do to it.
pub fn members() -> Result<Vec<Member>> {
    let manifest = read("Cargo.toml")?;
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("members"))
        .context("no `members` in [workspace]")?;

    let mut dirs = Vec::new();
    for entry in line.split('"').skip(1).step_by(2) {
        match entry.strip_suffix("/*") {
            None => dirs.push(entry.to_string()),
            Some(parent) => {
                let mut expanded = Vec::new();
                let listing = std::fs::read_dir(root().join(parent))
                    .with_context(|| format!("could not list workspace glob `{entry}`"))?;
                for child in listing {
                    let child = child?.file_name().to_string_lossy().into_owned();
                    let dir = format!("{parent}/{child}");
                    if root().join(&dir).join("Cargo.toml").is_file() {
                        expanded.push(dir);
                    }
                }
                expanded.sort();
                dirs.extend(expanded);
            }
        }
    }

    dirs.into_iter()
        .map(|dir| {
            let name = package_name(&dir)?;
            Ok(Member { dir, name })
        })
        .collect()
}

/// `[package] name` — read from that table specifically, because a manifest with
/// a `[[bin]]` section has a second `name = "…"` in it, and `apps/leaf-tui`'s
/// says `leaf`.
fn package_name(dir: &str) -> Result<String> {
    let manifest = read(format!("{dir}/Cargo.toml"))?;
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(name) = rest.split('"').nth(1)
        {
            return Ok(name.to_string());
        }
    }
    bail!("no `[package] name` in {dir}/Cargo.toml")
}

// ---------------------------------------------------------------------------
// Moving the version
// ---------------------------------------------------------------------------

/// Move the whole workspace to `next`, and hand back every file it wrote.
///
/// The root manifest holds the version twice over: `[workspace.package] version`,
/// and the `version = "…"` inside every internal `{ path = "…", version = "…" }`
/// entry in `[workspace.dependencies]`. Members hold it again, in the path
/// dependencies they declare directly (`apps/leaf-tui` on `leaf-ratatui`,
/// `crates/leaf-ffi` on `leaf-core`). None of it is cosmetic — that version is
/// what `cargo publish` uploads as the dependency requirement, so a stale one
/// either fails the publish (the version is not on the index yet) or, worse,
/// succeeds and ships a crate pinned to last release's siblings.
fn set_version(next: Version) -> Result<Vec<String>> {
    let members = members()?;
    let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
    let mut written = Vec::new();

    let (text, own, internal) = retarget(&read("Cargo.toml")?, next, &names);
    if own != 1 {
        bail!("expected exactly one `version = \"…\"` line in Cargo.toml, found {own}");
    }
    write("Cargo.toml", &text)?;
    written.push("Cargo.toml".to_string());
    println!("Cargo.toml -> {next} (workspace.package, and {internal} internal dependencies)");

    for member in &members {
        let path = format!("{}/Cargo.toml", member.dir);
        let current = read(&path)?;
        let (text, own, internal) = retarget(&current, next, &names);
        // Every member inherits `version.workspace = true`. One that pins its
        // own version instead would be left behind by this whole command, and
        // silently — so say so rather than move it.
        if own != 0 {
            bail!(
                "{path} sets its own `version = \"…\"`\n\
                 hint: workspace members inherit the release version with \
                 `version.workspace = true`",
            );
        }
        if text != current {
            write(&path, &text)?;
            written.push(path.clone());
            println!("{path} -> {next} ({internal} internal dependencies)");
        }
    }

    // The lockfile records the members' own versions, so it moves with them.
    // `--workspace` touches nothing else: a release is not the moment to pick up
    // a new upstream dependency.
    run(cargo().args(["update", "--workspace", "--quiet"]))?;
    written.push("Cargo.lock".to_string());
    Ok(written)
}

/// One manifest with every version moved: the bare `version = "…"` line, and the
/// version inside each path dependency on a workspace member. Returns the new
/// text and a count of each, so the caller can insist on the shape it expects.
fn retarget(text: &str, next: Version, members: &[&str]) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let (mut own, mut internal) = (0, 0);

    for line in text.lines() {
        if line.starts_with("version = \"") {
            out.push_str(&format!("version = \"{next}\""));
            own += 1;
        } else if let Some(rewritten) = retarget_path_dependency(line, next, members) {
            out.push_str(&rewritten);
            internal += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, own, internal)
}

/// `leaf-core = { path = "crates/leaf-core", version = "0.1.0" }` with the
/// version moved, or `None` if this line is not such an entry.
///
/// The key has to name a workspace member: a path dependency on something
/// *outside* the workspace — a sibling checkout of twig, say — is pinned to that
/// crate's version, not to leaf's, and moving it would be a lie.
fn retarget_path_dependency(line: &str, next: Version, members: &[&str]) -> Option<String> {
    if !line.contains("path = \"") {
        return None;
    }
    let key = line.split('=').next()?.trim().trim_matches('"');
    if !members.contains(&key) {
        return None;
    }
    let marker = "version = \"";
    let start = line.find(marker)? + marker.len();
    let end = start + line[start..].find('"')?;
    Some(format!("{}{next}{}", &line[..start], &line[end..]))
}

pub fn bump(spec: &str) -> Result<()> {
    let current = workspace_version()?;
    let next = current.bump(spec)?;
    println!("{current} -> {next}");
    set_version(next)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The changelog
// ---------------------------------------------------------------------------

/// The unreleased commits, rendered by git-cliff through `.config/cliff.toml`.
///
/// git-cliff exits non-zero when there is nothing unreleased, which is a normal
/// state right after a tag rather than a failure — hence the placeholder rather
/// than an error.
fn generated() -> Result<String> {
    require_cliff()?;
    let rendered = capture(cmd("git-cliff").args([
        "--config",
        CLIFF_CONFIG,
        "--unreleased",
        "--strip",
        "all",
    ]))
    .unwrap_or_default();
    let body = rendered.trim();
    Ok(if body.is_empty() {
        EMPTY_REGION.to_string()
    } else {
        body.to_string()
    })
}

fn require_cliff() -> Result<()> {
    require_tool(
        "git-cliff",
        "nix profile install nixpkgs#git-cliff, or cargo install git-cliff",
    )
}

/// The commits a tag covers, rendered the same way the unreleased region is.
///
/// The range starts at the previous tag, or — for the first tag ever — at the
/// repository's root commit, which is the closest thing to "before everything"
/// that git-cliff will accept as a range.
fn tagged(previous: Option<&str>, tag: &str) -> Result<String> {
    let root_commit;
    let start = match previous {
        Some(previous) => previous,
        None => {
            root_commit = capture(cmd("git").args(["rev-list", "--max-parents=0", "HEAD"]))?;
            root_commit.trim()
        }
    };
    let rendered = capture(cmd("git-cliff").args([
        "--config",
        CLIFF_CONFIG,
        "--strip",
        "all",
        &format!("{start}..{tag}"),
    ]))
    .unwrap_or_default();
    let body = rendered.trim();
    Ok(if body.is_empty() {
        "_Nothing recorded._".to_string()
    } else {
        body.to_string()
    })
}

/// Every `v*` tag, oldest first — the same pattern `.config/cliff.toml` sections
/// history by.
fn tags() -> Result<Vec<String>> {
    Ok(
        capture(cmd("git").args(["tag", "--sort=v:refname", "--list", "v[0-9]*"]))?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// Tags with no section of their own, rendered and dated, newest first.
///
/// A tag can appear after the fact — leaf's v0.1.0 names the crates that went to
/// crates.io by hand — and without this, those commits simply leave the file: no
/// longer unreleased, and in no section either. So the tag list, not the last
/// write, decides what the changelog owes.
fn missing_sections(text: &str) -> Result<Vec<String>> {
    let tags = tags()?;
    let mut sections = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        if text
            .lines()
            .any(|line| section_tag(line) == Some(tag.as_str()))
        {
            continue;
        }
        let previous = index.checked_sub(1).map(|i| tags[i].as_str());
        let body = tagged(previous, tag)?;
        let date = capture(cmd("git").args(["log", "-1", "--format=%cs", tag]))?;
        sections.push(format!("## {tag} — {}\n\n{body}\n", date.trim()));
    }
    Ok(sections)
}

/// `## v0.1.0 — 2026-08-17` -> `v0.1.0`, and anything else -> `None`.
fn section_tag(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    let tag = rest.split_whitespace().next()?;
    tag.strip_prefix('v')
        .filter(|version| version.starts_with(|c: char| c.is_ascii_digit()))
        .map(|_| tag)
}

/// Put sections in their place: newest first, and an older one folded in above
/// the first section it outranks rather than appended to the end.
fn insert_sections(text: &str, sections: Vec<String>) -> String {
    if sections.is_empty() {
        return text.to_string();
    }
    let order = |tag: &str| Version::parse(tag.trim_start_matches('v')).ok();

    let mut out = String::with_capacity(text.len());
    let mut pending: Vec<String> = sections;
    for line in text.lines() {
        if let Some(tag) = section_tag(line)
            && let Some(here) = order(tag)
        {
            // Everything newer than the section starting on this line goes in
            // ahead of it.
            let (newer, rest): (Vec<String>, Vec<String>) = pending.into_iter().partition(|s| {
                section_tag(s.lines().next().unwrap_or_default())
                    .and_then(order)
                    .is_some_and(|candidate| candidate.ordered() > here.ordered())
            });
            for section in newer {
                blank_line(&mut out);
                out.push_str(&section);
                out.push('\n');
            }
            pending = rest;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Whatever is older than every section already in the file lands at the end.
    for section in pending {
        blank_line(&mut out);
        out.push_str(&section);
        out.push('\n');
    }
    out
}

/// A blank line before a `##` heading, so a section spliced in against the end
/// marker (or against another section) is still a heading when the file is read
/// as Markdown rather than a line of text after a comment.
fn blank_line(out: &mut String) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn region(body: &str) -> String {
    format!("{BEGIN}\n\n{body}\n\n{END}")
}

/// Replace the marked region, and optionally drop a fresh released section in
/// immediately below it. Everything above `## Unreleased` and every released
/// section below is left byte-for-byte alone.
fn rewrite(text: &str, body: &str, released: Option<&str>) -> Result<String> {
    for marker in [BEGIN, END] {
        if !text.lines().any(|line| line == marker) {
            bail!("marker not found in {CHANGELOG}:\n  {marker}");
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.lines() {
        if line == BEGIN {
            out.push_str(&region(body));
            out.push('\n');
            skipping = true;
        } else if line == END {
            skipping = false;
            if let Some(section) = released {
                out.push('\n');
                out.push_str(section);
                out.push('\n');
            }
        } else if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

#[derive(clap::Args)]
pub struct ChangelogArgs {
    /// Splice the regenerated region back into docs/CHANGELOG.md.
    #[arg(long, conflicts_with = "check")]
    write: bool,

    /// Fail if the file on disk is not what regenerating would produce.
    #[arg(long)]
    check: bool,
}

/// `cargo xtask changelog [--write|--check]` — print, splice, or verify.
pub fn changelog(args: ChangelogArgs) -> Result<()> {
    let body = generated()?;
    if !args.write && !args.check {
        println!("{}", region(&body));
        return Ok(());
    }

    let current = read(CHANGELOG)?;
    // Two jobs, because a tag can arrive after the commits it names: refresh the
    // unreleased region, and give any tag still without a section one.
    let missing = missing_sections(&current)?;
    let named: Vec<String> = missing
        .iter()
        .filter_map(|s| section_tag(s.lines().next().unwrap_or_default()).map(str::to_owned))
        .collect();
    let spliced = insert_sections(&rewrite(&current, &body, None)?, missing);

    if args.check {
        if !named.is_empty() {
            bail!(
                "{CHANGELOG} has no section for {}\nhint: run `cargo xtask changelog --write`",
                named.join(", "),
            );
        }
        if spliced != current {
            bail!(
                "{CHANGELOG}'s generated region is stale\n\
                 hint: run `cargo xtask changelog --write`",
            );
        }
        println!("{CHANGELOG}'s generated region is up to date");
        return Ok(());
    }

    write(CHANGELOG, &spliced)?;
    for tag in &named {
        println!("{CHANGELOG} -> new section `## {tag}`");
    }
    println!("wrote {CHANGELOG}");
    Ok(())
}

#[derive(clap::Args)]
pub struct NotesArgs {
    /// The tag to read, defaulting to the workspace's current version.
    tag: Option<String>,
}

/// One release's section of the changelog, without its heading — a release body
/// to paste into `gh release create`.
///
/// Read from the changelog rather than rendered from the commits, and
/// deliberately: the tag's tree already carries the section `release` cut, so the
/// notes on the release page are byte-identical to the ones the repository ships,
/// and producing them needs no git-cliff.
pub fn release_notes(args: NotesArgs) -> Result<()> {
    let tag = match args.tag {
        Some(tag) => tag,
        None => format!("v{}", workspace_version()?),
    };
    let changelog = read(CHANGELOG)?;
    let body = section(&changelog, &tag).with_context(|| {
        format!(
            "{CHANGELOG} has no section for {tag}\n\
             hint: `cargo xtask changelog --write`, commit, and re-run — a tag whose \
             changelog section is missing is a tag cut without one"
        )
    })?;

    println!("{body}");
    println!(
        "\n---\n\n\
         Every change in this release and the ones before it: \
         [docs/CHANGELOG.md]({REPO}/blob/{tag}/docs/CHANGELOG.md)."
    );
    Ok(())
}

/// The lines under `## <tag> — …`, up to the next `##` heading. A `###` group
/// heading is part of the section, so the search is for that heading level
/// exactly — and so a handwritten release intro, which sits directly under the
/// heading, comes along with the generated groups.
fn section(changelog: &str, tag: &str) -> Option<String> {
    let mut lines = changelog
        .lines()
        .skip_while(|line| section_tag(line) != Some(tag));
    lines.next()?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    Some(body.join("\n").trim().to_string())
}

/// Turn the unreleased region into a released section headed `## vX.Y.Z — date`,
/// and reset the region. Called by `release`, between the version bump and the
/// commit, so the release commit carries both.
fn cut_changelog(version: Version) -> Result<()> {
    let body = generated()?;
    let date = capture(cmd("date").arg("+%Y-%m-%d"))?.trim().to_string();
    let released = format!("## v{version} — {date}\n\n{body}\n");
    let current = read(CHANGELOG)?;
    let cut = rewrite(&current, EMPTY_REGION, Some(&released))?;
    write(CHANGELOG, &cut)?;
    println!("{CHANGELOG} -> new section `## v{version} — {date}`");
    Ok(())
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// What one member's manifest says about its place in the workspace.
#[derive(Default)]
struct Survey {
    /// Every dependency it names, in any dependency table.
    deps: Vec<String>,
    /// The ones whose source is a git remote.
    git: Vec<String>,
    /// `publish = false`.
    opted_out: bool,
}

/// Read a manifest for the three things publishing turns on: what it depends on,
/// what of that comes from a git remote, and whether it has opted out.
///
/// Dev-dependencies count as dependencies here: cargo verifies a published crate
/// by building it, tests and all, so a dev-dependency on a sibling has to be on
/// the index just as much as a real one.
fn survey(manifest: &str, workspace_git: &[String]) -> Survey {
    let mut found = Survey::default();
    let mut in_dependencies = false;
    let mut header_dep: Option<String> = None;

    for line in manifest.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let segments: Vec<&str> = header.split('.').collect();
            let table = |s: &str| s.ends_with("dependencies");
            in_dependencies = segments.last().is_some_and(|s| table(s));
            // `[dependencies.leaf-core]` — the dependency is named by the header
            // itself, and the lines under it are its fields.
            header_dep = segments
                .iter()
                .position(|s| table(s))
                .and_then(|position| segments.get(position + 1))
                .map(|name| name.trim_matches('"').to_string());
            if let Some(name) = &header_dep {
                found.deps.push(name.clone());
            }
            continue;
        }
        if line.starts_with("publish") && line.contains("false") {
            found.opted_out = true;
        }
        if !in_dependencies {
            continue;
        }
        // A field of the `[dependencies.foo]` table above, rather than a
        // dependency of its own.
        if let Some(name) = &header_dep {
            if line.contains("git = \"") {
                found.git.push(name.clone());
            }
            continue;
        }
        // `leaf-core = { workspace = true }` and `leaf-core.workspace = true`
        // are the same dependency written two ways.
        let Some(key) = line.split('=').next() else {
            continue;
        };
        let name = key.trim().trim_matches('"');
        let name = name.strip_suffix(".workspace").unwrap_or(name);
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        found.deps.push(name.to_string());
        // Either the git source is right here, or it is in the workspace table
        // this line inherits from.
        if line.contains("git = \"") || workspace_git.iter().any(|dep| dep == name) {
            found.git.push(name.to_string());
        }
    }

    found.deps.sort();
    found.deps.dedup();
    found.git.sort();
    found.git.dedup();
    found
}

/// The `[workspace.dependencies]` entries that come from a git remote. A member
/// spelling one `foo.workspace = true` inherits the git source with it, and with
/// it the reason it can never be published.
fn workspace_git_dependencies(manifest: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_table = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[workspace.dependencies]";
            continue;
        }
        if in_table
            && line.contains("git = \"")
            && let Some(key) = line.split('=').next()
        {
            found.push(key.trim().trim_matches('"').to_string());
        }
    }
    found
}

/// What a release can and cannot upload.
pub struct Plan {
    /// The publishable members, ordered so that no crate goes up before
    /// something it depends on. crates.io enforces that — an upload whose
    /// dependencies are not yet on the index is rejected — and the order is
    /// derived rather than written down, so adding a crate is enough.
    order: Vec<String>,
    /// The members that never go to crates.io, each with the reason, so a short
    /// list is read as a decision rather than as something missing.
    blocked: Vec<(String, String)>,
}

/// Work out both halves of the plan from the manifests.
///
/// Three things keep a crate off crates.io, and leaf has all three: `publish =
/// false` (`leaf-wasm`), a dependency from a git remote (`leaf-gpui` and `leaf`
/// pin gpui to a Zed commit, and crates.io rejects any upload carrying one), and
/// depending on a crate blocked for either reason. The last is why this is a
/// fixpoint rather than a filter.
fn plan() -> Result<Plan> {
    let workspace_git = workspace_git_dependencies(&read("Cargo.toml")?);
    let members = members()?;
    let mut surveys: BTreeMap<String, Survey> = BTreeMap::new();
    for member in &members {
        let manifest = read(format!("{}/Cargo.toml", member.dir))?;
        surveys.insert(member.name.clone(), survey(&manifest, &workspace_git));
    }

    let mut blocked: BTreeMap<String, String> = BTreeMap::new();
    for (name, survey) in &surveys {
        if survey.opted_out {
            blocked.insert(name.clone(), "`publish = false`".to_string());
        } else if let Some(dep) = survey.git.first() {
            blocked.insert(
                name.clone(),
                format!("depends on `{dep}` from a git remote, which crates.io will not accept"),
            );
        }
    }

    // A crate is only as publishable as the crates under it, so the reasons have
    // to propagate up before the order is taken.
    loop {
        let mut grew = false;
        for (name, survey) in &surveys {
            if blocked.contains_key(name) {
                continue;
            }
            if let Some(dep) = survey
                .deps
                .iter()
                .find(|dep| blocked.contains_key(*dep) && surveys.contains_key(*dep))
            {
                blocked.insert(
                    name.clone(),
                    format!("depends on `{dep}`, which is not published"),
                );
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    // Depth-first over the dependency edges, so a crate is pushed only after
    // everything it needs is already on the list.
    fn visit(
        name: &str,
        surveys: &BTreeMap<String, Survey>,
        blocked: &BTreeMap<String, String>,
        order: &mut Vec<String>,
        open: &mut Vec<String>,
    ) -> Result<()> {
        if order.iter().any(|done| done == name) {
            return Ok(());
        }
        if open.iter().any(|pending| pending == name) {
            bail!("dependency cycle through `{name}`: {}", open.join(" -> "));
        }
        open.push(name.to_string());
        let survey = &surveys[name];
        for dep in &survey.deps {
            if surveys.contains_key(dep) {
                visit(dep, surveys, blocked, order, open)?;
            }
        }
        open.pop();
        if !blocked.contains_key(name) {
            order.push(name.to_string());
        }
        Ok(())
    }

    let mut order = Vec::new();
    let mut open = Vec::new();
    for member in &members {
        visit(&member.name, &surveys, &blocked, &mut order, &mut open)?;
    }

    Ok(Plan {
        order,
        blocked: blocked.into_iter().collect(),
    })
}

impl Plan {
    fn print(&self, version: Version) {
        println!(
            "publishing {} crates at {version}, in order:",
            self.order.len()
        );
        for (n, name) in self.order.iter().enumerate() {
            println!("  {}. {name}", n + 1);
        }
        if !self.blocked.is_empty() {
            println!("\nnot published:");
            for (name, why) in &self.blocked {
                println!("  {name} — {why}");
            }
        }
    }
}

/// Is this exact version already on crates.io?
///
/// `GET /api/v1/crates/<crate>/<version>` is 200 only when it is (a yanked
/// version also 200s — correct to skip, since a version number can never be
/// reused). Anything else — 404 for a new version, 404 for a crate nobody has
/// ever published — means "go publish".
fn already_published(name: &str, version: Version) -> Result<bool> {
    let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
    let code = capture(cmd("curl").args([
        "-sS",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "-A",
        USER_AGENT,
        &url,
    ]))?;
    Ok(code.trim() == "200")
}

#[derive(clap::Args)]
pub struct PublishArgs {
    /// Print what would be published, and in what order, without publishing.
    #[arg(long)]
    list: bool,
}

/// `cargo xtask publish [--list]` — the upload, run by hand.
///
/// Idempotent per crate: each version already on crates.io is skipped rather than
/// attempted, so re-running after a partial release publishes exactly the crates
/// that are missing.
pub fn publish(args: PublishArgs) -> Result<()> {
    let version = workspace_version()?;
    let plan = plan()?;
    plan.print(version);

    if args.list {
        return Ok(());
    }

    for name in &plan.order {
        if already_published(name, version)? {
            println!("\n✅ {name} {version} already on crates.io — skipping");
            continue;
        }
        println!("\n📦 publishing {name} {version}");
        // No manual wait between crates: cargo blocks until a freshly published
        // version is visible on the index before it returns, which is exactly
        // what the next crate in the order needs.
        run(cargo().args(["publish", "-p", name])).with_context(|| {
            // The one failure worth naming, because the message crates.io sends
            // does not name its own cause: a token that cannot reach this crate.
            // A crates.io token can be scoped to particular crates, and
            // publishing a crate for the first time needs `publish-new` where
            // updating an existing one needs only `publish-update`.
            format!(
                "if that was `403 … token is not valid for crate {name}`, the token in use \
                 cannot publish it: either it is scoped to a subset of the crates that leaves \
                 {name} out, or {name} is new to crates.io and the token lacks `publish-new`.\n\
                 Issue one at crates.io -> Account Settings -> API Tokens with both scopes and \
                 no crate restriction. Re-running picks up where this stopped."
            )
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Releasing
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct ReleaseArgs {
    /// patch, minor, major, or a literal X.Y.Z to move to.
    spec: String,

    /// Push the branch and the tag once everything else is done.
    #[arg(long)]
    push: bool,

    /// Skip the checks `cargo xtask ci` runs.
    #[arg(long)]
    no_verify: bool,
}

/// `cargo xtask release <patch|minor|major|X.Y.Z> [--push] [--no-verify]`.
///
/// Check, bump, regenerate, commit, tag — and push only when asked. Even the
/// push publishes nothing: leaf has no release workflow, so crates.io waits for
/// `cargo xtask publish`. The tag's job is to say which commit a version means.
pub fn release(args: ReleaseArgs) -> Result<()> {
    let current = workspace_version()?;
    let next = current.bump(&args.spec)?;
    let tag = format!("v{next}");

    // Everything that can say "no" says it before anything is written. A
    // half-applied release is a working tree to untangle by hand, and the whole
    // point of this command is not doing that.
    preflight(next, &tag)?;

    if args.no_verify {
        println!("skipping the checks (--no-verify)");
    } else {
        ci::run_all()?;
    }

    println!("\n\x1b[1m━━ {current} -> {next} ━━\x1b[0m");
    let mut touched = set_version(next)?;
    cut_changelog(next)?;
    touched.push(CHANGELOG.to_string());

    // Only the files a release moves, named explicitly: whatever else is in the
    // tree stays out of the release commit.
    run(cmd("git").arg("add").args(&touched))?;
    run(cmd("git").args(["commit", "-m", &format!("chore: bump to {next}")]))?;
    // Annotated, like every tag before it — `git describe` wants an object to
    // read, and a release tag is worth a message of its own.
    run(cmd("git").args(["tag", "-a", &tag, "-m", &tag]))?;

    let branch = capture(cmd("git").args(["rev-parse", "--abbrev-ref", "HEAD"]))?
        .trim()
        .to_string();
    let crates = plan()?.order.len();

    if !args.push {
        println!(
            "\n\x1b[32m{tag} is committed and tagged locally.\x1b[0m\n\n\
             Nothing has left this machine. To publish this release:\n\n    \
             git push origin {branch}\n    \
             git push origin {tag}\n    \
             cargo xtask publish\n\n\
             The tag publishes nothing by itself — leaf has no release workflow. The third \
             command is the\nirreversible one: it uploads {crates} crates to crates.io, where a \
             version number can be\nyanked but never reused.\n\n\
             To undo locally instead: git tag -d {tag} && git reset --hard HEAD~1\n",
        );
        return Ok(());
    }

    run(cmd("git").args(["push", "origin", &branch]))?;
    run(cmd("git").args(["push", "origin", &tag]))?;
    println!(
        "\n\x1b[32m{tag} pushed.\x1b[0m Nothing is published yet: `cargo xtask publish` \
         uploads the {crates}\ncrates, and that step is still yours to run.\n",
    );
    Ok(())
}

/// Refuse a release that is already doomed: dirty tree, wrong branch, a tag that
/// exists, a version crates.io has already seen, a changelog with nowhere to
/// write, or no git-cliff to write it with.
fn preflight(next: Version, tag: &str) -> Result<()> {
    require_cliff()?;

    // `cut_changelog` runs after the version has been written to a dozen
    // manifests; a changelog it cannot splice into has to be found before that,
    // not after.
    let changelog = read(CHANGELOG)?;
    for marker in [BEGIN, END] {
        if !changelog.lines().any(|line| line == marker) {
            bail!("marker not found in {CHANGELOG}:\n  {marker}");
        }
    }

    if !capture(cmd("git").args(["status", "--porcelain"]))?
        .trim()
        .is_empty()
    {
        bail!(
            "the working tree is dirty — commit or stash first, so the release commit holds \
             only the version bump and the changelog",
        );
    }

    let branch = capture(cmd("git").args(["rev-parse", "--abbrev-ref", "HEAD"]))?;
    if branch.trim() != RELEASE_BRANCH {
        bail!(
            "on branch `{}`, and leaf releases from `{RELEASE_BRANCH}`",
            branch.trim(),
        );
    }

    if !capture(cmd("git").args(["tag", "--list", tag]))?
        .trim()
        .is_empty()
    {
        bail!("tag {tag} already exists locally");
    }
    if !capture(cmd("git").args(["ls-remote", "--tags", "origin", tag]))?
        .trim()
        .is_empty()
    {
        bail!("tag {tag} already exists on origin");
    }

    // The tag is not the only way a version gets spent — leaf-core and leaf-ffi
    // 0.1.0 went to crates.io from a laptop before there was a tag — so ask the
    // registry rather than the tag list.
    for name in plan()?.order {
        if already_published(&name, next)? {
            bail!(
                "{name} {next} is already on crates.io\n\
                 hint: a published version number can never be reused; release {} instead",
                Version {
                    patch: next.patch + 1,
                    ..next
                },
            );
        }
    }

    // A release cut on a stale main is a release missing commits. Fetch is
    // advisory — a laptop offline enough to fail it can still cut the local
    // commit and push later.
    if capture(cmd("git").args(["fetch", "--quiet", "origin", RELEASE_BRANCH])).is_ok() {
        let behind = capture(cmd("git").args([
            "rev-list",
            "--count",
            &format!("HEAD..origin/{RELEASE_BRANCH}"),
        ]))?;
        if behind.trim() != "0" {
            bail!(
                "{RELEASE_BRANCH} is {} commits behind origin/{RELEASE_BRANCH} — pull first",
                behind.trim(),
            );
        }
    } else {
        println!("warning: could not reach origin; releasing against the local {RELEASE_BRANCH}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_move_forward_only() {
        let current = Version::parse("0.1.0").unwrap();
        assert_eq!(current.bump("patch").unwrap().to_string(), "0.1.1");
        assert_eq!(current.bump("minor").unwrap().to_string(), "0.2.0");
        assert_eq!(current.bump("major").unwrap().to_string(), "1.0.0");
        assert_eq!(current.bump("0.9.3").unwrap().to_string(), "0.9.3");
        assert!(current.bump("0.0.9").is_err(), "a release cannot go back");
        assert!(current.bump("0.1.0").is_err(), "nor stand still");
        assert!(current.bump("0.1").is_err());
        assert!(
            current.bump("0.1.0-rc.1").is_err(),
            "unparsed, not truncated"
        );
    }

    /// The internal dependency rewrite is the half of the bump that nothing
    /// would notice going wrong until a consumer resolved last release's
    /// siblings.
    #[test]
    fn path_dependencies_follow_the_workspace_version() {
        let next = Version::parse("0.2.0").unwrap();
        let members = ["leaf-core", "leaf-ratatui"];
        assert_eq!(
            retarget_path_dependency(
                r#"leaf-core = { path = "crates/leaf-core", version = "0.1.0", default-features = false }"#,
                next,
                &members,
            )
            .unwrap(),
            r#"leaf-core = { path = "crates/leaf-core", version = "0.2.0", default-features = false }"#
        );
        // A path dependency on something outside the workspace is pinned to that
        // crate's version, not to leaf's.
        assert_eq!(
            retarget_path_dependency(
                r#"twig-doc = { path = "../twig/bindings/rust", version = "3.2" }"#,
                next,
                &members,
            ),
            None,
        );
        // No path, and no version, are both "not this kind of line".
        assert_eq!(
            retarget_path_dependency(r#"leaf-core = { version = "0.1.0" }"#, next, &members),
            None,
        );
        assert_eq!(
            retarget_path_dependency(
                r#"leaf-core = { path = "../leaf-core", default-features = false }"#,
                next,
                &members,
            ),
            None,
        );
        assert_eq!(
            retarget_path_dependency(r#"version = "0.1.0""#, next, &members),
            None,
        );
    }

    /// The workspace version moves in the root manifest and nowhere else; a
    /// member's `version.workspace = true` must survive untouched.
    #[test]
    fn only_the_root_manifest_carries_its_own_version() {
        let next = Version::parse("0.2.0").unwrap();
        let (text, own, internal) = retarget(
            "[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\n\
             leaf-core = { path = \"crates/leaf-core\", version = \"0.1.0\" }\n",
            next,
            &["leaf-core"],
        );
        assert_eq!((own, internal), (1, 1));
        assert_eq!(text.matches("0.2.0").count(), 2);

        let member = "[package]\nname = \"leaf-tui\"\nversion.workspace = true\n";
        let (text, own, internal) = retarget(member, next, &["leaf-core"]);
        assert_eq!((own, internal), (0, 0));
        assert_eq!(text, member);
    }

    /// `apps/leaf-tui` is the crate `leaf-tui` and ships a binary called `leaf`;
    /// reading the first `name = ` in that manifest would get the binary.
    #[test]
    fn a_member_is_named_by_its_package_table() {
        let names: Vec<String> = members().unwrap().into_iter().map(|m| m.name).collect();
        for expected in ["leaf-core", "leaf-ffi", "leaf-ratatui", "leaf-tui", "leaf"] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        // `crates/*` is a glob in the manifest, so nothing here is written down.
        assert!(names.len() >= 7, "{names:?}");
    }

    /// Every spelling of a dependency the manifests actually use, and the git
    /// source that keeps a crate off crates.io however it is written.
    #[test]
    fn a_survey_finds_dependencies_and_their_sources() {
        let manifest = r#"
[package]
name = "leaf-gpui"

[dependencies]
leaf-core.workspace = true
anyhow = { workspace = true }
gpui = { git = "https://github.com/zed-industries/zed", rev = "bd72919" }

[dev-dependencies.leaf-ratatui]
path = "../leaf-ratatui"
"#;
        let found = survey(manifest, &[]);
        assert_eq!(
            found.deps,
            ["anyhow", "gpui", "leaf-core", "leaf-ratatui"]
                .map(str::to_string)
                .to_vec(),
        );
        assert_eq!(found.git, ["gpui"]);
        assert!(!found.opted_out);

        // A git source inherited from `[workspace.dependencies]` blocks the
        // crate just as surely as one written here.
        assert_eq!(
            survey(manifest, &["anyhow".to_string()]).git,
            ["anyhow", "gpui"]
        );
        assert!(survey("[package]\npublish = false\n", &[]).opted_out);
    }

    #[test]
    fn workspace_git_dependencies_are_read_from_their_own_table() {
        let manifest = "[workspace.dependencies]\n\
                        twig-doc = \"3.2\"\n\
                        gpui = { git = \"https://github.com/zed-industries/zed\" }\n\
                        \n[patch.crates-io]\n\
                        other = { git = \"https://example.invalid/other\" }\n";
        assert_eq!(workspace_git_dependencies(manifest), ["gpui"]);
    }

    /// The real workspace: every crate after the ones it depends on, and every
    /// crate that cannot go to crates.io held back with a reason rather than
    /// dropped.
    #[test]
    fn the_plan_orders_what_it_can_and_explains_what_it_cannot() {
        let plan = plan().unwrap();

        for name in ["leaf-core", "leaf-ffi", "leaf-ratatui", "leaf-tui"] {
            assert!(plan.order.contains(&name.to_string()), "{name} unpublished");
        }
        let position = |name: &str| plan.order.iter().position(|c| c == name);
        assert!(position("leaf-core") < position("leaf-ffi"));
        assert!(position("leaf-ratatui") < position("leaf-tui"));

        // gpui is pinned to a Zed commit, so neither the widget nor the app it
        // backs can be uploaded; xtask and the wasm binding opt out.
        for name in ["leaf-gpui", "leaf", "leaf-wasm", "xtask"] {
            assert!(
                plan.blocked.iter().any(|(blocked, _)| blocked == name),
                "{name} would be published",
            );
            assert!(!plan.order.contains(&name.to_string()));
        }
        assert!(plan.blocked.iter().all(|(_, why)| !why.is_empty()));
    }

    /// The splice touches the region and nothing else — not the prose above it,
    /// and not a single released section below.
    #[test]
    fn rewriting_leaves_everything_outside_the_region_alone() {
        let text = format!(
            "# Changelog\n\nprose\n\n## Unreleased\n\n{BEGIN}\n\nold\n\n{END}\n\n\
             ## v0.1.0 — 2026-08-17\n\nkept\n"
        );
        let refreshed = rewrite(&text, "new", None).unwrap();
        assert!(refreshed.contains("\nnew\n") && !refreshed.contains("\nold\n"));
        assert!(refreshed.contains("# Changelog\n\nprose"));
        assert!(refreshed.ends_with("## v0.1.0 — 2026-08-17\n\nkept\n"));

        let cut = rewrite(&text, EMPTY_REGION, Some("## v0.2.0 — 2026-08-19\n\nnew\n")).unwrap();
        let released = cut.find("## v0.2.0").unwrap();
        assert!(
            released > cut.find(END).unwrap(),
            "released sections go below the region",
        );
        assert!(
            released < cut.find("## v0.1.0").unwrap(),
            "newest release first",
        );
        assert!(cut.contains(EMPTY_REGION));
    }

    /// A tag section is recognised by its heading and nothing else, so that a
    /// handwritten intro under one is never mistaken for a heading — and a
    /// heading that is prose (`## Unreleased`) is never mistaken for a tag.
    #[test]
    fn tag_headings_are_told_from_prose_ones() {
        assert_eq!(section_tag("## v0.1.0 — 2026-08-17"), Some("v0.1.0"));
        assert_eq!(section_tag("## v0.1.0"), Some("v0.1.0"));
        assert_eq!(section_tag("## Unreleased"), None);
        assert_eq!(section_tag("## verify the build"), None);
        assert_eq!(section_tag("### Added"), None);
        assert_eq!(section_tag("- **core** — ## v1.0.0 in a bullet"), None);
    }

    /// A tag can be cut long after the commits it names, so a backfilled section
    /// has to land in version order rather than on top.
    #[test]
    fn backfilled_sections_land_in_version_order() {
        let text = "## v0.6.0 — c\n\nsix\n\n## v0.4.0 — a\n\nfour\n";
        let merged = insert_sections(
            text,
            vec![
                "## v0.5.0 — b\n\nfive\n".to_string(),
                "## v0.3.0 — z\n\nthree\n".to_string(),
            ],
        );
        let order: Vec<&str> = merged.lines().filter_map(section_tag).collect();
        assert_eq!(order, ["v0.6.0", "v0.5.0", "v0.4.0", "v0.3.0"]);
        // Nothing already in the file is rewritten on the way past.
        for kept in ["six", "four"] {
            assert_eq!(merged.matches(kept).count(), 1);
        }
    }

    #[test]
    fn nothing_missing_leaves_the_file_untouched() {
        let text = "## v0.6.0 — c\n\nsix\n";
        assert_eq!(insert_sections(text, vec![]), text);
    }

    /// The release body is a slice of the changelog, so the slice has to end
    /// where the next release begins — and not at a group heading inside it.
    #[test]
    fn a_release_section_stops_at_the_next_release() {
        let changelog = "\
# Changelog

## Unreleased

nothing

## v0.2.0 — 2026-08-19

An intro, handwritten.

### Added

- a thing

## v0.1.0 — 2026-08-17

### Added

- an older thing
";
        let body = section(changelog, "v0.2.0").unwrap();
        assert!(body.starts_with("An intro, handwritten."), "{body}");
        assert!(body.contains("### Added") && body.contains("- a thing"));
        assert!(
            !body.contains("older"),
            "the next release is not part of it"
        );
        assert!(!body.contains("## v0.1.0"));

        assert_eq!(
            section(changelog, "v0.1.0").unwrap(),
            "### Added\n\n- an older thing",
        );
        assert_eq!(section(changelog, "v9.9.9"), None, "a tag with no section");
    }

    #[test]
    fn rewriting_a_changelog_without_markers_is_an_error() {
        assert!(rewrite("# Changelog\n", "new", None).is_err());
    }
}
