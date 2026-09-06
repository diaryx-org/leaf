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
//! The formats are ragged (see [`leaf_core::Capabilities`]): `+underline+` is
//! djot-only, HTML spells no heading marker and no task box, and only Markdown
//! and djot spell a footnote. Core already *refuses* a gesture its format can't
//! spell and says so in the status line, so the keyboard is safe without any
//! help from here. What the keyboard can't do is tell you in advance — and a
//! menu can, which is why [`Command::enabled`] exists and why the menu, palette,
//! and help screen all dim rather than hide: a control that vanishes teaches
//! nothing about the document you're in.

use leaf_core::{
    Alignment, BlockKind, Capabilities, Doc, InlineKind, InlineMarks, LineFlow, MarkColor,
    MarkupMode, View,
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
    /// `Doc::caret_in_mark` — the highlight-colour rows' half of the same pair
    /// `in_table` forms with `caps.table`: djot spells a highlight and no colour
    /// on it, and a Markdown document with no highlight under the caret has
    /// nothing to colour either.
    pub in_mark: bool,
    /// The colour of that highlight, for the `✓` on the row naming it.
    pub mark_color: Option<MarkColor>,
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
            in_mark: doc.caret_in_mark(),
            mark_color: doc.mark_color_at_caret(),
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
    /// A highlight's colour — `Some` sets one, `None` takes it off. Its own
    /// verb rather than an `Inline` variant because it is not a mark: it is a
    /// property of a highlight that already exists, and the row is dim wherever
    /// there is no highlight for it to belong to.
    HighlightColor(Option<MarkColor>),

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

    // ── find ──
    Find,
    Replace,

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

            // Named "Highlight …" rather than bare "Red", because the palette
            // is a flat list of every verb: a row saying only "Red" would be a
            // riddle there, and reads no worse inside the flyout.
            HighlightColor(Some(MarkColor::Red)) => "Highlight: Red",
            HighlightColor(Some(MarkColor::Orange)) => "Highlight: Orange",
            HighlightColor(Some(MarkColor::Yellow)) => "Highlight: Yellow",
            HighlightColor(Some(MarkColor::Green)) => "Highlight: Green",
            HighlightColor(Some(MarkColor::Blue)) => "Highlight: Blue",
            HighlightColor(Some(MarkColor::Purple)) => "Highlight: Purple",
            HighlightColor(Some(MarkColor::Brown)) => "Highlight: Brown",
            HighlightColor(None) => "Highlight: No Colour",

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

            Find => "Find…",
            Replace => "Find and Replace…",

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

            // The pair everything from a browser to a word processor spells
            // this way, and both were free. ⌥h is *not* available for the
            // replace half — it is the key reference, and has been longer.
            Find => "^f",
            Replace => "^h",

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

            // Both halves again: a format that spells a colour on a highlight
            // (Markdown does, djot doesn't), and a highlight for it to go on —
            // one the caret is in, or one this press would make out of the
            // selection. A bare caret in plain text has neither.
            HighlightColor(_) => c.mark_color && (ctx.in_mark || ctx.has_selection),

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
                // Finding is reading; replacing is not.
                | Find
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
            // The colour of the highlight the caret is in — a radio group, like
            // the heading levels. "No Colour" ticks only *inside* an uncoloured
            // highlight: outside one there is no colour to be none of.
            HighlightColor(Some(c)) => ctx.mark_color == Some(c),
            HighlightColor(None) => ctx.in_mark && ctx.mark_color.is_none(),
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
            // The compound, not the bare gesture: over a selection this both
            // highlights and colours, as one undo step. See `Doc::highlight`.
            HighlightColor(c) => doc.highlight(c),

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

            Find => return Outcome::Find,
            Replace => return Outcome::Replace,

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
    ("Find", FIND),
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
    // Under the highlight itself, in the palette as in the flyout — the colours
    // are the one family here with no key of their own, and the palette is how
    // they are reached.
    Command::HighlightColor(Some(MarkColor::Red)),
    Command::HighlightColor(Some(MarkColor::Orange)),
    Command::HighlightColor(Some(MarkColor::Yellow)),
    Command::HighlightColor(Some(MarkColor::Green)),
    Command::HighlightColor(Some(MarkColor::Blue)),
    Command::HighlightColor(Some(MarkColor::Purple)),
    Command::HighlightColor(Some(MarkColor::Brown)),
    Command::HighlightColor(None),
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

const FIND: &[Command] = &[Command::Find, Command::Replace];

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
    ///
    /// It is the underline that carries this now. The highlight used to, and
    /// stopped when twig 3.3.1 made `==x==` authorable for an editor holding the
    /// `highlight` extension — which every leaf document does, so the row below
    /// asserts the opposite of what it once did.
    #[test]
    fn underline_is_offered_in_djot_and_dimmed_in_markdown() {
        let underline = Command::Inline(InlineKind::Insert);

        let mut dj = Doc::from_source("hello\n".into(), leaf_core::Format::Djot).unwrap();
        assert!(underline.enabled(&Ctx::read(&mut dj)));

        let mut md = Doc::from_source("hello\n".into(), leaf_core::Format::Markdown).unwrap();
        assert!(!underline.enabled(&Ctx::read(&mut md)));
    }

    /// The pair twig 3.3.1 added, offered in both lightweight formats — the
    /// menu row and the palette row for a gesture core will now carry out.
    #[test]
    fn highlight_and_strikethrough_are_offered_in_both_markdown_and_djot() {
        for fmt in [leaf_core::Format::Markdown, leaf_core::Format::Djot] {
            let mut doc = Doc::from_source("hello\n".into(), fmt).unwrap();
            let ctx = Ctx::read(&mut doc);
            assert!(Command::Inline(InlineKind::Mark).enabled(&ctx), "{fmt:?}");
            assert!(Command::Inline(InlineKind::Delete).enabled(&ctx), "{fmt:?}");
        }
    }

    /// The highlight's colours: dim with nothing to colour, live inside a
    /// highlight and over a selection — and gone entirely in a format that
    /// spells no colour, whatever the caret is standing in.
    #[test]
    fn highlight_colours_need_a_highlight_or_a_selection() {
        let red = Command::HighlightColor(Some(MarkColor::Red));

        let mut md =
            Doc::from_source("a ==word== b\n".into(), leaf_core::Format::Markdown).unwrap();
        md.caret = 0;
        assert!(
            !red.enabled(&Ctx::read(&mut md)),
            "no highlight at the caret"
        );

        md.caret = md.source.find("word").unwrap();
        assert!(red.enabled(&Ctx::read(&mut md)), "inside the highlight");

        md.caret = 1;
        md.anchor = Some(0);
        assert!(
            red.enabled(&Ctx::read(&mut md)),
            "a selection is a highlight this press would make"
        );

        // djot writes `{=word=}` and has no colour for it, so the whole family
        // is dark there even standing in one.
        let mut dj = Doc::from_source("a {=word=} b\n".into(), leaf_core::Format::Djot).unwrap();
        dj.caret = dj.source.find("word").unwrap();
        let ctx = Ctx::read(&mut dj);
        assert!(dj.caret_in_mark());
        assert!(!red.enabled(&ctx));
    }

    /// The `✓` follows the caret's own highlight, and "No Colour" ticks only
    /// where there is a highlight to have none.
    #[test]
    fn the_ticked_colour_is_the_caret_s_own() {
        let mut doc = Doc::from_source(
            "a ==\u{1F534} red== and ==plain== b\n".into(),
            leaf_core::Format::Markdown,
        )
        .unwrap();

        doc.caret = doc.source.find("red=").unwrap();
        let ctx = Ctx::read(&mut doc);
        assert!(Command::HighlightColor(Some(MarkColor::Red)).active(&ctx));
        assert!(!Command::HighlightColor(Some(MarkColor::Blue)).active(&ctx));
        assert!(!Command::HighlightColor(None).active(&ctx));

        doc.caret = doc.source.find("plain").unwrap();
        let ctx = Ctx::read(&mut doc);
        assert!(Command::HighlightColor(None).active(&ctx));
        assert!(!Command::HighlightColor(Some(MarkColor::Red)).active(&ctx));

        doc.caret = 0;
        let ctx = Ctx::read(&mut doc);
        assert!(
            !Command::HighlightColor(None).active(&ctx),
            "outside a highlight there is no colour to be none of"
        );
    }

    /// One row, one press: over a selection the command both highlights and
    /// colours, and one undo takes the whole press back.
    #[test]
    fn a_colour_row_highlights_and_colours_in_one_press() {
        let mut doc = Doc::from_source("a word b\n".into(), leaf_core::Format::Markdown).unwrap();
        doc.anchor = Some(2);
        doc.caret = 6;
        Command::HighlightColor(Some(MarkColor::Green)).run(&mut doc);
        assert_eq!(doc.source, "a ==\u{1F7E2} word== b\n");
        doc.undo();
        assert_eq!(doc.source, "a word b\n");
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
                            | Command::Find
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
