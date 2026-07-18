//! The host's chrome around the editor widget: a header with a live AST
//! breadcrumb, a toolbar-hint / status footer, and the modal overlays (the
//! Save/Discard/Cancel and conflict prompts, the right-click context menu, and
//! the single-line text prompt). The editing surface itself — the document body,
//! its code boxes, images, scrollbar, and caret — is drawn by
//! [`leaf_ratatui::render`] into the middle chunk.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use leaf_core::{Doc, InlineKind};

use crate::{App, ConflictPrompt, ContextMenu, DirtyAction, DirtyPrompt, MENU_ITEMS, TextPrompt};

pub fn render(f: &mut Frame, doc: &mut Doc, breadcrumb: &str, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(1),    // body
        Constraint::Length(2), // footer
    ])
    .split(f.area());

    render_header(f, chunks[0], doc, breadcrumb);
    // The editing surface is the widget's job; the host owns everything around it.
    leaf_ratatui::render(f, chunks[1], doc, &mut app.editor);
    render_footer(f, chunks[2], doc, app.dirty_prompt.as_ref(), app.conflict.as_ref());

    if let Some(menu) = &mut app.context_menu {
        render_context_menu(f, f.area(), menu);
    }
    if let Some(prompt) = &app.text_prompt {
        render_text_prompt(f, f.area(), prompt);
    }
}

fn render_header(f: &mut Frame, area: Rect, doc: &Doc, breadcrumb: &str) {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![
        Span::styled(
            "leaf ",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▸ ", dim),
        Span::raw(doc.file_name()),
        Span::styled(format!("  [{}]", doc.format_name()), dim),
    ];
    if doc.dirty {
        spans.push(Span::styled(
            "  ● modified",
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(format!("  ⌥w {}", doc.view_name()), dim));
    if !breadcrumb.is_empty() {
        spans.push(Span::styled(format!("   {breadcrumb}"), Style::default().fg(Color::Cyan)));
    }
    f.render_widget(Line::from(spans), area);
}

fn render_footer(
    f: &mut Frame,
    area: Rect,
    doc: &mut Doc,
    dirty_prompt: Option<&DirtyPrompt>,
    conflict: Option<&ConflictPrompt>,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);

    // Both prompts take over the whole footer until answered, so an
    // accidental Ctrl+Q/⌥s/^S on a document with something at stake can't
    // lose or clobber work to a stray keystroke.
    if let Some(prompt) = dirty_prompt {
        let verb = match prompt.action {
            DirtyAction::Quit => "quit",
            DirtyAction::New => "start a new document",
        };
        let lines = choice_prompt_lines(
            &format!("Unsaved changes — {verb}?"),
            &["Save", "Discard", "Cancel"],
            prompt.selected,
            key,
            dim,
        );
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    if let Some(prompt) = conflict {
        let lines = choice_prompt_lines(
            "File changed on disk since it was opened",
            &["Overwrite", "Reload", "Cancel"],
            prompt.selected,
            key,
            dim,
        );
        f.render_widget(Paragraph::new(lines), area);
        return;
    }

    let line1 = if let Some(msg) = &doc.status {
        Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Yellow)))
    } else {
        format_state_line(doc, key)
    };
    let line2 = Line::from(vec![
        Span::styled("type ", key),
        Span::styled("to edit   ", dim),
        Span::styled("⇧+move ", key),
        Span::styled("select   ", dim),
        Span::styled("⌥←/→/⌫/⌦ ", key),
        Span::styled("word   ", dim),
        Span::styled("^a/c/x/v ", key),
        Span::styled("all·copy·cut·paste   ", dim),
        Span::styled("⌥v ", key),
        Span::styled("paste plain   ", dim),
        Span::styled("⌥w ", key),
        Span::styled("view   ", dim),
        Span::styled("^s/⌥s ", key),
        Span::styled("save·save as   ", dim),
        Span::styled("^q ", key),
        Span::styled("quit", dim),
    ]);
    f.render_widget(Paragraph::new(vec![line1, line2]), area);
}

