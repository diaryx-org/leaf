//! The reader's display choices — the body and code faces, the text size,
//! the line spacing, the column width, the paper a PDF is laid on, whether
//! a new window keeps soft line breaks, and the appearance — and what
//! [`Type`] they resolve to, with the ⌘+ / ⌘− / ⌘0 text zoom over them.
//!
//! They live in the app's user defaults (`org.diaryx.leaf`) under the keys
//! the SwiftUI Mac app this one replaced used, and the iOS app uses
//! (`DisplayChoice.swift`), with the same values — so a reader's choices
//! carry over. The zoom is not stored, as a browser's does not outlive its
//! window.
//!
//! Everything here is pure but [`Settings::load`], [`Settings::store`] and
//! [`forget`], which go through `platform::defaults`; `prefs.rs` draws the
//! Settings window and `page.rs` hands each editor the [`leaf_dioxus::Look`] they make.

use leaf_dioxus::{Appearance, Type};

/// The body faces offered as chips, as (name, CSS family list): the book
/// and screen faces macOS ships. Any other installed family is a pick from
/// the Settings window's Other… menu.
pub const BODY_FACES: &[(&str, &str)] = &[
    ("Iowan Old Style", r#""Iowan Old Style", Georgia, serif"#),
    ("System", r#"system-ui, "Helvetica Neue", sans-serif"#),
    ("Charter", "Charter, Georgia, serif"),
    ("Georgia", "Georgia, serif"),
    ("Palatino", "Palatino, Georgia, serif"),
    ("Baskerville", "Baskerville, Georgia, serif"),
    (
        "Avenir Next",
        r#""Avenir Next", "Helvetica Neue", sans-serif"#,
    ),
    ("Helvetica Neue", r#""Helvetica Neue", sans-serif"#),
];

/// The code faces offered as chips, the same way.
pub const MONO_FACES: &[(&str, &str)] = &[
    ("Menlo", "Menlo, monospace"),
    ("SF Mono", r#""SF Mono", Menlo, monospace"#),
    ("Monaco", "Monaco, Menlo, monospace"),
    ("Courier New", r#""Courier New", Menlo, monospace"#),
];

/// A face's CSS family list: the table's, or — for any other family — the
/// name itself, then the table's first face, for a family since
/// uninstalled.
pub fn family(faces: &[(&str, &str)], name: &str, generic: &str) -> String {
    match faces.iter().find(|(n, _)| *n == name) {
        Some((_, css)) => css.to_string(),
        None if faces.is_empty() => format!("\"{}\", {generic}", name.replace(['"', '\\'], "")),
        None => format!("\"{}\", {}", name.replace(['"', '\\'], ""), faces[0].1),
    }
}

/// A choice from a short list, with a stored name and a label.
pub trait Choice: Copy + PartialEq + Sized + 'static {
    const ALL: &'static [Self];
    fn key(self) -> &'static str;
    fn label(self) -> &'static str;
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == s)
    }
}

macro_rules! choice {
    ($(#[$m:meta])* $name:ident { $($v:ident = $key:literal, $label:literal;)* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        pub enum $name { $($v),* }
        choice!(impl $name { $($v = $key, $label;)* });
    };
    // A choice over an enum defined elsewhere: the editor's `Appearance`.
    (impl $name:ident { $($v:ident = $key:literal, $label:literal;)* }) => {
        impl Choice for $name {
            const ALL: &'static [Self] = &[$(Self::$v),*];
            fn key(self) -> &'static str { match self { $(Self::$v => $key),* } }
            fn label(self) -> &'static str { match self { $(Self::$v => $label),* } }
        }
    };
}

choice!(
    /// The body size — Swift's `TextSize`.
    TextSize { Small = "small", "Small"; Medium = "medium", "Medium"; Large = "large", "Large"; }
);
choice!(
    /// The line box over the size — Swift's `LineSpacing`.
    LineSpacing {
        Compact = "compact", "Compact"; Normal = "normal", "Normal";
        Relaxed = "relaxed", "Relaxed"; Double = "double", "Double";
    }
);
choice!(
    /// How wide the column runs, in characters of the body face — Swift's
    /// `ColumnWidth`.
    ColumnWidth { Narrow = "narrow", "Narrow"; Medium = "medium", "Medium"; Wide = "wide", "Wide"; Full = "full", "Full width"; }
);
choice!(
    /// The sheet a PDF is laid on — Swift's `Paper`.
    Paper { UsLetter = "usLetter", "US Letter"; A4 = "a4", "A4"; }
);
/// The template the pages, the reflowed view and a PDF are set in, by what
/// names it: one of twig-export's, by its name (`classic`), or one
/// installed, by its package's spec (`@adapted/charged-ieee:0.1.4`). See
/// `templates.rs`.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Template(String);

impl Template {
    /// The template `key` names, if it can name one.
    pub fn parse(key: &str) -> Option<Template> {
        crate::templates::valid(key).then(|| Template(key.to_owned()))
    }

    /// What names it, as stored.
    pub fn key(&self) -> &str {
        &self.0
    }
}
// Light or dark: the system's, or one of them for this app. The Swift app
// followed the system; this is how to see one appearance without changing
// the whole Mac's.
choice!(impl Appearance { System = "system", "System"; Light = "light", "Light"; Dark = "dark", "Dark"; });

// `choice!` writes the enum and has no way to mark a variant `#[default]`.
#[allow(clippy::derivable_impls)]
impl Default for TextSize {
    fn default() -> Self {
        TextSize::Medium
    }
}

impl Default for Template {
    fn default() -> Self {
        Template(crate::templates::FALLBACK.to_owned())
    }
}

#[allow(clippy::derivable_impls)]
impl Default for ColumnWidth {
    fn default() -> Self {
        ColumnWidth::Medium
    }
}

impl TextSize {
    /// In CSS pixels, which the Swift editor's points are on a Mac.
    pub fn px(self) -> f32 {
        match self {
            TextSize::Small => 14.0,
            TextSize::Medium => 16.0,
            TextSize::Large => 19.0,
        }
    }
}

impl LineSpacing {
    pub fn ratio(self) -> f32 {
        match self {
            LineSpacing::Compact => 1.3,
            LineSpacing::Normal => 1.5,
            LineSpacing::Relaxed => 1.75,
            LineSpacing::Double => 2.0,
        }
    }
}

impl ColumnWidth {
    /// The measure, in characters; `None` fills the window.
    pub fn chars(self) -> Option<f32> {
        match self {
            ColumnWidth::Narrow => Some(52.0),
            ColumnWidth::Medium => Some(68.0),
            ColumnWidth::Wide => Some(88.0),
            ColumnWidth::Full => None,
        }
    }
}

impl Paper {
    /// Its size, in points.
    pub fn size(self) -> (f32, f32) {
        match self {
            Paper::UsLetter => (612.0, 792.0),
            Paper::A4 => (595.28, 841.89),
        }
    }
}

/// Every choice, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub body: String,
    pub mono: String,
    pub size: TextSize,
    pub spacing: LineSpacing,
    pub column: ColumnWidth,
    pub appearance: Appearance,
    /// The sheet Export as PDF lays the document on.
    pub paper: Paper,
    /// The template the document is set in, paged or reflowed.
    pub template: Template,
    /// Whether a new window draws soft line breaks where they were written.
    pub preserve: bool,
}

impl Default for Settings {
    /// What a first launch has, which is the Swift app's defaults.
    fn default() -> Self {
        Self {
            body: BODY_FACES[0].0.into(),
            mono: MONO_FACES[0].0.into(),
            size: TextSize::Medium,
            spacing: LineSpacing::Normal,
            column: ColumnWidth::Medium,
            appearance: Appearance::System,
            paper: Paper::UsLetter,
            template: Template::default(),
            preserve: false,
        }
    }
}

impl Settings {
    /// The type these choices set the text in, at `zoom`.
    pub fn ty(&self, zoom: f32) -> Type {
        Type {
            body: family(BODY_FACES, &self.body, "serif"),
            mono: family(MONO_FACES, &self.mono, "monospace"),
            size: self.size.px() * zoom,
            leading: self.spacing.ratio(),
            grid: None,
            faces: Default::default(),
        }
    }

    /// The choices stored, or a first launch's.
    pub fn load() -> Self {
        Self::from_stored(&crate::platform::defaults::string)
    }

    /// Keep the choices for every window and the next launch.
    pub fn store(&self) {
        for (key, value) in self.to_stored() {
            match key {
                // `@AppStorage` keeps a Bool as a number.
                keys::FLOW => crate::platform::defaults::set_bool(key, value == "1"),
                _ => crate::platform::defaults::set(key, &value),
            }
        }
    }

    /// As the user defaults spell them: (key, value) pairs.
    pub fn to_stored(&self) -> Vec<(&'static str, String)> {
        vec![
            (keys::BODY, stored_face(&self.body)),
            (keys::MONO, self.mono.clone()),
            (keys::SIZE, self.size.key().into()),
            (keys::SPACING, self.spacing.key().into()),
            (keys::COLUMN, self.column.key().into()),
            (keys::APPEARANCE, self.appearance.key().into()),
            (keys::PAPER, self.paper.key().into()),
            (keys::TEMPLATE, self.template.key().into()),
            (keys::FLOW, if self.preserve { "1" } else { "0" }.into()),
        ]
    }

    /// From what `read` finds under each key. A value it does not know
    /// keeps the default, so another version's choices still load.
    pub fn from_stored(read: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut s = Self::default();
        if let Some(v) = read(keys::BODY).filter(|v| !v.is_empty()) {
            s.body = face_of(&v);
        }
        if let Some(v) = read(keys::MONO).filter(|v| !v.is_empty()) {
            s.mono = v;
        }
        let pick = |key| read(key).unwrap_or_default();
        s.size = TextSize::parse(&pick(keys::SIZE)).unwrap_or(s.size);
        s.spacing = LineSpacing::parse(&pick(keys::SPACING)).unwrap_or(s.spacing);
        s.column = ColumnWidth::parse(&pick(keys::COLUMN)).unwrap_or(s.column);
        s.appearance = Appearance::parse(&pick(keys::APPEARANCE)).unwrap_or(s.appearance);
        s.paper = Paper::parse(&pick(keys::PAPER)).unwrap_or(s.paper);
        s.template = Template::parse(&pick(keys::TEMPLATE)).unwrap_or(s.template.clone());
        // `@AppStorage` keeps a Bool as a number.
        s.preserve = matches!(pick(keys::FLOW).as_str(), "1" | "true" | "YES");
        s
    }
}

/// The user-defaults keys, as `DisplayChoice.swift` names them.
pub mod keys {
    pub const COLUMN: &str = "display.columnWidth";
    pub const SIZE: &str = "display.textSize";
    pub const FLOW: &str = "display.preserveLineBreaks";
    pub const BODY: &str = "display.bodyFont";
    pub const MONO: &str = "display.codeFont";
    pub const SPACING: &str = "display.lineSpacing";
    pub const PAPER: &str = "display.paper";
    /// This app's alone: the Swift app followed the system.
    pub const APPEARANCE: &str = "display.appearance";
    /// This app's alone: the Swift app set no template.
    pub const TEMPLATE: &str = "display.template";

    pub const ALL: [&str; 9] = [
        COLUMN, SIZE, FLOW, BODY, MONO, SPACING, PAPER, APPEARANCE, TEMPLATE,
    ];
}

/// The body faces the Swift app stored by a font's name rather than its
/// family's: (stored, the face's name here).
const BODY_NAMES: &[(&str, &str)] = &[
    ("IowanOldStyle-Roman", "Iowan Old Style"),
    (".AppleSystemUIFont", "System"),
];

/// A stored body face as this app names it.
fn face_of(stored: &str) -> String {
    BODY_NAMES
        .iter()
        .find(|(s, _)| *s == stored)
        .map_or(stored, |(_, n)| n)
        .to_string()
}

/// A body face as the defaults store it: the Swift app's name for one of
/// its two defaults, else the family.
fn stored_face(name: &str) -> String {
    BODY_NAMES
        .iter()
        .find(|(_, n)| *n == name)
        .map_or(name, |(s, _)| s)
        .to_string()
}

/// Restore Defaults: the keys removed rather than the defaults written, so
/// a later release that changes a default reaches a reader who never chose
/// otherwise.
pub fn forget() {
    for key in keys::ALL {
        crate::platform::defaults::remove(key);
    }
}

/// The text zoom's steps: a browser's, which a reader already knows.
pub const ZOOMS: &[f32] = &[
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The next step in from `z`, or out; the last one either way stays.
pub fn zoom_step(z: f32, bigger: bool) -> f32 {
    const EPS: f32 = 0.001;
    let next = match bigger {
        true => ZOOMS.iter().copied().find(|&s| s > z + EPS),
        false => ZOOMS.iter().rev().copied().find(|&s| s < z - EPS),
    };
    next.unwrap_or(if bigger {
        ZOOMS[ZOOMS.len() - 1]
    } else {
        ZOOMS[0]
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn the_defaults_are_the_swift_editors() {
        let t = Settings::default().ty(1.0);
        assert_eq!(t, Type::default());
        assert_eq!(Settings::default().column.chars(), Some(68.0));
    }

    #[test]
    fn a_first_launch_looks_as_the_editor_does_by_default() {
        let s = Settings::default();
        let look = leaf_dioxus::Look {
            ty: s.ty(1.0),
            measure: s.column.chars(),
            appearance: s.appearance,
            ..Default::default()
        };
        assert_eq!(look, leaf_dioxus::Look::default());
    }

    #[test]
    fn the_zoom_scales_the_size_and_not_the_leading() {
        let s = Settings {
            size: TextSize::Large,
            spacing: LineSpacing::Relaxed,
            ..Settings::default()
        };
        let t = s.ty(1.5);
        assert_eq!(t.size, 19.0 * 1.5);
        assert_eq!(t.leading, 1.75);
    }

    #[test]
    fn the_settings_round_trip_through_the_defaults() {
        let s = Settings {
            body: r#"Odd "Quoted" Face"#.into(),
            mono: "Monaco".into(),
            size: TextSize::Small,
            spacing: LineSpacing::Double,
            column: ColumnWidth::Full,
            appearance: Appearance::Dark,
            paper: Paper::A4,
            template: Template::parse("editorial").unwrap(),
            preserve: true,
        };
        let stored: HashMap<_, _> = s.to_stored().into_iter().collect();
        assert_eq!(Settings::from_stored(&|k| stored.get(k).cloned()), s);
    }

    #[test]
    fn the_swift_apps_choices_load_as_they_were_made() {
        let swift = [
            ("display.bodyFont", ".AppleSystemUIFont"),
            ("display.textSize", "large"),
            ("display.columnWidth", "wide"),
            ("display.preserveLineBreaks", "1"),
            ("display.paper", "a4"),
            ("display.lineSpacing", "huge"),
        ];
        let s = Settings::from_stored(&|k| {
            swift
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        });
        assert_eq!(s.body, "System");
        assert_eq!(s.size, TextSize::Large);
        assert_eq!(s.column, ColumnWidth::Wide);
        assert!(s.preserve);
        assert_eq!(s.paper, Paper::A4);
        // A value this version does not know keeps the default.
        assert_eq!(s.spacing, LineSpacing::Normal);
        // And they are stored back as the Swift app spells them.
        let back: HashMap<_, _> = s.to_stored().into_iter().collect();
        assert_eq!(back["display.bodyFont"], ".AppleSystemUIFont");
    }

    #[test]
    fn another_face_is_named_with_the_default_after_it() {
        assert_eq!(family(MONO_FACES, "Menlo", "monospace"), "Menlo, monospace");
        assert_eq!(
            family(BODY_FACES, "Hoefler Text", "serif"),
            r#""Hoefler Text", "Iowan Old Style", Georgia, serif"#
        );
    }

    #[test]
    fn the_zoom_steps_and_stops_at_its_ends() {
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(3.0, true), 3.0);
        assert_eq!(zoom_step(0.5, false), 0.5);
        // Off the steps, to the nearest one in that direction.
        assert_eq!(zoom_step(1.05, true), 1.1);
        assert_eq!(zoom_step(1.05, false), 1.0);
    }
}
