//! A winit [`Window`] that is a terminal: the editor and Blitz ask their
//! window for its size, its scale and its theme, and ask it to redraw. They
//! need nothing else of winit, so this answers from the terminal's size in
//! cells, multiplied out to CSS pixels by [`crate::cell`], and does nothing
//! else. `apps/leaf-dioxus/src/offscreen.rs` is the same idea for a sheet.

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// The terminal, as a winit window of `size` pixels at a scale of one.
pub struct TermWindow {
    size: Mutex<PhysicalSize<u32>>,
    dark: bool,
    /// Set by the editor's `request_redraw`, taken by the loop in `main.rs`.
    dirty: AtomicBool,
    ime: Mutex<Option<ImeCapabilities>>,
}

impl fmt::Debug for TermWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TermWindow")
            .field("size", &*self.size.lock().unwrap())
            .finish()
    }
}

impl TermWindow {
    pub fn new(width: u32, height: u32, dark: bool) -> Self {
        Self {
            size: Mutex::new(PhysicalSize::new(width.max(1), height.max(1))),
            dark,
            dirty: AtomicBool::new(true),
            ime: Mutex::new(None),
        }
    }

    pub fn resize(&self, width: u32, height: u32) {
        *self.size.lock().unwrap() = PhysicalSize::new(width.max(1), height.max(1));
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Whether anything asked for a redraw since the last call.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

impl rwh::HasWindowHandle for TermWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        Err(rwh::HandleError::NotSupported)
    }
}

impl rwh::HasDisplayHandle for TermWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Err(rwh::HandleError::NotSupported)
    }
}

impl Window for TermWindow {
    fn window_type(&self) -> WindowType {
        WindowType::default()
    }
    fn id(&self) -> WindowId {
        WindowId::from_raw(1)
    }
    fn scale_factor(&self) -> f64 {
        1.0
    }
    fn request_redraw(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }
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
        *self.size.lock().unwrap()
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
        false
    }
    fn set_window_level(&self, _level: WindowLevel) {}
    fn set_window_icon(&self, _window_icon: Option<Icon>) {}
    /// A terminal has no input method to place: its own composes in place.
    fn request_ime_update(&self, request: ImeRequest) -> Result<(), ImeRequestError> {
        match request {
            ImeRequest::Enable(enable) => {
                let (caps, _) = enable.into_raw();
                *self.ime.lock().unwrap() = Some(caps);
            }
            ImeRequest::Disable => *self.ime.lock().unwrap() = None,
            _ => {}
        }
        Ok(())
    }
    fn ime_capabilities(&self) -> Option<ImeCapabilities> {
        *self.ime.lock().unwrap()
    }
    fn focus_window(&self) {}
    fn has_focus(&self) -> bool {
        true
    }
    fn request_user_attention(&self, _request_type: Option<UserAttentionType>) {}
    fn set_theme(&self, _theme: Option<Theme>) {}
    fn theme(&self) -> Option<Theme> {
        Some(if self.dark { Theme::Dark } else { Theme::Light })
    }
    fn set_content_protected(&self, _protected: bool) {}
    fn title(&self) -> String {
        String::new()
    }
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
