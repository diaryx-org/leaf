//! Block-level images, video and audio, drawn as pictures from core's
//! `MediaInfo` in place of the one-row `🖼 alt` placeholder.
//!
//! Core reserves the placeholder row and names what goes there: the media's
//! kind, its destination (verbatim — core does no I/O), its alt text, a
//! video's poster and a `<picture>`'s `<source>`s. Like the Swift editor and
//! leaf-gpui, this lays the box out in pixels and never calls
//! `Doc::set_media_rows`, which is for a terminal answering in character
//! cells: the row stays one row, and the block stands in for it at the box's
//! height.
//!
//! Blitz draws the picture itself: an `img` whose `src` is the file's
//! `file://` URL, which its `net` provider reads and decodes (PNG, JPEG, GIF,
//! WebP, TIFF, and SVG). What it cannot say synchronously is how big the
//! picture is, and the block's height has to be known at layout, so this
//! reads that off the file's header (`image`) or the SVG's canvas (`usvg`)
//! once per path, and caches it — a failure too, so a broken path is tried
//! once rather than every frame.
//!
//! Playback is out of scope, as it is in Swift's `MediaLayout`: a video draws
//! its poster (or a dark 16:9 box) with a play badge, audio a fixed-height
//! chip, and a picture that would not load a dashed chip with its alt text.
//! The caret's two homes — core's `MediaStop::Before` and `After`, the media's
//! start and the row's end — are drawn as a bar at the box's left or right.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_widget_spike::rows::{RowView, Run};
use dioxus_widget_spike::style::class_of;
use leaf_core::{ColorScheme, Glyph, MediaInfo, MediaKind, Role, VRow};

use crate::row::{LayoutHandle, PAD, Pointer, PointerSink, Seg, Target, lines, press_mode};
use crate::text::{Fonts, Kind};

/// Room above and below a box, so it does not crowd the prose.
pub const GAP: f32 = 6.0;
/// The tallest a picture is drawn, so one image cannot fill the window — the
/// Swift editor's `MediaMetrics.maxHeight`.
pub const MAX_H: f32 = 420.0;
/// The narrowest column a box lays out into.
const MIN_W: f32 = 80.0;
/// Audio has no picture: a transport this size.
const AUDIO: (f32, f32) = (360.0, 44.0);
/// A chip standing in for a picture that would not load.
const CHIP: (f32, f32) = (280.0, 44.0);
/// The shape a video with no poster is reserved at.
const VIDEO_ASPECT: (f32, f32) = (1600.0, 900.0);
/// The play badge's diameter.
const BADGE: f32 = 44.0;

/// Where a destination is on disk: an absolute path or a `file:` URL as it
/// is, a relative one against the document's directory. `None` for anything
/// that is not a local file (a web URL, a `data:` URI) or a relative path in
/// a document that has not been saved anywhere yet.
pub fn resolve(dest: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") || dest.starts_with("//")
    {
        return None;
    }
    let raw = dest.strip_prefix("file://").unwrap_or(dest);
    // Markdown links are URLs: a space in a file name is written `%20`.
    let raw = percent_decode(raw);
    let path = Path::new(&raw);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        doc_dir.filter(|d| !d.as_os_str().is_empty()).map(|d| d.join(path))
    }
}

/// `%XX` escapes decoded; anything malformed is kept as written.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        if b[i] == b'%'
            && let (Some(Some(hi)), Some(Some(lo))) = (b.get(i + 1).map(|&c| hex(c)), b.get(i + 2).map(|&c| hex(c)))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// A path as the `file://` URL Blitz loads, with the characters a URL cannot
/// hold escaped.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => url.push(b as char),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// Fit `natural` inside `max_w` × `max_h` without distorting it or scaling it
/// up: a small picture stays its own size, an upscaled icon spanning the
/// column looking like a bug. Swift's `MediaLayout.fit`.
pub fn fit(natural: (f32, f32), max_w: f32, max_h: f32) -> (f32, f32) {
    let (w, h) = natural;
    if w <= 0.0 || h <= 0.0 {
        return (max_w, max_h);
    }
    let scale = 1f32.min(max_w / w).min(max_h / h);
    ((w * scale).round(), (h * scale).round())
}

