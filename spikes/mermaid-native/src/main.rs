//! Render every diagram in a corpus with mmdr, rasterize each SVG through the
//! resvg leaf already carries, and write a page that sets them beside
//! mermaid.js's own rendering of the same source.
//!
//! Usage: mermaid-native-spike <corpus dir> <out dir>

use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use mermaid_rs_renderer::{RenderOptions, render_with_timing};

struct Row {
    name: String,
    source: String,
    /// The first render, which loads fonts; then the median of ten more.
    cold_ms: f64,
    warm_ms: f64,
    svg_bytes: usize,
    /// What went wrong, if anything: the parse, or the raster.
    error: Option<String>,
    raster: Option<String>,
    foreign_object: bool,
    text_nodes: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (corpus, out) = (Path::new(&args[1]), Path::new(&args[2]));
    std::fs::create_dir_all(out).unwrap();

    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    let fonts = std::sync::Arc::new(fonts);

    let mut names: Vec<_> = std::fs::read_dir(corpus)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
        .collect();
    names.sort();

    let mut rows = Vec::new();
    for path in names {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path).unwrap();
        let t = Instant::now();
        let first = render_with_timing(&source, RenderOptions::mermaid_default());
        let cold_ms = t.elapsed().as_secs_f64() * 1000.0;
        let mut row = Row {
            name: name.clone(),
            source: source.clone(),
            cold_ms,
            warm_ms: 0.0,
            svg_bytes: 0,
            error: None,
            raster: None,
            foreign_object: false,
            text_nodes: 0,
        };
        match first {
            Err(e) => row.error = Some(format!("{e:#}")),
            Ok(r) => {
                let mut times: Vec<f64> = (0..10)
                    .map(|_| {
                        let t = Instant::now();
                        let _ = render_with_timing(&source, RenderOptions::mermaid_default());
                        t.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                times.sort_by(f64::total_cmp);
                row.warm_ms = times[5];
                row.svg_bytes = r.svg.len();
                row.foreign_object = r.svg.contains("<foreignObject");
                row.text_nodes = r.svg.matches("<text").count();
                std::fs::write(out.join(format!("{name}.svg")), &r.svg).unwrap();
                row.raster = raster(&r.svg, &fonts, &out.join(format!("{name}.png"))).err();
            }
        }
        rows.push(row);
    }

    let mut tsv = String::from("name\tcold_ms\twarm_ms\tsvg_bytes\ttext\tforeignObject\terror\traster\n");
    for r in &rows {
        writeln!(
            tsv,
            "{}\t{:.2}\t{:.3}\t{}\t{}\t{}\t{}\t{}",
            r.name,
            r.cold_ms,
            r.warm_ms,
            r.svg_bytes,
            r.text_nodes,
            r.foreign_object,
            r.error.as_deref().unwrap_or("").replace(['\n', '\t'], " "),
            r.raster.as_deref().unwrap_or("")
        )
        .unwrap();
    }
    std::fs::write(out.join("report.tsv"), &tsv).unwrap();
    std::fs::write(out.join("index.html"), page(&rows)).unwrap();
    print!("{tsv}");
}

/// The SVG as resvg 0.46 draws it, at 2x, into `to`.
fn raster(svg: &str, fonts: &std::sync::Arc<usvg::fontdb::Database>, to: &Path) -> Result<(), String> {
    let opt = usvg::Options {
        fontdb: fonts.clone(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size().scale_by(2.0).ok_or("empty")?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("too big")?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(2.0, 2.0), &mut pixmap.as_mut());
    pixmap.save_png(to).map_err(|e| e.to_string())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Three columns per diagram: mermaid.js live, mmdr's SVG in an `<img>`,
/// and that SVG through resvg.
fn page(rows: &[Row]) -> String {
    let mut body = String::new();
    for r in rows {
        let ours = match &r.error {
            Some(e) => format!("<pre class=fault>{}</pre>", escape(e)),
            None => format!(
                "<img src=\"{0}.svg\"><div class=meta>{1:.2} ms warm · {2:.0} ms cold · {3} B</div>",
                r.name, r.warm_ms, r.cold_ms, r.svg_bytes
            ),
        };
        let raster = match (&r.error, &r.raster) {
            (Some(_), _) => String::new(),
            (None, Some(e)) => format!("<pre class=fault>{}</pre>", escape(e)),
            (None, None) => format!("<img src=\"{}.png\" class=half>", r.name),
        };
        write!(
            body,
            "<section id=\"{0}\"><h2>{0}</h2><div class=row>\
             <div><h3>mermaid.js</h3><pre class=\"mermaid\">{1}</pre></div>\
             <div><h3>mmdr (SVG)</h3>{2}</div>\
             <div><h3>mmdr → resvg</h3>{3}</div></div></section>",
            r.name,
            escape(&r.source),
            ours,
            raster
        )
        .unwrap();
    }
    format!(
        r#"<!doctype html><meta charset=utf-8><title>mmdr vs mermaid.js</title>
<style>
body {{ font: 13px system-ui; margin: 16px; background: #fff; color: #222; }}
.row {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }}
.row > div {{ border: 1px solid #ddd; padding: 8px; overflow: auto; }}
img, svg {{ max-width: 100%; height: auto; }}
img.half {{ zoom: 0.5; }}
h2 {{ font-size: 15px; margin: 24px 0 6px; }} h3 {{ font-size: 12px; color: #777; margin: 0 0 6px; }}
.meta {{ color: #777; font-size: 11px; }} .fault {{ color: #c0392b; white-space: pre-wrap; }}
</style>
<body>{body}
<script type=module>
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
mermaid.initialize({{ startOnLoad: false, theme: "default" }});
await mermaid.run({{ querySelector: "pre.mermaid", suppressErrors: true }});
document.title = "rendered";
</script>"#
    )
}
