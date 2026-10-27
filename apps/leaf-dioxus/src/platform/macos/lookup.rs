//! Look Up: the Dictionary's popover for a word, drawn over the word where
//! it stands — by a force click, where System Settings gives Look Up to one,
//! or from the context menu.
//!
//! `NSView`'s `showDefinitionForAttributedString:atPoint:` draws the word
//! again, in the font it is given, with its baseline at the point; so the
//! word's face and size are read off the editor's layout (`Editor::font_at`)
//! and its line off `Editor::rect_of`, both in the window's CSS pixels,
//! which are the flipped view's points.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSFontAttributeName};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSAttributedString, NSDictionary, NSString, NSUserDefaults};

use crate::page::Page;
use crate::text;

/// Whether the system has a dictionary to look a word up in.
pub const AVAILABLE: bool = true;

/// Look up the UTF-16 range `loc`..`loc + len` of the system's text.
pub fn show(page: &Page, loc: usize, len: usize) {
    let Some(Some((word, rect, font))) = page.with_editor(|ed, _| {
        let (from, to, word) = {
            let mut d = ed.doc();
            let visible: Vec<u16> = text::visible(&mut d).encode_utf16().collect();
            let word = String::from_utf16_lossy(visible.get(loc..loc + len)?);
            let (from, to) = text::bounds(&mut d, loc, len);
            (from, to, word)
        };
        let rect = ed.rect_of(from, to)?;
        let font = ed
            .font_at(from)
            .and_then(|(data, px)| postscript_name(&data).map(|name| (name, px)));
        Some((word, rect, font))
    }) else {
        return;
    };
    let Some(view) = super::ns_view(&*page.window) else {
        return;
    };
    let (x, y, _, h) = rect;
    leaf_dioxus::trace(|| format!("look up {word:?} at {x:.0},{y:.0}"));
    let size = font.as_ref().map_or(16.0, |(_, px)| f64::from(*px));
    let ns_font = font
        .and_then(|(name, px)| NSFont::fontWithName_size(&NSString::from_str(&name), px.into()))
        .unwrap_or_else(|| NSFont::systemFontOfSize(size));
    // The line box centres the face's ascent and descent.
    let baseline = y + (h + ns_font.ascender() + ns_font.descender()) / 2.0;
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[unsafe { NSFontAttributeName }],
        &[ns_font.as_ref() as &AnyObject],
    );
    let string = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&word),
            Some(&attrs),
        )
    };
    super::later(move || {
        view.showDefinitionForAttributedString_atPoint(Some(&string), CGPoint::new(x, baseline));
    });
}

/// The word under the pointer, at the force click's second stage — where
/// System Settings ▸ Trackpad gives Look Up to a force click rather than a
/// three-finger tap.
pub fn force_click(page: &Page) {
    if !force_click_looks_up() {
        return;
    }
    let Some((x, y)) = page.pointer() else { return };
    let Some(Some(index)) = page.with_editor(|ed, _| {
        let off = ed.offset_at_point(x, y)?;
        Some(text::utf16_of(&mut ed.doc(), off))
    }) else {
        return;
    };
    if let Some((loc, len)) = page
        .with_doc(|d| word_at(&text::visible(d), index))
        .flatten()
    {
        show(page, loc, len);
    }
}

/// Whether a force click is Look Up: force clicking on, and Look Up not
/// given to a three-finger tap instead.
fn force_click_looks_up() -> bool {
    let global = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str("com.apple.trackpad.forceClick");
    let force = global
        .objectForKey(&key)
        .is_none_or(|_| global.boolForKey(&key));
    let trackpad = NSUserDefaults::initWithSuiteName(
        NSUserDefaults::alloc(),
        Some(&NSString::from_str("com.apple.AppleMultitouchTrackpad")),
    );
    let three = trackpad.map_or(0, |t| {
        t.integerForKey(&NSString::from_str("TrackpadThreeFingerTapGesture"))
    });
    force && three != 2
}

/// The word around UTF-16 `index` of `text`, as (location, length): a run of
/// letters, digits and the apostrophes inside a word.
pub fn word_at(text: &str, index: usize) -> Option<(usize, usize)> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let is_word = |i: usize| {
        char::from_u32(u32::from(units[i]))
            .is_some_and(|c| c.is_alphanumeric() || c == '\'' || c == '’')
    };
    if index >= units.len() || !is_word(index) {
        return None;
    }
    let mut start = index;
    while start > 0 && is_word(start - 1) {
        start -= 1;
    }
    let mut end = index + 1;
    while end < units.len() && is_word(end) {
        end += 1;
    }
    Some((start, end - start))
}

/// A face's PostScript name, which `NSFont` finds it by.
fn postscript_name(font: &parley::FontData) -> Option<String> {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(font.data.data(), font.index)
        .ok()?
        .localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME)
        .english_or_first()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_runs_to_the_letters_either_side() {
        assert_eq!(word_at("the cat's whiskers", 5), Some((4, 5)));
        assert_eq!(word_at("the cat", 3), None);
    }
}
