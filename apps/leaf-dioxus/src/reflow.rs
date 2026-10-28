//! The reflowed view, laid out a chunk at a time.
//!
//! Reflowed, the document is one page as tall as its text, and nothing on it
//! depends on where a page breaks: a block is set the same however much text
//! is above it. So the document is cut into chunks of consecutive top-level
//! blocks, each set by Typst on its own and kept, and a keystroke sets again
//! only the chunk it is in. A refresh costs what a chunk costs, however long
//! the document; the first one sets every chunk, side by side on the
//! machine's cores.
//!
//! Each chunk's source is its blocks, after the block before them (the
//! *context*) and before the footnote and link definitions they call on; a
//! segment table takes each byte of it back to the document's. Where a
//! chunk goes down the page is found by its context: the block it begins
//! with is the previous chunk's last, and lining the two up puts the space
//! between them exactly where Typst put it. A chunk counts its footnotes on
//! from the ones above it, and hides their notes; the notes are set once,
//! together, below the last chunk, as the one page sets them.
//!
//! Chunks are cut where a block's own text says so (its hash), within
//! bounds on their size, not every so many blocks: a paragraph put in or
//! taken out moves no other chunk's boundaries, so every other chunk is the
//! same text, and kept.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use leaf_core::{Format, TopBlock, TopKind};
use twig_export::pdf::Glyph;

use crate::pages::{Geometry, Line, Sheet};
use crate::pdf::Type;
use crate::typeset::Tiled;

/// The least a chunk holds before it may end, and the most, in bytes.
const LEAST: usize = 1500;
const MOST: usize = 6000;

/// What a segment of a chunk's source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Role {
    /// The block before the chunk's, set to place the chunk under it.
    Context,
    /// One of the chunk's own blocks.
    Own,
    /// A footnote's definition, which sets where it is called.
    Note,
    /// Anything else the chunk calls on and does not show: a link
    /// definition, the frontmatter.
    Quiet,
    /// Text the chunk makes, from no byte of the document: the notes'
    /// calls.
    Made,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Segment {
    role: Role,
    /// Its bytes in the document, or the text made for it.
    global: Range<usize>,
    made: Option<String>,
}

/// A chunk to set: its segments, where its footnotes count from, and
/// whether it shows them (the notes) or hides them (every other chunk).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Chunk {
    segments: Vec<Segment>,
    footnotes_from: usize,
    notes: bool,
}

/// A chunk as set: its pieces and lines, down its own page, and where its
/// first and last own blocks begin.
struct Set {
    pieces: Vec<Tiled>,
    lines: Vec<Line>,
    /// Where its context block, and its last own block, begin: the top of
    /// each one's piece, and the first baseline of its text if it has text.
    context_top: Option<(f64, Option<f64>)>,
    last_top: (f64, Option<f64>),
    /// The bottom of its last own block, and of its page.
    content_bottom: f64,
    height: f64,
}

/// The document as a reflowed view draws it: the one sheet, the lines of
/// its glyphs, and its pieces, each with its picture as an SVG if the
/// window asking had none of its key.
pub struct Reflowed {
    pub sheet: Sheet,
    pub geometry: Geometry,
    pub pieces: Vec<Tiled>,
}

