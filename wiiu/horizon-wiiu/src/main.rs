//! Horizon for Wii U: GX2 client. Fly over a voxel world with the GamePad (left stick moves, right stick
//! looks, ZL/ZR down/up); with no input the camera flies a circuit by itself.
extern crate wiiu_atomics;

mod atlas;
mod gfx;
mod math;
mod world;

use core::mem::zeroed;
use wut_sys::*;

const FOV: f32 = 70.0 * core::f32::consts::PI / 180.0;
const SKY: [f32; 3] = [0.55, 0.72, 0.95];

struct Camera {
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
}

fn main() {
    println!("horizon-wiiu: start");
    unsafe {
        WHBProcInit();
        VPADInit();
    }
    let Some(mut gfx) = gfx::Gfx::new() else {
        println!("horizon-wiiu: GX2 init failed");
        return;
    };
    let ok = gfx.set_texture(atlas::SIZE as u32, atlas::SIZE as u32, &atlas::build());
    println!("horizon-wiiu: atlas texture -> {ok}");

    let world = world::World::generate();
    let mut meshes = Vec::new();
    let mut quads = 0u32;
    for cz in 0..world::SIZE_Z / world::CHUNK {
        for cx in 0..world::SIZE_X / world::CHUNK {
            let verts = world.mesh_chunk(cx, cz);
            quads += verts.len() as u32 / 4;
            if let Some(m) = gfx.upload(&verts) {
                meshes.push(m);
            }
        }
    }
    println!("horizon-wiiu: {} chunk meshes, {} quads", meshes.len(), quads);

    let centre = (world::SIZE_X / 2, world::SIZE_Z / 2);
    let mut cam = Camera { pos: [centre.0 as f32, world.surface(centre.0, centre.1) as f32 + 12.0, centre.1 as f32], yaw: 0.0, pitch: -0.45 };
    let mut frame = 0u32;
    let mut manual = false;

    while unsafe { WHBProcIsRunning() } != 0 {
        // Input
        let mut status: VPADStatus = unsafe { zeroed() };
        let mut error: VPADReadError = 0;
        let n = unsafe { VPADRead(VPADChan_VPAD_CHAN_0, &mut status, 1, &mut error) };
        if n > 0 && error == 0 {
            let (lx, ly) = (status.leftStick.x, status.leftStick.y);
            let (rx, ry) = (status.rightStick.x, status.rightStick.y);
            if lx.abs() > 0.15 || ly.abs() > 0.15 || rx.abs() > 0.15 || ry.abs() > 0.15 {
                manual = true;
            }
            cam.yaw -= rx * 0.04;
            cam.pitch = (cam.pitch + ry * 0.03).clamp(-1.5, 1.5);
            let speed = 0.5;
            let (s, c) = (cam.yaw.sin(), cam.yaw.cos());
            cam.pos[0] += (-s * ly - c * lx) * speed;
            cam.pos[2] += (-c * ly + s * lx) * speed;
            if status.hold & VPADButtons_VPAD_BUTTON_ZR != 0 {
                cam.pos[1] += 0.4;
            }
            if status.hold & VPADButtons_VPAD_BUTTON_ZL != 0 {
                cam.pos[1] -= 0.4;
            }
        }
        if !manual {
            let t = frame as f32 * 0.012;
            let r = 42.0;
            cam.pos = [centre.0 as f32 + r * t.cos(), 0.0, centre.1 as f32 + r * t.sin()];
            let ground = world.surface(cam.pos[0] as usize, cam.pos[2] as usize) as f32;
            cam.pos[1] = ground.max(world::SEA as f32) + 9.0;
            // face the centre of the world
            cam.yaw = core::f32::consts::FRAC_PI_2 - t;
            cam.pitch = -0.35;
        }

        let proj = math::perspective(FOV, 16.0 / 9.0, 0.1, 400.0);
        let view = math::view(cam.pos, cam.yaw, cam.pitch);
        let mvp = math::mul(&proj, &view);

        gfx.begin_frame();
        gfx.draw_screen(true, SKY, &mvp, &mut meshes);
        gfx.draw_screen(false, SKY, &mvp, &mut meshes);
        gfx.end_frame();

        frame += 1;
        if frame == 60 {
            println!("horizon-wiiu: 60 frames");
        }
    }
    drop(meshes);
    drop(gfx);
    unsafe {
        VPADShutdown();
        WHBProcShutdown();
    }
}
