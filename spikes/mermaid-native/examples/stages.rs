//! Where one diagram's render time goes: parse, layout and its stages, SVG.
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(path).unwrap();
    let opts = mermaid_rs_renderer::RenderOptions::mermaid_default();
    let _ = mermaid_rs_renderer::render_with_detailed_timing(&src, opts.clone());
    let r = mermaid_rs_renderer::render_with_detailed_timing(&src, opts).unwrap();
    println!("parse {}us layout {}us render {}us", r.parse_us, r.layout_us, r.render_us);
    println!("{:#?}", r.layout_stages);
}
