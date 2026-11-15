use std::collections::VecDeque;
use std::sync::Mutex;

use crate::cursor::Cursor;
use crate::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use crate::platform_impl::Fullscreen;
use crate::window::ImePurpose;
use crate::{error, window};

use super::{ActiveEventLoop, MonitorHandle, OsError, WindowId, SCREEN};

/// The TV picture. Its title and flags are kept so they read back as set.
pub struct Window {
    title: Mutex<String>,
    visible: Mutex<bool>,
}

fn unsupported() -> error::ExternalError {
    error::ExternalError::NotSupported(error::NotSupportedError::new())
}

impl Window {
    pub(crate) fn new(
        _el: &ActiveEventLoop,
        attrs: window::WindowAttributes,
    ) -> Result<Self, error::OsError> {
        let _ = OsError("no window system");
        Ok(Self { title: Mutex::new(attrs.title), visible: Mutex::new(attrs.visible) })
    }

    pub(crate) fn maybe_queue_on_main(&self, f: impl FnOnce(&Self) + Send + 'static) {
        f(self)
    }

    pub(crate) fn maybe_wait_on_main<R: Send>(&self, f: impl FnOnce(&Self) -> R + Send) -> R {
        f(self)
    }

    pub fn id(&self) -> WindowId {
        WindowId(0)
    }

    pub fn primary_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle)
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        VecDeque::from([MonitorHandle])
    }

    pub fn current_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle)
    }

    pub fn scale_factor(&self) -> f64 {
        1.0
    }

    pub fn request_redraw(&self) {}

    pub fn pre_present_notify(&self) {}

    pub fn reset_dead_keys(&self) {}

    pub fn inner_position(&self) -> Result<PhysicalPosition<i32>, error::NotSupportedError> {
        Ok((0, 0).into())
    }

    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>, error::NotSupportedError> {
        Ok((0, 0).into())
    }

    pub fn set_outer_position(&self, _position: Position) {}

    pub fn inner_size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(SCREEN.0, SCREEN.1)
    }

    pub fn request_inner_size(&self, _size: Size) -> Option<PhysicalSize<u32>> {
        Some(self.inner_size())
    }

    pub fn outer_size(&self) -> PhysicalSize<u32> {
        self.inner_size()
    }

    pub fn set_min_inner_size(&self, _: Option<Size>) {}

    pub fn set_max_inner_size(&self, _: Option<Size>) {}

    pub fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }

    pub fn set_title(&self, title: &str) {
        *self.title.lock().unwrap() = title.to_owned();
    }

    pub fn set_transparent(&self, _transparent: bool) {}

    pub fn set_blur(&self, _blur: bool) {}

    pub fn set_visible(&self, visible: bool) {
        *self.visible.lock().unwrap() = visible;
    }

    pub fn is_visible(&self) -> Option<bool> {
        Some(*self.visible.lock().unwrap())
    }

    pub fn resize_increments(&self) -> Option<PhysicalSize<u32>> {
        None
    }

    pub fn set_resize_increments(&self, _increments: Option<Size>) {}

    pub fn set_resizable(&self, _resizeable: bool) {}

    pub fn is_resizable(&self) -> bool {
        false
    }

    pub fn set_minimized(&self, _minimized: bool) {}

    pub fn is_minimized(&self) -> Option<bool> {
        Some(false)
    }

    pub fn set_maximized(&self, _maximized: bool) {}

    pub fn is_maximized(&self) -> bool {
        true
    }

    pub(crate) fn set_fullscreen(&self, _monitor: Option<Fullscreen>) {}

    pub(crate) fn fullscreen(&self) -> Option<Fullscreen> {
        Some(Fullscreen::Borderless(Some(MonitorHandle)))
    }

    pub fn set_decorations(&self, _decorations: bool) {}

    pub fn is_decorated(&self) -> bool {
        false
    }

    pub fn set_window_level(&self, _level: window::WindowLevel) {}

    pub fn set_window_icon(&self, _window_icon: Option<crate::icon::Icon>) {}

    pub fn set_ime_cursor_area(&self, _position: Position, _size: Size) {}

    pub fn set_ime_allowed(&self, _allowed: bool) {}

    pub fn set_ime_purpose(&self, _purpose: ImePurpose) {}

    pub fn focus_window(&self) {}

    pub fn request_user_attention(&self, _request_type: Option<window::UserAttentionType>) {}

    pub fn set_cursor(&self, _: Cursor) {}

    pub fn set_cursor_position(&self, _: Position) -> Result<(), error::ExternalError> {
        Err(unsupported())
    }

    pub fn set_cursor_grab(&self, _mode: window::CursorGrabMode) -> Result<(), error::ExternalError> {
        Err(unsupported())
    }

    pub fn set_cursor_visible(&self, _visible: bool) {}

    pub fn drag_window(&self) -> Result<(), error::ExternalError> {
        Err(unsupported())
    }

    pub fn drag_resize_window(
        &self,
        _direction: window::ResizeDirection,
    ) -> Result<(), error::ExternalError> {
        Err(unsupported())
    }

    pub fn show_window_menu(&self, _position: Position) {}

    pub fn set_cursor_hittest(&self, _hittest: bool) -> Result<(), error::ExternalError> {
        Err(unsupported())
    }

    #[cfg(feature = "rwh_04")]
    pub fn raw_window_handle_rwh_04(&self) -> rwh_04::RawWindowHandle {
        rwh_04::RawWindowHandle::Orbital(rwh_04::OrbitalHandle::empty())
    }

    #[cfg(feature = "rwh_05")]
    pub fn raw_window_handle_rwh_05(&self) -> rwh_05::RawWindowHandle {
        rwh_05::RawWindowHandle::Orbital(rwh_05::OrbitalWindowHandle::empty())
    }

    #[cfg(feature = "rwh_05")]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::RawDisplayHandle::Orbital(rwh_05::OrbitalDisplayHandle::empty())
    }

    /// An Orbital handle with no window behind it: the GX2 backend draws to the TV whatever it is given.
    #[cfg(feature = "rwh_06")]
    pub fn raw_window_handle_rwh_06(&self) -> Result<rwh_06::RawWindowHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawWindowHandle::Orbital(rwh_06::OrbitalWindowHandle::new(std::ptr::NonNull::dangling())))
    }

    #[cfg(feature = "rwh_06")]
    pub fn raw_display_handle_rwh_06(&self) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawDisplayHandle::Orbital(rwh_06::OrbitalDisplayHandle::new()))
    }

    pub fn set_enabled_buttons(&self, _buttons: window::WindowButtons) {}

    pub fn enabled_buttons(&self) -> window::WindowButtons {
        window::WindowButtons::empty()
    }

    pub fn theme(&self) -> Option<window::Theme> {
        None
    }

    pub fn has_focus(&self) -> bool {
        true
    }

    pub fn set_theme(&self, _theme: Option<window::Theme>) {}

    pub fn set_content_protected(&self, _protected: bool) {}
}
