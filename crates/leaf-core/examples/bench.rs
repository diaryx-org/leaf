//! Where does leaf's per-keystroke and per-paint time actually go?
//!
//! `cargo run --release -p leaf-core --example bench`
use leaf_core::{Doc, View, wysiwyg};
use std::hash::{Hash, Hasher};
use std::time::Instant;
use twig::{Editor, Format};

fn body(bytes: usize) -> String {
    let mut s = String::new();
    let mut i = 0;
    while s.len() < bytes {
        s.push_str(&format!(
            "## Section {i}\n\nThe quick brown fox jumps over the lazy dog, and \
             **bold** text with a [link](https://example.dev) and `code` besides. \
             Another sentence follows to make the paragraph a realistic length.\n\n"
        ));
        i += 1;
    }
    s
}

fn time<T>(label: &str, n: usize, mut f: impl FnMut() -> T) -> f64 {
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(f());
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("  {label:<30}{ms:8.2} ms");
    ms
}

fn main() {
    for kb in [10usize, 100, 1000] {
        let src = body(kb * 1024);
        println!("=== {} KB ===", src.len() / 1024);

        let mut ed = Editor::new_str(&src, Format::Markdown).unwrap();
        let nodes = ed.nodes().unwrap();
        let map = wysiwyg::build(&nodes, &src, None);
        println!("  ({} AST nodes, {} map rows)", nodes.len(), map.rows.len());

        println!("  -- per edit (unavoidable today) --");
        time("twig edit_range (reparse)", 5, || {
            ed.edit_range(src.len() / 2, src.len() / 2, "x").is_ok()
        });
        time("twig nodes() FFI marshal", 5, || ed.nodes().unwrap().len());
        time("wysiwyg::build", 5, || wysiwyg::build(&nodes, &src, None).rows.len());
        {
            // The incremental path with a warm cache and nothing changed: the
            // floor cost the block cache adds even on a pure repaint — hash every
            // block, clone every reused row, recollect stops. No subtree is
            // marshalled (every block hits). The real keystroke win shows up in
            // "Doc::insert + rebuild" below, which re-marshals only the edited
            // block and reuses the rest.
            let mut cache = wysiwyg::BlockCache::default();
            let top = ed.child_spans(None).unwrap();
            let _ = wysiwyg::build_cached(&top, &src, None, &mut cache, |id| {
                ed.subtree(twig::NodeId(id)).unwrap_or_default()
            });
            time("wysiwyg::build_cached (all reused)", 5, || {
                let top = ed.child_spans(None).unwrap();
                wysiwyg::build_cached(&top, &src, None, &mut cache, |id| {
                    ed.subtree(twig::NodeId(id)).unwrap_or_default()
                })
                .rows
                .len()
            });
        }

        println!("  -- claimed hot, actually noise --");
        time("twig source_str() (full copy)", 5, || ed.source_str().unwrap().len());
        let clean = src.clone();
        time("dirty compare (full cmp)", 5, || src == clean);

        println!("  -- what the GUI adds on a cache miss --");
        time("clone every row's glyphs", 5, || {
            map.rows.iter().map(|r| r.glyphs.clone()).collect::<Vec<_>>().len()
        });
        time("hash every glyph (cache key?)", 5, || {
            let mut n = 0u64;
            for r in &map.rows {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                for g in &r.glyphs {
                    g.ch.hash(&mut h);
                }
                n ^= h.finish();
            }
            n
        });

        println!("  -- the whole path, as a frontend calls it --");
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_bench_{kb}.md"));
        std::fs::write(&p, &src).unwrap();
        let mut d = Doc::open(p).unwrap();
        d.view = View::Wysiwyg;
        d.place_caret(src.len() / 2, false);
        d.build_visual_unwrapped();
        time("build_visual (cached: a repaint)", 200, || d.build_visual_unwrapped());
        time("Doc::insert + rebuild (a keystroke)", 5, || {
            d.insert("x");
            d.build_visual_unwrapped();
        });
        println!();
    }
}
