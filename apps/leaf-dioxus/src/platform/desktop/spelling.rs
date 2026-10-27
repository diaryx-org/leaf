//! The spelling checker: none yet, so nothing is underlined, no guesses are
//! offered, and every switch is off. Each function answers as a system with
//! no checker would.

use dioxus_native::winit::window::WindowId;

use crate::commands::{Command, Switch};
use crate::page::{Chrome, Page};

/// The switches that are on: none.
pub fn switches() -> Vec<Switch> {
    Vec::new()
}

pub fn toggle(_which: Switch) {}

pub fn show_substitutions() {}

pub fn changed(_page: &Page, _chrome: &Chrome) {}

pub fn closed(_id: WindowId) {}

pub fn context_items(_page: &Page) -> Vec<(String, Option<Command>)> {
    Vec::new()
}

pub fn replace(_page: &Page, _loc: usize, _len: usize, _word: &str, _with: &str) {}

pub fn ignore(_page: &Page, _word: &str) {}

pub fn learn(_page: &Page, _word: &str) {}

pub fn check_document(page: &Page) {
    page.say("No spelling checker on this system yet");
}

pub fn after_typing(_page: &Page, _typed: &str) {}
