//! The reader's display choices — the Swift editor's Settings window
//! (`apps/leaf-editor/App/SettingsView.swift`, `DisplayChoice.swift`): the
//! body and code faces, the text size, the line spacing and the column width,
//! with the same names and the same values — plus an appearance override
//! and the ⌘+ / ⌘− / ⌘0 text zoom.
//!
//! Everything here is pure: the choices, what [`Type`] they resolve to, and
//! their file. `prefs.rs` draws the panel and carries a change out — a new
//! type for `text.rs` and the stylesheet together, then a relayout.
//!
//! The choices persist as a few lines of TOML in the user's config directory
//! (`LEAF_SETTINGS` names another file); the zoom does not, as a browser's
//! does not outlive its window.

use std::path::PathBuf;

use crate::text::Type;

/// The body faces offered, as (name, CSS family list). The Swift editor
/// offers every installed family; a panel of buttons cannot hold a few
/// hundred, so these are the book and screen faces macOS ships, and the
/// panel shows only those installed (`Fonts::has_family`).
pub const BODY_FACES: &[(&str, &str)] = &[
    ("Iowan Old Style", r#""Iowan Old Style", Georgia, serif"#),
    ("System", r#"system-ui, "Helvetica Neue", sans-serif"#),
    ("Charter", "Charter, Georgia, serif"),
    ("Georgia", "Georgia, serif"),
    ("Palatino", "Palatino, Georgia, serif"),
    ("Baskerville", "Baskerville, Georgia, serif"),
    ("Avenir Next", r#""Avenir Next", "Helvetica Neue", sans-serif"#),
    ("Helvetica Neue", r#""Helvetica Neue", sans-serif"#),
];

/// The code faces offered, the same way.
pub const MONO_FACES: &[(&str, &str)] = &[
    ("Menlo", "Menlo, monospace"),
    ("SF Mono", r#""SF Mono", Menlo, monospace"#),
    ("Monaco", "Monaco, Menlo, monospace"),
    ("Courier New", r#""Courier New", Menlo, monospace"#),
];

/// A face's CSS family list: the table's, or — for a name stored by hand or
/// by a later version — the name itself, then the generic.
pub fn family(faces: &[(&str, &str)], name: &str, generic: &str) -> String {
    match faces.iter().find(|(n, _)| *n == name) {
        Some((_, css)) => css.to_string(),
        None => format!("\"{}\", {generic}", name.replace(['"', '\\'], "")),
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
    /// Light or dark: the system's, or one of them for this app. Not a
    /// choice the Swift editor offers — it follows the system — but the only
    /// way to see the dark stylesheet without changing the whole Mac's.
    Appearance { System = "system", "System"; Light = "light", "Light"; Dark = "dark", "Dark"; }
);

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

/// Every choice, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub body: String,
    pub mono: String,
    pub size: TextSize,
    pub spacing: LineSpacing,
    pub column: ColumnWidth,
    pub appearance: Appearance,
}

impl Default for Settings {
    /// What a first launch has, which is the Swift editor's defaults.
    fn default() -> Self {
        Self {
            body: BODY_FACES[0].0.into(),
            mono: MONO_FACES[0].0.into(),
            size: TextSize::Medium,
            spacing: LineSpacing::Normal,
            column: ColumnWidth::Medium,
            appearance: Appearance::System,
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
        }
    }

    /// As the settings file has them.
    pub fn to_toml(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        format!(
            "# The native editor spike's display choices; see settings.rs.\n\
             body_font = {}\ncode_font = {}\ntext_size = {}\nline_spacing = {}\ncolumn_width = {}\nappearance = {}\n",
            q(&self.body),
            q(&self.mono),
            q(self.size.key()),
            q(self.spacing.key()),
            q(self.column.key()),
            q(self.appearance.key()),
        )
    }

    /// From the settings file: the `key = "string"` lines this writes. A key
    /// it does not know, or a value it cannot read, keeps the default, so a
    /// file from another version still opens.
    pub fn from_toml(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let Some(value) = string(value.trim()) else { continue };
            match key.trim() {
                "body_font" if !value.is_empty() => s.body = value,
                "code_font" if !value.is_empty() => s.mono = value,
                "text_size" => s.size = TextSize::parse(&value).unwrap_or(s.size),
                "line_spacing" => s.spacing = LineSpacing::parse(&value).unwrap_or(s.spacing),
                "column_width" => s.column = ColumnWidth::parse(&value).unwrap_or(s.column),
                "appearance" => s.appearance = Appearance::parse(&value).unwrap_or(s.appearance),
                _ => {}
            }
        }
        s
    }

    /// The choices saved last, or the defaults.
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::from_toml(&t))
            .unwrap_or_default()
    }

    /// Keep the choices for the next launch. Written beside and renamed over,
    /// so a crash mid-write never leaves half a file.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(p) = path() else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(tmp, p)
    }
}

/// A TOML basic string's contents, its two escapes undone.
fn string(v: &str) -> Option<String> {
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    Some(out)
}

/// The settings file: `LEAF_SETTINGS`, or `settings.toml` in the user's
/// config directory — Application Support on a Mac.
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LEAF_SETTINGS") {
        return Some(p.into());
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"))
    };
    Some(dir.join("leaf-native-spike/settings.toml"))
}

/// The text zoom's steps: a browser's, which a reader already knows.
pub const ZOOMS: &[f32] = &[0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

/// The next step in from `z`, or out; the last one either way stays.
pub fn zoom_step(z: f32, bigger: bool) -> f32 {
    const EPS: f32 = 0.001;
    let next = match bigger {
        true => ZOOMS.iter().copied().find(|&s| s > z + EPS),
        false => ZOOMS.iter().rev().copied().find(|&s| s < z - EPS),
    };
    next.unwrap_or(if bigger { ZOOMS[ZOOMS.len() - 1] } else { ZOOMS[0] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_swift_editors() {
        let t = Settings::default().ty(1.0);
        assert_eq!(t, Type::default());
        assert_eq!(Settings::default().column.chars(), Some(68.0));
    }

    #[test]
    fn the_zoom_scales_the_size_and_not_the_leading() {
        let s = Settings { size: TextSize::Large, spacing: LineSpacing::Relaxed, ..Settings::default() };
        let t = s.ty(1.5);
        assert_eq!(t.size, 19.0 * 1.5);
        assert_eq!(t.leading, 1.75);
    }

    #[test]
    fn the_settings_round_trip_through_their_file() {
        let s = Settings {
            body: r#"Odd "Quoted" \ Face"#.into(),
            mono: "Monaco".into(),
            size: TextSize::Small,
            spacing: LineSpacing::Double,
            column: ColumnWidth::Full,
            appearance: Appearance::Dark,
        };
        assert_eq!(Settings::from_toml(&s.to_toml()), s);
    }

    #[test]
    fn a_file_from_another_version_keeps_what_it_can() {
        let s = Settings::from_toml(
            "# a comment\ntext_size = \"huge\"\nline_spacing = \"compact\"\nfuture = \"x\"\nbody_font = 3\n",
        );
        assert_eq!(s.size, TextSize::Medium);
        assert_eq!(s.spacing, LineSpacing::Compact);
        assert_eq!(s.body, Settings::default().body);
    }

    #[test]
    fn an_unknown_face_is_named_with_its_generic_after_it() {
        assert_eq!(family(MONO_FACES, "Menlo", "monospace"), "Menlo, monospace");
        assert_eq!(family(BODY_FACES, "Hoefler Text", "serif"), r#""Hoefler Text", serif"#);
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
