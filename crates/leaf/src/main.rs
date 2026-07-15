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
use leaf_core::{Doc, View};
use leaf_gpui::{Editor, register_keybindings};

// App-level actions the host owns — the editor never quits or opens files itself.
actions!(leaf_app, [Quit, Cancel, OpenFile]);

/// The application shell: window chrome (header, file-open `+`, quit guard) around
/// an embedded [`Editor`]. All editing lives in the widget; this is the app.
struct LeafApp {
    editor: Entity<Editor>,
    focus_handle: FocusHandle,
}

impl LeafApp {
    /// ⌘Q defers to the widget's own close guard (see `Editor::confirm_close`)
    /// so the window's close button, wired the same way below, warns and
    /// confirms identically instead of the host tracking a second arm/disarm bit.
    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.update(cx, |editor, cx| editor.confirm_close(cx)) {
            cx.quit();
        }
    }

    /// Esc is unconditionally bound (`ctx: None`, below) so it always resolves
    /// to this action first, ahead of anything the embedded widget itself
    /// might want to do with the same key — including dismissing its own
    /// modal text prompt. So this asks the widget's prompt to close first and
    /// only falls back to the close-warning guard when there wasn't one.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            if !editor.cancel_prompt(window, cx) {
                editor.cancel_close(cx);
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
        let (has_doc, name, dirty, view, is_dirty, close_armed) = {
            let e = self.editor.read(cx);
            (
                e.has_doc(),
                e.file_name(),
                if e.is_dirty() { " ●" } else { "" },
                e.view_label(),
                e.is_dirty(),
                e.close_armed(),
            )
        };
        let warn = close_armed && is_dirty;
        let header = if warn {
            "Unsaved changes — ⌘Q again to quit without saving, ⌘S to save, Esc to cancel".to_string()
        } else if has_doc {
            format!(
                "leaf — {name}{dirty}   [{view}]   ⌘e view · ⌘b/⌘i/⌘⇧c/⌘⇧m bold/italic/code/mark · \
                 ⌃0-6 ¶/heading · ⌥←/→ word · ⌘↑/↓ doc ends · ⌘z/⇧⌘z undo · ⌘s save"
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
                    .px_3()
                    .py_1()
                    .bg(if warn {
                        gpui::rgb(0xffe9b0)
                    } else {
                        gpui::rgb(0xf0f0f0)
                    })
                    .text_color(gpui::rgb(0x555555))
                    .child(header),
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
                        // Re-render the header when the editor changes (dirty ● /
                        // the close guard's warning banner).
                        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
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
