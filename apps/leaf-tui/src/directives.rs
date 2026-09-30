//! The directives this host names, inserts and draws: one, `::embed{src=…}`.
//!
//! A demonstration of the hook rather than a vocabulary — leaf adopts no name
//! here. The same kind is painted by the web demo, leaf-editor and the Android
//! app, as a titled card holding the URL, so all four show the host's side of
//! `docs/proposals/host-directives.md` doing one recognisable thing.

use std::sync::LazyLock;

use leaf_core::DirectiveInfo;
use leaf_ratatui::{DirectiveContent, DirectiveItem, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// The directive's name, as written in the document.
pub const EMBED: &str = "embed";

/// Every directive the palette offers to insert, in the order it lists them
/// under Insert. A `static` because the command table is `Copy` and its labels
/// `&'static str`: a row names its item by index, and the title it shows is
/// borrowed from here for as long as the program runs.
pub static CATALOGUE: LazyLock<Vec<DirectiveItem>> = LazyLock::new(|| {
    vec![DirectiveItem::ask(
        "directive.embed",
        EMBED,
        "Embed…",
        "⧉",
        "Embed URL",
        |reply| {
            let url = reply.trim();
            (!url.is_empty()).then(|| DirectiveContent {
                label: None,
                attrs: vec![("src".into(), Some(url.into()))],
            })
        },
    )]
});

/// The fewest columns a card is drawn in: room for its title in the top
/// border and a few characters of URL.
const MIN_CARD: usize = 16;

/// leaf-tui's [`leaf_ratatui::DirectiveRenderer`]: an embed with a `src` is a
/// bordered card, "Embed" in its top border and the URL on the line under it.
/// Anything else — another name, an embed with nothing to show — is left to
/// leaf's placeholder.
///
/// The card is as wide as the URL needs and no wider than it is given; a URL
/// longer than that is cut short with an ellipsis, since the card is one line
/// tall inside and the whole URL is in the source.
pub fn draw(info: &DirectiveInfo, width: u16, theme: &Theme) -> Option<Vec<Line<'static>>> {
    if info.name != EMBED {
        return None;
    }
    let src = info.attr("src").filter(|s| !s.is_empty())?;
    let width = width as usize;
    if width < 4 {
        return None;
    }
    // Two border columns and a space either side of the URL.
    let inner = (src.chars().count() + 2).clamp(MIN_CARD - 2, width - 2);
    let shown = fit(src, inner - 2);
    let border = Style::default().fg(theme.image_border);
    let title = " Embed ";
    let top = format!(
        "╭─{title}{}╮",
        "─".repeat(inner.saturating_sub(title.chars().count() + 1))
    );
    let pad = inner - 1 - shown.chars().count();
    Some(vec![
        Line::from(Span::styled(top, border)),
        Line::from(vec![
            Span::styled("│ ", border),
            Span::styled(
                shown,
                Style::default()
                    .fg(theme.link)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(format!("{}│", " ".repeat(pad)), border),
        ]),
        Line::from(Span::styled(format!("╰{}╯", "─".repeat(inner)), border)),
    ])
}

/// `s` in at most `cols` characters, the last one an ellipsis when it had to
/// be cut.
fn fit(s: &str, cols: usize) -> String {
    if s.chars().count() <= cols {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::{Doc, Format};

    fn embed(src: &str) -> DirectiveInfo {
        let doc_src = format!("::embed{{src=\"{src}\"}}\n");
        let mut doc = Doc::from_source(doc_src, Format::Markdown).unwrap();
        doc.view = leaf_core::View::Wysiwyg;
        doc.build_visual(80);
        doc.vmap.directives[0].clone()
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn an_embed_is_a_titled_card_holding_its_url() {
        let lines = draw(&embed("https://x.org/a"), 40, &Theme::dark()).unwrap();
        assert_eq!(
            text(&lines),
            [
                "╭─ Embed ─────────╮",
                "│ https://x.org/a │",
                "╰─────────────────╯",
            ]
        );
    }

    #[test]
    fn a_long_url_is_cut_to_the_width() {
        let lines = draw(&embed(&"a".repeat(80)), 20, &Theme::dark()).unwrap();
        let t = text(&lines);
        assert!(t.iter().all(|l| l.chars().count() == 20), "{t:?}");
        assert!(t[1].ends_with("a… │"), "{t:?}");
    }

    #[test]
    fn a_short_url_still_has_room_for_the_title() {
        let lines = draw(&embed("u"), 40, &Theme::dark()).unwrap();
        assert!(text(&lines)[0].contains(" Embed "));
    }

    #[test]
    fn anything_else_is_left_to_the_placeholder() {
        let mut doc =
            Doc::from_source("::other{src=\"u\"}\n\n::embed\n".into(), Format::Markdown).unwrap();
        doc.view = leaf_core::View::Wysiwyg;
        doc.build_visual(80);
        for info in &doc.vmap.directives {
            assert!(draw(info, 40, &Theme::dark()).is_none(), "{}", info.name);
        }
    }
}
