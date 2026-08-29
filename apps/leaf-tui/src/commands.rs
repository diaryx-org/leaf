//! Every command the editor offers, as one table.
//!
//! The TUI grew four different surfaces onto the same verbs — the keyboard, the
//! right-click menu, the command palette, and the key reference — and the way
//! that normally rots is that a command gains a key but not a menu row, or a
//! menu row whose label drifts from the one the help screen prints. So the verbs
//! live here, once: [`Command`] names them, and each one knows its own label,
//! its key (if it has one), whether this document's format can spell it, whether
//! it's currently *on*, and how to run itself.
//!
//! Running a command returns a [`leaf_ratatui::Outcome`] — the same type the
//! editing surface hands back for a key — so a command chosen from a menu and
//! the key that would have run it converge on one place in `main`. A command
//! that only mutates the document returns `Outcome::Continue`; one that needs
//! the clipboard, the filesystem, or a prompt names that instead.
//!
//! # Why capability gating lives here
//!
//! The formats are ragged (see [`leaf_core::Capabilities`]): `==highlight==` is
//! djot-only, HTML spells no heading marker and no task box, and only Markdown
//! and djot spell a footnote. Core already *refuses* a gesture its format can't
//! spell and says so in the status line, so the keyboard is safe without any
//! help from here. What the keyboard can't do is tell you in advance — and a
//! menu can, which is why [`Command::enabled`] exists and why the menu, palette,
//! and help screen all dim rather than hide: a control that vanishes teaches
//! nothing about the document you're in.

use leaf_core::{
    Alignment, BlockKind, Capabilities, Doc, InlineKind, InlineMarks, LineFlow, MarkupMode, View,
};
use leaf_ratatui::Outcome;

/// Everything about the document a command needs in order to say whether it's
/// available and whether it's currently on — read once per frame rather than
/// per row, because a menu of thirty rows asking the AST thirty questions is
/// thirty walks of the same tree.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub caps: Capabilities,
    /// `Doc::caret_in_table` — the other half of `caps.table`: an HTML `<table>`
    /// holds the caret and still can't be edited, and a Markdown document with
    /// no table in it can't have a row inserted into one.
    pub in_table: bool,
    pub in_code: bool,
    pub marks: InlineMarks,
    pub heading: Option<u32>,
    pub markup: MarkupMode,
    pub flow: LineFlow,
    pub has_selection: bool,
    pub view: View,
    /// `Doc::read_only` — a whole axis of availability on its own, orthogonal
    /// to the format's. A Markdown document can spell a heading and still
    /// refuse to be given one.
    pub read_only: bool,
}

impl Ctx {
    pub fn read(doc: &mut Doc) -> Self {
        Ctx {
            caps: doc.capabilities(),
            in_table: doc.caret_in_table(),
            in_code: doc.caret_in_fenced_code(),
            marks: doc.active_inline_marks(),
            heading: doc.current_heading_level(),
            markup: doc.markup_mode(),
            flow: doc.line_flow(),
            has_selection: doc.selection().is_some(),
            view: doc.view,
            read_only: doc.read_only(),
        }
    }
}

/// One command. Variants carry their parameter where the same verb applies to a
/// family (`Heading(2)`, `Inline(Strong)`, `Align(Center)`) so the table below
/// stays a table rather than becoming sixteen near-identical arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    // ── edit ──
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PastePlain,
    SelectAll,

    // ── block ──
    Paragraph,
    Heading(u32),
    BulletList,
    NumberedList,
    Quote,
    TaskItem,
    TaskChecked,

    // ── inline ──
    Inline(InlineKind),

    // ── insert ──
    Link,
    Image,
    Video,
    Audio,
    Footnote,
    ThematicBreak,
    CodeLanguage,

    // ── table ──
    RowAbove,
    RowBelow,
    DeleteRow,
    ColumnLeft,
    ColumnRight,
    DeleteColumn,
    MoveRowUp,
    MoveRowDown,
    MoveColumnLeft,
    MoveColumnRight,
    Align(Alignment),

    // ── view ──
    ToggleView,
    /// Step the markup dial one notch — the *key's* command. The three
    /// `Markup(_)` below set a specific notch and are what a menu offers, since
    /// a menu can show all three at once and a key cannot.
    CycleMarkup,
    Markup(MarkupMode),
    /// Flip line flow; the keyed counterpart of the two `Flow(_)` setters, for
    /// `CycleMarkup`'s reason.
    ToggleFlow,
    Flow(LineFlow),

    // ── navigate ──
    Follow,

    // ── file ──
    Save,
    SaveAs,
    New,
    Quit,

    // ── meta ──
    Help,
}