/// Chunks set, kept by what they were set from, and the refresh that last
/// used each: those no refresh has used lately are let go.
struct Kept {
    sets: HashMap<u64, (Arc<Set>, u64)>,
    refresh: u64,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

/// How a document is set: the template, the size, the width.
#[derive(Clone, Debug)]
pub struct How {
    pub format: Format,
    pub ty: Type,
    pub width: f32,
    pub root: std::path::PathBuf,
}

/// Whether a document set in `ty` is reflowed a chunk at a time: when the
/// template it reflows in is one of twig-export's own, set in this process.
/// Any other is set whole, in a typesetter of its own (`typeset.rs`).
pub fn chunks(ty: &Type) -> bool {
    let cfg = crate::pdf::config(ty, crate::pdf::Sheet::Reflow { width: 400.0 });
    cfg.document
        .template
        .as_deref()
        .is_some_and(|t| twig_export::templates::builtin(t).is_some())
}

/// `source`, whose top-level blocks are `blocks`, reflowed as `how` says,
/// with a stand-in at `hole` if there is one (`preview.rs`); pictures made
/// for the pieces whose keys are not `known`.
pub fn lay_out(
    source: &str,
    blocks: &[TopBlock],
    hole: Option<usize>,
    how: &How,
    known: &HashSet<u128>,
) -> Result<Reflowed, String> {
    let started = std::time::Instant::now();
    let (chunks, notes) = plan(source, blocks);
    let all: Vec<&Chunk> = chunks.iter().chain(notes.iter()).collect();

    // What each chunk is set from, as a key: its text, the stand-in in it,
    // and how it is set.
    let keyed: Vec<(u64, &Chunk, Option<usize>)> = all
        .iter()
        .map(|c| {
            let hole = hole.and_then(|h| local(c, h, source));
            (key(c, source, hole, how), *c, hole)
        })
        .collect();

    let mut kept = KEPT.lock().map_err(|_| "the reflow's chunks are lost")?;
    let kept = kept.get_or_insert_with(|| Kept {
        sets: HashMap::new(),
        refresh: 0,
    });
    kept.refresh += 1;
    let now = kept.refresh;

    // Set the chunks not kept, each on a core of its own.
    let missing: Vec<&(u64, &Chunk, Option<usize>)> = keyed
        .iter()
        .filter(|(k, ..)| !kept.sets.contains_key(k))
        .collect();
    leaf_dioxus::trace(|| {
        format!(
            "reflow: {} of {} chunks to set, width {} size {} dark {} hole {hole:?}",
            missing.len(),
            keyed.len(),
            how.width,
            how.ty.size,
            how.ty.dark
        )
    });
    let made: Vec<(u64, Result<Set, String>)> = if missing.len() <= 1 {
        missing
            .iter()
            .map(|(k, c, h)| (*k, set_chunk(c, source, *h, how, known)))
            .collect()
    } else {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let out = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..threads.min(missing.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((k, c, h)) = missing.get(i) else {
                            break;
                        };
                        let set = set_chunk(c, source, *h, how, known);
                        out.lock().expect("the chunks set").push((*k, set));
                    }
                });
            }
        });
        out.into_inner().expect("the chunks set")
    };
    for (k, set) in made {
        kept.sets.insert(k, (Arc::new(set?), now));
    }

    let set_by = started.elapsed();
    // Down the page: each chunk under the one before, by its context.
    let mut pieces = Vec::new();
    let mut lines = Vec::new();
    let mut y = 0.0;
    let mut prev: Option<(f64, Arc<Set>)> = None;
    let mut bottom = 0.0;
    let mut height = 0.0;
    for (k, ..) in keyed.iter().filter(|(_, c, _)| !c.notes) {
        let entry = kept.sets.get_mut(k).expect("every chunk set");
        entry.1 = now;
        let set = Arc::clone(&entry.0);
        if let (Some((py, p)), Some(ctx)) = (&prev, set.context_top) {
            // The same block in both: by its text's first baseline if it has
            // text, else by its piece.
            y = match (p.last_top.1, ctx.1) {
                (Some(above), Some(here)) => py + above - here,
                _ => py + p.last_top.0 - ctx.0,
            };
        }
        place(&set, y, known, &mut pieces, &mut lines);
        bottom = y + set.content_bottom;
        height = y + set.height;
        prev = Some((y, set));
    }
    // The notes, under the text, as the one page sets them.
    if let Some((k, ..)) = keyed.iter().find(|(_, c, _)| c.notes) {
        let entry = kept.sets.get_mut(k).expect("the notes set");
        entry.1 = now;
        let set = Arc::clone(&entry.0);
        let first = set
            .pieces
            .iter()
            .map(|p| p.top)
            .fold(f64::INFINITY, f64::min);
        if first.is_finite() {
            let gap = f64::from(how.ty.size) * 2.0;
            let y = bottom + gap - first;
            place(&set, y, known, &mut pieces, &mut lines);
            let margin = (height - bottom).max(0.0);
            height = y + set.content_bottom + margin;
        }
    }
    // Let go of what no refresh has used in a while.
    kept.sets.retain(|_, (_, used)| now - *used < 8);

    lines.sort_by(|a: &Line, b: &Line| a.baseline().total_cmp(&b.baseline()));
    leaf_dioxus::trace(|| {
        format!(
            "reflow: chunks set in {set_by:.1?}, all in {:.1?}",
            started.elapsed()
        )
    });
    Ok(Reflowed {
        sheet: Sheet {
            width: f64::from(how.width),
            height,
        },
        geometry: Geometry::from_lines(lines, vec![0.0]),
        pieces,
    })
}