/// The box a medium draws in a column `avail` pixels wide, given the still's
/// natural size if it has one that loaded.
pub fn box_size(kind: MediaKind, still: Option<(f32, f32)>, avail: f32) -> (f32, f32) {
    let max_w = avail.max(MIN_W);
    match (kind, still) {
        (MediaKind::Audio, _) => (max_w.min(AUDIO.0), AUDIO.1),
        (_, Some(natural)) => fit(natural, max_w, MAX_H),
        (MediaKind::Video, None) => fit(VIDEO_ASPECT, max_w, MAX_H),
        (MediaKind::Image, None) => (max_w.min(CHIP.0), CHIP.1),
    }
}

/// A still that loaded: where Blitz reads it from, and its natural size.
#[derive(Clone, PartialEq, Debug)]
pub struct Still {
    pub url: String,
    pub size: (f32, f32),
}

thread_local! {
    /// Every path measured so far, a failure included. The editor's state is
    /// all on the UI thread, as is this.
    static STILLS: RefCell<HashMap<PathBuf, Option<Rc<Still>>>> = RefCell::default();
}

/// The still at `path`, measured once.
fn still(path: &Path) -> Option<Rc<Still>> {
    STILLS.with(|c| {
        c.borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let size = measure(path);
                crate::trace::trace(|| format!("media: {} -> {size:?}", path.display()));
                size.map(|size| Rc::new(Still { url: file_url(path), size }))
            })
            .clone()
    })
}

/// A picture's natural size, from its header — or an SVG's declared canvas —
/// without decoding a pixel.
fn measure(path: &Path) -> Option<(f32, f32)> {
    let svg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if svg {
        let data = std::fs::read(path).ok()?;
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
        return Some((tree.size().width(), tree.size().height()));
    }
    let (w, h) = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok()?;
    Some((w as f32, h as f32))
}

/// Where the caret is, if it is at the picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Caret {
    Before,
    After,
}

/// A medium as it draws, laid out.
#[derive(Clone, PartialEq)]
pub struct Block {
    pub first: usize,
    pub rows: std::ops::Range<usize>,
    pub kind: MediaKind,
    /// What is drawn in the box, if anything loaded.
    pub still: Option<Rc<Still>>,
    /// The chip's text when nothing did: the alt, or the file's name.
    pub label: String,
    /// A list's indent or a quote's gutter, drawn before the box.
    pub prefix: RowView,
    pub prefix_layout: LayoutHandle,
    pub indent: f32,
    /// The box, in CSS pixels.
    pub size: (f32, f32),
    /// The caret's two homes: the media's start and the row's end.
    pub start: usize,
    pub end: usize,
    pub caret: Option<Caret>,
    pub selected: bool,
}

impl Block {
    /// How tall it draws.
    pub fn height(&self) -> f32 {
        self.size.1 + 2.0 * GAP
    }

    /// Where a click `x` pixels into the row lands: in front of the picture
    /// on its left half, past it on its right — the two stops core gives the
    /// placeholder row, as a click on the Swift editor's box lands on one.
    pub fn hit(&self, x: f32) -> usize {
        if x < self.indent + self.size.0 / 2.0 { self.start } else { self.end }
    }
}

/// The media's start offset, read off its label, and the row's end. Not the
/// row's first glyph: on a listed or quoted picture that is the prefix.
pub fn stops(row: &VRow) -> Option<(usize, usize)> {
    let start = row.glyphs.iter().find(|g| g.style.role == Role::Image)?.src;
    Some((start, row.end_src))
}

/// The prefix glyphs before the label, as a view.
fn prefix_of(glyphs: &[Glyph]) -> RowView {
    let mut runs: Vec<Run> = Vec::new();
    let mut open = None;
    for g in glyphs.iter().take_while(|g| g.style.role != Role::Image) {
        if open != Some(g.style) {
            let mut class = String::new();
            class_of(&g.style, &mut class);
            runs.push(Run { text: String::new(), class, src: None });
            open = Some(g.style);
        }
        runs.last_mut().expect("a run was just opened").text.push(g.drawn());
    }
    RowView { runs, ..RowView::default() }
}

