//! A small voxel world and its mesher. The terrain here is procedural; a loader for Minecraft region
//! files replaces `generate` once the engine crates are linked in.

use crate::types::{Types, CROSS, CUTOUT, LIQUID, TINT_FOLIAGE, TINT_GRASS, TINT_TOP, TINT_WATER};
#[allow(unused_imports)]
use crate::atlas;

pub const SIZE_X: usize = 128;
pub const SIZE_Z: usize = 128;
pub const SIZE_Y: usize = 64;
pub const CHUNK: usize = 16;
pub const SEA: usize = 22;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Block {
    Air = 0,
    Stone,
    Dirt,
    Grass,
    Sand,
    Water,
    Snow,
    Log,
    Leaves,
    Planks,
    Gravel,
    Sandstone,
    Ice,
    Lava,
    Bedrock,
}

impl Block {
    pub fn from_u8(v: u8) -> Block {
        match v {
            1 => Block::Stone,
            2 => Block::Dirt,
            3 => Block::Grass,
            4 => Block::Sand,
            5 => Block::Water,
            6 => Block::Snow,
            7 => Block::Log,
            8 => Block::Leaves,
            9 => Block::Planks,
            10 => Block::Gravel,
            11 => Block::Sandstone,
            12 => Block::Ice,
            13 => Block::Lava,
            14 => Block::Bedrock,
            _ => Block::Air,
        }
    }

    pub fn solid(self) -> bool {
        !matches!(self, Block::Air | Block::Water | Block::Lava)
    }