/// Line 1's normal (no-status) content: what's active at the caret, read live
/// off `Doc::active_inline_marks`/`current_heading_level` every frame — the
/// same thing a mouse-driven toolbar shows by lighting up a button, as text
/// since there are no buttons here to light. Only what's actually *on* is
/// listed (a status line states facts; a row of every possible mark grayed
/// out beside it is the toolbar this deliberately isn't), so a bare caret in
/// plain body text prints a dim placeholder rather than leaving the line
/// looking broken or blank.
fn format_state_line(doc: &mut Doc, key: Style) -> Line<'static> {
    let heading = doc.current_heading_level();
    let marks = doc.active_inline_marks();
    let mut spans = Vec::new();
    if let Some(level) = heading {
        spans.push(Span::styled(format!("H{level}"), key.add_modifier(Modifier::BOLD)));
    }
    for kind in marks.iter() {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(mark_label(kind), mark_style(kind)));
    }
    if spans.is_empty() {
        return Line::from(Span::styled("plain text", Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

/// The word a mark reads as in the footer — the toolbar-button name, not the
/// AST node kind `Doc::toggle`/`active_inline_marks` traffic in.
fn mark_label(kind: InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "Bold",
        InlineKind::Emph => "Italic",
        InlineKind::Verbatim => "Code",
        InlineKind::Mark => "Mark",
        InlineKind::Superscript => "Superscript",
        InlineKind::Subscript => "Subscript",
        InlineKind::Insert => "Underline",
        InlineKind::Delete => "Strikethrough",
    }
}

/// Style a mark's footer label the way the WYSIWYG view itself renders that
/// mark (see leaf-core's `wysiwyg::Builder::inline`), so the footer reads as a
/// mirror of the caret's actual formatting instead of an unrelated palette.
fn mark_style(kind: InlineKind) -> Style {
    let base = Style::default();
    match kind {
        InlineKind::Strong => base.add_modifier(Modifier::BOLD),
        InlineKind::Emph => base.add_modifier(Modifier::ITALIC),
        InlineKind::Verbatim => base.fg(Color::Green),
        InlineKind::Mark => base.bg(Color::Yellow).fg(Color::Black),
        InlineKind::Insert => base.add_modifier(Modifier::UNDERLINED),
        InlineKind::Delete => base.add_modifier(Modifier::CROSSED_OUT),
        InlineKind::Superscript | InlineKind::Subscript => base.fg(Color::Cyan),
    }
}

/// Shared two-line rendering for `dirty_prompt` and `conflict`: a warning
/// line naming what's at stake, then `items` laid out with `selected`
/// reversed — arrow-key highlighted the same way the context menu is, plus a
/// first-letter mnemonic per item (the caller's key handling and this must
/// agree on what those letters are; there's only three items either prompt
/// ever has, so they're spelled out in the label rather than derived).
fn choice_prompt_lines(message: &str, items: &[&str], selected: usize, key: Style, dim: Style) -> Vec<Line<'static>> {
    let warn = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (i, label) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", dim));
        }
        let style = if i == selected { key.add_modifier(Modifier::REVERSED) } else { key };
        let mnemonic = label.chars().next().unwrap_or(' ').to_ascii_lowercase();
        spans.push(Span::styled(format!(" {label} ({mnemonic}) "), style));
    }
    vec![Line::from(Span::styled(message.to_string(), warn)), Line::from(spans)]
}

/// The right-click menu: Cut / Copy / Paste / Select All, anchored at the
/// click and nudged back onto `screen` if it wouldn't otherwise fit (the
/// terminal equivalent of the GUI menu's `snap_to_window`). Stashes the rect
/// it painted at back onto `menu` so `main::menu_item_at` can hit-test clicks
/// against the exact geometry drawn here, the same way `doc.body_origin`
/// carries the body's geometry back out to `handle_mouse`.
fn render_context_menu(f: &mut Frame, screen: Rect, menu: &mut ContextMenu) {
    let width = MENU_ITEMS.iter().map(|(label, _)| label.len()).max().unwrap_or(0) as u16 + 4;
    let height = MENU_ITEMS.len() as u16;
    let (anchor_x, anchor_y) = menu.anchor;
    let x = anchor_x.min(screen.width.saturating_sub(width));
    let y = anchor_y.min(screen.height.saturating_sub(height));
    let rect = Rect { x, y, width, height };
    menu.rect = Some(rect);

    let base = Style::default().bg(Color::DarkGray).fg(Color::White);
    let lines: Vec<Line<'static>> = MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let style = if i == menu.selected {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            };
            Line::from(Span::styled(format!(" {label:<pad$}", pad = width as usize - 1), style))
        })
        .collect();

    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(base), rect);
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
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

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
