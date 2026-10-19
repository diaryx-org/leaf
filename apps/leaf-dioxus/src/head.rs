//! Each window's `document` context: what `dioxus::document` — `Style`,
//! `Title`, `Link`, `Meta` — and a component library's injected stylesheet
//! write to the page's `<head>`.
//!
//! dioxus-native's `launch` provides its own, but keeps it private, and this
//! app builds its windows itself (`app.rs`). Without one, a `<style>` meant
//! for the head goes nowhere. This one asks the handler to put each element
//! into the window's Blitz document on its next turn, as dioxus-native's does.

use dioxus_document::{Document, Eval, NoOpDocument};
use dioxus_native::winit::window::WindowId;

use crate::app::{self, AppEvent};

pub struct Head {
    pub window: WindowId,
}

impl Document for Head {
    /// No JavaScript runs under Blitz.
    fn eval(&self, js: String) -> Eval {
        NoOpDocument.eval(js)
    }

    fn create_head_element(
        &self,
        name: &str,
        attributes: &[(&str, String)],
        contents: Option<String>,
    ) {
        app::post(AppEvent::Head(
            self.window,
            name.to_string(),
            attributes
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            contents,
        ));
    }

    fn set_title(&self, title: String) {
        self.create_head_element("title", &[], Some(title));
    }

    fn create_meta(&self, props: dioxus_document::MetaProps) {
        self.create_head_element("meta", &props.attributes(), None);
    }

    fn create_script(&self, props: dioxus_document::ScriptProps) {
        self.create_head_element("script", &props.attributes(), props.script_contents().ok());
    }

    fn create_style(&self, props: dioxus_document::StyleProps) {
        self.create_head_element("style", &props.attributes(), props.style_contents().ok());
    }

    fn create_link(&self, props: dioxus_document::LinkProps) {
        self.create_head_element("link", &props.attributes(), None);
    }

    fn create_head_component(&self) -> bool {
        true
    }
}
