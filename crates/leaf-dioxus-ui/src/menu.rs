//! A menu, as data and as a state machine: what it holds ([`Entry`]), where
//! each of its open levels stands, which item is highlighted, and what a key
//! or the pointer does to it. Nothing here draws; `layer.rs` does, from this.
//!
//! Every geometry is computed rather than measured: a menu's levels are
//! absolutely positioned boxes in the page's overlay, and a submenu has to
//! know where its parent's item is. Measuring the parent after layout would
//! need the DOM, which a page on dioxus-native can only read from a task
//! while Blitz holds it borrowed. So an item is [`ITEM`] tall, a separator
//! [`SEPARATOR`], and a level's width is estimated from its longest label.

use dioxus::html::{Key, Modifiers};

/// An item's height, a separator's, and a level's padding top and bottom,
/// in CSS pixels — the stylesheet's `.ui-item`, `.ui-sep` and `.ui-menu`.
pub const ITEM: f64 = 22.0;
pub const SEPARATOR: f64 = 9.0;
pub const PAD: f64 = 5.0;
/// An average character of the menus' face at 13px, for estimating width.
const CHAR: f64 = 7.0;
const MIN_WIDTH: f64 = 160.0;
const MAX_WIDTH: f64 = 380.0;
/// Room for the tick on the left and the submenu arrow on the right.
const GUTTERS: f64 = 44.0;
/// How far a submenu overlaps its parent, as the system's do.
const OVERLAP: f64 = 4.0;
/// A level kept this far off the viewport's edges.
const MARGIN: f64 = 4.0;

/// One thing a menu shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    Separator,
    /// A label that opens another menu beside it.
    Submenu {
        label: String,
        entries: Vec<Entry>,
        enabled: bool,
    },
}

/// An item that does something when chosen: its `id` is what the menu's
/// pick handler is given.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: usize,
    pub label: String,
    /// The key that does the same, shown at the right: `⌘B`.
    pub shortcut: Option<String>,
    pub checked: bool,
    pub enabled: bool,
}

impl Item {
    pub fn new(id: usize, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            shortcut: None,
            checked: false,
            enabled: true,
        }
    }

    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.shortcut = (!s.is_empty()).then_some(s);
        self
    }

    pub fn checked(mut self, yes: bool) -> Self {
        self.checked = yes;
        self
    }

    pub fn enabled(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }
}

impl From<Item> for Entry {
    fn from(i: Item) -> Self {
        Entry::Item(i)
    }
}

impl Entry {
    /// A submenu of `entries`, enabled while any of them is.
    pub fn submenu(label: impl Into<String>, entries: Vec<Entry>) -> Self {
        let enabled = entries.iter().any(Entry::enabled);
        Entry::Submenu {
            label: label.into(),
            entries,
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        match self {
            Entry::Item(i) => i.enabled,
            Entry::Separator => false,
            Entry::Submenu { enabled, .. } => *enabled,
        }
    }

    fn height(&self) -> f64 {
        match self {
            Entry::Separator => SEPARATOR,
            _ => ITEM,
        }
    }

    fn width(&self) -> f64 {
        let chars = |s: &str| s.chars().count() as f64;
        match self {
            Entry::Item(i) => {
                chars(&i.label) * CHAR
                    + i.shortcut
                        .as_deref()
                        .map_or(0.0, |s| chars(s) * CHAR + 24.0)
            }
            Entry::Separator => 0.0,
            Entry::Submenu { label, .. } => chars(label) * CHAR + 16.0,
        }
    }
}

/// One open level of a menu: its entries, its box, and its highlighted
/// item.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub entries: Vec<Entry>,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub hot: Option<usize>,
}

impl Level {
    fn new(entries: Vec<Entry>) -> Self {
        let width = (entries.iter().map(Entry::width).fold(0.0, f64::max) + GUTTERS)
            .clamp(MIN_WIDTH, MAX_WIDTH);
        Self {
            entries,
            left: 0.0,
            top: 0.0,
            width,
            hot: None,
        }
    }

    pub fn height(&self) -> f64 {
        2.0 * PAD + self.entries.iter().map(Entry::height).sum::<f64>()
    }

    /// The top of entry `i`, from the level's top.
    fn entry_top(&self, i: usize) -> f64 {
        PAD + self.entries[..i].iter().map(Entry::height).sum::<f64>()
    }

    /// The next entry that can be highlighted from `from`, going `down` —
    /// wrapping, as a system menu does.
    fn step(&self, from: Option<usize>, down: bool) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        let mut i = from;
        for _ in 0..n {
            let next = match (i, down) {
                (None, true) => 0,
                (None, false) => n - 1,
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
            };
            if self.entries[next].enabled() {
                return Some(next);
            }
            i = Some(next);
        }
        None
    }
}

/// What a key or a press did to the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Not the menu's: the key goes on to whoever is under it.
    Ignored,
    /// The menu changed, or the key was swallowed.
    Handled,
    /// This item was chosen; the menu closes.
    Pick(usize),
    /// The menu closes with nothing chosen.
    Close,
}

