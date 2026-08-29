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

use leaf_core::Doc;

use crate::commands::{Ctx, GROUPS};
use crate::palette::Palette;
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
        let ctx = Ctx::read(doc);
        render_context_menu(f, f.area(), menu, &ctx);
    }
    if let Some(palette) = &mut app.palette {
        let ctx = Ctx::read(doc);
        render_palette(f, f.area(), palette, &ctx);
    }
    if let Some(prompt) = &app.text_prompt {
        render_text_prompt(f, f.area(), prompt);
    }
    // Last, and over everything: the key reference is the one overlay that is
    // asked for *while* something else is confusing, so it must not be able to
    // end up underneath whatever prompted the question.
    if app.help {
        render_help(f, f.area(), doc);
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
        let style = if i == selected {
            key.add_modifier(Modifier::REVERSED)
        } else {
            key
        };
        let mnemonic = label.chars().next().unwrap_or(' ').to_ascii_lowercase();
        choices.push(Span::styled(format!(" {label} ({mnemonic}) "), style));
    }
    let lines = vec![
        Line::from(Span::styled(format!(" {message} "), warn)),
        Line::from(choices),
    ];

    let screen = f.area();
    let choices_w: usize = items.iter().map(|l| l.chars().count() + 7).sum::<usize>() + 2;
    let width = (message.chars().count() + 2)
        .max(choices_w)
        .min(screen.width.max(1) as usize) as u16;
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
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).style(style),
        rect,
    );
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
fn render_context_menu(f: &mut Frame, screen: Rect, menu: &mut ContextMenu, ctx: &Ctx) {
    let base = Style::default().bg(Color::DarkGray).fg(Color::White);

    // Walk parent → child: a submenu's position depends on the rect its parent
    // was just painted at, and its top aligns with the parent row it opened from.
    let mut parent: Option<(Rect, usize)> = None;
    for i in 0..menu.levels.len() {
        let items = menu.levels[i].items;
        let selected = menu.levels[i].selected;
        let (label_w, hint_w) = menu_columns(items);
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
        let rect = Rect {
            x,
            y,
            width,
            height,
        };
        menu.levels[i].rect = Some(rect);

        let lines: Vec<Line<'static>> = items
            .iter()
            .enumerate()
            .map(|(r, entry)| menu_row(*entry, r == selected, ctx, label_w, hint_w, base))
            .collect();

        f.render_widget(Clear, rect);
        f.render_widget(Paragraph::new(lines).style(base), rect);

        parent = Some((rect, selected));
    }
}

/// A level's two variable columns: the widest label, and the widest key hint
/// (zero when no row in the level has a key, which collapses the column away
/// rather than leaving a ragged gutter of blanks).
fn menu_columns(items: &[MenuEntry]) -> (usize, usize) {
    let label = items
        .iter()
        .map(|e| e.label().chars().count())
        .max()
        .unwrap_or(0);
    let hint = items
        .iter()
        .map(|e| e.hint().chars().count())
        .max()
        .unwrap_or(0);
    (label, hint)
}

/// A menu level's box width: its widest label plus the fixed gutters — a left
/// check column (`✓`/blank), the key column when the level has any keys, and a
/// right submenu-arrow column (`▸`/blank) — so every row aligns whether or not
/// it's checked, keyed, or a submenu.
fn menu_level_width(items: &[MenuEntry]) -> u16 {
    let (label, hint) = menu_columns(items);
    // " ✓ " (3) + label + ["  " + hint] + " ▸ " (3)
    let keys = if hint > 0 { hint + 2 } else { 0 };
    (label + keys + 6) as u16
}

/// One rendered menu row. Actions carry a check gutter (lit when the style is
/// active) and their key on the right; submenus carry a trailing `▸`; headers
/// are a dim, unhighlightable section label.
///
/// A row this document can't run is drawn dim and never highlighted — the
/// gray-out `Capabilities` exists for. It stays *present*, because the absence
/// of a Highlight row in a Markdown document teaches nothing, while a dim one
/// says "this exists, and not in this format".
fn menu_row(
    entry: MenuEntry,
    selected: bool,
    ctx: &Ctx,
    label_w: usize,
    hint_w: usize,
    base: Style,
) -> Line<'static> {
    let keys = |hint: &str| -> String {
        if hint_w == 0 {
            String::new()
        } else {
            format!("  {hint:>hint_w$}")
        }
    };
    match entry {
        MenuEntry::Header(label) => {
            // Non-selectable: dim and never reversed, so it reads as a divider
            // rather than a choice.
            let style = base.fg(Color::Gray).add_modifier(Modifier::DIM);
            let w = label_w + hint_w + if hint_w > 0 { 2 } else { 0 } + 4;
            Line::from(Span::styled(format!(" {label:<w$} "), style))
        }
        MenuEntry::Action(cmd) => {
            let enabled = cmd.enabled(ctx);
            let active = enabled && cmd.active(ctx);
            let check = if active { '✓' } else { ' ' };
            let style = if !enabled {
                base.fg(Color::Gray).add_modifier(Modifier::DIM)
            } else if selected {
                base.add_modifier(Modifier::REVERSED)
            } else if active {
                // Lit even without the pointer on it, so what's already on is
                // legible at a glance, not only under the highlight.
                base.fg(Color::Cyan)
            } else {
                base
            };
            Line::from(Span::styled(
                format!(
                    " {check} {label:<label_w$}{k}   ",
                    label = cmd.label(),
                    k = keys(cmd.hint())
                ),
                style,
            ))
        }
        MenuEntry::Submenu(label, items) => {
            let enabled = items.iter().any(|e| match e {
                MenuEntry::Action(c) => c.enabled(ctx),
                _ => false,
            });
            let style = if !enabled {
                base.fg(Color::Gray).add_modifier(Modifier::DIM)
            } else if selected {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            };
            Line::from(Span::styled(
                format!("   {label:<label_w$}{k} ▸ ", k = keys("")),
                style,
            ))
        }
    }
}

