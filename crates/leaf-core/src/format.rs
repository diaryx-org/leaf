//! The document languages leaf opens.
//!
//! Up to twig 4, `twig::Format` was an enum naming every language twig
//! parsed, and leaf passed it through as its own. Since 6.0 twig's core names
//! none: a language is a crate (`twig-markdown`, `twig-djot`, …) that
//! registers itself on first use and hands back an opaque `twig::Format`
//! handle. [`Format`] is leaf's own enum over the languages it links, under
//! the variant names it always had, and [`Format::twig`] is the handle twig
//! is called with.

use twig::Gesture;

/// A language a leaf document is written in.
///
/// `#[non_exhaustive]` as `twig::Format` was before it: a language leaf
/// learns later is not a breaking change to a host that matches on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    Markdown,
    Djot,
    Html,
    /// Parsed, and authorable in part; leaf never opens a file in it by
    /// extension (see `Doc::open`).
    Asciidoc,
    /// Parse-only: every gesture refuses.
    Xml,
}

impl Format {
    /// twig's handle for this language, registering it on first use.
    pub fn twig(self) -> twig::Format {
        match self {
            Format::Markdown => twig_markdown::format(),
            Format::Djot => twig_djot::format(),
            Format::Html => twig_html::format(),
            Format::Asciidoc => twig_asciidoc::format(),
            Format::Xml => twig_xml::format(),
        }
    }

    /// twig's name for this language (`"markdown"`, `"djot"`, …).
    pub fn name(self) -> &'static str {
        self.twig().name()
    }

    /// Whether this format can spell `gesture` with no features laid over it.
    /// What a leaf document can spell is [`Format::supports_with`] under
    /// leaf's own flags — [`Capabilities::of`](crate::Capabilities::of)
    /// answers that per control.
    pub fn supports(self, gesture: Gesture) -> bool {
        self.twig().supports(gesture)
    }

    /// Whether this format can spell `gesture` under the feature mask `flags`
    /// (twig's `Format::feature_flags`).
    pub fn supports_with(self, flags: u32, gesture: Gesture) -> bool {
        self.twig().supports_flags(flags, gesture)
    }

    /// Whether this format can be authored into at all — `false` for a
    /// parse-only format (XML).
    pub fn is_authorable(self) -> bool {
        self.twig().is_authorable()
    }
}

impl From<Format> for twig::Format {
    fn from(format: Format) -> Self {
        format.twig()
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
