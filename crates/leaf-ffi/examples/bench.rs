//! How long the binding's calls take on a long document — the ones a native
//! frontend makes per interaction, so a regression in any of them is a
//! keystroke, a click, or a scroll that lags.
//!
//! ```text
//! cargo run -p leaf-ffi --example bench              # dev profile, generated document
//! cargo run -p leaf-ffi --example bench --release    # what a shipping app sees
//! cargo run -p leaf-ffi --example bench -- path.md   # a document of your own
//! cargo run -p leaf-ffi --example bench -- --dump path.md   # write the generated one out
//! ```
//!
//! Both profiles matter: a debug build is what a developer drives the app with
//! and is ten times slower on exactly the scans this measures, so "fine in
//! release" is not the whole answer. The generated document is deterministic —
//! about 14,000 words of prose with the inline markup a design document has,
//! plus a list, a table, and a fence — so two runs compare.
//!
//! This is a table, not a benchmark harness: no statistics, no dependency. The
//! numbers are medians of a few runs each, which is enough to tell a linear
//! scan from a lookup, and that is the question it exists to answer.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use leaf_ffi::LeafDoc;

fn main() {
    let mut args = std::env::args().skip(1);
    let source = match args.next().as_deref() {
        // The generated document, written out — to open in the app, or to
        // concatenate with itself and see which rows grow with it.
        Some("--dump") => {
            let path = args.next().expect("--dump takes a path");
            std::fs::write(&path, generated()).unwrap_or_else(|e| panic!("{path}: {e}"));
            return;
        }
        Some(path) => std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}")),
        None => generated(),
    };
    let words = source.split_whitespace().count();
    let profile = if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    };
    println!(
        "document: {} bytes, {words} words · profile: {profile}\n",
        source.len()
    );

    let open = Instant::now();
    let doc = LeafDoc::new(source.clone(), "markdown".into()).expect("parse");
    let opened = open.elapsed();
    let first = Instant::now();
    let view = doc.set_unwrapped();
    let first_view = first.elapsed();
    let runs: usize = view.rows.iter().map(|r| r.runs.len()).sum();
    println!("rows: {} · runs: {runs}", view.rows.len());
    println!("{:<44} {:>10}", "open (twig parse)", fmt(opened));
    println!(
        "{:<44} {:>10}",
        "first view (build the map + the frame)",
        fmt(first_view)
    );

    let len = doc.doc_end_offset();
    let mid = len / 2;
    let mid_utf16 = doc.utf16_index_for_offset(mid);
    let mid_row = doc.pos_for_offset(mid).row;

    // Reads a frontend makes per frame or per gesture.
    row("view() — the whole frame, unchanged", || {
        std::hint::black_box(doc.view());
    });
    row("pos_for_offset (mid-document)", || {
        std::hint::black_box(doc.pos_for_offset(mid));
    });
    row("utf16_index_for_offset (mid-document)", || {
        doc.utf16_index_for_offset(mid);
    });
    row("offset_for_utf16_index (mid-document)", || {
        doc.offset_for_utf16_index(mid_utf16);
    });
    row("text_in_range (whole document)", || {
        std::hint::black_box(doc.text_in_range(0, len));
    });
    row("counts", || {
        std::hint::black_box(doc.counts());
    });

    // Gestures: each answers with a whole frame.
    row("click_ch (place the caret)", || {
        std::hint::black_box(doc.click_ch(mid_row, 3, false));
    });
    row("click_ch extend (grow a selection)", || {
        doc.click_ch(mid_row, 0, false);
        std::hint::black_box(doc.click_ch(mid_row + 4, 0, true));
    });
    doc.click_ch(mid_row, 3, false);
    row("selection_counts (rows selected)", || {
        doc.click_ch(mid_row, 0, false);
        doc.click_ch(mid_row + 4, 0, true);
        std::hint::black_box(doc.selection_counts());
    });
    doc.click_ch(mid_row, 3, false);
    row("insert one character", || {
        std::hint::black_box(doc.insert("x".into()));
    });
    // The UTF-16 table is derived on the first lookup after a map rebuild,
    // so the first conversion after a keystroke pays for it; every later one
    // is the lookup alone.
    row("utf16_index_for_offset (first after an edit)", || {
        doc.insert("x".into());
        doc.utf16_index_for_offset(mid);
    });
    row("newline", || {
        std::hint::black_box(doc.newline());
    });
}

/// Time `f` a handful of times and print the median.
fn row(label: &str, mut f: impl FnMut()) {
    let mut samples: Vec<Duration> = (0..7)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    samples.sort();
    println!("{label:<44} {:>10}", fmt(samples[samples.len() / 2]));
}

fn fmt(d: Duration) -> String {
    let us = d.as_micros();
    if us >= 10_000 {
        format!("{:.1} ms", us as f64 / 1000.0)
    } else {
        format!("{us} µs")
    }
}

/// A long design document: prose in paragraphs of varied length with inline
/// code, emphasis and links, under headings, with a list, a table and a fence
/// every so often. Deterministic — no randomness — so runs compare.
fn generated() -> String {
    let words: Vec<&str> = "the document carries its own identity and a reference resolves \
        against whatever archive holds it so that moving a file between checkouts changes \
        nothing a reader can see each node names its root which is what lets a foreign \
        parent stand in for a path nobody has written down yet"
        .split_whitespace()
        .collect();
    let mut out = String::from("# A generated design document\n\n");
    let mut w = 0usize;
    let mut para = 0usize;
    while w < 14_000 {
        if para.is_multiple_of(9) {
            let _ = writeln!(out, "## Section {}\n", para / 9 + 1);
        }
        match para % 11 {
            7 => {
                for i in 0..5 {
                    let _ = writeln!(out, "- item {i} with `code_{i}` and *emphasis* in it");
                    w += 8;
                }
                out.push('\n');
            }
            9 => {
                out.push_str("| key | value | note |\n|---|---|---|\n");
                for i in 0..4 {
                    let _ = writeln!(out, "| `k{i}` | value {i} | a short note |");
                    w += 6;
                }
                out.push('\n');
            }
            10 => {
                out.push_str("```rust\nfn example() -> u32 {\n    42\n}\n```\n\n");
                w += 6;
            }
            _ => {
                let n = 40 + (para * 17) % 90;
                for i in 0..n {
                    let word = words[(para * 7 + i * 3) % words.len()];
                    match (para + i) % 23 {
                        0 => {
                            let _ = write!(out, "`{word}_{i}` ");
                        }
                        5 => {
                            let _ = write!(out, "*{word}* ");
                        }
                        11 => {
                            let _ = write!(out, "[{word}](https://example.org/{para}/{i}) ");
                        }
                        17 => {
                            let _ = write!(out, "**{word}** ");
                        }
                        _ => {
                            out.push_str(word);
                            out.push(' ');
                        }
                    }
                }
                w += n;
                out.push_str("\n\n");
            }
        }
        para += 1;
    }
    out
}
