//! The Swift binding for [`leaf_mermaid`]: a Mermaid diagram to a standalone
//! SVG, its labels measured by the host. For a host that registers a renderer
//! for `fence:mermaid` — the leaf-editor app does — and not part of leaf's
//! default build: leaf-ffi links it only under its `mermaid` feature, so an
//! app that draws no diagrams carries no Mermaid renderer.
//!
//! Its own UniFFI namespace, `leaf_mermaid_ffi`, so the generated Swift is a
//! module of its own beside `LeafFFI`.

use std::sync::Arc;

uniffi::setup_scaffolding!("leaf_mermaid_ffi");

/// How a host measures text for [`render_mermaid`]: the advance width of
/// `text` at `size` points in the first face of the comma-separated `family`
/// list it can draw, or `None` to leave it to the renderer's estimate. On
/// Apple, Core Text — the faces the picture will be drawn in.
#[uniffi::export(with_foreign)]
pub trait MermaidMeasure: Send + Sync {
    fn width(&self, text: String, size: f32, family: String) -> Option<f32>;
}

/// A rendered Mermaid diagram: a standalone SVG and its size in points.
#[derive(uniffi::Record)]
pub struct MermaidPicture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
}

/// A diagram [`render_mermaid`] could not read: what the parser said.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MermaidError {
    #[error("mermaid error: {message}")]
    Parse { message: String },
}

/// [`MermaidMeasure`] as `leaf-mermaid`'s [`leaf_mermaid::Measure`].
struct HostMeasure(Arc<dyn MermaidMeasure>);

impl leaf_mermaid::Measure for HostMeasure {
    fn width(&self, text: &str, size: f32, family: &str) -> Option<f32> {
        self.0.width(text.to_string(), size, family.to_string())
    }
}

/// Render a Mermaid diagram — a ```` ```mermaid ```` fence's content — to SVG,
/// in Mermaid's light theme or its dark one, measuring text through
/// `measure`. A diagram that does not parse is a [`MermaidError::Parse`] with
/// the parser's message.
#[uniffi::export]
pub fn render_mermaid(
    source: String,
    dark: bool,
    measure: Option<Arc<dyn MermaidMeasure>>,
) -> Result<MermaidPicture, MermaidError> {
    let measure = measure.map(|m| Arc::new(HostMeasure(m)) as Arc<dyn leaf_mermaid::Measure>);
    let p = leaf_mermaid::render(&source, dark, measure)
        .map_err(|e| MermaidError::Parse { message: e.message })?;
    Ok(MermaidPicture {
        svg: p.svg,
        width: p.width,
        height: p.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mermaid_diagram_renders_through_the_host_s_measure() {
        struct Counting(std::sync::atomic::AtomicUsize);
        impl MermaidMeasure for Counting {
            fn width(&self, text: String, size: f32, _family: String) -> Option<f32> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(text.chars().count() as f32 * size * 0.5)
            }
        }
        let m = Arc::new(Counting(Default::default()));
        let p = render_mermaid("flowchart LR\n  A --> B\n".into(), false, Some(m.clone())).unwrap();
        assert!(p.svg.starts_with("<svg") && p.width > 0.0 && p.height > 0.0);
        assert!(m.0.load(std::sync::atomic::Ordering::Relaxed) > 0);
        assert!(matches!(
            render_mermaid("flowchart LR\n  A -->\n".into(), false, None),
            Err(MermaidError::Parse { .. })
        ));
    }
}
