//! Horizon for Wii U: GX2 client with placeholder assets. Walk with the left stick, look with the right,
//! A jumps, ZR breaks the block in front, ZL places the selected block, L/R change it. With no controller input
//! a scripted demo plays (walk, jump, break, place) so headless emulator runs show the game working.
extern crate wiiu_atomics;

mod atlas;
mod gfx;
mod hud;
mod loader;
mod math;
mod net;
mod player;
mod world;

use core::mem::zeroed;
use player::{Input, Player};
use wut_sys::*;

const FOV: f32 = 70.0 * core::f32::consts::PI / 180.0;
const SKY: [f32; 3] = [0.55, 0.72, 0.95];
const CHUNKS_X: usize = world::SIZE_X / world::CHUNK;

/// Debug aid: every two seconds report where the main thread was last descheduled (pc, lr, sp), to find a spin.
fn watchdog() {
    let main = unsafe { OSGetCurrentThread() } as usize;
    std::thread::spawn(move || {
        for _ in 0..8 {
            unsafe { OSSleepTicks(124_000_000) };
            let t = main as *const OSThread;
            unsafe {
                OSReport(c"WATCHDOG main pc=%08x lr=%08x sp=%08x\n".as_ptr(), (*t).context.srr0, (*t).context.lr, (*t).context.gpr[1]);
            }
        }
    });
}

struct Game {
    world: world::World,
    player: Player,
    meshes: Vec<Option<gfx::Mesh>>,
    /// Replaced meshes stay alive a few frames: the GPU may still be reading them.
    retired: Vec<(u32, gfx::Mesh)>,
    dirty: Vec<usize>,
    selected: usize,
    cooldown: u32,
    frame: u32,
    /// Networked play: where the world window sits in server coordinates (x, y, z of its corner).
    origin: [i32; 3],
    spawned: bool,
    pending: Vec<net::Event>,
}

impl Game {
    fn chunk_index(&self, cx: usize, cz: usize) -> usize {
        cz * (self.world.sx / world::CHUNK) + cx
    }

    fn mark_dirty(&mut self, x: i32, z: i32) {
        let (cx, cz) = ((x.div_euclid(16)) as i32, (z.div_euclid(16)) as i32);
        let (nx, nz) = (self.world.sx as i32 / 16, self.world.sz as i32 / 16);
        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            // a neighbour only needs a remesh when the edit was on the shared edge
            let on_edge = match (dx, dz) {
                (0, 0) => true,
                (-1, 0) => x.rem_euclid(16) == 0,
                (1, 0) => x.rem_euclid(16) == 15,
                (0, -1) => z.rem_euclid(16) == 0,
                _ => z.rem_euclid(16) == 15,
            };
            let (c, d) = (cx + dx, cz + dz);
            if on_edge && c >= 0 && d >= 0 && c < nx && d < nz {
                let i = self.chunk_index(c as usize, d as usize);
                if !self.dirty.contains(&i) {
                    self.dirty.push(i);
                }
            }
        }
    }

    fn edit(&mut self, pos: [i32; 3], block: world::Block) {
        if pos[0] < 0 || pos[1] < 0 || pos[2] < 0 {
            return;
        }
        self.world.set(pos[0] as usize, pos[1] as usize, pos[2] as usize, block);
        self.mark_dirty(pos[0], pos[2]);
    }

    fn break_block(&mut self) {
        let p = &self.player;
        if let Some((cell, _)) = player::raycast(&self.world, p.eye(), p.forward_vec(), player::REACH) {
            self.edit(cell, world::Block::Air);
        }
    }

    fn place_block(&mut self) {
        let p = &self.player;
        if let Some((cell, normal)) = player::raycast(&self.world, p.eye(), p.forward_vec(), player::REACH) {
            let at = [cell[0] + normal[0], cell[1] + normal[1], cell[2] + normal[2]];
            // not inside the player's own box
            let (px, py, pz) = (self.player.pos[0], self.player.pos[1], self.player.pos[2]);
            let inside = (at[0] as f32 + 0.5 - px).abs() < 0.8 && (at[2] as f32 + 0.5 - pz).abs() < 0.8 && (at[1] as f32 + 0.5 - py - 0.9).abs() < 1.4;
            if !inside && normal != [0, 0, 0] {
                self.edit(at, hud::HOTBAR[self.selected]);
            }
        }
    }

    /// Put a received chunk column into the window and queue its meshes.
    fn apply_chunk(&mut self, x: i32, z: i32, min_y: i32, sections: &[Vec<u8>]) {
        let (cx, cz) = (x - self.origin[0].div_euclid(16), z - self.origin[2].div_euclid(16));
        let (nx, nz) = (self.world.sx as i32 / 16, self.world.sz as i32 / 16);
        if cx < 0 || cz < 0 || cx >= nx || cz >= nz {
            return;
        }
        let sy = self.world.sy as i32;
        for (si, sec) in sections.iter().enumerate() {
            let base = min_y + si as i32 * 16 - self.origin[1];
            if base + 16 <= 0 || base >= sy {
                continue;
            }
            for (i, &b) in sec.iter().enumerate() {
                let wy = base + ((i >> 8) & 15) as i32;
                if wy >= 0 && wy < sy {
                    self.world.set((cx * 16) as usize + (i & 15), wy as usize, (cz * 16) as usize + ((i >> 4) & 15), world::Block::from_u8(b));
                }
            }
        }
        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (c, d) = (cx + dx, cz + dz);
            if c >= 0 && d >= 0 && c < nx && d < nz {
                let i = self.chunk_index(c as usize, d as usize);
                if !self.dirty.contains(&i) {
                    self.dirty.push(i);
                }
            }
        }
    }

    fn handle(&mut self, ev: net::Event) {
        match ev {
            net::Event::Spawn(p) => {
                if !self.spawned {
                    let (ox, oz) = ((p[0].floor() as i32).div_euclid(16) * 16 - 8 * 16, (p[2].floor() as i32).div_euclid(16) * 16 - 8 * 16);
                    let oy = ((p[1].floor() as i32) - 32).div_euclid(16) * 16;
                    self.origin = [ox, oy, oz];
                    self.world.y0 = oy;
                    self.player = Player::new([(p[0] - ox as f64) as f32, (p[1] - oy as f64) as f32, (p[2] - oz as f64) as f32]);
                    self.spawned = true;
                    println!("net: spawn at ({:.1}, {:.1}, {:.1}); window origin {:?}", p[0], p[1], p[2], self.origin);
                    for ev in std::mem::take(&mut self.pending) {
                        self.handle(ev);
                    }
                }
            }
            net::Event::Chunk { x, z, min_y, sections } => {
                if self.spawned {
                    self.apply_chunk(x, z, min_y, &sections);
                } else {
                    self.pending.push(net::Event::Chunk { x, z, min_y, sections });
                }
            }
            net::Event::Block { pos, block } => {
                if self.spawned {
                    let (lx, ly, lz) = (pos[0] - self.origin[0], pos[1] - self.origin[1], pos[2] - self.origin[2]);
                    if lx >= 0 && ly >= 0 && lz >= 0 && (lx as usize) < self.world.sx && (ly as usize) < self.world.sy && (lz as usize) < self.world.sz {
                        self.world.set(lx as usize, ly as usize, lz as usize, world::Block::from_u8(block));
                        self.mark_dirty(lx, lz);
                    }
                }
            }
            net::Event::Closed(why) => println!("net: connection closed: {why}"),
        }
    }

    fn remesh(&mut self, gfx: &gfx::Gfx) {
        // a bounded number per frame: a joining client receives hundreds of chunks at once
        let take = self.dirty.len().min(6);
        let dirty: Vec<usize> = self.dirty.drain(..take).collect();
        for i in dirty {
            let (cx, cz) = (i % CHUNKS_X, i / CHUNKS_X);
            let verts = self.world.mesh_chunk(cx, cz);
            let fresh = gfx.upload(&verts);
            if let Some(old) = std::mem::replace(&mut self.meshes[i], fresh) {
                self.retired.push((self.frame, old));
            }
        }
        let frame = self.frame;
        self.retired.retain(|(f, _)| frame < f + 4);
    }
}

