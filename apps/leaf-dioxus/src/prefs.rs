//! The Settings window: the reader's display choices (`settings.rs`) as
//! one page, every row a [`leaf_dioxus_ui`] control. A choice is stored at
//! once and every document window redraws with it.
//!
//! The body and code faces are chips for the book and screen faces the
//! system ships, and an Other ▾ menu of every family installed, grouped by
//! letter so that no menu runs past the window.

use std::collections::BTreeMap;
use std::rc::Rc;

use dioxus::prelude::*;
use leaf_dioxus::{Appearance, themed};
use leaf_dioxus_ui::{Chips, Choice as Pill, Entry, Item, MenuButton, use_layer_provider};

use crate::app;
use crate::page::Made;
use crate::platform;
use crate::settings::{
    BODY_FACES, Choice, ColumnWidth, LineSpacing, MONO_FACES, Paper, Settings, TextSize,
};

/// The window's size, in points.
pub const SIZE: (f64, f64) = (500.0, 640.0);

const CSS: &str = "
:root { --prefs-bg: #ececec; --prefs-text: #1f1f1f; --prefs-dim: #6b6b6b; }
@media (prefers-color-scheme: dark) {
  :root { --prefs-bg: #262626; --prefs-text: #e8e8e8; --prefs-dim: #9a9a9a; }
}
body { background: var(--prefs-bg); }
.prefs { padding: 18px 22px; color: var(--prefs-text); font: 13px system-ui, -apple-system, sans-serif;
         line-height: 1.3; }
.prefs .section { margin: 14px 0 4px; color: var(--prefs-dim); font-size: 11px; font-weight: 600; }
.prefs .prow { display: flex; gap: 10px; margin-top: 8px; }
.prefs .plabel { flex: none; width: 104px; text-align: right; padding-top: 3px; }
.prefs .pcontrols { flex: 1; }
.prefs .other { margin-top: 5px; }
.prefs .foot { display: flex; margin-top: 22px; }
.prefs .grow { flex: 1; }
.prefs .btn { height: 22px; line-height: 22px; padding: 0 12px; border-radius: 5px;
              background: var(--ui-field-bg); border: 1px solid var(--ui-border); }";

/// The Settings window's page.
#[allow(non_snake_case)]
pub fn SettingsPage() -> Element {
    let made = use_context::<Made>();
    let layer = use_layer_provider();
    use_hook(|| made.layer.set(Some(layer)));
    let mut s = use_signal(app::settings);
    let families = use_hook(|| Rc::new(platform::font_families()));
    // A choice made: the settings copied, changed and stored — the copy
    // taken first, so no read of the signal is held across its write.
    let mut set = move |change: &dyn Fn(&mut Settings)| {
        let mut next = s.peek().clone();
        change(&mut next);
        s.set(next.clone());
        app::set_settings(next);
    };
    let now = s.read().clone();
    let appearance = now.appearance;

    let faces = |list: &'static [(&'static str, &'static str)],
                 chosen: &str|
     -> (Vec<Pill>, Vec<bool>, Vec<String>) {
        // The table's faces in its order — the default always, as it is
        // the system's own, and the rest where installed — then a chosen
        // face from elsewhere.
        let mut names: Vec<String> = list
            .iter()
            .enumerate()
            .filter(|(i, (n, _))| {
                *i == 0 || *n == "System" || *n == chosen || families.iter().any(|f| f == n)
            })
            .map(|(_, (n, _))| n.to_string())
            .collect();
        if !names.iter().any(|n| n == chosen) {
            names.push(chosen.to_string());
        }
        let pills = names
            .iter()
            .map(|n| {
                Pill::new(n.clone()).styled(format!(
                    "font-family: {}",
                    crate::settings::family(list, n, "serif")
                ))
            })
            .collect();
        let on = names.iter().map(|n| n == chosen).collect();
        (pills, on, names)
    };
    let (body_pills, body_on, body_names) = faces(BODY_FACES, &now.body);
    let (mono_pills, mono_on, mono_names) = faces(MONO_FACES, &now.mono);

    fn choices<C: Choice>(chosen: C) -> (Vec<Pill>, Vec<bool>) {
        (
            C::ALL.iter().map(|c| Pill::new(c.label())).collect(),
            C::ALL.iter().map(|c| *c == chosen).collect(),
        )
    }
    let (size_p, size_on) = choices(now.size);
    let (spacing_p, spacing_on) = choices(now.spacing);
    let (column_p, column_on) = choices(now.column);
    let (appearance_p, appearance_on) = choices(now.appearance);
    let (paper_p, paper_on) = choices(now.paper);
    let (others, other_names) = by_letter(&families);
    let other_names = Rc::new(other_names);
    let mono_other = other_names.clone();

    rsx! {
        style { {themed(&format!("{CSS}{}", leaf_dioxus_ui::CSS), appearance)} }
        div { class: "prefs",
            div { class: "section", "TEXT" }
            div { class: "prow",
                span { class: "plabel", "Font:" }
                div { class: "pcontrols",
                    Chips { choices: body_pills, on: body_on, onpick: move |i: usize| {
                        set(&|x| x.body = body_names[i].clone());
                    } }
                    div { class: "other",
                        MenuButton { class: "ui-select", entries: others.clone(), height: 22.0,
                            onpick: move |i: usize| {
                                set(&|x| x.body = other_names[i].clone());
                            },
                            "Other ▾"
                        }
                    }
                }
            }
            div { class: "prow",
                span { class: "plabel", "Code font:" }
                div { class: "pcontrols",
                    Chips { choices: mono_pills, on: mono_on, onpick: move |i: usize| {
                        set(&|x| x.mono = mono_names[i].clone());
                    } }
                    div { class: "other",
                        MenuButton { class: "ui-select", entries: others, height: 22.0,
                            onpick: move |i: usize| {
                                set(&|x| x.mono = mono_other[i].clone());
                            },
                            "Other ▾"
                        }
                    }
                }
            }
            div { class: "prow",
                span { class: "plabel", "Size:" }
                div { class: "pcontrols",
                    Chips { choices: size_p, on: size_on, onpick: move |i: usize| {
                        set(&|x| x.size = TextSize::ALL[i]);
                    } }
                }
            }
            div { class: "section", "LAYOUT" }
            div { class: "prow",
                span { class: "plabel", "Line spacing:" }
                div { class: "pcontrols",
                    Chips { choices: spacing_p, on: spacing_on, onpick: move |i: usize| {
                        set(&|x| x.spacing = LineSpacing::ALL[i]);
                    } }
                }
            }
            div { class: "prow",
                span { class: "plabel", "Column width:" }
                div { class: "pcontrols",
                    Chips { choices: column_p, on: column_on, onpick: move |i: usize| {
                        set(&|x| x.column = ColumnWidth::ALL[i]);
                    } }
                }
            }
            div { class: "prow",
                span { class: "plabel", "Line breaks:" }
                div { class: "pcontrols",
                    Chips {
                        choices: vec![Pill::new("Reflow"), Pill::new("Preserve in new windows")],
                        on: vec![!now.preserve, now.preserve],
                        onpick: move |i: usize| {
                            set(&|x| x.preserve = i == 1);
                        }
                    }
                }
            }
            div { class: "prow",
                span { class: "plabel", "Appearance:" }
                div { class: "pcontrols",
                    Chips { choices: appearance_p, on: appearance_on, onpick: move |i: usize| {
                        set(&|x| x.appearance = Appearance::ALL[i]);
                    } }
                }
            }
            div { class: "section", "PAGE" }
            div { class: "prow",
                span { class: "plabel", "Paper:" }
                div { class: "pcontrols",
                    Chips { choices: paper_p, on: paper_on, onpick: move |i: usize| {
                        set(&|x| x.paper = Paper::ALL[i]);
                    } }
                }
            }
            div { class: "foot",
                span { class: "grow" }
                span { class: "btn",
                    onmousedown: move |e: MouseEvent| {
                        e.prevent_default();
                        crate::settings::forget();
                        set(&|x| *x = Settings::default());
                    },
                    "Restore Defaults"
                }
            }
        }
        {layer.render()}
    }
}

/// Every family, in a submenu per first letter: the entries, and the
/// family each item's id names.
fn by_letter(families: &[String]) -> (Vec<Entry>, Vec<String>) {
    let mut groups: BTreeMap<char, Vec<&String>> = BTreeMap::new();
    for f in families {
        let first = f.chars().next().map_or('#', |c| c.to_ascii_uppercase());
        let key = if first.is_ascii_alphabetic() {
            first
        } else {
            '#'
        };
        groups.entry(key).or_default().push(f);
    }
    let mut names = Vec::new();
    let entries = groups
        .into_iter()
        .map(|(letter, fs)| {
            let items = fs
                .into_iter()
                .map(|f| {
                    names.push(f.clone());
                    Item::new(names.len() - 1, f.clone()).into()
                })
                .collect();
            Entry::submenu(letter.to_string(), items)
        })
        .collect();
    (entries, names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_grouped_by_their_first_letter() {
        let fs: Vec<String> = ["Avenir", "Arial", "Baskerville", "8bit"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (entries, names) = by_letter(&fs);
        assert_eq!(entries.len(), 3);
        let Entry::Submenu {
            label, entries: a, ..
        } = &entries[1]
        else {
            panic!()
        };
        assert_eq!(label, "A");
        assert_eq!(a.len(), 2);
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn the_window_is_drawn_in_colours_set_for_both_appearances() {
        let dark_at = CSS.find("@media").unwrap();
        let (light, dark) = CSS.split_at(dark_at);
        for name in ["--prefs-bg", "--prefs-text", "--prefs-dim"] {
            assert!(light.contains(&format!("{name}:")) && dark.contains(&format!("{name}:")));
        }
    }
}