/// Lay out one medium at `rows[info.rows_span.start]` in a column `width`
/// pixels wide, its relative paths resolved against `doc_dir`. `reuse` hands
/// back last frame's layout of the same prefix, so a block nothing changed
/// in compares equal and is not rendered again.
#[allow(clippy::too_many_arguments)]
pub fn block(
    info: &MediaInfo,
    rows: &[VRow],
    doc_dir: Option<&Path>,
    caret: usize,
    selection: Option<(usize, usize)>,
    fonts: &mut Fonts,
    width: f32,
    reuse: impl Fn(&RowView) -> Option<LayoutHandle>,
) -> Option<Block> {
    let row = rows.get(info.rows_span.start)?;
    let (start, end) = stops(row)?;
    let still = info.still(ColorScheme::Light).and_then(|s| resolve(s, doc_dir)).and_then(|p| still(&p));
    let prefix = prefix_of(&row.glyphs);
    let prefix_layout =
        reuse(&prefix).unwrap_or_else(|| LayoutHandle(Rc::new(fonts.layout(&prefix, f32::INFINITY, Kind::Prose))));
    let indent = if prefix.runs.is_empty() { 0.0 } else { prefix_layout.0.width };
    let size = box_size(info.kind, still.as_ref().map(|s| s.size), width - indent);
    let label = if info.alt.is_empty() {
        let dest = info.resolve(ColorScheme::Light);
        dest.rsplit('/').next().unwrap_or(dest).to_owned()
    } else {
        info.alt.clone()
    };
    let caret = match caret {
        c if c == start => Some(Caret::Before),
        c if c == end => Some(Caret::After),
        _ => None,
    };
    let selected = selection.is_some_and(|(lo, hi)| lo <= start && hi >= end && lo < hi);
    Some(Block {
        first: info.rows_span.start,
        rows: info.rows_span.clone(),
        kind: info.kind,
        still,
        label,
        prefix,
        prefix_layout,
        indent,
        size,
        start,
        end,
        caret,
        selected,
    })
}

