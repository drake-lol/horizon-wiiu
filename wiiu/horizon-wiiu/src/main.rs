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
mod types;
mod world;

use core::mem::zeroed;
use player::{Input, Player};
use wut_sys::*;

const FOV: f32 = 70.0 * core::f32::consts::PI / 180.0;
const SKY: [f32; 3] = [0.55, 0.72, 0.95];

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

/// A chunk column as the server sent it, kept so the window can move over it again. All-air sections are dropped.
struct Cached {
    min_y: i32,
    sections: Vec<Option<Vec<u16>>>,
    light: Vec<Option<Vec<u8>>>,
}

/// How far the window's floor sits below the player when it is placed.
const WINDOW_BELOW: i32 = 72;

struct Game {
    cache: std::collections::HashMap<(i32, i32), Cached>,
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
    verts_total: u64,
    resent: u32,
    forgotten: u32,
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

    fn edit(&mut self, pos: [i32; 3], block: u16) {
        if pos[0] < 0 || pos[1] < 0 || pos[2] < 0 {
            return;
        }
        self.world.set(pos[0] as usize, pos[1] as usize, pos[2] as usize, block);
        self.mark_dirty(pos[0], pos[2]);
    }

    fn break_block(&mut self) {
        let p = &self.player;
        if let Some((cell, _)) = player::raycast(&self.world, p.eye(), p.forward_vec(), player::REACH) {
            self.edit(cell, 0);
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
                let t = self.world.types.hotbar[self.selected];
                self.edit(at, t);
            }
        }
    }

    /// Put a received chunk column into the window and queue its meshes.
    /// Copy a cached chunk column into the window (if it is inside) and queue the meshes it touches.
    fn apply_cached(&mut self, x: i32, z: i32) {
        let (cx, cz) = (x - self.origin[0].div_euclid(16), z - self.origin[2].div_euclid(16));
        let (nx, nz) = (self.world.sx as i32 / 16, self.world.sz as i32 / 16);
        if cx < 0 || cz < 0 || cx >= nx || cz >= nz {
            return;
        }
        let Some(c) = self.cache.get(&(x, z)) else { return };
        let sy = self.world.sy as i32;
        for si in 0..c.sections.len() {
            let base = c.min_y + si as i32 * 16 - self.origin[1];
            if base + 16 <= 0 || base >= sy {
                continue;
            }
            for i in 0..4096usize {
                let wy = base + ((i >> 8) & 15) as i32;
                if wy < 0 || wy >= sy {
                    continue;
                }
                let b = c.sections[si].as_ref().map_or(0, |s| s[i]);
                let l = c.light[si].as_ref().map_or(15, |l| l[i]);
                let (lx, lz) = ((cx * 16) as usize + (i & 15), (cz * 16) as usize + ((i >> 4) & 15));
                self.world.set(lx, wy as usize, lz, b);
                self.world.set_light(lx, wy as usize, lz, l);
            }
        }
        self.queue_around(cx, cz);
    }

    fn queue_around(&mut self, cx: i32, cz: i32) {
        let (nx, nz) = (self.world.sx as i32 / 16, self.world.sz as i32 / 16);
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

    /// Server coordinates of the player's feet.
    fn player_server_pos(&self) -> [f64; 3] {
        let p = &self.player.pos;
        [p[0] as f64 + self.origin[0] as f64, p[1] as f64 + self.origin[1] as f64, p[2] as f64 + self.origin[2] as f64]
    }

    /// Move the window so the player is near its centre again, once they walk more than 3 chunks from it or come
    /// near its floor or ceiling; everything is copied back from the chunk cache and remeshed.
    fn recenter(&mut self) {
        if !self.spawned || self.cache.is_empty() {
            return;
        }
        let p = self.player_server_pos();
        let (pcx, pcz) = ((p[0].floor() as i32).div_euclid(16), (p[2].floor() as i32).div_euclid(16));
        let (ccx, ccz) = (self.origin[0].div_euclid(16) + 8, self.origin[2].div_euclid(16) + 8);
        let ly = self.player.pos[1];
        let far = (pcx - ccx).abs() > 3 || (pcz - ccz).abs() > 3 || ly < 24.0 || ly > self.world.sy as f32 - 24.0;
        if !far {
            return;
        }
        let new = [pcx * 16 - 8 * 16, ((p[1].floor() as i32) - WINDOW_BELOW).div_euclid(16) * 16, pcz * 16 - 8 * 16];
        let shift = [new[0] - self.origin[0], new[1] - self.origin[1], new[2] - self.origin[2]];
        self.origin = new;
        self.world.y0 = new[1];
        for i in 0..3 {
            self.player.pos[i] -= shift[i] as f32;
        }
        self.world.clear();
        for slot in self.meshes.iter_mut() {
            if let Some(m) = slot.take() {
                self.retired.push((self.frame, m));
            }
        }
        self.dirty.clear();
        let keys: Vec<(i32, i32)> = self.cache.keys().copied().collect();
        for (x, z) in keys {
            self.apply_cached(x, z);
        }
        println!("horizon-wiiu: window moved to {:?} ({} cached chunks)", self.origin, self.cache.len());
    }

    fn handle(&mut self, ev: net::Event) {
        match ev {
            net::Event::Spawn(p) => {
                if !self.spawned {
                    let (ox, oz) = ((p[0].floor() as i32).div_euclid(16) * 16 - 8 * 16, (p[2].floor() as i32).div_euclid(16) * 16 - 8 * 16);
                    let oy = ((p[1].floor() as i32) - WINDOW_BELOW).div_euclid(16) * 16;
                    self.origin = [ox, oy, oz];
                    self.world.y0 = oy;
                    self.player = Player::new([(p[0] - ox as f64) as f32, (p[1] - oy as f64) as f32, (p[2] - oz as f64) as f32]);
                    self.player.pitch = -0.5;
                    self.spawned = true;
                    println!("net: spawn at ({:.1}, {:.1}, {:.1}); window origin {:?}", p[0], p[1], p[2], self.origin);
                    for ev in std::mem::take(&mut self.pending) {
                        self.handle(ev);
                    }
                    let keys: Vec<(i32, i32)> = self.cache.keys().copied().collect();
                    for (x, z) in keys {
                        self.apply_cached(x, z);
                    }
                }
            }
            net::Event::Chunk { x, z, min_y, sections, light } => {
                let mut keep_s = Vec::with_capacity(sections.len());
                let mut keep_l = Vec::with_capacity(sections.len());
                for (s, l) in sections.into_iter().zip(light) {
                    let air = s.iter().all(|&b| b == 0);
                    keep_s.push(if air { None } else { Some(s) });
                    keep_l.push(if air && l.iter().all(|&v| v == 15) { None } else { Some(l) });
                }
                if self.cache.insert((x, z), Cached { min_y, sections: keep_s, light: keep_l }).is_some() {
                    self.resent += 1;
                }
                if self.spawned {
                    self.apply_cached(x, z);
                }
            }
            net::Event::Forget { x, z } => {
                self.forgotten += 1;
                self.cache.remove(&(x, z));
                let (cx, cz) = (x - self.origin[0].div_euclid(16), z - self.origin[2].div_euclid(16));
                if self.spawned && cx >= 0 && cz >= 0 && (cx as usize) < self.world.sx / 16 && (cz as usize) < self.world.sz / 16 {
                    for y in 0..self.world.sy {
                        for lz in 0..16 {
                            for lx in 0..16 {
                                self.world.set(cx as usize * 16 + lx, y, cz as usize * 16 + lz, 0);
                            }
                        }
                    }
                    self.queue_around(cx, cz);
                }
            }
            net::Event::Block { pos, block } => {
                let (kx, kz) = (pos[0].div_euclid(16), pos[2].div_euclid(16));
                if let Some(c) = self.cache.get_mut(&(kx, kz)) {
                    let si = (pos[1] - c.min_y).div_euclid(16);
                    if si >= 0 && (si as usize) < c.sections.len() {
                        let i = ((pos[1].rem_euclid(16) as usize) << 8) | ((pos[2].rem_euclid(16) as usize) << 4) | pos[0].rem_euclid(16) as usize;
                        let sec = c.sections[si as usize].get_or_insert_with(|| vec![0u16; 4096]);
                        sec[i] = block;
                    }
                }
                if self.spawned {
                    let (lx, ly, lz) = (pos[0] - self.origin[0], pos[1] - self.origin[1], pos[2] - self.origin[2]);
                    if lx >= 0 && ly >= 0 && lz >= 0 && (lx as usize) < self.world.sx && (ly as usize) < self.world.sy && (lz as usize) < self.world.sz {
                        self.world.set(lx as usize, ly as usize, lz as usize, block);
                        self.mark_dirty(lx, lz);
                    }
                }
            }
            net::Event::Closed(why) => println!("net: connection closed: {why}"),
        }
    }

    fn remesh(&mut self, gfx: &gfx::Gfx) {
        // a bounded number per frame: a joining client receives hundreds of chunks at once
        let take = self.dirty.len().min(if self.dirty.len() > 40 { 12 } else { 6 });
        let dirty: Vec<usize> = self.dirty.drain(..take).collect();
        let per_row = self.world.sx / world::CHUNK;
        for i in dirty {
            let (cx, cz) = (i % per_row, i / per_row);
            let verts = self.world.mesh_chunk(cx, cz);
            let fresh = gfx.upload(&verts);
            if fresh.is_none() && !verts.is_empty() {
                println!("horizon-wiiu: chunk {cx},{cz}: upload of {} vertices failed", verts.len());
            }
            self.verts_total += verts.len() as u64;
            if let Some(old) = std::mem::replace(&mut self.meshes[i], fresh) {
                self.retired.push((self.frame, old));
            }
        }
        let frame = self.frame;
        self.retired.retain(|(f, _)| frame < f + 4);
    }
}

