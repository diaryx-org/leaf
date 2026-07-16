//! The clipboard's rich flavor: source ⇄ HTML, both directions through twig.
//!
//! twig parses HTML and renders it, so a copy can publish `text/html` beside the
//! Markdown source and a paste can take the HTML back. What twig does *not* do is
//! defend itself against the clipboard: `Format::Html` is a document parser aimed
//! at real HTML documents, and the HTML on a pasteboard is neither real nor a
//! document. It is a fragment, wrapped in whatever the source app felt like —
//! Word ships its `<head>` and a stylesheet, Google Docs a `<meta>` and a
//! `<b style="font-weight:normal">` that means the opposite of bold, Slack a
//! `<div>` per line.
//!
//! Handed that raw, twig is faithful to a fault and the Markdown comes out wrong
//! in two distinct ways, both verified against twig-doc 2.1.0:
//!
//!   - **Djot leaks in.** A `<div>` is a fenced div, and the Markdown serializer
//!     spells it `::: … :::` — syntax Markdown does not have, so Slack's HTML
//!     pastes as literal colons around your text.
//!   - **Unknown elements pass through raw.** `<meta>`, `<script>`, `<style>`,
//!     `<table>` (twig builds no table from HTML) survive into the output as the
//!     tags themselves, dropping markup into a prose document.
//!
//! So the paste direction is three steps, not one: [`sanitize`] rewrites the
//! clipboard's dialect into the subset twig reads faithfully, twig converts, and
//! [`unfaithful`] reads the result back to catch what step one didn't anticipate.
//! A failure at any step is `None`, which the caller answers by pasting the plain
//! flavor instead — the whole point of carrying two.

use twig::{Document, Format};

/// Render a `format` source fragment to HTML, for the clipboard's `text/html`.
///
/// `None` when twig can't parse or render it, which for a *substring* of a live
/// document is a real possibility and not an error worth reporting: the caller
/// still has the plain flavor, which is what the user copied.
pub(crate) fn render_fragment(source: &str, format: Format) -> Option<String> {
    let mut doc = Document::parse_str(source, format).ok()?;
    let html = String::from_utf8(doc.render_html().ok()?).ok()?;
    let html = html.trim();
    (!html.is_empty()).then(|| html.to_string())
}

/// Convert clipboard `html` to `format`'s source syntax, or `None` when it does
/// not survive the trip well enough to paste (see [`unfaithful`]).
pub(crate) fn parse_fragment(html: &str, format: Format) -> Option<String> {
    let cleaned = sanitize(html);
    let mut doc = Document::parse_str(&cleaned, Format::Html).ok()?;
    let source = String::from_utf8(doc.serialize(format).ok()?).ok()?;

    // twig's serializers terminate a block with a newline, because a document
    // ends in one. A paste is not a document — it lands at a caret, usually
    // mid-sentence — so the terminator would push the text after the caret onto
    // its own line. Only `\n` is trimmed: a trailing *space* pair is Markdown's
    // hard line break and belongs to the text.
    let source = source.trim_matches('\n');
    if source.is_empty() || unfaithful(source) {
        return None;
    }
    Some(source.to_string())
}

/// Strip a sole wrapping `<p>`, for a selection that lives inside one block.
///
/// `**bold**` renders as `<p><strong>bold</strong></p>`, and the paragraph is an
/// artifact of parsing the fragment standalone rather than anything the user
/// selected — pasted into Docs it breaks the line where they copied a word. The
/// caller decides whether the selection is inline (it has the document; this has
/// only the fragment); this is the textual half, and it declines when the render
/// is anything but one paragraph, so a multi-block selection keeps its structure.
pub(crate) fn strip_sole_paragraph(html: String) -> String {
    let trimmed = html.trim();
    let Some(inner) = trimmed.strip_prefix("<p>").and_then(|h| h.strip_suffix("</p>")) else {
        return html;
    };
    // A second `<p>` means the render is more than the one paragraph this is
    // allowed to unwrap (`<p>a</p>\n<p>b</p>` also starts `<p>` and ends `</p>`).
    match inner.contains("<p>") {
        true => html,
        false => inner.to_string(),
    }
}

/// Does this converted source still carry markup twig didn't understand?
///
/// Read on the *output*, not the input, because that is where the two failures
/// show up in the same shape however they got there — a `:::` fence marker or a
/// block that is still a raw tag. Both mean the paste would put visible markup
/// into a prose document, which is worse than the plain flavor by any measure.
///
/// Deliberately checks block starts only. Markdown that legitimately contains
/// inline HTML is left alone, and every faithful conversion — a list, a heading,
/// a quote, a fenced code block — begins its lines with something else.
fn unfaithful(source: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(":::")
            || line
                .strip_prefix('<')
                .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/'))
    })
}

