//! The templates a document can be set in: twig-export's three, and any
//! installed in the app's folder of packages ([`packages`]), each a Typst
//! package (`twig_export::templates`).
//!
//! A template is installed from Typst Universe by its spec ([`install`]):
//! the package, and every package it imports, fetched once, into the
//! folder; a Universe template, made one twig-export can call by an adapter
//! twig-export writes from its starter; then checked, by setting a sample
//! in each of its layouts. Only one that sets is listed. After that, a
//! document is set offline, the same way every time.
//!
//! A template is someone else's code, run over the reader's document.
//! Typst keeps a package to its own files, so it reads nothing of the
//! reader's; and a template not twig-export's own is set in another
//! process (`typeset.rs`), which a template that loops or eats memory
//! takes down without taking the app.

use std::path::PathBuf;
use std::sync::RwLock;

use twig_export::templates::{self as tx, Spec};

/// A template, as a menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What a setting stores: a built-in's name, or a package's spec.
    pub key: String,
    pub label: String,
    /// Whether it sets a reflowed page; one that does not is reflowed in
    /// Classic.
    pub reflows: bool,
    /// Whether it is twig-export's own, set in the app's process.
    pub builtin: bool,
}

/// The template a reflowed view falls back to.
pub const FALLBACK: &str = "classic";

/// The folder templates are installed in, laid out as Typst's own: the
/// app's data folder's `packages`.
pub fn packages() -> PathBuf {
    data_dir().join("packages")
}

#[cfg(target_os = "macos")]
fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/Leaf")
}

#[cfg(not(target_os = "macos"))]
fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_default()
        .join("leaf")
}

static INSTALLED: RwLock<Option<Vec<Entry>>> = RwLock::new(None);

/// Every template: the built-ins, then those installed, by label.
pub fn all() -> Vec<Entry> {
    let mut out: Vec<Entry> = tx::BUILTIN
        .iter()
        .map(|b| {
            let m = b.manifest();
            Entry {
                key: b.name.into(),
                label: m.twig().map_or(b.name.into(), |t| t.label.clone()),
                reflows: m.sets("reflow"),
                builtin: true,
            }
        })
        .collect();
    out.extend(installed());
    out
}

/// The templates installed, read from the folder the first time asked.
fn installed() -> Vec<Entry> {
    if let Some(found) = INSTALLED.read().ok().and_then(|g| g.clone()) {
        return found;
    }
    let mut found: Vec<Entry> = tx::installed(&packages())
        .into_iter()
        .filter_map(|t| {
            let twig = t.manifest.twig()?;
            Some(Entry {
                key: t.spec.to_string(),
                label: twig.label.clone(),
                reflows: t.manifest.sets("reflow"),
                builtin: false,
            })
        })
        .collect();
    found.sort_by(|a, b| a.label.cmp(&b.label).then(a.key.cmp(&b.key)));
    if let Ok(mut g) = INSTALLED.write() {
        *g = Some(found.clone());
    }
    found
}

/// Read the folder again, after an install.
pub fn reload() {
    if let Ok(mut g) = INSTALLED.write() {
        *g = None;
    }
    crate::menus::changed();
}

/// The template a setting names.
pub fn entry(key: &str) -> Option<Entry> {
    all().into_iter().find(|e| e.key == key)
}

/// Whether `key` names a template that can be: a built-in, or a spec.
pub fn valid(key: &str) -> bool {
    tx::spec(key).is_some()
}

/// What the reader gives to install a template: a spec (`@preview/name:1.0.0`),
/// or a bare Universe name with its version (`name:1.0.0`).
pub fn spec_of(text: &str) -> Option<Spec> {
    let text = text.trim();
    Spec::parse(text).or_else(|| Spec::parse(&format!("@preview/{text}")))
}

/// Install the template `spec`: fetch it and what it imports, adapt it if
/// it is a Universe template, and check it sets. Answers the key a setting
/// names it by. Blocks on the network: call it off the UI's thread.
pub fn install(spec: &Spec) -> Result<String, String> {
    let dir = packages();
    std::fs::create_dir_all(&dir).map_err(|e| format!("making {}: {e}", dir.display()))?;
    twig_export::install::install(spec, &dir, Fetch::new()?).map_err(|e| format!("{e:#}"))?;
    let at = spec.dir_in(&dir);
    let toml = std::fs::read_to_string(at.join("typst.toml")).map_err(|e| e.to_string())?;
    let manifest = tx::Manifest::parse(&toml).map_err(|e| format!("{e:#}"))?;
    // A package that is a twig-export template already is one as it is; a
    // Universe template is called through an adapter.
    let (key, layouts) = match manifest.twig() {
        Some(t) => (spec.to_string(), t.layouts.clone()),
        None => {
            let adapter = twig_export::adapt::adapt(spec, &at).map_err(|e| format!("{e:#}"))?;
            adapter.write(&dir).map_err(|e| format!("{e:#}"))?;
            (adapter.spec.to_string(), vec!["paged".to_owned()])
        }
    };
    let checked = crate::typeset::check(&key, &layouts);
    if let Err(e) = checked {
        if key != spec.to_string() {
            // The adapter is ours to take back; the package stays fetched.
            if let Some(s) = Spec::parse(&key) {
                let _ = std::fs::remove_dir_all(s.dir_in(&dir));
            }
        }
        return Err(format!("{key} does not set a sample document:\n{e}"));
    }
    reload();
    Ok(key)
}

