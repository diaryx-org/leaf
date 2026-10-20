//! The renderer the editor's view draws with: Vello on wgpu, as
//! dioxus-native's own, but with exact-coverage antialiasing.
//!
//! dioxus-native wraps Vello in [`DioxusNativeWindowRenderer`] with Vello's
//! default of 16× multisampling, and keeps the constructor that would take
//! another private. Sixteen samples give an edge pixel one of 17 levels of
//! coverage, which at text sizes reads as a faintly stepped edge beside the
//! system's text, whose edges are continuous. Area antialiasing integrates
//! each pixel's coverage exactly: the edges a native text view draws. Its
//! weakness — conflation where shapes overlap with a winding number past one
//! — does not arise in glyphs or the editor's boxes.
//!
//! [`DioxusNativeWindowRenderer`]: dioxus_native::DioxusNativeWindowRenderer

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyrender::{RenderContext, WindowRenderer};
use anyrender_vello::{VelloRendererOptions, VelloWindowRenderer};
use vello::AaConfig;

/// Vello with area antialiasing, shareable as dioxus-native's renderer is:
/// the view and the document's context hold the same one.
#[derive(Clone)]
pub struct Renderer {
    inner: Rc<RefCell<VelloWindowRenderer>>,
}

impl Renderer {
    pub fn new() -> Self {
        let options = VelloRendererOptions::new().antialiasing_method(AaConfig::Area);
        Self {
            inner: Rc::new(RefCell::new(VelloWindowRenderer::with_options(options))),
        }
    }
}

impl RenderContext for Renderer {
    fn try_register_custom_resource(
        &mut self,
        resource: Box<dyn Any>,
    ) -> Result<anyrender::ResourceId, anyrender::RegisterResourceError> {
        self.inner
            .borrow_mut()
            .try_register_custom_resource(resource)
    }

    fn unregister_resource(&mut self, resource_id: anyrender::ResourceId) {
        self.inner.borrow_mut().unregister_resource(resource_id)
    }

    fn renderer_specific_context(&self) -> Option<Box<dyn Any>> {
        self.inner.borrow_mut().renderer_specific_context()
    }
}

impl WindowRenderer for Renderer {
    type ScenePainter<'a>
        = <VelloWindowRenderer as WindowRenderer>::ScenePainter<'a>
    where
        Self: 'a;

    fn resume<F: FnOnce() + 'static>(
        &mut self,
        window: Arc<dyn anyrender::WindowHandle>,
        width: u32,
        height: u32,
        on_ready: F,
    ) {
        self.inner
            .borrow_mut()
            .resume(window, width, height, on_ready)
    }

    fn complete_resume(&mut self) -> bool {
        self.inner.borrow_mut().complete_resume()
    }

    fn suspend(&mut self) {
        self.inner.borrow_mut().suspend()
    }

    fn is_active(&self) -> bool {
        self.inner.borrow().is_active()
    }

    fn is_pending(&self) -> bool {
        self.inner.borrow().is_pending()
    }

    fn set_size(&mut self, width: u32, height: u32) {
        self.inner.borrow_mut().set_size(width, height)
    }

    fn render<F: FnOnce(&mut Self::ScenePainter<'_>)>(&mut self, draw_fn: F) {
        self.inner.borrow_mut().render(draw_fn)
    }
}
