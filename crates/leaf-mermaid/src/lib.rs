//! Mermaid diagrams to SVG, for a host's fence editor.
//!
//! leaf draws no Mermaid itself: a host that edits ```` ```mermaid ```` fences
//! (`fence:mermaid`, see `docs/proposals/embedded-editors.md`) asks this crate
//! for the picture and draws it — as an `<img>` on the web, through resvg on
//! Apple — the way a formula's picture is drawn. The renderer is mmdr; this is
//! the door onto it, so its API is the bindings' and not mmdr's.
//!
//! Text is the one thing the renderer cannot know by itself: how wide a label
//! is in the face the host will draw it in. A host that can measure says so
//! through [`Measure`] — Core Text on Apple, a canvas on the web — and the
//! renderer never goes looking for fonts of its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mermaid_rs_renderer::{RenderOptions, Theme, metrics::TextMetrics};

/// How a host measures text: the advance width of `text` at `size` pixels in
/// the first face of the comma-separated `family` list it can draw, or `None`
/// to leave that one to the renderer's own estimate.
pub trait Measure: Send + Sync {
    fn width(&self, text: &str, size: f32, family: &str) -> Option<f32>;
}

/// A host's [`Measure`] as mmdr's [`TextMetrics`], asked once per label: mmdr
/// measures the same label several times in one layout (a 60-node flowchart
/// asks 412 times about 76 labels), and each ask may cross a binding into
/// the host's text system.
struct Host {
    measure: Arc<dyn Measure>,
    seen: Mutex<HashMap<(String, u32, String), Option<f32>>>,
}

impl Host {
    fn new(measure: Arc<dyn Measure>) -> Self {
        Host {
            measure,
            seen: Mutex::new(HashMap::new()),
        }
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Host")
    }
}

impl TextMetrics for Host {
    fn measure_text_width(&self, text: &str, font_size: f32, font_family: &str) -> Option<f32> {
        let key = (
            text.to_string(),
            font_size.to_bits(),
            font_family.to_string(),
        );
        if let Some(width) = self
            .seen
            .lock()
            .ok()
            .and_then(|seen| seen.get(&key).copied())
        {
            return width;
        }
        let width = self.measure.width(text, font_size, font_family);
        if let Ok(mut seen) = self.seen.lock() {
            seen.insert(key, width);
        }
        width
    }
}

/// A rendered diagram: a standalone SVG and its size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
}

/// Why a diagram did not render: what the parser said, for the host to show
/// in the diagram's place.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Render `source` — a fence's content — in Mermaid's own light theme, or its
/// dark one, measuring text through `measure` when the host gives one.
pub fn render(
    source: &str,
    dark: bool,
    measure: Option<Arc<dyn Measure>>,
) -> Result<Picture, Error> {
    let mut options = RenderOptions {
        theme: if dark {
            Theme::dark()
        } else {
            Theme::mermaid_default()
        },
        ..RenderOptions::default()
    };
    if let Some(m) = measure {
        options.layout.metrics = Some(Arc::new(Host::new(m)));
    }
    let svg = mermaid_rs_renderer::render_with_options(source, options).map_err(|e| Error {
        message: format!("{e:#}"),
    })?;
    let (width, height) = size(&svg).ok_or_else(|| Error {
        message: "the renderer wrote an SVG with no size".into(),
    })?;
    Ok(Picture { svg, width, height })
}

/// The root element's `width` and `height`, in pixels.
fn size(svg: &str) -> Option<(f64, f64)> {
    let head = &svg[..svg.find('>')?];
    let attr = |name: &str| -> Option<f64> {
        let at = head.find(&format!(" {name}=\""))? + name.len() + 3;
        head[at..]
            .split('"')
            .next()?
            .trim_end_matches("px")
            .parse()
            .ok()
    };
    Some((attr("width")?, attr("height")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flowchart_renders_to_an_svg_with_its_size() {
        let p = render("flowchart LR\n  A[Start] --> B[End]\n", false, None).unwrap();
        assert!(p.svg.starts_with("<svg"));
        assert!(p.width > 0.0 && p.height > 0.0);
        assert!(p.svg.contains(">Start<"));
    }

    #[test]
    fn a_source_that_does_not_parse_says_why() {
        let e = render("flowchart LR\n  A -->\n", false, None).unwrap_err();
        assert!(e.message.contains("edge"), "{}", e.message);
    }

    #[derive(Default)]
    struct Wide(std::sync::atomic::AtomicUsize);

    impl Measure for Wide {
        fn width(&self, text: &str, size: f32, _family: &str) -> Option<f32> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(text.chars().count() as f32 * size)
        }
    }

    #[test]
    fn a_host_that_measures_is_asked_and_its_widths_are_used() {
        let src = "flowchart LR\n  A[A label of some length] --> B[End]\n";
        let narrow = render(src, false, None).unwrap();
        let wide = Arc::new(Wide::default());
        let measured = render(src, false, Some(wide.clone())).unwrap();
        assert!(
            wide.0.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "the host was never asked"
        );
        assert!(
            measured.width > narrow.width,
            "the host's wider text did not widen the diagram"
        );
    }

    /// Every question a host was asked, in order.
    #[derive(Default)]
    struct Asked(Mutex<Vec<(String, u32, String)>>);

    impl Measure for Asked {
        fn width(&self, text: &str, size: f32, family: &str) -> Option<f32> {
            self.0
                .lock()
                .unwrap()
                .push((text.to_string(), size.to_bits(), family.to_string()));
            Some(text.chars().count() as f32 * size * 0.5)
        }
    }

    #[test]
    fn the_host_is_asked_once_per_label() {
        let src = "flowchart LR\n  A[Same] --> B[Same]\n  B --> C[Other]\n  C --> A\n";
        let asked = Arc::new(Asked::default());
        render(src, false, Some(asked.clone())).unwrap();
        let asks = asked.0.lock().unwrap().clone();
        let distinct: std::collections::HashSet<_> = asks.iter().collect();
        assert!(!asks.is_empty());
        assert_eq!(
            asks.len(),
            distinct.len(),
            "a label was asked about twice: {asks:?}"
        );
    }

    #[test]
    fn dark_is_a_different_picture() {
        let src = "flowchart LR\n  A --> B\n";
        assert_ne!(
            render(src, false, None).unwrap().svg,
            render(src, true, None).unwrap().svg
        );
    }
}