/// A scripted player for runs without a controller.
fn demo_input(frame: u32) -> (Input, bool, bool) {
    let mut input = Input { forward: 0.8, look_x: 0.0, ..Default::default() };
    // turn in bursts so the walk covers new ground
    if (frame / 90) % 2 == 1 {
        input.look_x = -0.35;
    }
    input.jump = frame % 75 == 0;
    input.look_y = if frame < 20 { -0.2 } else { 0.0 };
    let break_now = frame % 40 == 20;
    let place_now = frame % 40 == 30;
    (input, break_now, place_now)
}

fn main() {
    println!("horizon-wiiu: start");
    if std::env::var_os("HORIZON_WATCHDOG").is_some() {
        watchdog();
    }
    unsafe {
        WHBProcInit();
        VPADInit();
    }
    let Some(mut gfx) = gfx::Gfx::new() else {
        println!("horizon-wiiu: GX2 init failed");
        return;
    };
    let ok = gfx.set_texture(atlas::WIDTH as u32, atlas::HEIGHT as u32, &atlas::build());
    println!("horizon-wiiu: atlas texture -> {ok}");

    // The real game first: join a Horizon server; with none, the offline viewer world.
    let net = match net::join(net::server_address(), "WiiU", 6) {
        Ok(n) => {
            println!("net: joined {}", net::server_address());
            Some(n)
        }
        Err(e) => {
            println!("net: no server ({e}); offline world");
            None
        }
    };
    let world = if net.is_some() { world::World::empty(256, 96, 256, 0) } else { loader::load().unwrap_or_else(world::World::generate) };
    println!("horizon-wiiu: world {}x{}x{} (y0 {})", world.sx, world.sy, world.sz, world.y0);

    let (cw, cd) = (world.sx / world::CHUNK, world.sz / world::CHUNK);
    let mut meshes = Vec::new();
    let mut quads = 0u32;
    for cz in 0..cd {
        for cx in 0..cw {
            let verts = world.mesh_chunk(cx, cz);
            quads += verts.len() as u32 / 4;
            meshes.push(gfx.upload(&verts));
        }
    }
    println!("horizon-wiiu: {} chunk meshes, {} quads", meshes.iter().flatten().count(), quads);

    let (sx, sz) = (world.sx / 2, world.sz / 2);
    let spawn = [sx as f32 + 0.5, world.surface(sx, sz) as f32 + 2.0, sz as f32 + 0.5];
    let mut game = Game { world, player: Player::new(spawn), meshes, retired: Vec::new(), dirty: Vec::new(), selected: 0, cooldown: 0, frame: 0, origin: [0; 3], spawned: net.is_none(), pending: Vec::new() };
    let mut manual = false;
    let mut last_move = std::time::Instant::now();
    let mut loaded_at: Option<std::time::Instant> = None;
    let mut broke = 0u32;
    let mut placed = 0u32;

    while unsafe { WHBProcIsRunning() } != 0 {
        let mut input = Input::default();
        let (mut want_break, mut want_place) = (false, false);

        let mut status: VPADStatus = unsafe { zeroed() };
        let mut error: VPADReadError = 0;
        let n = unsafe { VPADRead(VPADChan_VPAD_CHAN_0, &mut status, 1, &mut error) };
        if n > 0 && error == 0 {
            let (lx, ly, rx, ry) = (status.leftStick.x, status.leftStick.y, status.rightStick.x, status.rightStick.y);
            if lx.abs() > 0.15 || ly.abs() > 0.15 || rx.abs() > 0.15 || ry.abs() > 0.15 || status.hold != 0 {
                manual = true;
            }
            if manual {
                let dead = |v: f32| if v.abs() < 0.12 { 0.0 } else { v };
                input = Input { strafe: dead(lx), forward: dead(ly), look_x: -dead(rx), look_y: dead(ry), jump: status.hold & VPADButtons_VPAD_BUTTON_A != 0 };
                want_break = status.hold & VPADButtons_VPAD_BUTTON_ZR != 0;
                want_place = status.hold & VPADButtons_VPAD_BUTTON_ZL != 0;
                if status.trigger & VPADButtons_VPAD_BUTTON_R != 0 {
                    game.selected = (game.selected + 1) % 9;
                }
                if status.trigger & VPADButtons_VPAD_BUTTON_L != 0 {
                    game.selected = (game.selected + 8) % 9;
                }
            }
        }
        if !manual {
            let (i, b, p) = demo_input(game.frame);
            input = i;
            want_break = b;
            want_place = p;
            if game.frame % 120 == 119 {
                game.selected = (game.selected + 1) % 9;
            }
        }

        if let Some(n) = &net {
            while let Ok(ev) = n.events.try_recv() {
                game.handle(ev);
            }
        }
        if game.spawned {
            game.player.update(&game.world, &input);
        }
        // The server allows one position packet per 50 ms tick and kicks for two ("Invalid move player packet"),
        // so send by the clock, not by frame count: Cemu delivers frames in bursts.
        if net.is_some() && net::loaded() && loaded_at.is_none() {
            loaded_at = Some(std::time::Instant::now());
        }
        // one position packet per 100 ms (two server ticks), starting a second after PlayerLoaded: a server still
        // generating its first chunks stalls its tick, and moves queued into one tick are a kick
        let moving = loaded_at.is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(1000));
        if let (Some(n), true) = (&net, game.spawned && moving && last_move.elapsed() >= std::time::Duration::from_millis(100)) {
            last_move = std::time::Instant::now();
            let p = &game.player;
            n.move_to([p.pos[0] as f64 + game.origin[0] as f64, p.pos[1] as f64 + game.origin[1] as f64, p.pos[2] as f64 + game.origin[2] as f64], p.on_ground);
        }
        if game.cooldown > 0 {
            game.cooldown -= 1;
        }
        if game.cooldown == 0 && (want_break || want_place) && net.is_none() {
            if want_break {
                game.break_block();
                broke += 1;
            } else {
                game.place_block();
                placed += 1;
            }
            game.cooldown = if manual { 10 } else { 0 };
        }
        game.remesh(&gfx);

        let proj = math::perspective(FOV, 16.0 / 9.0, 0.1, 400.0);
        let view = math::view(game.player.eye(), game.player.yaw, game.player.pitch);
        let mvp = math::mul(&proj, &view);
        gfx.set_hud(&hud::build(game.selected));
        let ortho = hud::ortho();

        gfx.begin_frame();
        gfx.draw_screen(true, SKY, &mvp, &ortho, &mut game.meshes);
        gfx.draw_screen(false, SKY, &mvp, &ortho, &mut game.meshes);
        gfx.end_frame();

        game.frame += 1;
        if game.frame % 120 == 0 {
            let p = &game.player;
            println!("horizon-wiiu: frame {} pos ({}, {}, {}) dm ground={} broke={broke} placed={placed}", game.frame, (p.pos[0] * 10.0) as i64, (p.pos[1] * 10.0) as i64, (p.pos[2] * 10.0) as i64, p.on_ground);
        }
    }
    drop(game);
    drop(gfx);
    unsafe {
        VPADShutdown();
        WHBProcShutdown();
    }
}
