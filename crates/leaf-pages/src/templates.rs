//! The templates a document can be set in: twig-export's three, and any the
//! host has installed ([`crate::Setup::installed`]), each a Typst package
//! (`twig_export::templates`).
//!
//! Installing one — fetching it from Typst Universe, adapting it, checking
//! it sets — is the host's: nothing here reaches the network.

use twig_export::templates as tx;

/// The template the pages, the reflowed view and a PDF are set in, by what
/// names it: one of twig-export's, by its name (`classic`), or one
/// installed, by its package's spec (`@adapted/charged-ieee:0.1.4`).
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Template(String);

impl Template {
    /// The template `key` names, if it can name one.
    pub fn parse(key: &str) -> Option<Template> {
        valid(key).then(|| Template(key.to_owned()))
    }

    /// What names it, as stored.
    pub fn key(&self) -> &str {
        &self.0
    }
}

impl Default for Template {
    fn default() -> Self {
        Template(FALLBACK.to_owned())
    }
}

/// A template, as a menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What a setting stores: a built-in's name, or a package's spec.
    pub key: String,
    pub label: String,
    /// Whether it sets a reflowed page; one that does not is reflowed in
    /// Classic.
    pub reflows: bool,
    /// Whether it is twig-export's own, set in this process.
    pub builtin: bool,
}

/// The template a reflowed view falls back to.
pub const FALLBACK: &str = "classic";

/// Every template: the built-ins, then those the host installed.
pub fn all() -> Vec<Entry> {
    let mut out = builtins();
    if let Some(installed) = &crate::setup().installed {
        out.extend(installed());
    }
    out
}

/// twig-export's own templates.
pub fn builtins() -> Vec<Entry> {
    tx::BUILTIN
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
        .collect()
}

/// The template a setting names.
pub fn entry(key: &str) -> Option<Entry> {
    all().into_iter().find(|e| e.key == key)
}

/// Whether `key` names a template that can be: a built-in, or a spec.
pub fn valid(key: &str) -> bool {
    tx::spec(key).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtins_lead_and_reflow() {
        let all = all();
        let keys: Vec<&str> = all.iter().take(3).map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["classic", "modern", "editorial"]);
        assert!(all.iter().take(3).all(|e| e.builtin && e.reflows));
    }
}
