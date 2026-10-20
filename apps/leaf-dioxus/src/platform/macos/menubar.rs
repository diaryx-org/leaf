//! The Mac's menu bar: `menus::bar()` as `NSMenu`s.
//!
//! An item that is the system's (`menus::Node::System`) sends its selector
//! up the responder chain: File's Open, Save, Duplicate, Rename, Move To,
//! Revert To, Page Setup and Print reach the key window's `NSDocument`;
//! Close, Minimize, Zoom and Full Screen its `NSWindow`; Hide, Quit and the
//! Services the app. AppKit fills Open Recent and the Services menu itself,
//! and adds Emoji & Symbols and Start Dictation to the menu titled Edit, and
//! the tab items to the Window menu.
//!
//! Every other item is `leafCommand:`, its tag an index into the commands
//! the bar was built with; the app delegate (`mod.rs`) answers it and
//! validates it with `commands::status`, as a page's own menus do.

use std::cell::RefCell;

use dioxus::html::{Code, Modifiers};
use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

use crate::commands::{Command, key_char};
use crate::menus::{self, Node};

thread_local! {
    /// The bar's commands, by tag.
    static COMMANDS: RefCell<Vec<Command>> = RefCell::default();
}

/// The command an item's tag names.
pub fn command(tag: isize) -> Option<Command> {
    COMMANDS.with_borrow(|c| usize::try_from(tag).ok().and_then(|i| c.get(i).cloned()))
}

/// Build the menu bar and make it the application's.
pub fn install(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    for (title, nodes) in menus::bar() {
        let menu = build(mtm, title, &nodes);
        match title {
            "Window" => app.setWindowsMenu(Some(&menu)),
            "Help" => app.setHelpMenu(Some(&menu)),
            _ => {}
        }
        bar.addItem(&submenu(mtm, title, &menu));
    }
    app.setMainMenu(Some(&bar));
}

fn build(mtm: MainThreadMarker, title: &str, nodes: &[Node]) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for n in nodes {
        let item = match n {
            Node::Separator => NSMenuItem::separatorItem(mtm),
            Node::System(label, selector, key) => item(
                mtm,
                label,
                Some(Sel::register(&std::ffi::CString::new(*selector).unwrap())),
                *key,
            ),
            Node::Item(label, c) => {
                let it = item(mtm, label, Some(sel!(leafCommand:)), c.shortcut());
                let tag = COMMANDS.with_borrow_mut(|cs| {
                    cs.push(c.clone());
                    cs.len() - 1
                });
                it.setTag(tag as isize);
                it
            }
            Node::Submenu(label, children) => {
                let sub = build(mtm, label, children);
                if *label == "Services" {
                    NSApplication::sharedApplication(mtm).setServicesMenu(Some(&sub));
                }
                submenu(mtm, label, &sub)
            }
        };
        menu.addItem(&item);
    }
    menu
}

fn submenu(mtm: MainThreadMarker, title: &str, menu: &NSMenu) -> Retained<NSMenuItem> {
    let it = item(mtm, title, None, None);
    it.setSubmenu(Some(menu));
    it
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: Option<(Code, Modifiers)>,
) -> Retained<NSMenuItem> {
    let (equivalent, mods) = match &key {
        Some((code, mods)) => (equivalent(code), *mods),
        None => (String::new(), Modifiers::empty()),
    };
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(&equivalent),
        )
    };
    if !equivalent.is_empty() {
        it.setKeyEquivalentModifierMask(mask(mods));
    }
    it
}

/// The key equivalent AppKit matches a code by: the unshifted character,
/// Shift in the modifier mask, as SwiftUI's `keyboardShortcut` sets it; an
/// arrow as its function-key character.
fn equivalent(code: &Code) -> String {
    match code {
        Code::ArrowUp => "\u{F700}".into(),
        Code::ArrowDown => "\u{F701}".into(),
        Code::ArrowLeft => "\u{F702}".into(),
        Code::ArrowRight => "\u{F703}".into(),
        c => key_char(c),
    }
}

/// Dioxus's modifiers as AppKit's.
fn mask(m: Modifiers) -> NSEventModifierFlags {
    let mut f = NSEventModifierFlags::empty();
    for (dx, ns) in [
        (Modifiers::META, NSEventModifierFlags::Command),
        (Modifiers::SHIFT, NSEventModifierFlags::Shift),
        (Modifiers::ALT, NSEventModifierFlags::Option),
        (Modifiers::CONTROL, NSEventModifierFlags::Control),
    ] {
        if m.contains(dx) {
            f |= ns;
        }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrow_is_its_function_key() {
        assert_eq!(equivalent(&Code::ArrowUp), "\u{F700}");
        assert_eq!(equivalent(&Code::Digit8), "8");
    }

    #[test]
    fn the_modifiers_are_appkits() {
        let m = mask(Modifiers::META | Modifiers::SHIFT);
        assert!(m.contains(NSEventModifierFlags::Command | NSEventModifierFlags::Shift));
        assert!(!m.contains(NSEventModifierFlags::Option));
    }
}
