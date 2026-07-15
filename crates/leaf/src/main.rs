//! The `leaf` crate (binary `leaf-gui`) — the standalone application. A thin host around the
//! embeddable [`leaf_gpui::Editor`] widget: it owns the window, a header bar, a
//! file-open button, and an unsaved-changes quit guard, and embeds the editor
//! for everything else. The same widget powers this app and any gpui host that
//! wants a document editor, which is the whole point of the split.

use std::path::PathBuf;

use gpui::{
    App, Bounds, Context, CursorStyle, Entity, FocusHandle, Focusable, IntoElement, KeyBinding,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use gpui_platform::application;
use leaf_core::{Doc, InlineKind, View};
use leaf_gpui::{Editor, EditorEvent, register_keybindings};

// App-level actions the host owns — the editor never quits or opens files itself.
actions!(leaf_app, [Quit, Cancel, OpenFile]);

/// The badge the header lights up for an inline mark in force at the caret.
/// Short enough to sit in a row of them without crowding the file name out.
fn mark_badge(kind: InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "B",
        InlineKind::Emph => "I",
        InlineKind::Verbatim => "code",
        InlineKind::Mark => "mark",
        InlineKind::Superscript => "sup",
        InlineKind::Subscript => "sub",
        // twig's edit-tracking pair, which leaf binds as underline (⌘⇧U) and
        // strikethrough (⌘⇧X) — so the badge names the key, not the AST node.
        InlineKind::Insert => "U",
        InlineKind::Delete => "S",
    }
}

/// The application shell: window chrome (header, file-open `+`, quit guard) around
/// an embedded [`Editor`]. All editing lives in the widget; this is the app.
struct LeafApp {
    editor: Entity<Editor>,
    focus_handle: FocusHandle,
}

impl LeafApp {
    /// ⌘Q defers to the widget's own close guard (see `Editor::confirm_close`)
    /// so the window's close button, wired the same way below, asks identically
    /// instead of the host growing a second copy of the question. A dirty
    /// document answers `false` and puts Save / Discard / Cancel on screen; the
    /// quit then arrives (or doesn't) as an `EditorEvent::CloseConfirmed`.
    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.update(cx, |editor, cx| editor.confirm_close(cx)) {
            cx.quit();
        }
    }

    /// Esc is unconditionally bound (`ctx: None`, below) so it always resolves
    /// to this action first, ahead of anything the embedded widget itself
    /// might want to do with the same key — including dismissing its own
    /// modal prompt or dialog. So this hands the keystroke to those in turn
    /// rather than letting the binding swallow it.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            if !editor.cancel_prompt(window, cx) {
                editor.dismiss_dialog(cx);
            }
        });
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_dialog(window, cx);
    }

    /// Open the platform file picker and load the chosen document into the
    /// embedded editor. twig handles markdown, djot, and HTML.
    fn open_file_dialog(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let editor = self.editor.downgrade();
        cx.spawn(async move |_app, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            match Doc::open(path) {
                // The editor is always in the tree (and focused), so populating
                // its document is all that's needed — no re-focus dance.
                Ok(doc) => {
                    editor.update(cx, |e, cx| e.set_doc(doc, cx)).ok();
                }
                Err(e) => eprintln!("leaf: {e}"),
            }
        })
        .detach();
    }
}

impl Render for LeafApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Snapshot what the header needs, then drop the editor borrow before we
        // build listeners that also touch `cx`.
        let (has_doc, name, dirty, view) = {
            let e = self.editor.read(cx);
            (
                e.has_doc(),
                e.file_name(),
                if e.is_dirty() { " ●" } else { "" },
                e.view_label(),
            )
        };
        // Which inline marks are in force where the caret is — Bold lights up
        // when the caret sits in bold text, not just when ⌘B was the last key.
        // A separate `update` because leaf-core answers this from the AST and so
        // needs `&mut` (see `Editor::active_marks`); it's an AST walk over a
        // `Copy` bitset, built to be asked every frame, not a file read.
        let marks = self.editor.update(cx, |e, _| e.active_marks());
        let header = if has_doc {
            format!(
                "leaf — {name}{dirty}   [{view}]   ⌘e view · ⌘b/⌘i/⌘⇧c/⌘⇧m/⌘⇧x/⌘⇧u \
                 bold/italic/code/mark/strike/underline · ⌃0-6 ¶/heading · ⇥/⇧⇥ indent · \
                 ⌥←/→ word · ⌘z/⇧⌘z undo · ⌘n new · ⌘s save · ⌘⇧s save as"
            )
        } else {
            "leaf — no file open   (⌘O or click + to open a markdown, djot, or HTML file)".to_string()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Helvetica")
            .bg(gpui::white())
            .text_color(gpui::rgb(0x1e1e1e))
            .key_context("LeafApp")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::open_file))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py_1()
                    .bg(gpui::rgb(0xf0f0f0))
                    .text_color(gpui::rgb(0x555555))
                    .child(header)
                    .children(marks.iter().map(|kind| {
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(gpui::rgb(0xd4dcf0))
                            .text_color(gpui::rgb(0x1e3a8a))
                            .child(mark_badge(kind))
                    })),
            )
            .child(
                // The editor fills the rest; the `+` overlay shows only when empty.
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.editor.clone())
                    .when(!has_doc, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .id("open-file")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(96.0))
                                        .rounded_full()
                                        .bg(gpui::rgb(0xf0f0f0))
                                        .text_color(gpui::rgb(0x555555))
                                        .text_size(px(56.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|s| s.bg(gpui::rgb(0xe4e4e4)))
                                        .child("+")
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.open_file_dialog(window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .mt_4()
                                        .text_color(gpui::rgb(0x999999))
                                        .child("Open a markdown, djot, or HTML file"),
                                ),
                        )
                    }),
            )
    }
}

