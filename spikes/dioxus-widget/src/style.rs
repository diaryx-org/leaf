//! A glyph's toolkit-agnostic [`Style`] as CSS — the same job
//! `leaf-ratatui`'s `style.rs` does for a terminal cell, and that
//! `leaf-web`'s `editor.js` does for a `<span>`.
//!
//! Two halves, for the reason the measurements in README.md turn on: a
//! **class** for everything a stylesheet can say once (the roles, the syntax
//! tokens, the emphasis flags), and an **inline style** only for what varies
//! per run. A class is a short, shared string, so a run that differs from its
//! neighbour costs one `set_attribute` of a few bytes rather than a rebuilt
//! declaration list — and runs with the same role compare equal, which is what
//! lets a row memoize.

use leaf_core::{Baseline, MarkColor, Role, Style, Token};

/// The class list for a run of glyphs sharing one [`Style`].
///
/// Written into a reused `String` rather than returned, because the renderer
/// builds one per run per row and the allocation is the cost being measured.
pub fn class_of(style: &Style, out: &mut String) {
    out.push('g');
    match style.role {
        Role::Body => {}
        Role::Heading(level) => {
            out.push_str(" h");
            out.push((b'0' + level.clamp(1, 6)) as char);
        }
        Role::Code => out.push_str(" code"),
        Role::Link => out.push_str(" link"),
        Role::Mark(color) => {
            out.push_str(" mark");
            if let Some(c) = color {
                out.push(' ');
                out.push_str(mark_color(c));
            }
        }
        Role::ListMarker => out.push_str(" marker"),
        Role::ListIndent => out.push_str(" indent"),
        Role::QuoteGutter => out.push_str(" gutter"),
        Role::Rule => out.push_str(" rule"),
        Role::Delimiter => out.push_str(" delim"),
        Role::Image => out.push_str(" image"),
        Role::Math => out.push_str(" math"),
    }
    if style.bold {
        out.push_str(" b");
    }
    if style.italic {
        out.push_str(" i");
    }
    if style.underline {
        out.push_str(" u");
    }
    if style.strikethrough {
        out.push_str(" s");
    }
    match style.baseline {
        Baseline::Normal => {}
        Baseline::Super => out.push_str(" sup"),
        Baseline::Sub => out.push_str(" sub"),
    }
    // Only meaningful beside `Role::Code`, and `None` everywhere else, so a
    // document without a highlighted fence never pays for the branch.
    if let Some(token) = style.token {
        out.push(' ');
        out.push_str(token_class(token));
    }
}

/// The seven names a [`MarkColor`] takes, as class suffixes. The same seven a
/// [`leaf_core::TextColor`] carries, so a stylesheet spells each red once.
fn mark_color(c: MarkColor) -> &'static str {
    match c {
        MarkColor::Red => "red",
        MarkColor::Orange => "orange",
        MarkColor::Yellow => "yellow",
        MarkColor::Green => "green",
        MarkColor::Blue => "blue",
        MarkColor::Purple => "purple",
        MarkColor::Brown => "brown",
    }
}

/// syntect's eight-way class per code glyph. leaf-core picks the class; the
/// colour is this frontend's, as it is every frontend's.
fn token_class(t: Token) -> &'static str {
    match t {
        Token::Punctuation => "t-punct",
        Token::Keyword => "t-kw",
        Token::Entity => "t-entity",
        Token::Support => "t-support",
        Token::Constant => "t-const",
        Token::String => "t-str",
        Token::Comment => "t-comment",
        Token::Invalid => "t-invalid",
    }
}

/// The stylesheet the classes above are written against. A spike's worth of it
/// — enough that the rendered document is legible in a browser and that the
/// class strings are real, not enough to be a theme.
pub const STYLESHEET: &str = r#"
.leaf { font: 15px/1.5 ui-monospace, monospace; white-space: pre; tab-size: 4;
        caret-color: transparent; outline: none; }
.leaf .row { position: relative; min-height: 1.5em; }
.leaf .row.code { background: #f6f8fa; }
.leaf .row.decoration { color: #9aa0a6; }
.leaf .caret { position: absolute; width: 2px; background: #1a73e8;
               top: 0.1em; bottom: 0.1em; animation: leaf-blink 1s step-end infinite; }
@keyframes leaf-blink { 50% { opacity: 0; } }
.leaf .sel { background: #b3d4fd; }
.leaf .h1 { font-size: 2em; font-weight: 700; }
.leaf .h2 { font-size: 1.6em; font-weight: 700; }
.leaf .h3 { font-size: 1.3em; font-weight: 700; }
.leaf .h4, .leaf .h5, .leaf .h6 { font-weight: 700; }
.leaf .b { font-weight: 700; }
.leaf .i { font-style: italic; }
.leaf .u { text-decoration: underline; }
.leaf .s { text-decoration: line-through; }
.leaf .sup { vertical-align: super; font-size: 0.8em; }
.leaf .sub { vertical-align: sub; font-size: 0.8em; }
.leaf .link { color: #1a73e8; text-decoration: underline; }
.leaf .delim { color: #9aa0a6; }
.leaf .marker, .leaf .gutter { color: #5f6368; }
.leaf .rule { color: #9aa0a6; }
.leaf .mark { background: #fff2a8; }
.leaf .mark.red { background: #ffd4d4; }
.leaf .mark.orange { background: #ffe0c2; }
.leaf .mark.yellow { background: #fff2a8; }
.leaf .mark.green { background: #ccf0d2; }
.leaf .mark.blue { background: #cfe3fd; }
.leaf .mark.purple { background: #e6d6fb; }
.leaf .mark.brown { background: #e8dcc8; }
.leaf .t-punct { color: #5f6368; }
.leaf .t-kw { color: #a626a4; }
.leaf .t-entity { color: #4078f2; }
.leaf .t-support { color: #0184bc; }
.leaf .t-const { color: #986801; }
.leaf .t-str { color: #50a14f; }
.leaf .t-comment { color: #9aa0a6; font-style: italic; }
.leaf .t-invalid { color: #e45649; text-decoration: underline wavy; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_text_is_one_class() {
        let mut s = String::new();
        class_of(&Style::default(), &mut s);
        assert_eq!(s, "g");
    }

    #[test]
    fn a_bold_heading_names_its_level() {
        let mut s = String::new();
        class_of(&Style::default().role(Role::Heading(2)).bold(), &mut s);
        assert_eq!(s, "g h2 b");
    }

    #[test]
    fn a_code_glyph_carries_its_token() {
        let mut style = Style::default().role(Role::Code);
        style.token = Some(Token::Keyword);
        let mut s = String::new();
        class_of(&style, &mut s);
        assert_eq!(s, "g code t-kw");
    }
}