/// An open menu: its levels, the first one's and each submenu after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub levels: Vec<Level>,
    /// The page's size, which no level is let past.
    viewport: (f64, f64),
    /// Whether the pointer has moved over the menu, or pressed in it, since
    /// it opened: until then a release is the end of the press that opened
    /// it — a right-click's — and picks nothing.
    pub armed: bool,
}

impl Menu {
    /// `entries`, opened with the first level's top-left at `at`, kept in a
    /// page of `viewport`'s size: moved left, or above `at` (less `above`,
    /// the height of whatever it opened under), where it would run off.
    pub fn open(entries: Vec<Entry>, at: (f64, f64), above: f64, viewport: (f64, f64)) -> Self {
        let mut level = Level::new(entries);
        let (w, h) = (level.width, level.height());
        level.left = at.0.min(viewport.0 - w - MARGIN).max(MARGIN);
        level.top = if at.1 + h > viewport.1 - MARGIN && at.1 - above - h > MARGIN {
            at.1 - above - h
        } else {
            at.1.min(viewport.1 - h - MARGIN).max(MARGIN)
        };
        Self {
            levels: vec![level],
            viewport,
            armed: false,
        }
    }

    /// Highlight entry `i` of level `depth`, opening its submenu or closing
    /// any deeper level.
    pub fn hover(&mut self, depth: usize, i: usize) {
        self.armed = true;
        if depth >= self.levels.len() {
            return;
        }
        self.levels.truncate(depth + 1);
        let level = &mut self.levels[depth];
        let enabled = level.entries.get(i).is_some_and(Entry::enabled);
        level.hot = enabled.then_some(i);
        if enabled {
            self.open_submenu(depth, i);
        }
    }

    /// Open the submenu at entry `i` of level `depth`, if it is one.
    fn open_submenu(&mut self, depth: usize, i: usize) -> bool {
        let parent = &self.levels[depth];
        let Some(Entry::Submenu {
            entries,
            enabled: true,
            ..
        }) = parent.entries.get(i)
        else {
            return false;
        };
        let mut level = Level::new(entries.clone());
        let (w, h) = (level.width, level.height());
        let right = parent.left + parent.width - OVERLAP;
        level.left = if right + w > self.viewport.0 - MARGIN {
            // No room to the right: to the left of the parent instead.
            (parent.left - w + OVERLAP).max(MARGIN)
        } else {
            right
        };
        // Its first item level with the parent's.
        level.top = (parent.top + parent.entry_top(i) - PAD)
            .min(self.viewport.1 - h - MARGIN)
            .max(MARGIN);
        self.levels.truncate(depth + 1);
        self.levels.push(level);
        true
    }

    /// The pointer pressed in the menu.
    pub fn press(&mut self) {
        self.armed = true;
    }

    /// The pointer let go over entry `i` of level `depth`.
    pub fn release(&mut self, depth: usize, i: usize) -> Outcome {
        if !self.armed {
            return Outcome::Handled;
        }
        self.activate(depth, i)
    }

    fn activate(&mut self, depth: usize, i: usize) -> Outcome {
        match self.levels.get(depth).and_then(|l| l.entries.get(i)) {
            Some(Entry::Item(item)) if item.enabled => Outcome::Pick(item.id),
            Some(Entry::Submenu { enabled: true, .. }) => {
                self.levels[depth].hot = Some(i);
                if self.open_submenu(depth, i) {
                    let deeper = depth + 1;
                    let first = self.levels[deeper].step(None, true);
                    self.levels[deeper].hot = first;
                }
                Outcome::Handled
            }
            _ => Outcome::Handled,
        }
    }

    /// A key, while the menu is open. Every key is the menu's — as a system
    /// menu's are — but those it does nothing with, which close it and go on.
    pub fn key(&mut self, key: &Key, mods: Modifiers) -> Outcome {
        use dioxus::html::NamedKey::*;
        let depth = self.levels.len() - 1;
        match key {
            Key::Named(Escape) => Outcome::Close,
            Key::Named(ArrowDown) | Key::Named(ArrowUp) => {
                let down = *key == Key::Named(ArrowDown);
                let level = &mut self.levels[depth];
                level.hot = level.step(level.hot, down);
                Outcome::Handled
            }
            Key::Named(ArrowRight) => {
                if let Some(i) = self.levels[depth].hot {
                    self.activate(depth, i);
                }
                Outcome::Handled
            }
            Key::Named(ArrowLeft) => {
                if depth > 0 {
                    self.levels.pop();
                }
                Outcome::Handled
            }
            Key::Named(Enter) => match self.levels[depth].hot {
                Some(i) => self.activate(depth, i),
                None => Outcome::Close,
            },
            Key::Character(c) if c == " " && mods.is_empty() => match self.levels[depth].hot {
                Some(i) => self.activate(depth, i),
                None => Outcome::Handled,
            },
            // A letter jumps to the next item starting with it.
            Key::Character(c) if mods.is_empty() || mods == Modifiers::SHIFT => {
                let c = c.to_lowercase();
                let level = &mut self.levels[depth];
                let n = level.entries.len();
                let start = level.hot.map_or(0, |h| h + 1);
                let found = (0..n).map(|k| (start + k) % n).find(|&k| {
                    let label = match &level.entries[k] {
                        Entry::Item(i) if i.enabled => &i.label,
                        Entry::Submenu {
                            label,
                            enabled: true,
                            ..
                        } => label,
                        _ => return false,
                    };
                    label.to_lowercase().starts_with(&c)
                });
                if found.is_some() {
                    level.hot = found;
                }
                Outcome::Handled
            }
            _ => Outcome::Close,
        }
    }
}

