//! The host's chrome around the editor widget. There is deliberately almost
//! none: the editing surface fills the entire terminal, and everything else —
//! the Save/Discard/Cancel and conflict dialogs, the right-click context menu,
//! the single-line text prompt, and the transient status toast — floats over it
//! only while it's needed, then gets out of the way. The editing surface itself
//! (the document body, its code boxes, images, scrollbar, and caret) is drawn by
//! [`leaf_ratatui::render`] into the whole frame.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use leaf_core::{Doc, InlineMarks};

use crate::{App, ContextMenu, DirtyAction, MenuEntry, TextPrompt};

pub fn render(f: &mut Frame, doc: &mut Doc, app: &mut App) {
    // The editing surface owns the whole terminal; the host paints only floating
    // overlays over it, and only when one is actually up.
    leaf_ratatui::render(f, f.area(), doc, &mut app.editor);

    // The two safety dialogs take over the keyboard until answered, so they float
    // centered and modal (the widest, most attention-drawing chrome we have) —
    // the terminal analogue of a sheet dropping over the document.
    if let Some(prompt) = &app.dirty_prompt {
        let verb = match prompt.action {
            DirtyAction::Quit => "quit",
            DirtyAction::New => "start a new document",
        };
        render_choice_overlay(
            f,
            &format!("Unsaved changes — {verb}?"),
            &["Save", "Discard", "Cancel"],
            prompt.selected,
        );
    } else if let Some(prompt) = &app.conflict {
        render_choice_overlay(
            f,
            "File changed on disk since it was opened",
            &["Overwrite", "Reload", "Cancel"],
            prompt.selected,
        );
    } else if let Some(msg) = &doc.status {
        // A status ("copied", "pasted", "clipboard unavailable", a list-nest
        // note) is feedback, not a question — so it's a small toast in the
        // bottom-right corner, drawn over the body and cleared by the next edit,
        // rather than a line of permanent chrome. Suppressed while a dialog is up
        // so the two never fight for the same glance.
        render_status_toast(f, msg);
    }

    if let Some(menu) = &mut app.context_menu {
        render_context_menu(f, f.area(), menu, doc);
    }
    if let Some(prompt) = &app.text_prompt {
        render_text_prompt(f, f.area(), prompt);
    }
}

/// A centered modal box for the two three-way safety dialogs: a warning line
/// naming what's at stake, then the choices with `selected` reversed and a
/// first-letter mnemonic per item (the caller's key handling and this agree on
/// what those letters are; there's only ever three, so they're spelled out in
/// the label rather than derived). Shaped like [`render_text_prompt`] — a
/// `Clear`ed, bordered island floated over the document — because both suspend
/// editing until answered.
fn render_choice_overlay(f: &mut Frame, message: &str, items: &[&str], selected: usize) {
    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let warn = base.fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let key = base.fg(Color::Cyan);

    let mut choices = Vec::new();
    for (i, label) in items.iter().enumerate() {
        if i > 0 {
            choices.push(Span::styled("   ", base));
        }
        let style = if i == selected { key.add_modifier(Modifier::REVERSED) } else { key };
        let mnemonic = label.chars().next().unwrap_or(' ').to_ascii_lowercase();
        choices.push(Span::styled(format!(" {label} ({mnemonic}) "), style));
    }
    let lines = vec![
        Line::from(Span::styled(format!(" {message} "), warn)),
        Line::from(choices),
    ];

    let screen = f.area();
    let choices_w: usize = items.iter().map(|l| l.chars().count() + 7).sum::<usize>() + 2;
    let width = (message.chars().count() + 2).max(choices_w).min(screen.width.max(1) as usize) as u16;
    let height = 2u16.min(screen.height.max(1));
    let rect = centered(screen, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);
}

/// A small feedback toast in the bottom-right corner, drawn over the body and
/// cleared by the next edit. Right-aligned and one row tall so it stays out of
/// the way of the text and the caret, which usually sit up and to the left.
fn render_status_toast(f: &mut Frame, msg: &str) {
    let screen = f.area();
    if screen.width == 0 || screen.height == 0 {
        return;
    }
    let text = format!(" {msg} ");
    let width = (text.chars().count() as u16).min(screen.width);
    let rect = Rect {
        x: screen.x + screen.width - width,
        y: screen.y + screen.height - 1,
        width,
        height: 1,
    };
    let style = Style::default().bg(Color::DarkGray).fg(Color::Yellow);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))).style(style), rect);
}