/// Elements whose *content* is not prose and must go with them. `head` is Word's
/// (and arboard's own wrapper's); `script`/`style` are every rich web page's.
const DROP_TREE: [&str; 4] = ["script", "style", "head", "title"];

/// Void elements that carry no text and that twig would pass through raw.
const DROP_TAG: [&str; 3] = ["meta", "link", "base"];

/// Rewrite clipboard HTML into the subset twig converts faithfully.
///
/// Not a general sanitizer and not a security boundary — twig renders no HTML we
/// hand back. It is exactly the set of dialect fixes the pasteboards in practice
/// require, each one verified against twig-doc 2.1.0:
///
///   - comments and doctypes go (Word's `<!--[if gte mso 9]>` conditionals ride
///     in comments, and twig keeps comments as raw blocks);
///   - `DROP_TREE` / `DROP_TAG` elements go, with their content where it isn't
///     prose;
///   - `<div>` becomes `<p>`, which is what a div-per-line pasteboard means and
///     the only way to keep Djot's `:::` out of Markdown;
///   - a `<span>` that spells bold or italic in CSS becomes `<strong>`/`<em>`,
///     because twig reads no stylesheet and Google Docs writes *all* of its
///     emphasis this way — without this, pasting from Docs silently loses it;
///   - Docs' `<b style="font-weight:normal">` document wrapper goes, or the
///     whole paste comes out bold.
///
/// Unknown elements are left for twig, and for [`unfaithful`] to catch if twig
/// doesn't know them either.
fn sanitize(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    // The close tag owed to each open element this rewrote, innermost last —
    // rewriting `<div>` to `<p>` is only half a rewrite until `</div>` becomes
    // `</p>`, and a dropped `<b>` wrapper whose `</b>` survives re-bolds the very
    // text the drop was for. Keyed by name and popped by name, so the malformed
    // nesting a pasteboard actually ships resolves to the nearest open match.
    // `None` is "close it as it came" — an element this left alone, tracked all
    // the same so a nested close pops it rather than an outer rewrite of the
    // same name.
    let mut owed: Vec<(String, Option<&'static str>)> = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let Some(lt) = html[i..].find('<').map(|p| i + p) else {
            out.push_str(&html[i..]);
            break;
        };
        out.push_str(&html[i..lt]);

        if html[lt..].starts_with("<!--") {
            i = html[lt + 4..]
                .find("-->")
                .map_or(bytes.len(), |p| lt + 4 + p + 3);
            continue;
        }
        if html[lt..].starts_with("<!") {
            i = html[lt..].find('>').map_or(bytes.len(), |p| lt + p + 1);
            continue;
        }

        let Some(tag) = parse_tag(html, lt) else {
            // A bare `<` that opens no tag is text — `a < b` — and belongs in the
            // output as itself.
            out.push('<');
            i = lt + 1;
            continue;
        };
        i = tag.end;

        if tag.close {
            match owed.iter().rposition(|(n, _)| *n == tag.name) {
                Some(at) => {
                    match owed[at].1 {
                        Some(close) => out.push_str(close),
                        None => out.push_str(&html[lt..tag.end]),
                    }
                    owed.truncate(at);
                }
                None if DROP_TREE.contains(&tag.name.as_str())
                    || DROP_TAG.contains(&tag.name.as_str()) => {}
                None => out.push_str(&html[lt..tag.end]),
            }
            continue;
        }

        if DROP_TREE.contains(&tag.name.as_str()) {
            i = skip_tree(html, tag.end, &tag.name);
            continue;
        }
        if DROP_TAG.contains(&tag.name.as_str()) {
            continue;
        }

        let rewrite = match tag.name.as_str() {
            "div" => Some(("<p>", "</p>")),
            "span" => match css_emphasis(&tag.attrs) {
                // A span carrying no emphasis carries nothing twig needs: it is
                // a styling hook, and dropping it is what keeps `<span>a</span>
                // <span>b</span>` from becoming two blocks.
                None => Some(("", "")),
                Some(pair) => Some(pair),
            },
            // `font-weight:normal` on a `<b>` is Google Docs' whole-document
            // wrapper, which means "not bold" and is the opposite of the tag.
            "b" | "strong" if is_unbolded(&tag.attrs) => Some(("", "")),
            _ => None,
        };

        match rewrite {
            Some((open, close)) => {
                out.push_str(open);
                if !tag.self_closing {
                    owed.push((tag.name, Some(close)));
                }
            }
            None => {
                out.push_str(&html[lt..tag.end]);
                if !tag.self_closing && !VOID.contains(&tag.name.as_str()) {
                    owed.push((tag.name, None));
                }
            }
        }
    }

    // An element left open by the fragment (`<p>unclosed <strong>bold`) owes
    // nothing: twig closes what the input didn't, and the tracking exists to
    // match closes that *do* arrive.
    out
}