/// A medium, drawn as its picture, poster or chip in place of its row.
#[component]
pub fn MediaBlock(block: Block) -> Element {
    let sink = use_context::<PointerSink>();
    let drag = sink.clone();
    let first = block.first;
    let (w, h) = block.size;
    let prefix = lines(&block.prefix, &block.prefix_layout.0).into_iter().next().unwrap_or_default();
    let caret = |at: Caret| {
        (block.caret == Some(at)).then(|| {
            rsx! { span { style: "flex: none; width: 0; height: {h}px; border-left: 2px solid #1a73e8; margin-right: -2px;" } }
        })
    };
    let ring = if block.selected { "outline: 3px solid #1a73e8;" } else { "" };
    let badge = rsx! {
        div { style: "width: {BADGE}px; height: {BADGE}px; border-radius: {BADGE / 2.0}px; background: rgba(0,0,0,0.55);
                      color: #fff; font-size: 20px; line-height: {BADGE}px; text-align: center;",
            "▶"
        }
    };
    let body = match (&block.still, block.kind) {
        (Some(s), MediaKind::Image) => rsx! {
            img { src: "{s.url}", style: "display: block; width: {w}px; height: {h}px; border-radius: 6px; {ring}" }
        },
        // A poster under the play badge: the poster as the box's background,
        // so the badge needs no absolute positioning (which Blitz lays out
        // only on the first paint).
        (Some(s), _) => rsx! {
            div { style: "display: flex; align-items: center; justify-content: center; width: {w}px; height: {h}px;
                          border-radius: 6px; background-image: url({s.url}); background-size: {w}px {h}px; {ring}",
                {badge}
            }
        },
        (None, MediaKind::Video) => rsx! {
            div { style: "display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px;
                          width: {w}px; height: {h}px; border-radius: 6px; background: #24292f; color: #d0d7de;
                          font-size: 13px; {ring}",
                {badge}
                span { "{block.label}" }
            }
        },
        (None, MediaKind::Audio) => rsx! {
            div { style: "display: flex; align-items: center; gap: 10px; padding: 0 10px; box-sizing: border-box;
                          width: {w}px; height: {h}px; border-radius: 6px; background: #f6f8fa;
                          border: 1px solid #d0d7de; font-size: 13px; color: #24292f; {ring}",
                span { "🔊" }
                span { "{block.label}" }
            }
        },
        (None, MediaKind::Image) => rsx! {
            div { style: "display: flex; align-items: center; gap: 8px; padding: 0 10px; box-sizing: border-box;
                          width: {w}px; height: {h}px; border-radius: 6px; border: 1px dashed #afb8c1;
                          font-size: 13px; color: #57606a; white-space: pre; {ring}",
                span { "🖼" }
                span { "{block.label}" }
            }
        },
    };
    rsx! {
        div { class: "media", style: "display: flex; align-items: flex-start; box-sizing: border-box;
                                      height: {block.height()}px; padding: {GAP}px 0;",
            onmousedown: move |e: MouseEvent| {
                let x = (e.client_coordinates().x - PAD) as f32;
                let (extend, mode) = press_mode(&e);
                (sink.0)(Pointer::Press { at: Target::Block { first }, line: 0, x, extend, mode });
            },
            onmousemove: move |e: MouseEvent| {
                if e.held_buttons().contains(MouseButton::Primary) {
                    let x = (e.client_coordinates().x - PAD) as f32;
                    (drag.0)(Pointer::Drag { at: Target::Block { first }, line: 0, x });
                }
            },
            span { style: "flex: none; width: {block.indent}px; white-space: pre;",
                for seg in prefix {
                    match seg {
                        Seg::Text(class, text) => rsx! { span { class, "{text}" } },
                        Seg::Caret => rsx! {},
                    }
                }
            }
            {caret(Caret::Before)}
            {body}
            {caret(Caret::After)}
        }
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format};

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        d
    }

    fn blocks_of(d: &Doc, caret: usize) -> Vec<Block> {
        let mut fonts = Fonts::new();
        let dir = Some(Path::new("/nowhere"));
        d.vmap
            .media
            .iter()
            .filter_map(|m| block(m, &d.vmap.rows, dir, caret, d.selection(), &mut fonts, 600.0, |_| None))
            .collect()
    }

    #[test]
    fn a_picture_stands_for_its_one_placeholder_row() {
        let src = "Above.\n\n![a cat](cat.png)\n\nBelow.\n";
        let d = doc(src);
        let b = blocks_of(&d, 0);
        assert_eq!(b.len(), 1);
        let row = d.vmap.rows.iter().position(|r| r.glyphs.iter().any(|g| g.style.role == Role::Image)).unwrap();
        assert_eq!((b[0].first, b[0].rows.clone()), (row, row..row + 1));
        // Its stops are the markup's two ends.
        let at = src.find("![").unwrap();
        assert_eq!((b[0].start, b[0].end), (at, at + "![a cat](cat.png)".len()));
        // Nothing at /nowhere: the alt on a chip.
        assert_eq!((b[0].still.clone(), b[0].label.as_str(), b[0].size), (None, "a cat", CHIP));
        assert!(b[0].prefix.runs.is_empty());
    }

    #[test]
    fn a_listed_picture_keeps_its_marker_before_the_box() {
        let src = "- one\n- ![](pics/dog.png)\n";
        let d = doc(src);
        let b = &blocks_of(&d, 0)[0];
        let prefix: String = b.prefix.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(!prefix.trim().is_empty(), "the list marker: {prefix:?}");
        assert!(b.indent > 0.0);
        assert_eq!(b.start, src.find("![").unwrap());
        // No alt: named by its file.
        assert_eq!(b.label, "dog.png");
    }

    #[test]
    fn the_caret_draws_at_either_stop_and_not_elsewhere() {
        let src = "![a cat](cat.png)\n\nBelow.\n";
        let d = doc(src);
        let end = "![a cat](cat.png)".len();
        assert_eq!(blocks_of(&d, 0)[0].caret, Some(Caret::Before));
        assert_eq!(blocks_of(&d, end)[0].caret, Some(Caret::After));
        assert_eq!(blocks_of(&d, src.len() - 2)[0].caret, None);
    }

    #[test]
    fn a_picture_on_disk_is_measured_and_drawn_from_its_file_url() {
        let dir = std::env::temp_dir().join(format!("agent-leaf-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbImage::new(64, 48).save(dir.join("pic.png")).unwrap();
        let d = doc("![](pic.png)\n");
        let b = block(&d.vmap.media[0], &d.vmap.rows, Some(&dir), 0, None, &mut Fonts::new(), 600.0, |_| None);
        std::fs::remove_dir_all(&dir).unwrap();
        let still = b.unwrap().still.expect("the picture loaded");
        assert_eq!(still.size, (64.0, 48.0));
        assert_eq!(still.url, file_url(&dir.join("pic.png")));
    }

    #[test]
    fn a_relative_path_resolves_against_the_documents_directory() {
        let dir = Path::new("/notes");
        assert_eq!(resolve("cat.png", Some(dir)), Some(PathBuf::from("/notes/cat.png")));
        assert_eq!(resolve("img/cat.png", Some(dir)), Some(PathBuf::from("/notes/img/cat.png")));
        assert_eq!(resolve("../cat.png", Some(dir)), Some(PathBuf::from("/notes/../cat.png")));
        assert_eq!(resolve(" my%20cat.png ", Some(dir)), Some(PathBuf::from("/notes/my cat.png")));
    }

    #[test]
    fn an_absolute_path_or_file_url_is_taken_as_it_is() {
        let dir = Path::new("/notes");
        assert_eq!(resolve("/pics/cat.png", Some(dir)), Some(PathBuf::from("/pics/cat.png")));
        assert_eq!(resolve("file:///pics/cat.png", None), Some(PathBuf::from("/pics/cat.png")));
    }

    #[test]
    fn what_is_not_a_local_file_does_not_resolve() {
        let dir = Some(Path::new("/notes"));
        assert_eq!(resolve("https://example.com/cat.png", dir), None);
        assert_eq!(resolve("//example.com/cat.png", dir), None);
        assert_eq!(resolve("data:image/png;base64,AAAA", dir), None);
        assert_eq!(resolve("", dir), None);
        // A relative path in a document saved nowhere has nothing to be
        // relative to.
        assert_eq!(resolve("cat.png", None), None);
        assert_eq!(resolve("cat.png", Some(Path::new(""))), None);
    }

    #[test]
    fn a_file_url_escapes_what_a_url_cannot_hold() {
        assert_eq!(file_url(Path::new("/a b/c%.png")), "file:///a%20b/c%25.png");
    }

    #[test]
    fn a_picture_fits_the_column_keeping_its_shape_and_never_grows() {
        // Wider than the column: scaled to it.
        assert_eq!(fit((1200.0, 600.0), 600.0, MAX_H), (600.0, 300.0));
        // Taller than the cap: scaled to that instead.
        assert_eq!(fit((400.0, 1600.0), 600.0, MAX_H), (105.0, 420.0));
        // Small: its own size.
        assert_eq!(fit((16.0, 16.0), 600.0, MAX_H), (16.0, 16.0));
        // No size to keep: the whole box.
        assert_eq!(fit((0.0, 10.0), 600.0, MAX_H), (600.0, MAX_H));
    }

    #[test]
    fn each_kind_has_its_box() {
        assert_eq!(box_size(MediaKind::Image, Some((1000.0, 500.0)), 500.0), (500.0, 250.0));
        assert_eq!(box_size(MediaKind::Image, None, 500.0), CHIP);
        assert_eq!(box_size(MediaKind::Audio, None, 200.0), (200.0, AUDIO.1));
        assert_eq!(box_size(MediaKind::Video, None, 640.0), (640.0, 360.0));
        // A poster is measured like a picture.
        assert_eq!(box_size(MediaKind::Video, Some((320.0, 240.0)), 640.0), (320.0, 240.0));
        // A column too narrow for anything still draws something.
        assert_eq!(box_size(MediaKind::Image, Some((1000.0, 500.0)), 10.0), (MIN_W, 40.0));
    }

    #[test]
    fn a_click_lands_before_or_after_the_picture_by_its_half() {
        let b = Block {
            first: 0,
            rows: 0..1,
            kind: MediaKind::Image,
            still: None,
            label: String::new(),
            prefix: RowView::default(),
            prefix_layout: LayoutHandle(Rc::new(Fonts::new().layout(&RowView::default(), f32::INFINITY, Kind::Prose))),
            indent: 20.0,
            size: (200.0, 100.0),
            start: 2,
            end: 19,
            caret: None,
            selected: false,
        };
        assert_eq!(b.hit(0.0), 2);
        assert_eq!(b.hit(119.0), 2);
        assert_eq!(b.hit(121.0), 19);
        assert_eq!(b.hit(900.0), 19);
        assert_eq!(b.height(), 100.0 + 2.0 * GAP);
    }
}