    /// The placeholder atlas tile for a face: `up` is +Y, `down` is -Y, anything else a side.
    pub fn tile_for(self, up: bool, down: bool) -> usize {
        match self {
            Block::Air | Block::Stone => atlas::STONE,
            Block::Dirt => atlas::DIRT,
            Block::Grass => {
                if up { atlas::GRASS_TOP } else if down { atlas::DIRT } else { atlas::GRASS_SIDE }
            }
            Block::Sand => atlas::SAND,
            Block::Water => atlas::WATER,
            Block::Snow => atlas::SNOW,
            Block::Log => {
                if up || down { atlas::LOG_TOP } else { atlas::LOG_SIDE }
            }
            Block::Leaves => atlas::LEAVES,
            Block::Planks => atlas::PLANKS,
            Block::Gravel => atlas::GRAVEL,
            Block::Sandstone => atlas::SANDSTONE,
            Block::Ice => atlas::ICE,
            Block::Lava => atlas::LAVA,
            Block::Bedrock => atlas::BEDROCK,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 4],
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

pub struct World {
    blocks: Vec<u16>,
    pub types: Types,
    pub sx: usize,
    pub sy: usize,
    pub sz: usize,
    /// World y of layer 0 (a saved world starts at -64).
    pub y0: i32,
}

fn hash(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (z as u32).wrapping_mul(0x1656_67b1) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h & 0xff_ffff) as f32 / 0x100_0000 as f32
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn value_noise(x: f32, z: f32, seed: u32) -> f32 {
    let (xi, zi) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (smooth(x - xi as f32), smooth(z - zi as f32));
    let a = hash(xi, zi, seed);
    let b = hash(xi + 1, zi, seed);
    let c = hash(xi, zi + 1, seed);
    let d = hash(xi + 1, zi + 1, seed);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    top + (bottom - top) * fz
}

fn height_at(x: usize, z: usize) -> usize {
    let (fx, fz) = (x as f32, z as f32);
    let mut amp = 1.0;
    let mut freq = 1.0 / 48.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for octave in 0..4 {
        sum += value_noise(fx * freq, fz * freq, 7 + octave) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    let n = sum / norm;
    (10.0 + n * n * 1.6 * 36.0).clamp(4.0, (SIZE_Y - 8) as f32) as usize
}

impl World {
    pub fn empty(sx: usize, sy: usize, sz: usize, y0: i32, types: Types) -> World {
        World { blocks: vec![0u16; sx * sy * sz], types, sx, sy, sz, y0 }
    }

    /// Store block type `b` (an index into `types`).
    pub fn set(&mut self, x: usize, y: usize, z: usize, b: u16) {
        let i = (y * self.sz + z) * self.sx + x;
        if let Some(slot) = self.blocks.get_mut(i) {
            *slot = b;
        }
    }

    #[inline(never)]
    pub fn generate(types: Types) -> World {
        let mut blocks = vec![Block::Air; SIZE_X * SIZE_Y * SIZE_Z];
        let idx = |x: usize, y: usize, z: usize| (y * SIZE_Z + z) * SIZE_X + x;
        for z in 0..SIZE_Z {
            for x in 0..SIZE_X {
                let h = height_at(x, z);
                for y in 0..=h {
                    let b = if y + 4 < h {
                        Block::Stone
                    } else if y < h {
                        if h <= SEA + 1 { Block::Sand } else { Block::Dirt }
                    } else if h <= SEA + 1 {
                        Block::Sand
                    } else if h > 46 {
                        Block::Snow
                    } else if h > 38 {
                        Block::Stone
                    } else {
                        Block::Grass
                    };
                    blocks[idx(x, y, z)] = b;
                }
                for y in (h + 1)..=SEA {
                    blocks[idx(x, y, z)] = Block::Water;
                }
                // a few trees on grass
                if h > SEA + 1 && h <= 38 && x > 3 && z > 3 && x < SIZE_X - 4 && z < SIZE_Z - 4 && hash(x as i32, z as i32, 99) < 0.012 {
                    for y in (h + 1)..(h + 5) {
                        blocks[idx(x, y, z)] = Block::Log;
                    }
                    for dy in 3..6usize {
                        let r: i32 = if dy == 5 { 1 } else { 2 };
                        for dz in -r..=r {
                            for dx in -r..=r {
                                let (lx, lz) = ((x as i32 + dx) as usize, (z as i32 + dz) as usize);
                                if blocks[idx(lx, h + dy, lz)] == Block::Air {
                                    blocks[idx(lx, h + dy, lz)] = Block::Leaves;
                                }
                            }
                        }
                    }
                }
            }
        }
        World { blocks: blocks.iter().map(|b| *b as u16).collect(), types, sx: SIZE_X, sy: SIZE_Y, sz: SIZE_Z, y0: 0 }
    }

    #[inline(never)]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        if x < 0 || z < 0 || y < 0 || x >= self.sx as i32 || z >= self.sz as i32 || y >= self.sy as i32 {
            return 0;
        }
        // `.get`, not indexing: on this target the indexed form of this expression, with runtime dimensions,
        // hung in the first meshed layer (see wiiu/README.md, "Known gaps").
        let i = (y as usize * self.sz + z as usize) * self.sx + x as usize;
        self.blocks.get(i).copied().unwrap_or(0)
    }

    /// A block the player cannot walk through: a full cube that is not a liquid.
    pub fn solid_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.types.solid(self.get(x, y, z))
    }

    pub fn surface(&self, x: usize, z: usize) -> usize {
        (0..self.sy).rev().find(|&y| self.solid_at(x as i32, y as i32, z as i32)).unwrap_or(0)
    }