/// HTML's void elements — no content, no close tag, so nothing to owe.
const VOID: [&str; 9] = ["br", "img", "hr", "input", "col", "area", "source", "wbr", "embed"];

struct Tag {
    name: String,
    attrs: String,
    close: bool,
    self_closing: bool,
    /// One past the `>`.
    end: usize,
}

/// Parse the tag opening at `lt` (which must be a `<`), or `None` when what
/// follows isn't a tag name at all.
fn parse_tag(html: &str, lt: usize) -> Option<Tag> {
    let rest = &html[lt + 1..];
    let close = rest.starts_with('/');
    let after_slash = lt + 1 + close as usize;
    let name_len = html[after_slash..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == ':' || c == '-' || c == '_'))
        .unwrap_or(html.len() - after_slash);
    if name_len == 0 {
        return None;
    }
    let name = html[after_slash..after_slash + name_len].to_ascii_lowercase();

    // Find the `>` that ends the tag, stepping over quoted attribute values —
    // Word writes `content="text/html; charset=utf-8"`, and a naive scan for `>`
    // is fine there but not for the `style="…>…"` a CSS author can legally emit.
    let mut j = after_slash + name_len;
    let b = html.as_bytes();
    let mut quote: Option<u8> = None;
    while j < b.len() {
        match (quote, b[j]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, c @ (b'"' | b'\'')) => quote = Some(c),
            (None, b'>') => break,
            (None, _) => {}
        }
        j += 1;
    }
    if j >= b.len() {
        return None;
    }
    let attrs = &html[after_slash + name_len..j];
    Some(Tag {
        name,
        attrs: attrs.to_string(),
        close,
        self_closing: attrs.trim_end().ends_with('/'),
        end: j + 1,
    })
}

/// Skip past `</name>`, or to the end when the fragment never closes it.
fn skip_tree(html: &str, from: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    let lower = html.to_ascii_lowercase();
    match lower[from..].find(&needle) {
        Some(p) => lower[from + p..]
            .find('>')
            .map_or(html.len(), |q| from + p + q + 1),
        None => html.len(),
    }
}

