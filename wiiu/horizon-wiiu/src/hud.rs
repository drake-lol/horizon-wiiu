//! The heads-up display, built as flat quads in a 1280x720 space: crosshair and a hotbar of block icons.

use crate::atlas;
use crate::math::Mat4;
use crate::types::Types;
use crate::world::Vertex;

pub const W: f32 = 1280.0;
pub const H: f32 = 720.0;

/// Pixels (top-left origin) to clip space.
pub fn ortho() -> Mat4 {
    let mut m = [0.0; 16];
    m[0] = 2.0 / W;
    m[5] = -2.0 / H;
    m[10] = 1.0;
    m[12] = -1.0;
    m[13] = 1.0;
    m[15] = 1.0;
    m
}

fn quad(out: &mut Vec<Vertex>, x: f32, y: f32, w: f32, h: f32, tile: usize, color: [u8; 4]) {
    let (u0, v0, su, sv) = atlas::uv_rect(tile);
    let corners = [(x, y, u0, v0), (x + w, y, u0 + su, v0), (x + w, y + h, u0 + su, v0 + sv), (x, y + h, u0, v0 + sv)];
    for (cx, cy, u, v) in corners {
        out.push(Vertex { pos: [cx, cy, 0.0, 1.0], color, uv: [u, v] });
    }
}

pub fn build(selected: usize, types: &Types) -> Vec<Vertex> {
    let mut v = Vec::new();
    // crosshair
    let (cx, cy) = (W / 2.0, H / 2.0);
    quad(&mut v, cx - 14.0, cy - 1.5, 28.0, 3.0, atlas::white(), [255, 255, 255, 255]);
    quad(&mut v, cx - 1.5, cy - 14.0, 3.0, 28.0, atlas::white(), [255, 255, 255, 255]);
    // hotbar
    let slot = 64.0;
    let x0 = (W - slot * 9.0) / 2.0;
    let y0 = H - slot - 24.0;
    quad(&mut v, x0 - 6.0, y0 - 6.0, slot * 9.0 + 12.0, slot + 12.0, atlas::white(), [30, 30, 30, 255]);
    for (i, t) in types.hotbar.iter().enumerate() {
        let x = x0 + i as f32 * slot;
        if i == selected {
            quad(&mut v, x - 3.0, y0 - 3.0, slot + 6.0, slot + 6.0, atlas::white(), [240, 240, 240, 255]);
        }
        quad(&mut v, x + 2.0, y0 + 2.0, slot - 4.0, slot - 4.0, atlas::white(), [70, 70, 70, 255]);
        quad(&mut v, x + 10.0, y0 + 10.0, slot - 20.0, slot - 20.0, types.info(*t).top as usize, [255, 255, 255, 255]);
    }
    v
}
