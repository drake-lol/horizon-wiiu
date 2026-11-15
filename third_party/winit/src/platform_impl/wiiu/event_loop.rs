use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::mpsc;

use crate::error::EventLoopError;
use crate::event::{self, StartCause};
use crate::event_loop::{self, ControlFlow, DeviceEvents};
use crate::window::{CustomCursor as RootCustomCursor, CustomCursorSource, Theme};

use super::{MonitorHandle, PlatformSpecificEventLoopAttributes};

pub struct EventLoop<T: 'static> {
    window_target: event_loop::ActiveEventLoop,
    user_events_sender: mpsc::Sender<T>,
    user_events_receiver: mpsc::Receiver<T>,
}

impl<T: 'static> EventLoop<T> {
    pub(crate) fn new(_: &PlatformSpecificEventLoopAttributes) -> Result<Self, EventLoopError> {
        let (user_events_sender, user_events_receiver) = mpsc::channel();
        Ok(Self {
            window_target: event_loop::ActiveEventLoop {
                p: ActiveEventLoop { control_flow: Cell::new(ControlFlow::default()), exit: Cell::new(false) },
                _marker: PhantomData,
            },
            user_events_sender,
            user_events_receiver,
        })
    }

    /// Runs the application while the Wii U keeps it in the foreground: a frame per pass (the surface's
    /// acquire waits for the TV's vsync), GamePad input as keyboard and mouse events.
    pub fn run<F>(self, mut handler: F) -> Result<(), EventLoopError>
    where
        F: FnMut(event::Event<T>, &event_loop::ActiveEventLoop),
    {
        let target = &self.window_target;
        handler(event::Event::NewEvents(StartCause::Init), target);
        handler(event::Event::Resumed, target);
        let window = crate::window::WindowId(super::WindowId(0));
        let send = |handler: &mut F, e: event::WindowEvent| handler(event::Event::WindowEvent { window_id: window, event: e }, target);
        let mut first = true;
        let mut pad = super::pad::Mapper::default();
        while super::pad::running() && !target.p.exiting() {
            handler(event::Event::NewEvents(StartCause::Poll), target);
            if first {
                let (w, h) = super::SCREEN;
                send(&mut handler, event::WindowEvent::Resized(crate::dpi::PhysicalSize::new(w, h)));
                send(&mut handler, event::WindowEvent::Focused(true));
                first = false;
            }
            while let Ok(e) = self.user_events_receiver.try_recv() {
                handler(event::Event::UserEvent(e), target);
            }
            for e in pad.poll() {
                match e {
                    super::pad::Out::Window(e) => send(&mut handler, e),
                    super::pad::Out::Device(e) => handler(
                        event::Event::DeviceEvent { device_id: crate::event::DeviceId(super::DeviceId), event: e },
                        target,
                    ),
                }
            }
            send(&mut handler, event::WindowEvent::RedrawRequested);
            handler(event::Event::AboutToWait, target);
        }
        handler(event::Event::LoopExiting, target);
        Ok(())
    }

    pub fn window_target(&self) -> &event_loop::ActiveEventLoop {
        &self.window_target
    }

    pub fn create_proxy(&self) -> EventLoopProxy<T> {
        EventLoopProxy { user_events_sender: self.user_events_sender.clone() }
    }
}

pub struct EventLoopProxy<T: 'static> {
    user_events_sender: mpsc::Sender<T>,
}

impl<T> EventLoopProxy<T> {
    pub fn send_event(&self, event: T) -> Result<(), event_loop::EventLoopClosed<T>> {
        self.user_events_sender.send(event).map_err(|mpsc::SendError(x)| event_loop::EventLoopClosed(x))
    }
}

impl<T> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self { user_events_sender: self.user_events_sender.clone() }
    }
}

impl<T> Unpin for EventLoopProxy<T> {}

pub struct ActiveEventLoop {
    control_flow: Cell<ControlFlow>,
    exit: Cell<bool>,
}

impl ActiveEventLoop {
    pub fn create_custom_cursor(&self, source: CustomCursorSource) -> RootCustomCursor {
        let _ = source.inner;
        RootCustomCursor { inner: super::PlatformCustomCursor }
    }

    pub fn primary_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle)
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        VecDeque::from([MonitorHandle])
    }

    #[inline]
    pub fn listen_device_events(&self, _allowed: DeviceEvents) {}

    #[cfg(feature = "rwh_05")]
    #[inline]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::RawDisplayHandle::Orbital(rwh_05::OrbitalDisplayHandle::empty())
    }

    #[inline]
    pub fn system_theme(&self) -> Option<Theme> {
        None
    }

    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(&self) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawDisplayHandle::Orbital(rwh_06::OrbitalDisplayHandle::new()))
    }

    pub fn set_control_flow(&self, control_flow: ControlFlow) {
        self.control_flow.set(control_flow)
    }

    pub fn control_flow(&self) -> ControlFlow {
        self.control_flow.get()
    }

    pub(crate) fn exit(&self) {
        self.exit.set(true);
    }

    pub(crate) fn exiting(&self) -> bool {
        self.exit.get()
    }

    pub(crate) fn owned_display_handle(&self) -> OwnedDisplayHandle {
        OwnedDisplayHandle
    }
}

#[derive(Clone)]
pub(crate) struct OwnedDisplayHandle;

impl OwnedDisplayHandle {
    #[cfg(feature = "rwh_05")]
    #[inline]
    pub fn raw_display_handle_rwh_05(&self) -> rwh_05::RawDisplayHandle {
        rwh_05::OrbitalDisplayHandle::empty().into()
    }

    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(&self) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::OrbitalDisplayHandle::new().into())
    }
}
