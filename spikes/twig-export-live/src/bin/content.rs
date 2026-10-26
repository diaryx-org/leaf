//! The words as Typst content built in Rust from the twig tree, with no Typst
//! markup for them at all; a Typst template for the contents page, the running
//! head, the page numbers and the footnotes. How fast, and does a glyph lead
//! straight back to the Markdown byte it came from?
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::{Duration as StdDuration, Instant};

use twig_export::doc::{Doc, Idx};
use typst::comemo::{Constraint, Track, Tracked};
use typst::diag::{FileError, FileResult};
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{
    Bytes, Content, Context, Datetime, Duration, Func, NativeElement, Output, Smart, StyleChain,
    Target, TargetElem, Value,
};
use typst::introspection::{EmptyIntrospector, Introspector, MAX_ITERS};
use typst::layout::{Frame, FrameItem};
use typst::model::{
    EmphElem, FootnoteElem, HeadingElem, ListElem, ListItem, ParbreakElem, StrongElem,
};
use typst::syntax::{FileId, RootedPath, Source, Span, SpanKind, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook, SpaceElem, TextElem};
use typst::utils::{LazyHash, Protected};
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;

struct W {
    library: LazyHash<Library>,
    fonts: FontStore,
    main: Source,
}
impl World for W {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }
    fn main(&self) -> FileId {
        self.main.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotFound(id.vpath().get_without_slash().into()))
        }
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(id.vpath().get_without_slash().into()))
    }
    fn font(&self, i: usize) -> Option<Font> {
        self.fonts.font(i)
    }
    fn today(&self, _: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// A span that names a place in the Markdown file `md` as *a block and an
/// offset into it*, not as an absolute byte: typing in one paragraph moves the
/// byte offset of every later one, and a span is part of the content's hash,
/// so absolute offsets made every block after the caret new to Typst's cache.
/// `block` is the top-level block's index (stable while typing inside it); the
/// caller keeps where each block starts now.
///
/// Typst's raw-range span carries two 23-bit numbers, which is these two.
/// `Span::from_range` is crate-private in typst-syntax 0.15, so this packs it
/// as that function does (span.rs: 16 bits of file id, then
/// `RANGE_BASE + a << 23 | b`). A real build asks Typst for the constructor.
fn span(md: FileId, (block, offset): (usize, usize)) -> Span {
    const RANGE_BASE: u64 = (1 << 47) + (1 << 46);
    const MAX: u64 = (1 << 23) - 1;
    let (s, e) = ((block as u64).min(MAX), (offset as u64).min(MAX));
    let bits = ((md.into_raw().get() as u64) << 48) | (RANGE_BASE + ((s << 23) | e));
    Span::from_raw(NonZeroU64::new(bits).unwrap())
}

struct Build<'a> {
    doc: &'a Doc,
    md: FileId,
    /// The top-level block being built, and where it starts in the source.
    block: std::cell::Cell<(usize, usize)>,
}

impl Build<'_> {
    fn span(&self, (start, _): (usize, usize)) -> Span {
        let (block, from) = self.block.get();
        span(self.md, (block, start.saturating_sub(from)))
    }

    fn blocks(&self, kids: &[Idx]) -> Content {
        Content::sequence(kids.iter().map(|&k| self.block(k)))
    }

    fn block(&self, i: Idx) -> Content {
        let n = self.doc.node(i);
        let sp = self.span(n.span);
        match n.kind.as_str() {
            "metadata" | "footnote" => Content::empty(),
            "heading" => {
                let level = n.int_field("level").unwrap_or(1).clamp(1, 6) as usize;
                HeadingElem::new(self.inlines(&n.children))
                    .with_level(Smart::Custom(NonZeroUsize::new(level).unwrap()))
                    .pack()
                    .spanned(sp)
            }
            "para" => {
                Content::sequence([self.inlines(&n.children), ParbreakElem::shared().clone()])
            }
            "bullet_list" => {
                let items = n
                    .children
                    .iter()
                    .map(|&c| {
                        ListItem::new(self.blocks(&self.doc.node(c).children))
                            .pack()
                            .spanned(sp)
                    })
                    .map(|c| c.into_packed::<ListItem>().unwrap())
                    .collect();
                Content::sequence([
                    ListElem::new(items).pack().spanned(sp),
                    ParbreakElem::shared().clone(),
                ])
            }
            _ => self.blocks(&n.children),
        }
    }

    fn inlines(&self, kids: &[Idx]) -> Content {
        Content::sequence(kids.iter().map(|&k| self.inline(k)))
    }

    fn inline(&self, i: Idx) -> Content {
        let n = self.doc.node(i);
        let sp = self.span(n.span);
        match n.kind.as_str() {
            "str" | "text_leaf" => {
                TextElem::packed(n.str_field("text").unwrap_or_default()).spanned(sp)
            }
            "soft_break" => SpaceElem::shared().clone(),
            "emph" => EmphElem::new(self.inlines(&n.children)).pack().spanned(sp),
            "strong" => StrongElem::new(self.inlines(&n.children))
                .pack()
                .spanned(sp),
            "footnote_reference" => {
                let label = n.str_field("text").unwrap_or_default();
                let def = self.doc.nodes.iter().position(|d| {
                    d.kind.as_str() == "footnote"
                        && d.str_field("label").or(d.str_field("text")) == Some(label)
                });
                match def {
                    Some(d) => {
                        let body = self.blocks(&self.doc.node(d).children);
                        FootnoteElem::with_content(body).pack().spanned(sp)
                    }
                    None => Content::empty(),
                }
            }
            _ => self.inlines(&n.children),
        }
    }
}