/// The command palette: a query line above a scrolling list of every command,
/// each with its key and its availability. Centered and wide, because unlike the
/// context menu it is read as much as it is aimed at — the only surface in the
/// editor that answers "what can I do here?".
///
/// The list scrolls to keep the highlight visible rather than paging, and stashes
/// the rect it painted at so a click maps to a row the same way the menu's does.
fn render_palette(f: &mut Frame, screen: Rect, palette: &mut Palette, ctx: &Ctx) {
    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let bold = base.add_modifier(Modifier::BOLD);
    let dim = base.fg(Color::Gray).add_modifier(Modifier::DIM);
    let key = base.fg(Color::Cyan);

    let width = 52u16.min(screen.width.max(1));
    // Two rows of chrome (the query line and the hint line) plus the list. The
    // list is as tall as it has rows, capped both by what fits and by a ceiling
    // that keeps the palette from swallowing the document behind it — so a query
    // narrowed to two matches draws a box two rows tall rather than a mostly
    // empty panel that has to be read to discover it's empty.
    let room = (screen.height.saturating_sub(6)).clamp(1, 14);
    let list_h = (palette.rows.len() as u16).clamp(1, room);
    let height = (list_h + 2).min(screen.height.max(1));
    let rect = centered(screen, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(Vec::<Line>::new()).style(base), rect);

    // The query line, with a `›` prompt so an empty box still reads as a box.
    let query = Rect { height: 1, ..rect };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" › ", key),
            Span::styled(palette.query.clone(), bold),
        ]))
        .style(base),
        query,
    );

    // Scroll the window so the highlight is always inside it.
    let list_h = rect.height.saturating_sub(2) as usize;
    let first = palette.selected.saturating_sub(list_h.saturating_sub(1));
    let list_rect = Rect {
        y: rect.y + 1,
        height: rect.height.saturating_sub(2),
        ..rect
    };
    let lines: Vec<Line<'static>> = palette
        .rows
        .iter()
        .skip(first)
        .take(list_h)
        .enumerate()
        .map(|(i, row)| {
            let selected = first + i == palette.selected;
            let label_w = (width as usize).saturating_sub(14);
            let style = if !row.enabled {
                dim
            } else if selected {
                base.add_modifier(Modifier::REVERSED)
            } else if row.command.active(ctx) {
                base.fg(Color::Cyan)
            } else {
                base
            };
            let check = if row.enabled && row.command.active(ctx) {
                '✓'
            } else {
                ' '
            };
            Line::from(Span::styled(
                format!(
                    " {check} {label:<label_w$} {hint:>8} ",
                    label = truncate(row.command.label(), label_w),
                    hint = row.command.hint()
                ),
                style,
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(base), list_rect);
    // Stashed against the *painted* geometry, exactly as the menu does — and
    // offset by the scroll, so a click maps to the row under the pointer rather
    // than to the row that would be there if the list had never scrolled.
    palette.list_rect = Some(list_rect);
    palette.scrolled_by = first;

    let hint = Rect {
        y: rect.y + rect.height - 1,
        height: 1,
        ..rect
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ ", key),
            Span::styled("choose  ", dim),
            Span::styled("enter ", key),
            Span::styled("run  ", dim),
            Span::styled("esc ", key),
            Span::styled("close ", dim),
        ]))
        .style(base),
        hint,
    );

    let cursor_x = rect.x + 3 + palette.query[..palette.cursor].chars().count() as u16;
    if cursor_x < rect.x + rect.width {
        f.set_cursor_position(Position::new(cursor_x, rect.y));
    }
}