/// A chunk's pieces and lines, `y` points down the page: pictures sent only
/// for keys the window has not got.
fn place(set: &Set, y: f64, known: &HashSet<u128>, pieces: &mut Vec<Tiled>, lines: &mut Vec<Line>) {
    for p in &set.pieces {
        pieces.push(Tiled {
            top: p.top + y,
            svg: if known.contains(&p.key) {
                None
            } else {
                p.svg.clone()
            },
            ..p.clone()
        });
    }
    lines.extend(set.lines.iter().map(|l| l.shifted(y)));
}

/// The chunks of the document's blocks, and the notes.
fn plan(source: &str, blocks: &[TopBlock]) -> (Vec<Chunk>, Option<Chunk>) {
    let text = |b: &TopBlock| source.get(b.span.clone()).unwrap_or_default();
    let notes: Vec<&TopBlock> = blocks
        .iter()
        .filter(|b| b.kind == TopKind::Footnote)
        .collect();
    let quiet: Vec<&TopBlock> = blocks
        .iter()
        .filter(|b| b.kind == TopKind::Reference)
        .collect();
    let label = |b: &TopBlock| {
        let t = text(b);
        t.strip_prefix("[^")
            .and_then(|r| r.split_once(']'))
            .map(|(l, _)| l.to_owned())
    };
    // Each note by its label, found once.
    let by_label: HashMap<String, &TopBlock> =
        notes.iter().filter_map(|n| Some((label(n)?, *n))).collect();
    let shown: Vec<&TopBlock> = blocks
        .iter()
        .filter(|b| matches!(b.kind, TopKind::Block | TopKind::Frontmatter))
        .collect();

    // Cut after a block once a chunk holds the least, where its text's hash
    // says so, or once it holds the most.
    let mut groups: Vec<Vec<&TopBlock>> = Vec::new();
    let mut current: Vec<&TopBlock> = Vec::new();
    let mut size = 0;
    for b in shown {
        current.push(b);
        size += b.span.len();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text(b).hash(&mut h);
        if (size >= LEAST && h.finish().is_multiple_of(4)) || size >= MOST {
            groups.push(std::mem::take(&mut current));
            size = 0;
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let calls = |t: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = t;
        while let Some(i) = rest.find("[^") {
            rest = &rest[i + 2..];
            if let Some(end) = rest.find(']') {
                let l = &rest[..end];
                if !l.is_empty() && !l.contains(char::is_whitespace) {
                    out.push(l.to_owned());
                }
                rest = &rest[end..];
            }
        }
        out
    };

    let mut chunks = Vec::new();
    let mut counted = 0;
    let mut called: Vec<String> = Vec::new();
    for (i, group) in groups.iter().enumerate() {
        let mut segments = Vec::new();
        // The context's footnotes are counted before the chunk's own, as
        // the page counts them, so that it sets as it does above.
        let mut from = counted;
        if i > 0 {
            let before = groups[i - 1].last().expect("a group has a block");
            segments.push(Segment {
                role: Role::Context,
                global: before.span.clone(),
                made: None,
            });
            if before.kind == TopKind::Block {
                from -= calls(text(before)).len().min(from);
            }
        }
        let mut mine = Vec::new();
        for b in group {
            let role = if b.kind == TopKind::Frontmatter {
                Role::Quiet
            } else {
                Role::Own
            };
            segments.push(Segment {
                role,
                global: b.span.clone(),
                made: None,
            });
            if b.kind == TopKind::Block {
                mine.extend(calls(text(b)));
            }
        }
        let mut noted: Vec<&TopBlock> = mine
            .iter()
            .filter_map(|l| by_label.get(l).copied())
            .collect();
        noted.sort_by_key(|n| n.span.start);
        noted.dedup_by_key(|n| n.span.start);
        for n in noted {
            segments.push(Segment {
                role: Role::Note,
                global: n.span.clone(),
                made: None,
            });
        }
        for q in &quiet {
            segments.push(Segment {
                role: Role::Quiet,
                global: q.span.clone(),
                made: None,
            });
        }
        chunks.push(Chunk {
            segments,
            footnotes_from: from,
            notes: false,
        });
        counted += mine.len();
        for l in mine {
            if !called.contains(&l) {
                called.push(l);
            }
        }
    }

    let notes = (!called.is_empty() && !notes.is_empty()).then(|| {
        let made: String = called.iter().map(|l| format!("[^{l}]")).collect();
        let mut segments = vec![Segment {
            role: Role::Made,
            global: 0..0,
            made: Some(made),
        }];
        for l in &called {
            if let Some(n) = by_label.get(l) {
                segments.push(Segment {
                    role: Role::Note,
                    global: n.span.clone(),
                    made: None,
                });
            }
        }
        Chunk {
            segments,
            footnotes_from: 0,
            notes: true,
        }
    });
    (chunks, notes)
}

/// A chunk's source, and where each segment of it begins.
fn source_of(chunk: &Chunk, source: &str) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut starts = Vec::with_capacity(chunk.segments.len());
    for s in &chunk.segments {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        starts.push(out.len());
        match &s.made {
            Some(t) => out.push_str(t),
            None => out.push_str(source.get(s.global.clone()).unwrap_or_default()),
        }
    }
    out.push('\n');
    (out, starts)
}

