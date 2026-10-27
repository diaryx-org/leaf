//! A winit [`Window`] that is no window: a sheet of a given size that Blitz
//! lays a page out on, offscreen, for tests (`headless.rs`, `sheet.rs`).
//!
//! Blitz and the editor talk to their window through winit's `Window`
//! trait — its size and scale, a redraw, the input method's caret area —
//! and none of them needs winit to have made it. So this answers from the
//! sheet's size and does nothing else: there is nothing on screen to move,
//! draw or point at.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dioxus_native::winit::cursor::Cursor;
use dioxus_native::winit::dpi::{PhysicalInsets, PhysicalPosition, PhysicalSize, Position, Size};
use dioxus_native::winit::error::RequestError;
use dioxus_native::winit::icon::Icon;
use dioxus_native::winit::monitor::{Fullscreen, MonitorHandle};
use dioxus_native::winit::window::{
    CursorGrabMode, ImeCapabilities, ImeRequest, ImeRequestError, ResizeDirection, Theme,
    UserAttentionType, Window, WindowButtons, WindowId, WindowLevel, WindowType,
};
use raw_window_handle as rwh;

/// A sheet, as a winit window.
pub struct HostedWindow {
    id: WindowId,
    view: NonNull<c_void>,
    state: Mutex<Geometry>,
    focused: AtomicBool,
    /// The input method's capabilities, once the editor has enabled it.
    ime: Mutex<Option<ImeCapabilities>>,
}

// SAFETY: the view pointer and the host's callbacks are only used on the main
// thread, where the host calls in and where every main-queue block runs. winit
// asks for `Send + Sync` because its own windows marshal to the main thread;
// nothing here is touched anywhere else.
unsafe impl Send for HostedWindow {}
unsafe impl Sync for HostedWindow {}

/// What the host last said about the view.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    /// In physical pixels.
    size: PhysicalSize<u32>,
    scale: f64,
    theme: Theme,
}

impl fmt::Debug for HostedWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostedWindow")
            .field("id", &self.id)
            .field("geometry", &*self.state.lock().unwrap())
            .finish()
    }
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

impl HostedWindow {
    /// The view `view`, `width` × `height` points at `scale` pixels a point.
    pub fn new(view: NonNull<c_void>, width: f64, height: f64, scale: f64, dark: bool) -> Self {
        Self {
            id: WindowId::from_raw(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            view,
            state: Mutex::new(Geometry {
                size: physical(width, height, scale),
                scale,
                theme: if dark { Theme::Dark } else { Theme::Light },
            }),
            focused: AtomicBool::new(false),
            ime: Mutex::new(None),
        }
    }
}

fn physical(width: f64, height: f64, scale: f64) -> PhysicalSize<u32> {
    PhysicalSize::new(
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

impl rwh::HasWindowHandle for HostedWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        #[cfg(target_os = "macos")]
        let raw = rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(self.view));
        #[cfg(target_os = "ios")]
        let raw = rwh::RawWindowHandle::UiKit(rwh::UiKitWindowHandle::new(self.view));
        #[cfg(not(target_vendor = "apple"))]
        return Err(rwh::HandleError::NotSupported);
        // SAFETY: the host keeps the view alive for as long as the editor in
        // it, and destroys the editor before the view.
        #[cfg(target_vendor = "apple")]
        Ok(unsafe { rwh::WindowHandle::borrow_raw(raw) })
    }
}

impl rwh::HasDisplayHandle for HostedWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        #[cfg(target_os = "macos")]
        return Ok(rwh::DisplayHandle::appkit());
        #[cfg(target_os = "ios")]
        return Ok(rwh::DisplayHandle::uikit());
        #[cfg(not(target_vendor = "apple"))]
        Err(rwh::HandleError::NotSupported)
    }
}