const WORDS: &[&str] = &[
    "the", "garden", "kept", "its", "own", "hours", "and", "we", "learned", "them", "slowly", "by",
    "walking", "there", "each", "morning", "before", "work", "while", "light", "was", "still",
    "low", "over", "wall", "a", "notebook", "recorded", "what", "bloomed",
];

fn prose(seed: usize, n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        let w = WORDS[(seed * 7 + i * 13 + i / 3) % WORDS.len()];
        if i == 0 {
            s.push_str(&w[..1].to_uppercase());
            s.push_str(&w[1..]);
        } else {
            s.push(' ');
            s.push_str(w);
        }
        if i % 17 == 16 {
            s.push(',');
        }
    }
    s.push('.');
    s
}

fn document(sections: usize) -> String {
    let mut md = String::from("---\ntitle: Field notes\n---\n\n");
    for k in 0..sections {
        md.push_str(&format!("# Section {k}\n\n"));
        for p in 0..6 {
            md.push_str(&prose(k * 10 + p, 90));
            if p == 2 {
                md.push_str(&format!(" A note[^n{k}]."));
            }
            md.push_str("\n\n");
        }
        md.push_str("- first item, *emphasised*\n- second item, **strong**\n\n");
        md.push_str(&format!("[^n{k}]: The note for section {k}.\n\n"));
    }
    md
}

/// The body, and where each top-level block starts in `md`.
fn body(md: &str, md_id: FileId) -> (Content, Vec<usize>) {
    let mut td = twig_export::parse(
        md,
        twig_markdown::format(),
        twig_export::Extensions::default(),
    )
    .unwrap();
    let doc = Doc::ingest(&mut td, md).unwrap();
    let b = Build {
        doc: &doc,
        md: md_id,
        block: Default::default(),
    };
    let mut starts = Vec::new();
    let content = Content::sequence(doc.node(doc.root()).children.iter().enumerate().map(
        |(i, &k)| {
            let start = doc.node(k).span.0;
            starts.push(start);
            b.block.set((i, start));
            b.block(k)
        },
    ));
    (content, starts)
}

/// `typst::compile`, less its first step: the template is evaluated as Typst,
/// and the body it is called with is content built in Rust, handed over as a
/// value. The body is not in the `World` and not in the `Library`, whose hash
/// every memoized layout depends on, so an edit costs only what it changes.
/// (A first version defined `body` in the library's global scope; every
/// keystroke then cost a cold compile.) The loop is `compile_impl`'s: lay out
/// again until the introspection (the contents page, the counters) settles.
fn compile(world: &W, body: &Content) -> PagedDocument {
    let world_t: Tracked<dyn World + '_> = (world as &dyn World).track();
    let traced = Traced::default();
    let mut sink = Sink::new();
    let library = world.library();
    let base = StyleChain::new(&library.styles);
    let target = TargetElem::target.set(Target::Paged).wrap();
    let styles = base.chain(&target);

    let main = world.source(world.main()).unwrap();
    let module = typst_eval::eval(
        world_t,
        library,
        traced.track(),
        sink.track_mut(),
        Route::default().track(),
        &main,
    )
    .unwrap_or_else(|e| {
        panic!(
            "{:?}",
            e.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        )
    });
    let book: Func = module
        .scope()
        .get("book")
        .unwrap()
        .read()
        .clone()
        .cast()
        .unwrap();

    let empty = EmptyIntrospector;
    let mut history: Vec<PagedDocument> = Vec::new();
    loop {
        let introspector: &dyn Introspector = match history.last() {
            Some(d) => Output::introspector(d),
            None => &empty,
        };
        let constraint = Constraint::new();
        let mut subsink = Sink::new();
        let mut engine = Engine {
            library,
            world: world_t,
            introspector: Protected::new(introspector.track_with(&constraint)),
            traced: traced.track(),
            sink: subsink.track_mut(),
            route: Route::default(),
        };
        let content = book
            .call(
                &mut engine,
                Context::none().track(),
                [Value::Content(body.clone())],
            )
            .unwrap()
            .display();
        let doc = PagedDocument::create(&mut engine, &content, styles).unwrap_or_else(|e| {
            panic!(
                "{:?}",
                e.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
            )
        });
        if constraint.validate(Output::introspector(&doc)) || history.len() + 1 >= MAX_ITERS {
            if std::env::var_os("PASSES").is_some() {
                eprintln!("layout passes: {}", history.len() + 1);
            }
            return doc;
        }
        history.push(doc);
    }
}