/// Where byte `at` of the document is in a chunk's source: in one of its
/// own blocks or notes, if it is.
fn local(chunk: &Chunk, at: usize, source: &str) -> Option<usize> {
    let (_, starts) = source_of(chunk, source);
    chunk.segments.iter().zip(starts).find_map(|(s, start)| {
        (matches!(s.role, Role::Own | Role::Note) && s.global.contains(&at)
            || s.global.end == at && s.role == Role::Own)
            .then(|| start + at - s.global.start)
    })
}

fn key(chunk: &Chunk, source: &str, hole: Option<usize>, how: &How) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source_of(chunk, source).0.hash(&mut h);
    for s in &chunk.segments {
        s.role.hash(&mut h);
    }
    (chunk.footnotes_from, chunk.notes, hole).hash(&mut h);
    (
        how.ty.template.key(),
        how.ty.size.to_bits(),
        how.ty.dark,
        how.width.to_bits(),
    )
        .hash(&mut h);
    format!("{:?}", how.format).hash(&mut h);
    h.finish()
}

/// Set one chunk: its own source, laid out by Typst, its pieces and glyphs
/// taken back to the document's bytes.
fn set_chunk(
    chunk: &Chunk,
    source: &str,
    hole: Option<usize>,
    how: &How,
    known: &HashSet<u128>,
) -> Result<Set, String> {
    let (mut text, starts) = source_of(chunk, source);
    if let Some(at) = hole {
        text.insert_str(at, crate::preview::STAND_IN);
    }
    // A byte of the source set, with the stand-in, as one of the chunk's
    // source without it: the stand-in's own bytes are where it stands.
    let unhole = |at: usize| match hole {
        Some(h) if at > h => at.saturating_sub(crate::preview::STAND_IN.len()).max(h),
        _ => at,
    };
    // A byte of the chunk's source as the document's, its segment's role,
    // and which segment.
    let to_global = |at: usize| -> Option<(usize, Role, usize)> {
        let at = unhole(at);
        let i = starts.iter().rposition(|&s| s <= at)?;
        let s = &chunk.segments[i];
        Some((s.global.start + at - starts[i], s.role, i))
    };
    let last_own = chunk.segments.iter().rposition(|s| s.role == Role::Own);

    let mut cfg = crate::pdf::config(&how.ty, crate::pdf::Sheet::Reflow { width: how.width });
    // Footnotes count on from the chunks above; their notes are set once,
    // in the notes, not under each chunk.
    let mut preamble = format!("#counter(footnote).update({})\n", chunk.footnotes_from);
    if chunk.notes {
        // The notes' chunk calls each note once, to set it; the calls are
        // laid out, and not drawn.
        preamble.push_str("#show footnote: hide\n");
    } else {
        preamble.push_str(
            "#set footnote.entry(separator: none, clearance: 0pt, gap: 0pt)\n\
             #show footnote.entry: none\n",
        );
    }
    cfg.document.raw_preamble = Some(preamble);
    let tree = leaf_core::parse_tree(&text, how.format).map_err(|e| e.to_string())?;
    let words = twig_export::render_words(tree, &text, &cfg).map_err(|e| format!("{e:#}"))?;
    let layout = crate::typeset::here_words(&words, &how.root)?;

    // Which role a tile's block has, by where the block begins.
    let role_of_block = |b: Option<usize>| -> Option<Role> {
        let start = *words.blocks.get(b?)?;
        Some(to_global(start)?.1)
    };
    let page = layout.pages().into_iter().next();
    // Where the context block's text, and the last own block's, begins: the
    // first baseline of each, which sets the same in either chunk, where a
    // piece's top is what was put beside it.
    let mut context_line: Option<f64> = None;
    let mut last_line: Option<f64> = None;
    for g in page.iter().flat_map(|p| &p.glyphs) {
        let Some((_, role, i)) = g.source.as_ref().and_then(|r| to_global(r.start)) else {
            continue;
        };
        let top = g.baseline;
        if role == Role::Context {
            context_line = Some(context_line.map_or(top, |c| c.min(top)));
        } else if Some(i) == last_own {
            last_line = Some(last_line.map_or(top, |c| c.min(top)));
        }
    }
    let tiles = layout.tiles();
    let shows = |r: Option<Role>| match r {
        Some(Role::Own) => !chunk.notes,
        Some(Role::Note) => chunk.notes,
        _ => false,
    };
    let mut context_top = None;
    let mut last_top: f64 = 0.0;
    let mut content_bottom: f64 = 0.0;
    let mut pieces = Vec::new();
    let mut made = HashSet::new();
    // A piece's role, by the glyphs it draws: a footnote's note is drawn in
    // the notes' area, a piece whose block is where the note is called, but
    // whose glyphs are the note's. By its block where it draws no text.
    let sourced: Vec<(f64, Role)> = page
        .iter()
        .flat_map(|p| &p.glyphs)
        .filter_map(|g| Some((g.baseline, to_global(g.source.as_ref()?.start)?.1)))
        .collect();
    let role_of = |t: &twig_export::tiles::Tile| -> Option<Role> {
        let (top, bottom) = (t.top, t.top + t.height);
        let mut drawn = sourced
            .iter()
            .filter(|(y, _)| (top..=bottom).contains(y))
            .map(|(_, r)| *r);
        let first = drawn.next();
        match first {
            Some(r) if drawn.all(|o| o == r) => Some(r),
            _ => role_of_block(t.block),
        }
    };
    for t in &tiles {
        let role = role_of(t);

        if role == Some(Role::Context) {
            context_top = Some(context_top.map_or(t.top, |c: f64| c.min(t.top)));
            continue;
        }
        if !shows(role) {
            continue;
        }
        if role == Some(Role::Own) {
            last_top = t.top;
        }
        content_bottom = content_bottom.max(t.top + t.height);
        pieces.push(Tiled {
            page: 0,
            top: t.top,
            height: t.height,
            width: t.width,
            key: t.key,
            svg: (!known.contains(&t.key) && made.insert(t.key)).then(|| layout.tile_svg(t)),
        });
    }
    let height = page.as_ref().map_or(0.0, |p| p.height);
    let glyphs: Vec<Glyph> = page
        .map(|p| p.glyphs)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mut g| {
            let r = g.source.take()?;
            let (start, role, _) = to_global(r.start)?;
            let shown = match role {
                Role::Own => !chunk.notes,
                Role::Note => chunk.notes,
                _ => false,
            };
            shown.then(|| {
                // The stand-in's glyph is where the caret is, and empty.
                let len = unhole(r.end).saturating_sub(unhole(r.start));
                g.source = Some(start..start + len);
                g
            })
        })
        .collect();
    let context_top = context_top.map(|t| (t, context_line));
    let last_top = (last_top, last_line);
    Ok(Set {
        pieces,
        lines: Geometry::lines_of(0, &glyphs, 0.0),
        context_top,
        last_top,
        content_bottom,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn blocks(src: &str) -> Vec<TopBlock> {
        let mut d = leaf_core::Doc::from_source(src.to_owned(), Format::Markdown).unwrap();
        d.top_level()
    }

    fn how() -> How {
        How {
            format: Format::Markdown,
            ty: Type {
                template: crate::settings::Template::default(),
                size: 12.0,
                dark: false,
            },
            width: 400.0,
            root: Path::new(".").into(),
        }
    }

    fn document(paragraphs: usize) -> String {
        let mut s = String::from("# Title\n\n");
        for i in 0..paragraphs {
            s.push_str(&format!(
                "Paragraph {i}, with a note[^n{i}] and enough words in it to wrap across a line or two at this width.\n\n"
            ));
        }
        for i in 0..paragraphs {
            s.push_str(&format!("[^n{i}]: The note to paragraph {i}.\n\n"));
        }
        s
    }

    /// The chunks' glyphs are the whole document's, every block's once, in
    /// order down the page, and the notes come last.
    #[test]
    fn the_chunks_set_the_whole_document_in_order() {
        let src = document(60);
        let b = blocks(&src);
        let (chunks, notes) = plan(&src, &b);
        assert!(chunks.len() > 2, "{} chunks", chunks.len());
        assert!(notes.is_some());
        let out = lay_out(&src, &b, None, &how(), &HashSet::new()).unwrap();
        // Every paragraph's first letter is on a line, top to bottom in
        // source order.
        let mut last = -1.0;
        for i in 0..60 {
            let at = src.find(&format!("Paragraph {i},")).unwrap();
            let caret = out.geometry.caret_box(at, &src).expect("a caret");
            assert!(caret.top > last, "paragraph {i} is below the one before");
            last = caret.top;
        }
        // The last note is below the last paragraph.
        let note = src.find("The note to paragraph 59").unwrap();
        assert!(out.geometry.caret_box(note, &src).unwrap().top > last);
        // And drawn there: the notes are pieces of their own, under the text.
        assert!(
            out.pieces.iter().any(|p| p.top > last),
            "the notes are drawn"
        );
        assert!(out.sheet.height > last);
    }

    /// Chunked, the space between two blocks is what the one page puts
    /// there: a chunk's first block lands where it lands set whole.
    #[test]
    fn a_chunk_is_placed_as_the_whole_page_places_it() {
        let src = document(40);
        let b = blocks(&src);
        let chunked = lay_out(&src, &b, None, &how(), &HashSet::new()).unwrap();
        let whole = crate::pdf::layout(
            &src,
            Format::Markdown,
            None,
            &how().ty,
            crate::pdf::Sheet::Reflow { width: 400.0 },
            &HashSet::new(),
        )
        .unwrap();
        let whole = Geometry::new(&[(whole.pages[0].height, whole.pages[0].glyphs.clone())]);
        for i in [0, 13, 27, 39] {
            let at = src.find(&format!("Paragraph {i},")).unwrap();
            let a = chunked.geometry.caret_box(at, &src).unwrap().top;
            let b = whole.caret_box(at, &src).unwrap().top;
            assert!((a - b).abs() < 0.5, "paragraph {i}: {a} chunked, {b} whole");
        }
    }

    /// An edit in one chunk sets that chunk again, and keeps the others.
    #[test]
    fn an_edit_sets_only_its_chunk() {
        let src = document(60);
        let b = blocks(&src);
        let first = lay_out(&src, &b, None, &how(), &HashSet::new()).unwrap();
        let known: HashSet<u128> = first.pieces.iter().map(|p| p.key).collect();
        let at = src.find("Paragraph 30,").unwrap();
        let mut edited = src.clone();
        edited.insert_str(at, "Edited. ");
        let b = blocks(&edited);
        let second = lay_out(&edited, &b, None, &how(), &known).unwrap();
        let drawn = second.pieces.iter().filter(|p| p.svg.is_some()).count();
        assert_eq!(drawn, 1, "one piece drawn anew");
    }
}