/// The key reference — every command that has a key, grouped the way the palette
/// groups them, in two columns so the whole map fits one screen.
///
/// Generated from [`GROUPS`] rather than written out, which is the only reason
/// it can be trusted: a command that gains a key gains a line here in the same
/// commit, and one that loses it loses the line.
fn render_help(f: &mut Frame, screen: Rect, doc: &mut Doc) {
    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let bold = base.add_modifier(Modifier::BOLD);
    let dim = base.fg(Color::Gray).add_modifier(Modifier::DIM);
    let key = base.fg(Color::Cyan);

    // Only the keyed commands: this is the *key* reference, and the palette is
    // where the keyless ones are found.
    let mut rows: Vec<HelpRow> = Vec::new();
    for (group, commands) in GROUPS {
        let keyed: Vec<_> = commands.iter().filter(|c| !c.hint().is_empty()).collect();
        if keyed.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(HelpRow::Blank);
        }
        rows.push(HelpRow::Group(group));
        for cmd in keyed {
            rows.push(HelpRow::Key(cmd.hint(), cmd.label()));
        }
    }
    // The one line the command table can't produce: the palette is how you reach
    // everything that has no key, so the key reference has to name it.
    rows.push(HelpRow::Blank);
    rows.push(HelpRow::Key("⌥p", "the command palette"));

    // Columns, because the reference is fifty-odd rows and a terminal is
    // typically twenty-four. Take the *fewest* columns that fit the screen's
    // height, bounded by how many fit its width: one wide column reads best, and
    // every extra column is paid for only when the height demands it. Nothing
    // fits on a genuinely tiny terminal, and there the widest allowed clips —
    // still a better answer than a card that shows only its first group.
    //
    // The split is by count rather than by group so the columns stay even; a
    // group heading landing at the foot of a column is the smaller cost.
    const COLUMN: usize = 32;
    let by_width = (screen.width as usize / COLUMN).clamp(1, 3);
    let fits = |n: usize| rows.len().div_ceil(n) < screen.height as usize;
    let columns = (1..=by_width).find(|n| fits(*n)).unwrap_or(by_width);
    let per_column = rows.len().div_ceil(columns);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for i in 0..per_column {
        let mut spans = Vec::new();
        for c in 0..columns {
            spans.extend(help_spans(
                rows.get(i + c * per_column),
                COLUMN,
                base,
                bold,
                key,
            ));
        }
        lines.push(Line::from(spans));
    }
    // The footer: what document this is, what the two rendering dials are set to,
    // and how to put the card away. Useful precisely here, because the ⌥⇧W and
    // ⌥⇧F rows above are the keys that move them.
    lines.push(Line::from(Span::styled(
        format!(
            " {} · {} · markup {} · line flow {} · any key closes",
            doc.format_name(),
            doc.view_name(),
            leaf_ratatui::markup_mode_name(doc.markup_mode()),
            leaf_ratatui::line_flow_name(doc.line_flow()),
        ),
        dim,
    )));

    let width = ((COLUMN * columns) as u16).min(screen.width.max(1));
    let height = (lines.len() as u16).min(screen.height.max(1));
    let rect = centered(screen, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);
}

/// One line of the key reference before it's been placed in a column.
#[derive(Clone, Copy)]
enum HelpRow {
    Group(&'static str),
    Key(&'static str, &'static str),
    Blank,
}

/// Render one help row into exactly `width` columns, so the second column of a
/// two-up card starts in the same place on every line. `None` — the right column
/// running out of rows before the left does — is that many spaces.
fn help_spans(
    row: Option<&HelpRow>,
    width: usize,
    base: Style,
    bold: Style,
    key: Style,
) -> Vec<Span<'static>> {
    match row {
        None | Some(HelpRow::Blank) => vec![Span::styled(" ".repeat(width), base)],
        Some(HelpRow::Group(name)) => {
            vec![Span::styled(format!(" {name:<w$}", w = width - 1), bold)]
        }
        Some(HelpRow::Key(hint, label)) => {
            // The key is right-aligned in its own narrow column so the chords
            // line up as a list rather than as ragged text.
            let label_w = width.saturating_sub(9);
            vec![
                Span::styled(format!("  {hint:>4}  "), key),
                Span::styled(format!("{:<label_w$} ", truncate(label, label_w)), base),
            ]
        }
    }
}

/// Cut `s` to `width` columns, with an ellipsis when it doesn't fit. Counted in
/// `char`s, which is what the rest of this file counts in.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
}

/// The single-line input: a label row, a value row, and an Enter/Esc hint,
/// centered over `screen` — there's no click anchor to hang it off the way
/// the context menu has, and nothing in it is clickable, so unlike that menu
/// this stashes no rect back for hit-testing. The caret is the real terminal
/// cursor, positioned into the value row exactly the way the document body
/// positions it into the source — one visible caret, one mechanism.
fn render_text_prompt(f: &mut Frame, screen: Rect, prompt: &TextPrompt) {
    let hint = " enter confirm  esc cancel ";
    let content = [
        prompt.label.chars().count(),
        prompt.value.chars().count(),
        hint.chars().count(),
    ]
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