/// The `style` attribute's value, lowercased.
fn style_of(attrs: &str) -> String {
    let lower = attrs.to_ascii_lowercase();
    let Some(at) = lower.find("style") else {
        return String::new();
    };
    let Some(eq) = lower[at..].find('=').map(|p| at + p + 1) else {
        return String::new();
    };
    let rest = lower[eq..].trim_start();
    match rest.starts_with(['"', '\'']) {
        true => rest[1..]
            .find(rest.chars().next().unwrap())
            .map_or(String::new(), |e| rest[1..1 + e].to_string()),
        false => rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// The emphasis a `<span>`'s CSS spells, as the tag twig would have wanted.
///
/// Bold before italic: a span is one tag and can only become one, and a rewrite
/// that kept both would have to invent nesting the source never had. Bold is the
/// commoner of the two by a distance, so it wins the tie.
fn css_emphasis(attrs: &str) -> Option<(&'static str, &'static str)> {
    let style = style_of(attrs);
    let weight = css_value(&style, "font-weight");
    let bold = weight == Some("bold".into())
        || weight
            .as_deref()
            .and_then(|w| w.parse::<u32>().ok())
            .is_some_and(|w| w >= 600);
    if bold {
        return Some(("<strong>", "</strong>"));
    }
    match css_value(&style, "font-style").as_deref() {
        Some("italic") | Some("oblique") => Some(("<em>", "</em>")),
        _ => None,
    }
}

/// Is this `<b>`/`<strong>` explicitly styled *un*-bold (Docs' wrapper)?
fn is_unbolded(attrs: &str) -> bool {
    matches!(
        css_value(&style_of(attrs), "font-weight").as_deref(),
        Some("normal") | Some("400")
    )
}

/// One declaration's value out of a lowercased `style` attribute.
fn css_value(style: &str, prop: &str) -> Option<String> {
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        (k.trim() == prop).then(|| v.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> Option<String> {
        parse_fragment(html, Format::Markdown)
    }
    fn html(src: &str) -> Option<String> {
        render_fragment(src, Format::Markdown)
    }

    // ── source → HTML ────────────────────────────────────────────────────────

    #[test]
    fn renders_inline_and_block_markdown() {
        assert_eq!(html("a **b** c").as_deref(), Some("<p>a <strong>b</strong> c</p>"));
        assert_eq!(html("- one\n- two").as_deref(), Some("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"));
        assert_eq!(html("# head").as_deref(), Some("<h1>head</h1>"));
    }

    #[test]
    fn empty_selection_renders_nothing() {
        assert_eq!(html("   "), None);
    }

    #[test]
    fn strip_sole_paragraph_unwraps_only_a_lone_paragraph() {
        assert_eq!(strip_sole_paragraph("<p><strong>b</strong></p>".into()), "<strong>b</strong>");
        // Two blocks: the wrapper is structure the user selected, not an artifact.
        let two = "<p>a</p>\n<p>b</p>".to_string();
        assert_eq!(strip_sole_paragraph(two.clone()), two);
        let list = "<ul>\n<li>one</li>\n</ul>".to_string();
        assert_eq!(strip_sole_paragraph(list.clone()), list);
    }

    // ── HTML → source ────────────────────────────────────────────────────────

    #[test]
    fn converts_clean_fragments() {
        assert_eq!(md("<ul><li>one</li></ul>").as_deref(), Some("- one"));
        assert_eq!(md("<p>a <strong>b</strong> c</p>").as_deref(), Some("a **b** c"));
        assert_eq!(md("<strong>bold</strong>").as_deref(), Some("**bold**"));
        assert_eq!(md("<h1>head</h1>").as_deref(), Some("# head"));
        assert_eq!(
            md(r#"<a href="https://x.dev">l</a>"#).as_deref(),
            Some("[l](https://x.dev)")
        );
    }

    #[test]
    fn round_trips_through_html() {
        for src in ["a **b** and [l](https://x.dev)", "- one\n- two", "# head", "> quote"] {
            let rendered = html(src).expect("render");
            assert_eq!(md(&rendered).as_deref(), Some(src), "round trip of {src:?}");
        }
    }

    #[test]
    fn paste_does_not_carry_the_serializer_s_trailing_newline() {
        // A paste lands at a caret: a trailing newline would break the line.
        assert_eq!(md("<p>word</p>").as_deref(), Some("word"));
    }

    // ── HTML from the wild ───────────────────────────────────────────────────

    #[test]
    fn google_docs_paste_keeps_its_emphasis_and_drops_the_wrapper() {
        // The real shape: a `<meta>`, a `font-weight:normal` `<b>` around the
        // whole document, and emphasis spelled only in CSS on `<span>`s.
        let clip = r#"<meta charset='utf-8'><b style="font-weight:normal;" id="docs-internal-guid-9c1">"#.to_string()
            + r#"<p dir="ltr" style="line-height:1.38;margin-top:0pt;"><span style="font-size:11pt;font-family:Arial;font-weight:400;">Hello </span>"#
            + r#"<span style="font-size:11pt;font-weight:700;">bold</span><span style="font-size:11pt;"> world</span></p></b>"#;
        assert_eq!(md(&clip).as_deref(), Some("Hello **bold** world"));
    }

    #[test]
    fn word_paste_drops_the_head_and_the_office_cruft() {
        let clip = r#"<html xmlns:o="urn:schemas-microsoft-com:office:office"><head>"#.to_string()
            + r#"<meta http-equiv=Content-Type content="text/html; charset=utf-8">"#
            + r#"<meta name=Generator content="Microsoft Word 15">"#
            + r#"<!--[if gte mso 9]><xml><o:OfficeDocumentSettings/></xml><![endif]-->"#
            + r#"<style><!-- p.MsoNormal {margin:0in;font-size:11.0pt;} --></style></head>"#
            + r#"<body lang=EN-US><p class=MsoNormal><span style='font-size:12.0pt'>Word <b>bold</b> text<o:p></o:p></span></p></body></html>"#;
        assert_eq!(md(&clip).as_deref(), Some("Word **bold** text"));
    }

    #[test]
    fn div_per_line_html_becomes_paragraphs_not_djot_fences() {
        // Slack's shape. Raw, twig spells the div `::: … :::` into Markdown.
        assert_eq!(
            md(r#"<div class="p-rich_text_section">hi <b>there</b></div>"#).as_deref(),
            Some("hi **there**")
        );
        assert_eq!(md("<div>a</div><div>b</div>").as_deref(), Some("a\n\nb"));
    }

    #[test]
    fn arboard_s_own_wrapper_survives_the_round_trip() {
        // What leaf's *own* copy puts on the pasteboard: arboard wraps the HTML
        // in `<html><head><meta …></head><body>`, so leaf→leaf paste reads this.
        let clip = r#"<html><head><meta http-equiv="content-type" content="text/html; charset=utf-8"></head><body><p>a <strong>b</strong> c</p></body></html>"#;
        assert_eq!(md(clip).as_deref(), Some("a **b** c"));
    }

    #[test]
    fn script_and_style_never_reach_the_document() {
        assert_eq!(
            md("<p>ok</p><script>alert(1)</script><style>p{color:red}</style>").as_deref(),
            Some("ok")
        );
    }

    #[test]
    fn fragment_comments_are_dropped() {
        assert_eq!(md("<!--StartFragment--><p>frag</p><!--EndFragment-->").as_deref(), Some("frag"));
    }

    #[test]
    fn plain_text_and_entities_convert() {
        assert_eq!(md("just some plain text").as_deref(), Some("just some plain text"));
        assert_eq!(md("<p>a &amp; b &lt;c&gt;</p>").as_deref(), Some("a & b <c>"));
    }

    #[test]
    fn unclosed_tags_are_tolerated() {
        assert_eq!(md("<p>unclosed <strong>bold").as_deref(), Some("unclosed **bold**"));
    }

    #[test]
    fn code_and_hard_breaks_survive() {
        assert_eq!(md("<pre><code>fn x() {}</code></pre>").as_deref(), Some("```\nfn x() {}\n```"));
        assert_eq!(md("<p>line1<br>line2</p>").as_deref(), Some("line1  \nline2"));
    }

    // ── the bad cases fall back ──────────────────────────────────────────────

    #[test]
    fn a_table_declines_rather_than_pasting_raw_html() {
        // twig builds no table from HTML — it passes the tags through — and raw
        // `<table>` markup in a prose document is worse than the plain flavor.
        assert_eq!(md("<table><tr><td>a</td><td>b</td></tr></table>"), None);
    }

    #[test]
    fn empty_and_whitespace_html_declines() {
        assert_eq!(md(""), None);
        assert_eq!(md("   "), None);
        assert_eq!(md("<meta charset='utf-8'>"), None);
    }

    #[test]
    fn unfaithful_reads_the_two_failure_shapes() {
        assert!(unfaithful("::: \n  x\n:::"));
        assert!(unfaithful("<table><tr><td>a"));
        assert!(unfaithful("ok\n\n<script>alert(1)"));
        // …and leaves every faithful conversion alone.
        for good in ["a **b** c", "- one", "# head", "> quote", "```\nx\n```", "![p](u)"] {
            assert!(!unfaithful(good), "{good:?} should be faithful");
        }
    }

    // ── the sanitizer itself ─────────────────────────────────────────────────

    #[test]
    fn sanitize_rewrites_divs_and_keeps_nesting_straight() {
        assert_eq!(sanitize("<div>a</div>"), "<p>a</p>");
        assert_eq!(sanitize("<div><b>a</b></div>"), "<p><b>a</b></p>");
        // A nested `</b>` pops the inner `<b>`, not the dropped wrapper.
        assert_eq!(
            sanitize(r#"<b style="font-weight:normal"><b>x</b></b>"#),
            "<b>x</b>"
        );
    }

    #[test]
    fn sanitize_maps_css_emphasis_onto_tags() {
        assert_eq!(
            sanitize(r#"<span style="font-weight:700">b</span>"#),
            "<strong>b</strong>"
        );
        assert_eq!(sanitize(r#"<span style="font-weight:bold">b</span>"#), "<strong>b</strong>");
        assert_eq!(sanitize(r#"<span style="font-style:italic">i</span>"#), "<em>i</em>");
        // No emphasis: a styling hook with nothing for twig in it.
        assert_eq!(sanitize(r#"<span style="color:red">x</span>"#), "x");
        assert_eq!(sanitize("<span>x</span>"), "x");
    }

    #[test]
    fn sanitize_steps_over_a_gt_inside_a_quoted_attribute() {
        assert_eq!(sanitize(r#"<span style="font-family:'a>b'">x</span>"#), "x");
    }

    #[test]
    fn sanitize_leaves_a_bare_less_than_as_text() {
        assert_eq!(sanitize("a < b"), "a < b");
    }

    #[test]
    fn sanitize_leaves_unknown_elements_for_twig() {
        assert_eq!(sanitize("<figure>x</figure>"), "<figure>x</figure>");
    }
}