/// Milliseconds since `t`, as an integer. On this target `Duration`'s derived ordering (`>=` on two `Duration`s)
/// gave wrong answers with CR-bit instructions disabled, like the slice `Ord` in horizon-world; integers compare right.
fn ms_since(t: std::time::Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

/// A scripted player for runs without a controller.
fn demo_input(frame: u32, straight: bool) -> (Input, bool, bool) {
    let mut input = Input { forward: 0.8, look_x: 0.0, ..Default::default() };
    // turn in bursts so the walk covers new ground
    if !straight && (frame / 90) % 2 == 1 {
        input.look_x = -0.35;
    }
    // jump often when walking a straight line over real terrain
    if straight {
        input.jump = frame % 20 == 0;
        return (input, false, false);
    }
    input.jump = frame % 75 == 0;
    input.look_y = 0.0;
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
    // The player's real textures and block types from the SD card when joined; else the placeholders.
    let loaded = if net.is_some() { types::Types::from_sd().unwrap_or_else(types::Types::builtin) } else { types::Types::builtin() };
    let ok = gfx.set_texture(loaded.atlas_width as u32, loaded.atlas_height as u32, &loaded.rgba);
    println!("horizon-wiiu: atlas {}x{} ({} block types) -> {ok}", loaded.atlas_width, loaded.atlas_height, loaded.types.list.len());
    let types = loaded.types;
    let world = if net.is_some() { world::World::empty(256, 128, 256, 0, types.clone()) } else { loader::load(types.clone()).unwrap_or_else(|| world::World::generate(types.clone())) };
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
    let mut game = Game { world, player: Player::new(spawn), meshes, retired: Vec::new(), dirty: Vec::new(), selected: 0, cooldown: 0, frame: 0, origin: [0; 3], spawned: net.is_none(), pending: Vec::new(), verts_total: 0, cache: std::collections::HashMap::new(), resent: 0, forgotten: 0 };
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
            let (i, b, p) = demo_input(game.frame, net.is_some());
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
        if net.is_some() {
            game.recenter();
        }
        // The server allows one position packet per 50 ms tick and kicks for two ("Invalid move player packet"),
        // so send by the clock, not by frame count: Cemu delivers frames in bursts.
        if net.is_some() && net::loaded() && loaded_at.is_none() {
            loaded_at = Some(std::time::Instant::now());
        }
        // one position packet per 100 ms (two server ticks), starting a second after PlayerLoaded: a server still
        // generating its first chunks stalls its tick, and moves queued into one tick are a kick
        let moving = loaded_at.is_some_and(|t| ms_since(t) >= 1000) && net::chunks_quiet(2000);
        // plain ifs: a tuple pattern `(Some(n), true)` here never matched on this target, though both were true
        let due = ms_since(last_move) >= 100;
        if game.frame % 120 == 60 {
            println!("gate: spawned={} moving={} due={} net={}", game.spawned, moving, due, net.is_some());
        }
        if game.spawned && moving && due {
            if game.frame % 120 == 60 {
                println!("gate: inside");
            }
            if let Some(n) = net.as_ref() {
                last_move = std::time::Instant::now();
                n.move_to(game.player_server_pos(), game.player.on_ground);
            }
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
        gfx.set_hud(&hud::build(game.selected, &game.world.types));
        let ortho = hud::ortho();

        gfx.begin_frame();
        gfx.draw_screen(true, SKY, &mvp, &ortho, &mut game.meshes);
        gfx.draw_screen(false, SKY, &mvp, &ortho, &mut game.meshes);
        gfx.end_frame();

        game.frame += 1;
        if game.frame % 120 == 0 {
            let p = &game.player;
            println!("horizon-wiiu: meshes {} verts_total {} cached {} resent {} forgotten {} loaded {} quiet {} loaded_ms {:?} last_move_ms {} spawned {}", game.meshes.iter().flatten().count(), game.verts_total, game.cache.len(), game.resent, game.forgotten, net::loaded(), net::chunks_quiet(2000), loaded_at.map(|t| t.elapsed().as_millis() as u64), last_move.elapsed().as_millis() as u64, game.spawned);
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
