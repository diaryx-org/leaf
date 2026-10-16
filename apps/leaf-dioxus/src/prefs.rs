//! The settings panel (⌘,) and the text zoom (⌘+ / ⌘− / ⌘0): carrying a
//! change out, and the panel's markup. The choices themselves are
//! `settings.rs`.
//!
//! A change is one new [`Look`] handed to the editor, which lays its rows out
//! for exactly the faces, size and leading Blitz then draws them in — which
//! is what keeps a click on the glyph under it.
//!
//! The Swift editor's Settings is a window of its own. This is a panel in the
//! editor's window, over its top-right corner and not modal — the text under
//! it reflows as a choice is made and can still be typed in — because
//! dioxus-native 0.8's `launch` opens one window and its Blitz document
//! cannot be shared with a second.

use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_native::winit::window::Theme;
use leaf_dioxus::{Appearance, Editor, Look, ZoomCommand, trace};

use crate::settings::{
    BODY_FACES, Choice, ColumnWidth, LineSpacing, MONO_FACES, Settings, TextSize, zoom_step,
};
use crate::shell::Shell;

/// What the panel draws.
#[derive(Clone, PartialEq, Default)]
pub struct PrefsView {
    pub open: bool,
    pub settings: Settings,
    /// The faces offered that are installed, as (name, CSS family list).
    pub bodies: Vec<(String, String)>,
    pub monos: Vec<(String, String)>,
}

/// One of the panel's controls.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    Body(String),
    Mono(String),
    Size(TextSize),
    Spacing(LineSpacing),
    Column(ColumnWidth),
    Appearance(Appearance),
    Restore,
    Done,
}

/// The window's appearance: the system's, or one of its two.
fn theme(appearance: Appearance) -> Option<Theme> {
    match appearance {
        Appearance::System => None,
        Appearance::Light => Some(Theme::Light),
        Appearance::Dark => Some(Theme::Dark),
    }
}

impl Settings {
    /// This choice made.
    fn with(mut self, pick: &Pick) -> Self {
        match pick.clone() {
            Pick::Body(f) => self.body = f,
            Pick::Mono(f) => self.mono = f,
            Pick::Size(v) => self.size = v,
            Pick::Spacing(v) => self.spacing = v,
            Pick::Column(v) => self.column = v,
            Pick::Appearance(v) => self.appearance = v,
            // Every choice back to a first launch's.
            Pick::Restore => return Settings::default(),
            Pick::Done => {}
        }
        self
    }
}

impl Shell {
    /// What the editor draws with: the settings, at the zoom.
    pub fn look(&self) -> Look {
        let s = self.settings.borrow();
        Look {
            ty: s.ty(self.zoom.get()),
            measure: s.column.chars(),
            appearance: s.appearance,
        }
    }

    /// The settings as loaded, put into effect on the window and the panel —
    /// the editor's first paint already has them, through [`Shell::look`].
    pub fn apply_settings(&self, ed: &Editor) {
        let appearance = self.settings.borrow().appearance;
        self.window.set_theme(theme(appearance));
        self.sync_prefs(ed);
    }

    /// ⌘, — open the panel, or close it.
    pub fn toggle_prefs(&self) {
        let open = !self.prefs.peek().open;
        self.show_prefs(open);
    }

    pub fn prefs_open(&self) -> bool {
        self.prefs.peek().open
    }

    pub fn show_prefs(&self, open: bool) {
        let mut v = self.prefs.peek().clone();
        v.open = open;
        let mut prefs = self.prefs;
        prefs.set(v);
    }

    /// A click on the panel.
    pub fn pick(&self, ed: &Editor, pick: Pick) {
        trace(|| format!("settings {pick:?}"));
        if pick == Pick::Done {
            return self.show_prefs(false);
        }
        let now = self.settings.borrow().clone().with(&pick);
        if now == *self.settings.borrow() {
            return;
        }
        let theme_changed = now.appearance != self.settings.borrow().appearance;
        if let Err(e) = now.save() {
            eprintln!("settings: {e}");
        }
        *self.settings.borrow_mut() = now;
        if theme_changed {
            self.window
                .set_theme(theme(self.settings.borrow().appearance));
        }
        ed.set_look(self.look());
        self.sync_prefs(ed);
    }

    /// ⌘+, ⌘−, ⌘0: the text set larger or smaller, and the column with it,
    /// since its measure is counted in characters.
    pub fn zoom(&self, ed: &Editor, z: ZoomCommand) {
        let was = self.zoom.get();
        let now = match z {
            ZoomCommand::In => zoom_step(was, true),
            ZoomCommand::Out => zoom_step(was, false),
            ZoomCommand::Reset => 1.0,
        };
        ed.doc().status = Some(format!("Zoom {:.0}%", now * 100.0));
        if now != was {
            self.zoom.set(now);
            ed.set_look(self.look());
        } else {
            ed.refresh();
        }
    }

    /// Refresh what the panel draws.
    fn sync_prefs(&self, ed: &Editor) {
        let installed = |faces: &[(&str, &str)]| -> Vec<(String, String)> {
            faces
                .iter()
                .filter(|(name, _)| *name == "System" || ed.has_family(name))
                .map(|(n, css)| (n.to_string(), css.to_string()))
                .collect()
        };
        let v = PrefsView {
            open: self.prefs.peek().open,
            settings: self.settings.borrow().clone(),
            bodies: installed(BODY_FACES),
            monos: installed(MONO_FACES),
        };
        let mut prefs = self.prefs;
        prefs.set(v);
    }
}