impl Command {
    /// The row's words — the toolbar name a reader knows, not the AST kind
    /// underneath (`Highlight`, not `Mark`; `Strikethrough`, not `Delete`).
    pub fn label(self) -> &'static str {
        use Command::*;
        match self {
            Undo => "Undo",
            Redo => "Redo",
            Cut => "Cut",
            Copy => "Copy",
            Paste => "Paste",
            PastePlain => "Paste as Plain Text",
            SelectAll => "Select All",

            Paragraph => "Paragraph",
            Heading(1) => "Heading 1",
            Heading(2) => "Heading 2",
            Heading(3) => "Heading 3",
            Heading(4) => "Heading 4",
            Heading(5) => "Heading 5",
            Heading(_) => "Heading 6",
            BulletList => "Bulleted List",
            NumberedList => "Numbered List",
            Quote => "Quote",
            TaskItem => "Checklist Item",
            TaskChecked => "Tick Checkbox",

            Inline(InlineKind::Strong) => "Bold",
            Inline(InlineKind::Emph) => "Italic",
            Inline(InlineKind::Verbatim) => "Code",
            Inline(InlineKind::Mark) => "Highlight",
            Inline(InlineKind::Delete) => "Strikethrough",
            Inline(_) => "Underline",

            Link => "Link…",
            Image => "Image…",
            Video => "Video…",
            Audio => "Audio…",
            Footnote => "Footnote",
            ThematicBreak => "Horizontal Rule",
            CodeLanguage => "Code Language…",

            RowAbove => "Insert Row Above",
            RowBelow => "Insert Row Below",
            DeleteRow => "Delete Row",
            ColumnLeft => "Insert Column Left",
            ColumnRight => "Insert Column Right",
            DeleteColumn => "Delete Column",
            MoveRowUp => "Move Row Up",
            MoveRowDown => "Move Row Down",
            MoveColumnLeft => "Move Column Left",
            MoveColumnRight => "Move Column Right",
            Align(Alignment::Left) => "Align Column Left",
            Align(Alignment::Center) => "Align Column Center",
            Align(Alignment::Right) => "Align Column Right",
            Align(_) => "Align Column Default",

            ToggleView => "Toggle Rich View",
            CycleMarkup => "Cycle Markup Mode",
            ToggleFlow => "Toggle Line Flow",
            Markup(MarkupMode::None) => "Markup: Hidden",
            Markup(MarkupMode::Shortcuts) => "Markup: Shortcuts",
            Markup(MarkupMode::Full) => "Markup: Full",
            Flow(LineFlow::Fold) => "Line Flow: Fold",
            Flow(LineFlow::Preserve) => "Line Flow: Preserve",

            Follow => "Follow Link or Footnote",

            Save => "Save",
            SaveAs => "Save As…",
            New => "New Document",
            Quit => "Quit",

            Help => "Keyboard Reference",
        }
    }

    /// The key that runs this command, spelled the way the README spells it
    /// (`^s`, `⌥b`). Empty for the commands that have no key and are reached
    /// through the palette or a menu — which is most of the table, and the
    /// reason the palette exists.
    pub fn hint(self) -> &'static str {
        use Command::*;
        match self {
            Undo => "^z",
            Redo => "^⇧z",
            Cut => "^x",
            Copy => "^c",
            Paste => "^v",
            PastePlain => "⌥v",
            SelectAll => "^a",

            Paragraph => "⌥0",
            Heading(1) => "⌥1",
            Heading(2) => "⌥2",
            Heading(3) => "⌥3",
            Heading(4) => "⌥4",
            Heading(5) => "⌥5",
            Heading(6) => "⌥6",
            Heading(_) => "",
            NumberedList => "⌥7",
            BulletList => "⌥8",
            Quote => "⌥9",
            TaskItem => "⌥t",
            TaskChecked => "⌥x",

            Inline(InlineKind::Strong) => "⌥b",
            Inline(InlineKind::Emph) => "⌥i",
            Inline(InlineKind::Verbatim) => "⌥c",
            Inline(InlineKind::Mark) => "⌥m",
            Inline(InlineKind::Delete) => "⌥d",
            Inline(InlineKind::Insert) => "⌥u",
            Inline(_) => "",

            Link => "⌥k",
            Image => "⌥e",
            Footnote => "⌥f",
            ThematicBreak => "⌥r",
            CodeLanguage => "⌥l",

            ToggleView => "⌥w",
            // The keys cycle rather than select, so they belong to the cycling
            // commands. A `Markup: Full` row claiming ⌥⇧w would be a lie: the
            // key would only land there one press in three.
            CycleMarkup => "⌥⇧w",
            ToggleFlow => "⌥⇧f",

            Follow => "⌥g",

            Save => "^s",
            SaveAs => "⌥s",
            New => "⌥n",
            Quit => "^q",

            Help => "⌥h",

            _ => "",
        }
    }

    /// Whether this document can run the command at all — the format can spell
    /// it *and* the caret is somewhere it applies. False dims the row rather
    /// than removing it, so the surface stays the same shape in every format and
    /// what a format can't do is legible instead of merely absent.
    pub fn enabled(self, ctx: &Ctx) -> bool {
        use Command::*;
        // A reading session withholds everything that would change the document
        // or write a file, before any question about the format is asked — the
        // two are independent, and this one is the coarser.
        if ctx.read_only && self.writes() {
            return false;
        }
        let c = &ctx.caps;
        match self {
            // Nothing to cut or copy without a selection. Paste is always live:
            // what's on the clipboard isn't ours to know before we ask for it.
            Cut | Copy => ctx.has_selection,

            Paragraph | Heading(_) => c.heading,
            BulletList => c.bullet_list,
            NumberedList => c.ordered_list,
            Quote => c.blockquote,
            TaskItem | TaskChecked => c.task,

            Inline(InlineKind::Strong) => c.bold,
            Inline(InlineKind::Emph) => c.italic,
            Inline(InlineKind::Verbatim) => c.code,
            Inline(InlineKind::Mark) => c.mark,
            Inline(InlineKind::Delete) => c.strike,
            Inline(InlineKind::Insert) => c.underline,
            Inline(_) => false,

            Link => c.link,
            // One control with three kinds behind it — core gates all three on
            // the image gesture, for the reason spelled out in `insert_media`.
            Image | Video | Audio => c.image,
            Footnote => c.footnote,
            ThematicBreak => c.thematic_break,
            // Only ever offered with the caret already in a fence: there is no
            // "the language of no code block".
            CodeLanguage => c.code_language && ctx.in_code,

            // Both halves: a format whose tables are editable, and a caret that
            // is actually standing in one.
            RowAbove | RowBelow | DeleteRow | ColumnLeft | ColumnRight | DeleteColumn
            | MoveRowUp | MoveRowDown | MoveColumnLeft | MoveColumnRight | Align(_) => {
                c.table && ctx.in_table
            }

            // The rendering preferences only mean anything in the rich view —
            // the source view shows the source, which is all the markup there is.
            Markup(_) | Flow(_) | CycleMarkup | ToggleFlow => ctx.view == View::Wysiwyg,

            _ => true,
        }
    }

    /// Whether running this command would change the document or write a file
    /// — what [`Command::enabled`] withholds in a read-only session.
    ///
    /// Stated as the complement, because the reading half is the short and
    /// stable list: copy, select-all, the view dials, follow, quit, and the key
    /// reference. A command added to the table later is then withheld by
    /// default, which is the direction an omission should fail in.
    fn writes(self) -> bool {
        use Command::*;
        !matches!(
            self,
            Copy | SelectAll
                | ToggleView
                | CycleMarkup
                | Markup(_)
                | ToggleFlow
                | Flow(_)
                | Follow
                | Quit
                | Help
        )
    }

    /// Whether this command's state is currently *on*, for the row's `✓`.
    /// Only what answers cheaply and unambiguously: the inline marks, the
    /// caret's heading level, and the two rendering dials, which are radio
    /// groups and so are exactly the thing a checkmark is for. The list and
    /// quote toggles would need AST ancestry no frontend surface exposes, and
    /// showing a wrong check is worse than showing none.
    pub fn active(self, ctx: &Ctx) -> bool {
        use Command::*;
        match self {
            Inline(k) => ctx.marks.contains(k),
            Heading(n) => ctx.heading == Some(n),
            Markup(m) => ctx.markup == m,
            Flow(f) => ctx.flow == f,
            ToggleView => ctx.view == View::Wysiwyg,
            _ => false,
        }
    }

    /// Run it. Document mutations happen here and return `Outcome::Continue`;
    /// anything the host owns — the clipboard, the filesystem, a prompt — is
    /// named in the `Outcome` for `main` to carry out, exactly as a key press is.
    pub fn run(self, doc: &mut Doc) -> Outcome {
        use Command::*;
        match self {
            Undo => doc.undo(),
            Redo => doc.redo(),
            Cut => return Outcome::Cut,
            Copy => return Outcome::Copy,
            Paste => return Outcome::Paste,
            PastePlain => return Outcome::PastePlain,
            SelectAll => doc.select_all(),

            Paragraph => doc.set_block(BlockKind::Paragraph),
            Heading(n) => doc.toggle_heading(n),
            BulletList => doc.toggle_list(false),
            NumberedList => doc.toggle_list(true),
            Quote => doc.toggle_blockquote(),
            TaskItem => doc.toggle_task_item(),
            TaskChecked => doc.toggle_task_checked(),

            Inline(k) => doc.toggle(k),

            Link => return Outcome::LinkPrompt,
            Image => return Outcome::ImagePrompt,
            Video => return Outcome::VideoPrompt,
            Audio => return Outcome::AudioPrompt,
            Footnote => doc.insert_footnote(),
            ThematicBreak => doc.insert_thematic_break(),
            CodeLanguage => return Outcome::LanguagePrompt,

            RowAbove => doc.table_insert_row(false),
            RowBelow => doc.table_insert_row(true),
            DeleteRow => doc.table_delete_row(),
            ColumnLeft => doc.table_insert_column(false),
            ColumnRight => doc.table_insert_column(true),
            DeleteColumn => doc.table_delete_column(),
            MoveRowUp => doc.table_move_row(false),
            MoveRowDown => doc.table_move_row(true),
            MoveColumnLeft => doc.table_move_column(false),
            MoveColumnRight => doc.table_move_column(true),
            Align(a) => doc.table_set_alignment(a),

            ToggleView => doc.toggle_view(),
            CycleMarkup => leaf_ratatui::cycle_markup_mode(doc),
            ToggleFlow => leaf_ratatui::toggle_line_flow(doc),
            Markup(m) => {
                doc.set_markup_mode(m);
                doc.status = Some(format!("markup: {}", leaf_ratatui::markup_mode_name(m)));
            }
            Flow(f) => {
                doc.set_line_flow(f);
                doc.status = Some(format!("line flow: {}", leaf_ratatui::line_flow_name(f)));
            }

            Follow => leaf_ratatui::follow(doc),

            Save => return Outcome::Save,
            SaveAs => return Outcome::SaveAs,
            New => return Outcome::New,
            Quit => return Outcome::Quit,

            Help => return Outcome::Help,
        }
        Outcome::Continue
    }
}