/// Center a `width`×`height` rect within `screen`.
fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: screen.x + (screen.width.saturating_sub(width)) / 2,
        y: screen.y + (screen.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// The right-click menu and any submenu drilled into it. Each level is a
/// `Clear`ed, bordered-by-background island: the root anchored at the click
/// (nudged back onto `screen` if it wouldn't fit, the terminal equivalent of the
/// GUI menu's `snap_to_window`), each submenu flying out from its parent's
/// selected row (to the left instead if there's no room on the right). Every
/// level stashes the rect it painted at back onto itself, so `ContextMenu::hit`
/// can map a later click or hover to a row against the exact geometry drawn here.
///
/// Rows carry live state: an active inline mark or the caret's heading level
/// shows a `✓`, read once off `doc` up front so a menu of sixteen rows doesn't
/// re-query the AST sixteen times a frame.
fn render_context_menu(f: &mut Frame, screen: Rect, menu: &mut ContextMenu, doc: &mut Doc) {
    let marks = doc.active_inline_marks();
    let heading = doc.current_heading_level();
    let base = Style::default().bg(Color::DarkGray).fg(Color::White);

    // Walk parent → child: a submenu's position depends on the rect its parent
    // was just painted at, and its top aligns with the parent row it opened from.
    let mut parent: Option<(Rect, usize)> = None;
    for i in 0..menu.levels.len() {
        let items = menu.levels[i].items;
        let selected = menu.levels[i].selected;
        let width = menu_level_width(items);
        let height = items.len() as u16;
        let (x, y) = match parent {
            None => {
                let (ax, ay) = menu.anchor;
                (
                    ax.min(screen.width.saturating_sub(width)),
                    ay.min(screen.height.saturating_sub(height)),
                )
            }
            Some((prect, prow)) => {
                let x = if prect.x + prect.width + width <= screen.width {
                    prect.x + prect.width
                } else {
                    prect.x.saturating_sub(width)
                };
                let y = (prect.y + prow as u16).min(screen.height.saturating_sub(height));
                (x, y)
            }
        };
        let rect = Rect { x, y, width, height };
        menu.levels[i].rect = Some(rect);

        let lines: Vec<Line<'static>> = items
            .iter()
            .enumerate()
            .map(|(r, entry)| menu_row(*entry, r == selected, marks, heading, width, base))
            .collect();

        f.render_widget(Clear, rect);
        f.render_widget(Paragraph::new(lines).style(base), rect);

        parent = Some((rect, selected));
    }
}

/// A menu level's box width: its widest label plus the fixed gutters — a
/// left check column (`✓`/blank) and a right submenu-arrow column (`▸`/blank),
/// each with its own padding — so every row aligns whether or not it's checked
/// or a submenu.
fn menu_level_width(items: &[MenuEntry]) -> u16 {
    let label = items.iter().map(|e| e.label().chars().count()).max().unwrap_or(0);
    // " ✓ " (3) + label + " ▸ " (3)
    (label + 6) as u16
}

/// One rendered menu row. Actions carry a check gutter (lit when the style is
/// active); submenus carry a trailing `▸`; headers are a dim, unhighlightable
/// section label. `width` is the level's box width so every row fills it exactly.
fn menu_row(
    entry: MenuEntry,
    selected: bool,
    marks: InlineMarks,
    heading: Option<u32>,
    width: u16,
    base: Style,
) -> Line<'static> {
    let label_w = width as usize - 6;
    match entry {
        MenuEntry::Header(label) => {
            // Non-selectable: dim and never reversed, so it reads as a divider
            // rather than a choice.
            let style = base.fg(Color::Gray).add_modifier(Modifier::DIM);
            Line::from(Span::styled(format!(" {label:<w$} ", w = width as usize - 2), style))
        }
        MenuEntry::Action(label, act) => {
            let active = act.active(marks, heading);
            let check = if active { '✓' } else { ' ' };
            let style = if selected {
                base.add_modifier(Modifier::REVERSED)
            } else if active {
                // Lit even without the pointer on it, so what's already on is
                // legible at a glance, not only under the highlight.
                base.fg(Color::Cyan)
            } else {
                base
            };
            Line::from(Span::styled(format!(" {check} {label:<label_w$}   "), style))
        }
        MenuEntry::Submenu(label, _) => {
            let style = if selected {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            };
            Line::from(Span::styled(format!("   {label:<label_w$} ▸ "), style))
        }
    }
}

/// The single-line input: a label row, a value row, and an Enter/Esc hint,
/// centered over `screen` — there's no click anchor to hang it off the way
/// the context menu has, and nothing in it is clickable, so unlike that menu
/// this stashes no rect back for hit-testing. The caret is the real terminal
/// cursor, positioned into the value row exactly the way the document body
/// positions it into the source — one visible caret, one mechanism.
fn render_text_prompt(f: &mut Frame, screen: Rect, prompt: &TextPrompt) {
    let hint = " enter confirm  esc cancel ";
    let content = [prompt.label.chars().count(), prompt.value.chars().count(), hint.chars().count()]
        .into_iter()
        .max()
        .unwrap_or(0) as u16
        + 2;
    let width = content.max(24).min(screen.width.max(1));
    let height = 3u16.min(screen.height.max(1));
    let rect = centered(screen, width, height);

    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let bold = base.add_modifier(Modifier::BOLD);
    let key = base.fg(Color::Cyan);
    let dim = base.fg(Color::Gray);
    let lines = vec![
        Line::from(Span::styled(format!(" {} ", prompt.label), bold)),
        Line::from(Span::styled(format!(" {} ", prompt.value), base)),
        Line::from(vec![
            Span::styled(" enter ", key),
            Span::styled("confirm  ", dim),
            Span::styled("esc ", key),
            Span::styled("cancel ", dim),
        ]),
    ];

    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);

    let cursor_x = rect.x + 1 + prompt.value[..prompt.cursor].chars().count() as u16;
    if rect.height >= 2 && cursor_x < rect.x + rect.width {
        f.set_cursor_position(Position::new(cursor_x, rect.y + 1));
    }
}