/// The panel's look: the editor's chrome colours, and the chips' own,
/// written into the page beside the editor's stylesheet.
pub const CSS: &str = "
:root {
  --chip-on: #007aff;
  --chip-on-text: #ffffff;
}
@media (prefers-color-scheme: dark) {
  :root {
    --chip-on: #0a84ff;
    --chip-on-text: #ffffff;
  }
}
.prefs { position: absolute; top: 12px; right: 12px; width: 400px; z-index: 2; box-sizing: border-box;
         padding: 10px 14px 12px; background: var(--chrome-bg); color: var(--chrome-text);
         border: 1px solid var(--field-border); border-radius: 10px;
         font: 13px system-ui, sans-serif; line-height: 1.3; white-space: normal; }
.prefs .title { font-weight: 700; margin-bottom: 4px; }
.prefs .section { margin-top: 10px; color: var(--leaf-secondary); font-size: 11px; }
.prefs .prow { display: flex; gap: 8px; margin-top: 6px; }
.prefs .plabel { flex: none; width: 92px; text-align: right; padding-top: 3px; }
.prefs .chips { flex: 1; display: flex; flex-wrap: wrap; gap: 4px; }
.prefs .chip { padding: 2px 7px; border-radius: 5px; background: var(--field-bg);
               border: 1px solid var(--field-border); white-space: nowrap; }
.prefs .chip.on { background: var(--chip-on); color: var(--chip-on-text); border-color: var(--chip-on); }
.prefs .btn { height: 22px; line-height: 22px; padding: 0 9px; background: var(--field-bg);
              border: 1px solid var(--field-border); border-radius: 5px; }
.prefs .foot { display: flex; gap: 8px; margin-top: 12px; }
.prefs .foot .grow { flex: 1; }";

/// The panel's markup. Always present, shown or hidden, for the find bar's
/// reason: Blitz does not lay out an absolutely positioned element inserted
/// after the first frame.
pub fn render(ed: &Editor, shell: &Rc<Shell>, v: &PrefsView) -> Element {
    let s = &v.settings;
    let display = if v.open { "block" } else { "none" };
    let chip = |label: String, on: bool, font: Option<String>, pick: Pick| {
        let (ed, shell) = (ed.clone(), shell.clone());
        let class = if on { "chip on" } else { "chip" };
        let style = font
            .map(|f| format!("font-family: {f}"))
            .unwrap_or_default();
        rsx! {
            span { class, style,
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    shell.pick(&ed, pick.clone());
                },
                "{label}"
            }
        }
    };
    let faces = |list: &[(String, String)], chosen: &str, make: fn(String) -> Pick| {
        // A face stored that this machine does not have keeps its chip, so
        // the panel still says what is chosen.
        let mut list = list.to_vec();
        if !list.iter().any(|(n, _)| n == chosen) {
            list.push((chosen.to_string(), String::new()));
        }
        rsx! {
            for (name, css) in list {
                {chip(name.clone(), name == chosen, Some(css).filter(|c| !c.is_empty()), make(name))}
            }
        }
    };
    fn choices<C: Choice>(
        chosen: C,
        chip: &dyn Fn(String, bool, Option<String>, Pick) -> Element,
        make: fn(C) -> Pick,
    ) -> Element {
        rsx! {
            for c in C::ALL.iter().copied() {
                {chip(c.label().to_string(), c == chosen, None, make(c))}
            }
        }
    }
    let row = |label: &str, chips: Element| {
        rsx! {
            div { class: "prow",
                span { class: "plabel", "{label}" }
                div { class: "chips", {chips} }
            }
        }
    };
    let foot = |label: &str, pick: Pick| {
        let (ed, shell) = (ed.clone(), shell.clone());
        rsx! {
            span { class: "btn",
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    shell.pick(&ed, pick.clone());
                },
                "{label}"
            }
        }
    };
    rsx! {
        div { class: "prefs", style: "display: {display}",
            onmousedown: move |e: MouseEvent| e.prevent_default(),
            div { class: "title", "Settings" }
            div { class: "section", "TEXT" }
            {row("Font:", faces(&v.bodies, &s.body, Pick::Body))}
            {row("Code font:", faces(&v.monos, &s.mono, Pick::Mono))}
            {row("Size:", choices(s.size, &chip, Pick::Size))}
            div { class: "section", "LAYOUT" }
            {row("Line spacing:", choices(s.spacing, &chip, Pick::Spacing))}
            {row("Column width:", choices(s.column, &chip, Pick::Column))}
            {row("Appearance:", choices(s.appearance, &chip, Pick::Appearance))}
            div { class: "foot",
                {foot("Restore Defaults", Pick::Restore)}
                span { class: "grow" }
                {foot("Done", Pick::Done)}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_changes_one_choice_and_restore_changes_them_all_back() {
        let s = Settings::default()
            .with(&Pick::Size(TextSize::Large))
            .with(&Pick::Body("Georgia".into()));
        assert_eq!(s.size, TextSize::Large);
        assert_eq!(s.body, "Georgia");
        assert_eq!(s.spacing, Settings::default().spacing);
        assert_eq!(s.with(&Pick::Restore), Settings::default());
    }

    #[test]
    fn the_panel_is_drawn_in_colours_set_for_both_appearances() {
        let all = format!(
            "{}{CSS}",
            leaf_dioxus::stylesheet(&leaf_dioxus::Type::default())
        );
        leaf_dioxus::Palette::of(&all)
            .check()
            .expect("a whole palette");
        assert!(CSS.contains("var(--chip-on)") && CSS.contains("var(--chrome-bg)"));
    }
}