/// Every command, grouped under the heading it belongs to — the order the
/// palette lists them in and the order the key reference prints them in. One
/// list rather than two, so a command added to the palette can't be forgotten by
/// the help screen.
///
/// The palette isn't in it: a command palette that offers to open the command
/// palette is a joke the second time and an obstacle every time after.
pub const GROUPS: &[(&str, &[Command])] = &[
    ("Edit", EDIT),
    ("Block", BLOCK),
    ("Inline", INLINE),
    ("Insert", INSERT),
    ("Table", TABLE),
    ("View", VIEW),
    ("Navigate", NAVIGATE),
    ("File", FILE),
];

const EDIT: &[Command] = &[
    Command::Undo,
    Command::Redo,
    Command::Cut,
    Command::Copy,
    Command::Paste,
    Command::PastePlain,
    Command::SelectAll,
];

const BLOCK: &[Command] = &[
    Command::Paragraph,
    Command::Heading(1),
    Command::Heading(2),
    Command::Heading(3),
    Command::Heading(4),
    Command::Heading(5),
    Command::Heading(6),
    Command::BulletList,
    Command::NumberedList,
    Command::Quote,
    Command::TaskItem,
    Command::TaskChecked,
];

const INLINE: &[Command] = &[
    Command::Inline(InlineKind::Strong),
    Command::Inline(InlineKind::Emph),
    Command::Inline(InlineKind::Verbatim),
    Command::Inline(InlineKind::Mark),
    Command::Inline(InlineKind::Delete),
    Command::Inline(InlineKind::Insert),
];

