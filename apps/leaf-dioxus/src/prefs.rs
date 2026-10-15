//! The settings panel (⌘,) and the text zoom (⌘+ / ⌘− / ⌘0): carrying a
//! change out, and the panel's markup. The choices themselves are
//! `settings.rs`.
//!
//! A change is one new [`crate::text::Type`], handed to `text.rs` and the
//! stylesheet in the same repaint, so the rows are laid out for exactly the
//! faces, size and leading Blitz then draws them in — which is what keeps a
//! click on the glyph under it.
//!
//! The Swift editor's Settings is a window of its own. This is a panel in the
//! editor's window, over its top-right corner and not modal — the text under
//! it reflows as a choice is made and can still be typed in — because
//! dioxus-native 0.8's `launch` opens one window and its Blitz document
//! cannot be shared with a second.

use dioxus::prelude::*;
use dioxus_native::winit::window::Theme;

use crate::editor::{Editor, Measures, set};
use crate::settings::{
    Appearance, BODY_FACES, Choice, ColumnWidth, LineSpacing, MONO_FACES, Settings, TextSize, zoom_step,
};
use crate::trace::trace;

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

/// ⌘+, ⌘− and ⌘0.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZoomCommand {
    In,
    Out,
    Reset,
}

impl Appearance {
    /// The window's appearance: the system's, or one of its two.
    fn theme(self) -> Option<Theme> {
        match self {
            Appearance::System => None,
            Appearance::Light => Some(Theme::Light),
            Appearance::Dark => Some(Theme::Dark),
        }
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

impl Editor {
    /// The settings as loaded, put into effect: the first paint's.
    pub fn apply_settings(&self) {
        let appearance = self.st.settings.borrow().appearance;
        self.st.window.set_theme(appearance.theme());
        self.retype();
        self.sync_prefs();
    }

    /// ⌘, — open the panel, or close it.
    pub fn toggle_prefs(&self) {
        let open = !self.sig.prefs.peek().open;
        self.show_prefs(open);
    }

    pub fn prefs_open(&self) -> bool {
        self.sig.prefs.peek().open
    }

    pub fn show_prefs(&self, open: bool) {
        let mut v = self.sig.prefs.peek().clone();
        v.open = open;
        set(self.sig.prefs, v);
    }

    /// A click on the panel.
    pub fn pick(&self, pick: Pick) {
        trace(|| format!("settings {pick:?}"));
        if pick == Pick::Done {
            return self.show_prefs(false);
        }
        let now = self.st.settings.borrow().clone().with(&pick);
        if now == *self.st.settings.borrow() {
            return;
        }
        let theme_changed = now.appearance != self.st.settings.borrow().appearance;
        if let Err(e) = now.save() {
            eprintln!("settings: {e}");
        }
        *self.st.settings.borrow_mut() = now;
        if theme_changed {
            self.st.window.set_theme(self.st.settings.borrow().appearance.theme());
        }
        self.retype();
        self.sync_prefs();
    }

    /// ⌘+, ⌘−, ⌘0: the text set larger or smaller, and the column with it,
    /// since its measure is counted in characters.
    pub fn zoom(&self, z: ZoomCommand) {
        let was = self.st.zoom.get();
        let now = match z {
            ZoomCommand::In => zoom_step(was, true),
            ZoomCommand::Out => zoom_step(was, false),
            ZoomCommand::Reset => 1.0,
        };
        self.st.doc.borrow_mut().status = Some(format!("Zoom {:.0}%", now * 100.0));
        if now != was {
            self.st.zoom.set(now);
            self.retype();
        } else {
            self.refresh();
        }
    }

    /// Set the text in the type the settings and the zoom make, and lay
    /// every row out again for it.
    fn retype(&self) {
        let st = &*self.st;
        let ty = st.settings.borrow().ty(st.zoom.get());
        {
            let mut fonts = st.fonts.borrow_mut();
            fonts.set_ty(ty.clone());
            st.measures.set(Measures::of(&st.settings.borrow(), &mut fonts));
        }
        // Tables keep their layouts by width alone; a new type is new ones.
        st.block_cache.borrow_mut().clear();
        trace(|| format!("type {ty:?}, measures {:?}", st.measures.get()));
        set(self.sig.ty, ty);
        self.refresh();
    }

    /// Refresh what the panel draws.
    fn sync_prefs(&self) {
        let mut fonts = self.st.fonts.borrow_mut();
        let mut installed = |faces: &[(&str, &str)]| -> Vec<(String, String)> {
            faces
                .iter()
                .filter(|(name, _)| *name == "System" || fonts.has_family(name))
                .map(|(n, css)| (n.to_string(), css.to_string()))
                .collect()
        };
        let v = PrefsView {
            open: self.sig.prefs.peek().open,
            settings: self.st.settings.borrow().clone(),
            bodies: installed(BODY_FACES),
            monos: installed(MONO_FACES),
        };
        set(self.sig.prefs, v);
    }
}

/// The panel's markup. Always present, shown or hidden, for the find bar's
/// reason: Blitz does not lay out an absolutely positioned element inserted
/// after the first frame.
pub fn render(ed: &Editor, v: &PrefsView) -> Element {
    let s = &v.settings;
    let display = if v.open { "block" } else { "none" };
    let chip = |label: String, on: bool, font: Option<String>, pick: Pick| {
        let ed = ed.clone();
        let class = if on { "chip on" } else { "chip" };
        let style = font.map(|f| format!("font-family: {f}")).unwrap_or_default();
        rsx! {
            span { class, style,
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    ed.pick(pick.clone());
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
    fn choices<C: Choice>(chosen: C, chip: &dyn Fn(String, bool, Option<String>, Pick) -> Element, make: fn(C) -> Pick) -> Element {
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
        let ed = ed.clone();
        rsx! {
            span { class: "btn",
                onmousedown: move |e: MouseEvent| {
                    e.prevent_default();
                    ed.pick(pick.clone());
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
        let s = Settings::default().with(&Pick::Size(TextSize::Large)).with(&Pick::Body("Georgia".into()));
        assert_eq!(s.size, TextSize::Large);
        assert_eq!(s.body, "Georgia");
        assert_eq!(s.spacing, Settings::default().spacing);
        assert_eq!(s.with(&Pick::Restore), Settings::default());
    }
}