impl Window for HostedWindow {
    fn window_type(&self) -> WindowType {
        WindowType::default()
    }
    fn id(&self) -> WindowId {
        self.id
    }
    fn scale_factor(&self) -> f64 {
        self.state.lock().unwrap().scale
    }
    /// Nothing to draw to: a sheet is only laid out.
    fn request_redraw(&self) {}
    fn pre_present_notify(&self) {}
    fn reset_dead_keys(&self) {}
    fn surface_position(&self) -> PhysicalPosition<i32> {
        PhysicalPosition::new(0, 0)
    }
    fn outer_position(&self) -> Result<PhysicalPosition<i32>, RequestError> {
        Ok(PhysicalPosition::new(0, 0))
    }
    fn set_outer_position(&self, _position: Position) {}
    fn surface_size(&self) -> PhysicalSize<u32> {
        self.state.lock().unwrap().size
    }
    fn request_surface_size(&self, _size: Size) -> Option<PhysicalSize<u32>> {
        None
    }
    fn outer_size(&self) -> PhysicalSize<u32> {
        self.surface_size()
    }
    fn safe_area(&self) -> PhysicalInsets<u32> {
        PhysicalInsets::new(0, 0, 0, 0)
    }
    fn set_min_surface_size(&self, _min_size: Option<Size>) {}
    fn set_max_surface_size(&self, _max_size: Option<Size>) {}
    fn surface_resize_increments(&self) -> Option<PhysicalSize<u32>> {
        None
    }
    fn set_surface_resize_increments(&self, _increments: Option<Size>) {}
    /// The host's window has the document's title; the editor sets none.
    fn set_title(&self, _title: &str) {}
    fn set_transparent(&self, _transparent: bool) {}
    fn set_blur(&self, _blur: bool) {}
    fn set_visible(&self, _visible: bool) {}
    fn is_visible(&self) -> Option<bool> {
        Some(true)
    }
    fn set_resizable(&self, _resizable: bool) {}
    fn is_resizable(&self) -> bool {
        true
    }
    fn set_enabled_buttons(&self, _buttons: WindowButtons) {}
    fn enabled_buttons(&self) -> WindowButtons {
        WindowButtons::all()
    }
    fn set_minimized(&self, _minimized: bool) {}
    fn is_minimized(&self) -> Option<bool> {
        Some(false)
    }
    fn set_maximized(&self, _maximized: bool) {}
    fn is_maximized(&self) -> bool {
        false
    }
    fn set_fullscreen(&self, _fullscreen: Option<Fullscreen>) {}
    fn fullscreen(&self) -> Option<Fullscreen> {
        None
    }
    fn set_decorations(&self, _decorations: bool) {}
    fn is_decorated(&self) -> bool {
        true
    }
    fn set_window_level(&self, _level: WindowLevel) {}
    fn set_window_icon(&self, _window_icon: Option<Icon>) {}
    /// The input method's requests: where the caret is, for the host's text
    /// input client to answer `firstRectForCharacterRange:` with.
    fn request_ime_update(&self, request: ImeRequest) -> Result<(), ImeRequestError> {
        let data = match request {
            ImeRequest::Enable(enable) => {
                let (caps, data) = enable.into_raw();
                *self.ime.lock().unwrap() = Some(caps);
                data
            }
            ImeRequest::Update(data) => data,
            ImeRequest::Disable => {
                *self.ime.lock().unwrap() = None;
                return Ok(());
            }
            _ => return Ok(()),
        };
        let _ = data;
        Ok(())
    }
    fn ime_capabilities(&self) -> Option<ImeCapabilities> {
        *self.ime.lock().unwrap()
    }
    fn focus_window(&self) {}
    fn has_focus(&self) -> bool {
        self.focused.load(Ordering::Relaxed)
    }
    fn request_user_attention(&self, _request_type: Option<UserAttentionType>) {}
    fn set_theme(&self, _theme: Option<Theme>) {}
    fn theme(&self) -> Option<Theme> {
        Some(self.state.lock().unwrap().theme)
    }
    fn set_content_protected(&self, _protected: bool) {}
    fn title(&self) -> String {
        String::new()
    }
    /// The pointer's shape over what Blitz says is under it: an I-beam over
    /// text, a hand over a link.
    fn set_cursor(&self, _cursor: Cursor) {}
    fn set_cursor_position(&self, _position: Position) -> Result<(), RequestError> {
        Ok(())
    }
    fn set_cursor_grab(&self, _mode: CursorGrabMode) -> Result<(), RequestError> {
        Ok(())
    }
    fn set_cursor_visible(&self, _visible: bool) {}
    fn drag_window(&self) -> Result<(), RequestError> {
        Ok(())
    }
    fn drag_resize_window(&self, _direction: ResizeDirection) -> Result<(), RequestError> {
        Ok(())
    }
    fn show_window_menu(&self, _position: Position) {}
    fn set_cursor_hittest(&self, _hittest: bool) -> Result<(), RequestError> {
        Ok(())
    }
    fn current_monitor(&self) -> Option<MonitorHandle> {
        None
    }
    fn available_monitors(&self) -> Box<dyn Iterator<Item = MonitorHandle>> {
        Box::new(std::iter::empty())
    }
    fn primary_monitor(&self) -> Option<MonitorHandle> {
        None
    }
    fn rwh_06_display_handle(&self) -> &dyn rwh::HasDisplayHandle {
        self
    }
    fn rwh_06_window_handle(&self) -> &dyn rwh::HasWindowHandle {
        self
    }
}