const INSERT: &[Command] = &[
    Command::Link,
    Command::Image,
    Command::Video,
    Command::Audio,
    Command::Footnote,
    Command::ThematicBreak,
    Command::CodeLanguage,
];

const TABLE: &[Command] = &[
    Command::RowAbove,
    Command::RowBelow,
    Command::DeleteRow,
    Command::ColumnLeft,
    Command::ColumnRight,
    Command::DeleteColumn,
    Command::MoveRowUp,
    Command::MoveRowDown,
    Command::MoveColumnLeft,
    Command::MoveColumnRight,
    Command::Align(Alignment::Left),
    Command::Align(Alignment::Center),
    Command::Align(Alignment::Right),
    Command::Align(Alignment::Default),
];

const VIEW: &[Command] = &[
    Command::ToggleView,
    Command::CycleMarkup,
    Command::ToggleFlow,
    Command::Markup(MarkupMode::None),
    Command::Markup(MarkupMode::Shortcuts),
    Command::Markup(MarkupMode::Full),
    Command::Flow(LineFlow::Fold),
    Command::Flow(LineFlow::Preserve),
];

const NAVIGATE: &[Command] = &[Command::Follow];

const FILE: &[Command] = &[
    Command::Save,
    Command::SaveAs,
    Command::New,
    Command::Quit,
    Command::Help,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command in the table can be labelled and hinted without panicking,
    /// and no two share a key. A duplicate hint would mean two rows of the help
    /// screen claiming the same chord, which is exactly the drift this module
    /// exists to prevent.
    #[test]
    fn every_command_has_a_label_and_a_unique_key() {
        let mut keys: Vec<&str> = Vec::new();
        for (_, group) in GROUPS {
            for cmd in *group {
                assert!(!cmd.label().is_empty());
                let hint = cmd.hint();
                if !hint.is_empty() {
                    assert!(
                        !keys.contains(&hint),
                        "{hint} is claimed by more than one command ({})",
                        cmd.label()
                    );
                    keys.push(hint);
                }
            }
        }
    }

    /// The djot-only mark, in the format that spells it and the one that doesn't
    /// — the gating this module exists for, checked against a real document
    /// rather than against a hand-written `Capabilities`.
    #[test]
    fn highlight_is_offered_in_djot_and_dimmed_in_markdown() {
        let highlight = Command::Inline(InlineKind::Mark);

        let mut dj = Doc::from_source("hello\n".into(), leaf_core::Format::Djot).unwrap();
        assert!(highlight.enabled(&Ctx::read(&mut dj)));

        let mut md = Doc::from_source("hello\n".into(), leaf_core::Format::Markdown).unwrap();
        assert!(!highlight.enabled(&Ctx::read(&mut md)));
    }

    /// Table commands need both halves: a format whose tables are editable and a
    /// caret standing in one.
    #[test]
    fn table_commands_need_a_caret_in_a_table() {
        let mut doc = Doc::from_source(
            "para\n\n| a | b |\n| - | - |\n| c | d |\n".into(),
            leaf_core::Format::Markdown,
        )
        .unwrap();

        doc.caret = 1; // in "para"
        assert!(!Command::DeleteRow.enabled(&Ctx::read(&mut doc)));

        doc.caret = doc.source.find("c |").unwrap();
        assert!(Command::DeleteRow.enabled(&Ctx::read(&mut doc)));
    }

    /// The code-language command is offered only inside a fence — there is no
    /// "the language of no code block".
    #[test]
    fn code_language_needs_a_fence() {
        let mut doc = Doc::from_source(
            "para\n\n```\ncode\n```\n".into(),
            leaf_core::Format::Markdown,
        )
        .unwrap();

        doc.caret = 1;
        assert!(!Command::CodeLanguage.enabled(&Ctx::read(&mut doc)));

        doc.caret = doc.source.find("code").unwrap();
        assert!(Command::CodeLanguage.enabled(&Ctx::read(&mut doc)));
    }

    /// A read-only document dims everything that would change it or write it,
    /// and dims nothing a reader needs. The dimming is the *point* of the mode
    /// being visible rather than merely enforced: core refuses these silently,
    /// so a surface that offered them would teach the keyboard was broken.
    #[test]
    fn a_read_only_document_dims_what_would_change_it_and_nothing_else() {
        let mut doc = Doc::from_source("hello world\n".into(), leaf_core::Format::Djot).unwrap();
        doc.set_read_only(true);
        doc.anchor = Some(0);
        doc.caret = 5; // a selection, so Cut/Copy are otherwise both live
        let ctx = Ctx::read(&mut doc);

        for cmd in [
            Command::Undo,
            Command::Cut,
            Command::Paste,
            Command::Heading(1),
            Command::Inline(InlineKind::Strong),
            Command::Link,
            Command::Footnote,
            Command::Save,
            Command::SaveAs,
            Command::New,
        ] {
            assert!(
                !cmd.enabled(&ctx),
                "{} should be dimmed in a read-only document",
                cmd.label()
            );
        }
        for cmd in [
            Command::Copy,
            Command::SelectAll,
            Command::ToggleView,
            Command::CycleMarkup,
            Command::Follow,
            Command::Quit,
            Command::Help,
        ] {
            assert!(
                cmd.enabled(&ctx),
                "{} is what reading a document is made of",
                cmd.label()
            );
        }
    }

    /// Every command is on exactly one side of the read-only line, and the
    /// reading side is the one written out — so a command added to the table
    /// and forgotten here is withheld rather than quietly offered.
    #[test]
    fn every_command_is_classified_for_read_only() {
        for (_, group) in GROUPS {
            for cmd in *group {
                let reads = !cmd.writes();
                assert_eq!(
                    reads,
                    matches!(
                        cmd,
                        Command::Copy
                            | Command::SelectAll
                            | Command::ToggleView
                            | Command::CycleMarkup
                            | Command::Markup(_)
                            | Command::ToggleFlow
                            | Command::Flow(_)
                            | Command::Follow
                            | Command::Quit
                            | Command::Help
                    ),
                    "{} is on the wrong side of the read-only line",
                    cmd.label()
                );
            }
        }
    }

    /// A command that only touches the document reports `Continue`; one the host
    /// owns names itself instead. This is the contract that lets a menu row and
    /// a key press converge on the same handler in `main`.
    #[test]
    fn host_owned_commands_name_themselves_in_the_outcome() {
        let mut doc = Doc::from_source("hello\n".into(), leaf_core::Format::Markdown).unwrap();
        assert_eq!(Command::Heading(2).run(&mut doc), Outcome::Continue);
        assert_eq!(Command::Link.run(&mut doc), Outcome::LinkPrompt);
        assert_eq!(Command::Video.run(&mut doc), Outcome::VideoPrompt);
        assert_eq!(Command::Quit.run(&mut doc), Outcome::Quit);
    }
}