/// View ▸ Template ▸ Install Template…: ask for a template's name and
/// version, install it off the UI's thread, and set the document in it.
pub fn prompt_install(page: &crate::page::Page) {
    use unit::{Answer, Dialog};
    let Some(unit) = page.unit() else { return };
    let id = page.window.id();
    let dialog = Dialog::new("Install Template")
        .message(
            "A template from Typst Universe, by its name and version as \
             typst.app/universe shows them: charged-ieee:0.1.4. It is \
             fetched once, with the packages it uses, and checked before \
             it is listed.",
        )
        .field("", "name:1.0.0")
        .buttons("Install", "Cancel");
    page.in_scope(|| {
        unit.open_dialog(dialog, move |answer| {
            let Answer::Ok(text) = answer else { return };
            let Some(spec) = spec_of(&text) else {
                let text = text.trim().to_owned();
                crate::app::on_turn(move || {
                    alert(
                        id,
                        "Not a Template",
                        format!("“{text}” is not a template's name and version, as name:1.0.0."),
                    )
                });
                return;
            };
            let token = crate::app::park(move |done: Result<String, String>| match done {
                Ok(key) => {
                    let label = entry(&key).map_or(key.clone(), |e| e.label);
                    if let Some(template) = crate::settings::Template::parse(&key) {
                        crate::app::set_settings(crate::settings::Settings {
                            template,
                            ..crate::app::settings()
                        });
                    }
                    crate::platform::menus_changed();
                    alert(
                        id,
                        "Template Installed",
                        format!("{label} is installed, and the document is set in it."),
                    );
                }
                Err(e) => alert(id, "The Template Was Not Installed", e),
            });
            std::thread::spawn(move || crate::app::unpark(token, install(&spec)));
        })
    });
}

/// A notice in the window `id`.
fn alert(id: dioxus_native::winit::window::WindowId, title: &str, message: String) {
    let Some(page) = crate::app::page(id) else {
        return;
    };
    let Some(unit) = page.unit() else { return };
    let dialog = unit::Dialog::new(title).message(message).alert("OK");
    page.in_scope(|| unit.open_dialog(dialog, |_| {}));
}

/// What fetches a package: the app's HTTP client.
struct Fetch(reqwest::blocking::Client);

impl Fetch {
    fn new() -> Result<Fetch, String> {
        reqwest::blocking::Client::builder()
            .user_agent(concat!("Leaf/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map(Fetch)
            .map_err(|e| e.to_string())
    }
}

impl twig_export::install::Downloader for Fetch {
    fn stream(
        &self,
        _key: &dyn std::any::Any,
        url: &str,
    ) -> std::io::Result<(Option<usize>, Box<dyn std::io::Read>)> {
        let response = self.0.get(url).send().map_err(std::io::Error::other)?;
        match response.status() {
            s if s == reqwest::StatusCode::NOT_FOUND => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                url.to_owned(),
            )),
            s if !s.is_success() => Err(std::io::Error::other(format!("{url}: {s}"))),
            _ => {
                let len = response.content_length().map(|l| l as usize);
                Ok((len, Box::new(response)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_universe_name_is_given_its_namespace() {
        assert_eq!(
            spec_of(" charged-ieee:0.1.4 ")
                .map(|s| s.to_string())
                .as_deref(),
            Some("@preview/charged-ieee:0.1.4")
        );
        assert_eq!(
            spec_of("@local/x:1.0.0").map(|s| s.to_string()).as_deref(),
            Some("@local/x:1.0.0")
        );
        assert_eq!(spec_of("charged-ieee"), None);
    }

    #[test]
    fn the_builtins_lead_and_reflow() {
        let all = all();
        let keys: Vec<&str> = all.iter().take(3).map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["classic", "modern", "editorial"]);
        assert!(all.iter().take(3).all(|e| e.builtin && e.reflows));
    }
}
