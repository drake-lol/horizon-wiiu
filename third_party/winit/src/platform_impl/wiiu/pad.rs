//! The Wii U GamePad as keyboard and mouse: what the game would get from a PC's.
//!
//! Left stick: W A S D. Right stick: mouse look (or, in menus, the cursor). A jump (space), B sneak (left
//! shift), X inventory (E), Y drop (Q), click the left stick to sprint (left control), + pause (escape),
//! - perspective (F5), ZR / ZL left / right mouse button, L / R the hotbar (wheel), d-pad up chat (T).

use std::sync::atomic::Ordering;

use smol_str::SmolStr;

use crate::dpi::PhysicalPosition;
use crate::event::{DeviceEvent, ElementState, KeyEvent, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use crate::keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};

extern "C" {
    // wut (coreinit / vpad / whb), linked into the final executable
    fn WHBProcIsRunning() -> i32;
    fn VPADRead(chan: i32, buffers: *mut u8, count: u32, error: *mut i32) -> i32;
}

pub(crate) fn running() -> bool {
    unsafe { WHBProcIsRunning() != 0 }
}

const A: u32 = 0x8000;
const B: u32 = 0x4000;
const X: u32 = 0x2000;
const Y: u32 = 0x1000;
const UP: u32 = 0x0200;
const ZL: u32 = 0x0080;
const ZR: u32 = 0x0040;
const L: u32 = 0x0020;
const R: u32 = 0x0010;
const PLUS: u32 = 0x0008;
const MINUS: u32 = 0x0004;
const STICK_L: u32 = 0x40000;

pub(crate) enum Out {
    Window(WindowEvent),
    Device(DeviceEvent),
}

#[derive(Default)]
pub(crate) struct Mapper {
    hold: u32,
    keys: Vec<KeyCode>,
    cursor: (f64, f64),
}

fn did() -> crate::event::DeviceId {
    crate::event::DeviceId(super::DeviceId)
}

fn key_event(code: KeyCode, state: ElementState) -> WindowEvent {
    let (logical, text, location) = match code {
        KeyCode::Space => (Key::Named(NamedKey::Space), Some(" "), KeyLocation::Standard),
        KeyCode::ShiftLeft => (Key::Named(NamedKey::Shift), None, KeyLocation::Left),
        KeyCode::ControlLeft => (Key::Named(NamedKey::Control), None, KeyLocation::Left),
        KeyCode::Escape => (Key::Named(NamedKey::Escape), None, KeyLocation::Standard),
        KeyCode::F5 => (Key::Named(NamedKey::F5), None, KeyLocation::Standard),
        KeyCode::KeyW => (Key::Character("w".into()), Some("w"), KeyLocation::Standard),
        KeyCode::KeyA => (Key::Character("a".into()), Some("a"), KeyLocation::Standard),
        KeyCode::KeyS => (Key::Character("s".into()), Some("s"), KeyLocation::Standard),
        KeyCode::KeyD => (Key::Character("d".into()), Some("d"), KeyLocation::Standard),
        KeyCode::KeyE => (Key::Character("e".into()), Some("e"), KeyLocation::Standard),
        KeyCode::KeyQ => (Key::Character("q".into()), Some("q"), KeyLocation::Standard),
        KeyCode::KeyT => (Key::Character("t".into()), Some("t"), KeyLocation::Standard),
        _ => (Key::Unidentified(crate::keyboard::NativeKey::Unidentified), None, KeyLocation::Standard),
    };
    let text = if state == ElementState::Pressed { text.map(SmolStr::new) } else { None };
    WindowEvent::KeyboardInput {
        device_id: did(),
        event: KeyEvent {
            physical_key: PhysicalKey::Code(code),
            logical_key: logical.clone(),
            text: text.clone(),
            location,
            state,
            repeat: false,
            platform_specific: super::KeyEventExtra { key_without_modifiers: logical, text_with_all_modifiers: text },
        },
        is_synthetic: false,
    }
}

impl Mapper {
    pub(crate) fn poll(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        // VPADStatus: hold, trigger, release (u32), left stick, right stick (two f32 each), more after
        let mut raw = [0u8; 0x200];
        let mut err = 0i32;
        if unsafe { VPADRead(0, raw.as_mut_ptr(), 1, &mut err) } < 1 || err != 0 {
            return out;
        }
        let word = |o: usize| u32::from_be_bytes(raw[o..o + 4].try_into().unwrap());
        let float = |o: usize| f32::from_bits(word(o));
        let hold = word(0);
        let (lx, ly, rx, ry) = (float(12), float(16), float(20), float(24));

        let mut want = Vec::new();
        if ly > 0.5 {
            want.push(KeyCode::KeyW);
        }
        if ly < -0.5 {
            want.push(KeyCode::KeyS);
        }
        if lx < -0.5 {
            want.push(KeyCode::KeyA);
        }
        if lx > 0.5 {
            want.push(KeyCode::KeyD);
        }
        for (bit, code) in [
            (A, KeyCode::Space),
            (B, KeyCode::ShiftLeft),
            (X, KeyCode::KeyE),
            (Y, KeyCode::KeyQ),
            (STICK_L, KeyCode::ControlLeft),
            (PLUS, KeyCode::Escape),
            (MINUS, KeyCode::F5),
            (UP, KeyCode::KeyT),
        ] {
            if hold & bit != 0 {
                want.push(code);
            }
        }
        for k in self.keys.clone() {
            if !want.contains(&k) {
                out.push(Out::Window(key_event(k, ElementState::Released)));
            }
        }
        for k in &want {
            if !self.keys.contains(k) {
                out.push(Out::Window(key_event(*k, ElementState::Pressed)));
            }
        }
        self.keys = want;

        let pressed = hold & !self.hold;
        let released = self.hold & !hold;
        for (bit, button) in [(ZR, MouseButton::Left), (ZL, MouseButton::Right)] {
            if pressed & bit != 0 {
                out.push(Out::Window(WindowEvent::MouseInput { device_id: did(), state: ElementState::Pressed, button }));
            }
            if released & bit != 0 {
                out.push(Out::Window(WindowEvent::MouseInput { device_id: did(), state: ElementState::Released, button }));
            }
        }
        for (bit, lines) in [(L, 1.0), (R, -1.0)] {
            if pressed & bit != 0 {
                out.push(Out::Window(WindowEvent::MouseWheel {
                    device_id: did(),
                    delta: MouseScrollDelta::LineDelta(0.0, lines),
                    phase: TouchPhase::Moved,
                }));
            }
        }
        self.hold = hold;

        let dead = |v: f32| if v.abs() < 0.15 { 0.0 } else { v as f64 };
        let (dx, dy) = (dead(rx), -dead(ry));
        if dx != 0.0 || dy != 0.0 {
            if super::GRABBED.load(Ordering::Relaxed) {
                out.push(Out::Device(DeviceEvent::MouseMotion { delta: (dx * 24.0, dy * 24.0) }));
            } else {
                let (w, h) = super::SCREEN;
                self.cursor.0 = (self.cursor.0 + dx * 18.0).clamp(0.0, w as f64 - 1.0);
                self.cursor.1 = (self.cursor.1 + dy * 18.0).clamp(0.0, h as f64 - 1.0);
                out.push(Out::Window(WindowEvent::CursorMoved {
                    device_id: did(),
                    position: PhysicalPosition::new(self.cursor.0, self.cursor.1),
                }));
            }
        }
        out
    }
}
