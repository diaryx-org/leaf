//! Where one diagram's render time goes: mmdr's parse, layout and SVG, and
//! how many times layout asks the host to measure a label — and how many of
//! those asks are for a label it has asked about before.
//!
//!     cargo run -p leaf-mermaid --example timing --release -- diagram.mmd
//!
//! `MMDR_ENGINE=dagre` or `auto` picks mmdr's other flowchart engines.
//! `MMDR_SVG=out.svg` writes the SVG, to compare two builds' output.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mermaid_rs_renderer::{
    FlowchartLayoutEngine, RenderOptions, metrics::TextMetrics, render_with_detailed_timing,
};

/// How many measurements were asked for, and of which labels: text, size
/// and family.
type Asks = (usize, HashSet<(String, u32, String)>);

#[derive(Debug, Default)]
struct Counting {
    asks: Mutex<Asks>,
}

impl TextMetrics for Counting {
    fn measure_text_width(&self, text: &str, size: f32, family: &str) -> Option<f32> {
        let mut a = self.asks.lock().unwrap();
        a.0 += 1;
        a.1.insert((text.to_string(), size.to_bits(), family.to_string()));
        Some(text.chars().count() as f32 * size * 0.55)
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("a .mmd file");
    let src = std::fs::read_to_string(path).unwrap();
    let mut best = None::<(u128, u128, u128)>;
    let mut stages = String::new();
    let mut counts = (0, 0);
    for _ in 0..5 {
        let m = Arc::new(Counting::default());
        let mut opts = RenderOptions::mermaid_default();
        opts.layout.metrics = Some(m.clone());
        // `MMDR_ENGINE=dagre|auto` for mmdr's other flowchart engines.
        opts.layout.flowchart.engine = match std::env::var("MMDR_ENGINE").as_deref() {
            Ok("dagre") => FlowchartLayoutEngine::Dagre,
            Ok("auto") => FlowchartLayoutEngine::Auto,
            _ => FlowchartLayoutEngine::Current,
        };
        if std::env::var("MMDR_GRID").as_deref() == Ok("0") {
            opts.layout.flowchart.routing.enable_grid_router = false;
        }
        let r = render_with_detailed_timing(&src, opts).unwrap();
        if let Ok(out) = std::env::var("MMDR_SVG") {
            std::fs::write(out, &r.svg).unwrap();
        }
        let t = (r.parse_us, r.layout_us, r.render_us);
        if best.is_none_or(|b| t.0 + t.1 + t.2 < b.0 + b.1 + b.2) {
            best = Some(t);
            stages = format!("{:?}", r.layout_stages);
        }
        let a = m.asks.lock().unwrap();
        counts = (a.0, a.1.len());
    }
    let (p, l, r) = best.unwrap();
    println!(
        "parse {:.2} ms, layout {:.2} ms, svg {:.2} ms; {} label measurements, {} distinct",
        p as f64 / 1000.0,
        l as f64 / 1000.0,
        r as f64 / 1000.0,
        counts.0,
        counts.1
    );
    let n = |k: &str| {
        stages
            .split(k)
            .nth(1)
            .and_then(|s| s.split([',', ' ']).find(|w| !w.is_empty()))
            .unwrap_or("?")
            .to_string()
    };
    println!(
        "layout stages (us): layered {}, ports {}, edges {}, labels {}",
        n("layered_layout_us: "),
        n("port_assignment_us: "),
        n("edge_routing_us: "),
        n("label_placement_us: ")
    );
}
