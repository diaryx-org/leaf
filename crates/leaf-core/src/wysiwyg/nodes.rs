//! Classifying AST nodes: directives, HTML elements, run spans, inline kinds.

use super::*;

/// Is this `container` node a *directive* (`:::note{…}`, `::embed{…}`,
/// `:vis[…]`) rather than an HTML element (`<video>`, `<picture>`, `<div>`)?
///
/// twig 2.8 folded `div`/`span`/`directive`/`element` into one `container` kind,
/// and left nothing that separated them: `kind`, `name` and `directive_form` all
/// agree, field for field, on an HTML `<div>` and a Markdown `:::div`. Leaf
/// answered it by sniffing the span for whichever of `:` or `<` came first.
/// twig 3.0 records the answer at parse time as [`ContainerOrigin`], so this is
/// now the parser's own knowledge rather than a guess rebuilt from the bytes it
/// consumed.
pub(crate) fn container_is_directive(node: &FlatNode) -> bool {
    node.origin == Some(ContainerOrigin::Directive)
}

/// The tag a `container` node carries when it is an HTML element rather than a
/// directive — `Some("video")` for a promoted `<video>`, `None` for a `:::note`
/// or for any node that is not a container at all.
pub(crate) fn element_tag(node: &FlatNode) -> Option<&str> {
    (node.origin == Some(ContainerOrigin::Element))
        .then_some(node.name.as_deref())
        .flatten()
}

/// A leaf directive's name and whatever attributes are not part of spelling
/// it — the two things a [`DirectiveMark`] carries, which twig hands back
/// differently per format and which a frontend must not be able to tell apart.
///
/// Markdown's `::page-break` is a `Leaf`-form directive *named* `page-break`
/// with no attributes, and this returns it verbatim. Djot has no leaf form:
/// `insert_directive` writes the same document as an empty `::: page-break`
/// fence, whose container is anonymous (a djot div carries no name) and whose
/// name arrives as a class. Where the node has no name of its own, then, the
/// name is the word on the fence line when there is one — djot appends it
/// *after* the classes of an attribute line above (`{.wide}` over `:::
/// x-card` reads back as `class="wide x-card"`), so it is found by what the
/// fence says rather than by position — and otherwise, for a bare `:::` under
/// `{.page-break .wide}`, the first class. Whatever else the class said stays
/// an attribute.
pub(super) fn leaf_directive_identity(
    node: &FlatNode,
    source: &str,
) -> (String, Vec<(String, Option<String>)>) {
    let named = node.name.clone().unwrap_or_default();
    if !named.is_empty() {
        return (named, node.attrs.clone());
    }
    let class = node
        .attrs
        .iter()
        .find(|(k, _)| k == "class")
        .and_then(|(_, v)| v.as_deref())
        .unwrap_or_default();
    let mut tokens = class.split_whitespace().collect::<Vec<_>>();
    let fence_word = source
        .get(node.span.start..)
        .and_then(|rest| rest.lines().next())
        .map(|line| line.trim_start_matches(['>', ' ', '\t', ':']).trim())
        .and_then(|word| word.split_whitespace().next());
    let at = fence_word
        .and_then(|word| tokens.iter().rposition(|t| *t == word))
        .unwrap_or(0);
    if tokens.is_empty() {
        return (named, node.attrs.clone());
    }
    let name = tokens.remove(at).to_string();
    let rest = tokens.join(" ");
    let attrs = node
        .attrs
        .iter()
        .filter_map(|(k, v)| {
            if k != "class" {
                return Some((k.clone(), v.clone()));
            }
            (!rest.is_empty()).then(|| (k.clone(), Some(rest.clone())))
        })
        .collect();
    (name, attrs)
}