    /// Faces of the 16x16 column `(cx, cz)` as GX2 quads (four vertices each, counter-clockwise seen from outside).
    #[inline(never)]
    pub fn mesh_chunk(&self, cx: usize, cz: usize) -> Vec<Vertex> {
        // (neighbour offset, origin, u, v, brightness)
        const FACES: [([i32; 3], [f32; 3], [f32; 3], [f32; 3], f32); 6] = [
            ([1, 0, 0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 0.65),
            ([-1, 0, 0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 0.65),
            ([0, 1, 0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 1.0),
            ([0, -1, 0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.5),
            ([0, 0, 1], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.8),
            ([0, 0, -1], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 0.8),
        ];
        let mut out = Vec::new();
        for y in 0..self.sy as i32 {
            for z in (cz * CHUNK) as i32..((cz + 1) * CHUNK) as i32 {
                for x in (cx * CHUNK) as i32..((cx + 1) * CHUNK) as i32 {
                    let b = self.get(x, y, z);
                    if b == 0 {
                        continue;
                    }
                    let info = self.types.info(b);
                    if info.top == 0 && info.side == 0 && info.bottom == 0 && info.flags == 0 {
                        continue;
                    }
                    let liquid = info.flags & LIQUID != 0;
                    let tint = 0.94 + 0.12 * hash(x * 31 + y, z, 5);
                    if info.flags & CROSS != 0 {
                        let k = 0.9 * tint;
                        let rgb = if info.flags & TINT_GRASS != 0 { [k * 0.57, k * 0.74, k * 0.35] } else { [k, k, k] };
                        let color = [(rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8, 255];
                        let (u0, v0, su, sv) = atlas::uv_rect(info.side as usize);
                        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
                        // two diagonal planes, each emitted in both windings so back-face culling keeps one
                        for (a, b) in [([0.0, 0.0], [1.0, 1.0]), ([1.0, 0.0], [0.0, 1.0])] {
                            let bl = Vertex { pos: [fx + a[0], fy, fz + a[1], 1.0], color, uv: [u0, v0 + sv] };
                            let br = Vertex { pos: [fx + b[0], fy, fz + b[1], 1.0], color, uv: [u0 + su, v0 + sv] };
                            let tr = Vertex { pos: [fx + b[0], fy + 1.0, fz + b[1], 1.0], color, uv: [u0 + su, v0] };
                            let tl = Vertex { pos: [fx + a[0], fy + 1.0, fz + a[1], 1.0], color, uv: [u0, v0] };
                            out.extend_from_slice(&[bl, br, tr, tl]);
                            out.extend_from_slice(&[bl, tl, tr, br]);
                        }
                        continue;
                    }
                    let cutout = info.flags & CUTOUT != 0;
                    for (n, o, u, v, shade) in FACES.iter() {
                        let nb = self.get(x + n[0], y + n[1], z + n[2]);
                        // hidden behind an opaque block, or inside the same liquid
                        let visible = nb == 0 || (!self.types.opaque(nb) && !(liquid && self.types.liquid(nb)) && !(cutout && nb == b));
                        if !visible {
                            continue;
                        }
                        let tile = if n[1] == 1 { info.top } else if n[1] == -1 { info.bottom } else { info.side } as usize;
                        let k = shade * tint;
                        let mut rgb = [k, k, k];
                        let tinted: Option<[f32; 3]> = if info.flags & TINT_WATER != 0 {
                            Some([0.25, 0.46, 0.89])
                        } else if info.flags & TINT_FOLIAGE != 0 {
                            Some([0.47, 0.67, 0.18])
                        } else if info.flags & TINT_GRASS != 0 || (info.flags & TINT_TOP != 0 && n[1] == 1) {
                            Some([0.57, 0.74, 0.35])
                        } else {
                            None
                        };
                        if let Some(t) = tinted {
                            rgb = [k * t[0], k * t[1], k * t[2]];
                        }
                        let color = [(rgb[0] * 255.0).min(255.0) as u8, (rgb[1] * 255.0).min(255.0) as u8, (rgb[2] * 255.0).min(255.0) as u8, 255];
                        let (u0, v0, su, sv) = atlas::uv_rect(tile);
                        // which of the quad's two axes runs up the face
                        let u_vertical = u[1] != 0.0 && v[1] == 0.0;
                        let p = [x as f32 + o[0], y as f32 + o[1], z as f32 + o[2]];
                        let corner = |a: f32, c: f32| {
                            let (h, vert) = if u_vertical { (c, 1.0 - a) } else { (a, 1.0 - c) };
                            Vertex {
                                pos: [p[0] + u[0] * a + v[0] * c, p[1] + u[1] * a + v[1] * c, p[2] + u[2] * a + v[2] * c, 1.0],
                                color,
                                uv: [u0 + h * su, v0 + vert * sv],
                            }
                        };
                        out.push(corner(0.0, 0.0));
                        out.push(corner(1.0, 0.0));
                        out.push(corner(1.0, 1.0));
                        out.push(corner(0.0, 1.0));
                    }
                }
            }
        }
        out
    }
}