/// Every glyph's source: (into the Markdown, exact byte matches, into the
/// template, made by Typst).
#[derive(Default, Debug)]
struct Trace {
    markdown: usize,
    exact: usize,
    template: usize,
    detached: usize,
}

fn trace(frame: &Frame, md: &str, starts: &[usize], md_id: FileId, t: &mut Trace) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(g) => trace(&g.frame, md, starts, md_id, t),
            FrameItem::Text(text) => {
                for g in &text.glyphs {
                    let (sp, off) = g.span;
                    match sp.get() {
                        SpanKind::Range { id, range } if id == md_id => {
                            t.markdown += 1;
                            // The glyph's own characters, and the Markdown at
                            // the offset its span names.
                            let ch = &text.text[g.range()];
                            let at = starts[range.start] + range.end + off as usize;
                            if md.get(at..at + ch.len()) == Some(ch) {
                                t.exact += 1;
                            }
                        }
                        SpanKind::Detached => t.detached += 1,
                        _ => t.template += 1,
                    }
                }
            }
            _ => {}
        }
    }
}

fn median(mut v: Vec<StdDuration>) -> StdDuration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    let sections: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let template = include_str!("../../template.typ");
    let main_id =
        RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap()).intern();
    let md_id = RootedPath::new(VirtualRoot::Project, VirtualPath::new("doc.md").unwrap()).intern();

    let mut md = document(sections);
    let words = md.split_whitespace().count();

    let t = Instant::now();
    let (b, starts) = body(&md, md_id);
    let first_build = t.elapsed();

    let mut fonts = FontStore::new();
    fonts.extend(typst_kit::fonts::embedded());
    let w = W {
        library: LazyHash::new(Library::builder().build()),
        fonts,
        main: Source::new(main_id, template.into()),
    };

    let t = Instant::now();
    let doc = compile(&w, &b);
    let cold = t.elapsed();
    let pages = doc.pages().len();

    if let Some(out) = std::env::var_os("PDF") {
        std::fs::write(
            out,
            typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).unwrap(),
        )
        .unwrap();
    }

    let mut tr = Trace::default();
    for p in doc.pages() {
        trace(&p.frame, &md, &starts, md_id, &mut tr);
    }

    for (label, frac) in [("middle", 0.5), ("end", 0.97)] {
        let mut at = ((md.len() as f64) * frac) as usize;
        while !md.is_char_boundary(at) || md.as_bytes()[at] != b' ' {
            at += 1;
        }
        let (mut e, mut c) = (Vec::new(), Vec::new());
        for ch in "quietly".chars() {
            md.insert(at, ch);
            at += 1;
            let t = Instant::now();
            let (b, _) = body(&md, md_id);
            e.push(t.elapsed());
            let t = Instant::now();
            let _ = compile(&w, &b);
            c.push(t.elapsed());
        }
        println!(
            "keystroke at {label}: twig + content {:?}, typst {:?} (median of {})",
            median(e),
            median(c),
            "quietly".len()
        );
    }

    println!("{sections} sections, {words} words, {pages} pages (with a contents page)");
    println!("first build (twig parse + content) {first_build:?}; cold typst compile {cold:?}");
    println!(
        "glyphs: {} into the Markdown ({} at the exact byte), {} from the template, {} made by Typst",
        tr.markdown, tr.exact, tr.template, tr.detached
    );
}