impl Focusable for LeafApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn main() {
    // `leaf-gui <file> [wysiwyg|source]` — the optional second arg picks the
    // starting view. leaf defaults to the rich-text (WYSIWYG) view; pass
    // `source` to start in raw source. With no file we open to the empty `+` canvas.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let doc: Option<Doc> = match args.first() {
        None => None,
        Some(path) => {
            match Doc::open(PathBuf::from(path)) {
                Ok(mut d) => {
                    match args.get(1).map(|s| s.as_str()) {
                        Some("source") => d.view = View::Source,
                        Some("wysiwyg") => d.view = View::Wysiwyg,
                        _ => {} // keep Doc::open's default (WYSIWYG)
                    }
                    Some(d)
                }
                Err(e) => {
                    eprintln!("leaf: {e}");
                    std::process::exit(1);
                }
            }
        }
    };

    application().run(move |cx: &mut App| {
        register_keybindings(cx); // the editor's keys, scoped to its context
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("escape", Cancel, None),
            KeyBinding::new("cmd-o", OpenFile, None),
        ]);

        let bounds = Bounds::centered(None, size(px(820.0), px(640.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    let editor = cx.new(|cx| Editor::new(cx, doc));

                    // Route the window's close button through the same guard as
                    // ⌘Q (`LeafApp::quit`), so clicking it can't silently discard
                    // unsaved edits. `confirm_close` lives on the widget itself, so
                    // any other gpui host embedding `Editor` inherits this for free
                    // instead of reimplementing a quit-armed dance of its own.
                    let close_editor = editor.clone();
                    window.on_window_should_close(cx, move |_window, cx| {
                        if close_editor.update(cx, |editor, cx| editor.confirm_close(cx)) {
                            cx.quit();
                        }
                        // Always veto the native close: a confirmed close quits via
                        // `cx.quit()` above instead, since gpui doesn't otherwise
                        // treat "last window closed" as "quit the application".
                        false
                    });

                    cx.new(|cx| {
                        // Re-render the header when the editor changes (dirty ●,
                        // the caret's active marks).
                        cx.observe(&editor, |_: &mut LeafApp, _, cx| cx.notify()).detach();

                        // The widget owns the close question but can't act on the
                        // answer — quitting is the app's. Two of the three answers
                        // (Save, Discard) end here.
                        cx.subscribe(&editor, |_, _, event, cx| match event {
                            EditorEvent::CloseConfirmed => cx.quit(),
                        })
                        .detach();

                        // Notice a file edited by something else while leaf wasn't
                        // looking. Window activation is the moment for it: the user
                        // is coming back from whatever touched the file, and
                        // `disk_state` is a read + hash — affordable per
                        // activation, never per frame. gpui's activation observer
                        // fires on deactivation too, hence the `is_window_active`
                        // check rather than a read on the way out as well.
                        cx.observe_window_activation(window, |app, window, cx| {
                            if window.is_window_active() {
                                app.editor.update(cx, |e, cx| e.check_disk_state(cx));
                            }
                        })
                        .detach();

                        LeafApp {
                            editor,
                            focus_handle: cx.focus_handle(),
                        }
                    })
                },
            )
            .unwrap();

        // Focus the embedded editor so keystrokes land in it from the start.
        window
            .update(cx, |app, window, cx| {
                let handle = app.editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                cx.activate(true);
            })
            .unwrap();
    });
}