/// Is this inline `container` an **attributed span** — the node leaf's run-level
/// vocabulary rides — rather than a named directive?
///
/// The four formats spell one span four ways and twig hands the name back for
/// two of them: HTML's and Markdown's `<span …>` arrive named `span` with
/// `Element` origin, while djot's `[text]{…}` and AsciiDoc's `[.a]#text#`
/// arrive anonymous (an empty name) with `Directive` origin. All four are the
/// same node to `wrap_range_attrs`, which is what writes them, so they are the
/// same node here.
///
/// A *named* directive is not one, whatever its name: a Markdown `:span[…]{…}`
/// is a directive the parser read as a directive, twig's own
/// `wrap_range_attrs` says so, and it keeps the handling it has.
///
/// **Anonymous is not enough**, and the form is what finishes the question:
/// a djot fenced div (`{.center}` / `:::` / … / `:::`) is anonymous too, with
/// the same `Directive` origin, and is a *block* — `Container` form against the
/// span's `Text`. Reading one as a span made every gesture and every query lie
/// about it: `set_text_color` over a word inside such a div copied the whole
/// div's attribute set — its `id` along with the rest — onto the new span, and
/// `alignment_at_caret` reported the div's `.center` as a *run's* answer while
/// the walker drew none. So the anonymous arm asks the form [`is_inline`] asks.
pub(crate) fn is_run_span(node: &FlatNode) -> bool {
    if node.kind != Kind::Container {
        return false;
    }
    match node.name.as_deref() {
        None | Some("") => node.directive_form == Some(DirectiveForm::Text),
        Some("span") => node.origin == Some(ContainerOrigin::Element),
        Some(_) => false,
    }
}

/// `base` with an attributed span's three run-level keys written over it — the
/// nearest-wins fold [`is_run_span`] describes, for one span. `faces` is the
/// build's intern table, as it is for [`Presentation::under`].
pub(super) fn run_style(node: &FlatNode, base: Style, faces: &RefCell<FaceTable>) -> Style {
    Style {
        size: FontSize::from_attrs(&node.attrs).or(base.size),
        font: faces
            .borrow_mut()
            .face_from_attrs(&node.attrs)
            .or(base.font),
        color: TextColor::from_attrs(&node.attrs).or(base.color),
        ..base
    }
}

pub(crate) fn is_inline(node: &FlatNode) -> bool {
    // A directive is inline only in its `text` form (`:name[label]{…}`); the
    // `leaf` and `container` forms are blocks. All three report the same `kind`,
    // so the form is the only thing telling them apart — and getting it wrong
    // costs a whole paragraph: a text directive misread as a block makes its
    // paragraph fail the "all children inline" test in `block`, and the line is
    // then walked as a container of blocks, rendering as empty rows with no
    // caret home at all.
    //
    // An HTML element shares the `container` kind, and twig sets the same form
    // on the two tags the lightweight formats have a generic spelling for: a
    // `<span>` is `Text` and a `<div>` is `Container`, while a `<video>` or a
    // `<picture>` has no form at all. So the form answers for an element as it
    // answers for a directive, and the origin is not consulted — which is what
    // makes a `<span …>` inside a paragraph an inline node.
    //
    // It has to. `wrap_range_attrs` spells an attributed run as exactly that
    // span in Markdown and HTML, and a paragraph holding one whose kids were
    // not all inline failed the test below and was walked as a container of
    // blocks: the text either side of the span rendered as nothing at all.
    if node.kind == Kind::Container {
        return node.directive_form == Some(DirectiveForm::Text);
    }
    is_inline_kind(&node.kind)
}

/// [`is_inline`] by kind alone — for the ancestor walks, whose `QueryMatch`es
/// carry no `directive_form`. It answers `false` for every directive, which its
/// callers must (and do) reconcile: they pair it with `is_block_container`,
/// which claims every directive, so the pair's verdict is the same one a form
/// would have given. Anything looking at a *directive itself* wants [`is_inline`]
/// and a real node.
pub(crate) fn is_inline_kind(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Str
            | Kind::SoftBreak
            | Kind::HardBreak
            | Kind::NonBreakingSpace
            | Kind::Emph
            | Kind::Strong
            | Kind::Mark
            | Kind::Insert
            | Kind::Delete
            | Kind::Verbatim
            | Kind::InlineMath
            | Kind::DisplayMath
            | Kind::Url
            | Kind::Email
            | Kind::Link
            | Kind::Image
            | Kind::SmartPunctuation
            | Kind::Superscript
            | Kind::Subscript
            | Kind::FootnoteReference
    )
}
