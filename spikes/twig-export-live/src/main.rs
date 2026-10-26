//! How fast can twig-export + Typst re-set a long document after one keystroke,
//! and can a laid-out glyph be traced back to the .typ it came from?
use std::time::{Duration as StdDuration, Instant};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Frame, FrameItem};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
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

fn emit(md: &str, cfg: &twig_export::config::Config) -> String {
    twig_export::render(
        md,
        twig_markdown::format(),
        cfg,
        twig_export::Extensions::default(),
    )
    .expect("render")
    .typst
}

fn compile(w: &W) -> PagedDocument {
    let r = typst::compile::<PagedDocument>(w);
    match r.output {
        Ok(d) => d,
        Err(e) => panic!(
            "{:?}",
            e.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        ),
    }
}

fn glyphs(frame: &Frame, total: &mut usize, traced: &mut usize, w: &W) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(g) => glyphs(&g.frame, total, traced, w),
            FrameItem::Text(t) => {
                for g in &t.glyphs {
                    *total += 1;
                    if w.range(g.span.0).is_some() {
                        *traced += 1;
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
    let cfg_text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/report.toml")).unwrap();
    let (cfg, _) = twig_export::config::Config::parse(&cfg_text, "toml").unwrap();

    let mut md = document(sections);
    let words = md.split_whitespace().count();

    let t = Instant::now();
    let typ = emit(&md, &cfg);
    let first_emit = t.elapsed();

    let mut fonts = FontStore::new();
    fonts.extend(typst_kit::fonts::embedded());
    let id = RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap()).intern();
    let mut w = W {
        library: LazyHash::new(Library::builder().build()),
        fonts,
        main: Source::new(id, typ),
    };

    let t = Instant::now();
    let doc = compile(&w);
    let cold = t.elapsed();
    let pages = doc.pages().len();

    let (mut total, mut traced) = (0, 0);
    for p in doc.pages() {
        glyphs(&p.frame, &mut total, &mut traced, &w);
    }

    // Keystrokes: type a word one letter at a time into a paragraph at the
    // middle of the document, then at the end.
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
            let typ = emit(&md, &cfg);
            e.push(t.elapsed());
            let t = Instant::now();
            w.main.replace(&typ);
            let _ = compile(&w);
            c.push(t.elapsed());
        }
        println!(
            "keystroke at {label}: emit {:?}, typst {:?} (median of {})",
            median(e),
            median(c),
            "quietly".len()
        );
    }

    let t = Instant::now();
    let pdf = typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).unwrap();
    let pdf_t = t.elapsed();

    println!("{sections} sections, {words} words, {pages} pages");
    println!(
        "first emit (twig parse + typst source) {first_emit:?}; cold typst compile {cold:?}; pdf write {pdf_t:?} ({} KiB)",
        pdf.len() / 1024
    );
    println!("glyphs traced to generated .typ: {traced}/{total}");
}