#[cfg(test)]
mod tests {
    use dioxus::html::NamedKey;

    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Item::new(1, "Bold").shortcut("⌘B").into(),
            Item::new(2, "Italic").enabled(false).into(),
            Entry::Separator,
            Entry::submenu(
                "Heading",
                vec![
                    Item::new(10, "Heading 1").into(),
                    Item::new(11, "Heading 2").into(),
                ],
            ),
        ]
    }

    const PAGE: (f64, f64) = (800.0, 600.0);

    #[test]
    fn a_menu_opens_where_it_was_asked_and_inside_the_page() {
        let m = Menu::open(entries(), (100.0, 50.0), 0.0, PAGE);
        assert_eq!((m.levels[0].left, m.levels[0].top), (100.0, 50.0));
        let m = Menu::open(entries(), (790.0, 50.0), 0.0, PAGE);
        let l = &m.levels[0];
        assert!(l.left + l.width <= PAGE.0, "{}", l.left);
        // Near the bottom it opens above the point instead.
        let m = Menu::open(entries(), (100.0, 590.0), 20.0, PAGE);
        let l = &m.levels[0];
        assert_eq!(l.top + l.height(), 570.0);
    }

    #[test]
    fn the_arrows_skip_what_cannot_be_chosen_and_wrap() {
        let mut m = Menu::open(entries(), (0.0, 0.0), 0.0, PAGE);
        let down = Key::Named(NamedKey::ArrowDown);
        let none = Modifiers::empty();
        m.key(&down, none);
        assert_eq!(m.levels[0].hot, Some(0));
        m.key(&down, none);
        // Past the disabled item and the separator.
        assert_eq!(m.levels[0].hot, Some(3));
        m.key(&down, none);
        assert_eq!(m.levels[0].hot, Some(0));
        m.key(&Key::Named(NamedKey::ArrowUp), none);
        assert_eq!(m.levels[0].hot, Some(3));
    }

    #[test]
    fn a_submenu_opens_beside_its_item_and_picks_from_there() {
        let mut m = Menu::open(entries(), (100.0, 50.0), 0.0, PAGE);
        m.hover(0, 3);
        assert_eq!(m.levels.len(), 2);
        let (parent, sub) = (&m.levels[0], &m.levels[1]);
        assert_eq!(sub.left, parent.left + parent.width - OVERLAP);
        assert_eq!(sub.top, parent.top + parent.entry_top(3) - PAD);
        assert_eq!(m.release(1, 1), Outcome::Pick(11));
        // Back to the parent: the submenu closes.
        m.hover(0, 0);
        assert_eq!(m.levels.len(), 1);
    }

    #[test]
    fn the_keyboard_walks_into_a_submenu_and_out() {
        let mut m = Menu::open(entries(), (0.0, 0.0), 0.0, PAGE);
        let none = Modifiers::empty();
        m.key(&Key::Character("h".into()), none);
        assert_eq!(m.levels[0].hot, Some(3));
        m.key(&Key::Named(NamedKey::ArrowRight), none);
        assert_eq!(m.levels.len(), 2);
        assert_eq!(m.levels[1].hot, Some(0));
        assert_eq!(m.key(&Key::Named(NamedKey::Enter), none), Outcome::Pick(10));
        m.key(&Key::Named(NamedKey::ArrowLeft), none);
        assert_eq!(m.levels.len(), 1);
        assert_eq!(m.key(&Key::Named(NamedKey::Escape), none), Outcome::Close);
    }

    #[test]
    fn the_release_that_ends_the_opening_press_picks_nothing() {
        let mut m = Menu::open(entries(), (0.0, 0.0), 0.0, PAGE);
        assert_eq!(m.release(0, 0), Outcome::Handled);
        m.hover(0, 0);
        assert_eq!(m.release(0, 0), Outcome::Pick(1));
        // A disabled item never picks.
        assert_eq!(m.release(0, 1), Outcome::Handled);
    }

    #[test]
    fn a_submenu_with_nothing_to_choose_is_disabled() {
        let s = Entry::submenu(
            "Table",
            vec![Item::new(1, "Delete Row").enabled(false).into()],
        );
        assert!(!s.enabled());
    }
}
