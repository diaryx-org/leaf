//! The document's text in each window's accessibility tree.
//!
//! Blitz builds the tree from the page, where the editor's rows are divs of
//! static text. Each tree built passes through the hook installed here,
//! which hands the editor's element ([`leaf_dioxus::TEXT_ID`]) to the
//! editor to make a text area of — its value, text runs and selection — in
//! place of the nodes under it. What assistive tech asks of that text area
//! comes back through [`action`].

use std::collections::HashSet;

use accesskit::{ActionRequest, NodeId, TreeUpdate};
use blitz_dom::BaseDocument;
use blitz_shell::View;
use dioxus_native::winit::window::WindowId;
use leaf_dioxus::{TEXT_ID, is_text_run};

use crate::app::page;
use crate::renderer::Renderer;

/// What AccessKit calls the window: the root Blitz gives the tree.
const WINDOW: NodeId = NodeId(u64::MAX);

/// Pass every tree built for window `id` through [`describe`].
pub fn install(view: &mut View<Renderer>, id: WindowId) {
    view.set_accessibility_tree_hook(move |doc, tree| describe(id, doc, tree));
}

/// Make the editor's element in `tree` the document's text area, if the
/// window has an editor.
fn describe(id: WindowId, doc: &BaseDocument, tree: &mut TreeUpdate) {
    let Some(ed) = page(id).and_then(|p| p.editor()) else {
        return;
    };
    let Some(text) = doc.get_element_by_id(TEXT_ID) else {
        return;
    };
    let text_id = NodeId(text.as_u64());
    let Some(at) = tree.nodes.iter().position(|(n, _)| *n == text_id) else {
        return;
    };
    let Some(runs) = ed.describe(&mut tree.nodes[at].1) else {
        return;
    };
    // The rows' nodes go: the runs stand for them.
    let mut under = HashSet::new();
    let mut stack: Vec<_> = doc
        .get_node(text)
        .map_or(Vec::new(), |n| n.children.to_vec());
    while let Some(n) = stack.pop() {
        under.insert(n.as_u64());
        if let Some(node) = doc.get_node(n) {
            stack.extend(node.children.iter().copied());
        }
    }
    tree.nodes.retain(|(n, _)| !under.contains(&n.0));
    tree.nodes.extend(runs);
    // The editor has the keys unless one of the page's controls has them,
    // so it has the focus too: the caret VoiceOver follows is its.
    let focus = tree.focus;
    let root = NodeId(doc.root_node().id.as_u64());
    if focus == WINDOW
        || focus == root
        || under.contains(&focus.0)
        || !tree.nodes.iter().any(|(n, _)| *n == focus)
    {
        tree.focus = text_id;
    }
}

/// Carry out what assistive tech asks of the editor's text area or one of
/// its runs, in window `id`: whether it was the editor's to do.
pub fn action(view: &View<Renderer>, id: WindowId, request: &ActionRequest) -> bool {
    let text = view.doc.inner().get_element_by_id(TEXT_ID);
    let target = request.target_node;
    if !is_text_run(target) && text.map(|t| NodeId(t.as_u64())) != Some(target) {
        return false;
    }
    page(id)
        .and_then(|p| p.with_editor(|ed, _| ed.accessibility_action(request)))
        .unwrap_or(false)
}
