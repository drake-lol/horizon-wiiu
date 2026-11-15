//! A procedural block texture atlas (4x4 tiles of 16x16 RGBA), standing in for the player's resource
//! pack until the asset path is wired in. Tiles are numbered row by row.

pub const TILE: usize = 16;
pub const TILES_PER_ROW: usize = 4;
pub const SIZE: usize = TILE * TILES_PER_ROW;

pub const GRASS_TOP: usize = 0;
pub const GRASS_SIDE: usize = 1;
pub const DIRT: usize = 2;
pub const STONE: usize = 3;
pub const SAND: usize = 4;
pub const WATER: usize = 5;
pub const SNOW: usize = 6;
pub const LOG_TOP: usize = 7;
pub const LOG_SIDE: usize = 8;
pub const LEAVES: usize = 9;

fn noise(tile: usize, x: usize, y: usize) -> f32 {
    let mut h = (tile as u32).wrapping_mul(0x9e37_79b9) ^ (x as u32).wrapping_mul(0x85eb_ca6b) ^ (y as u32).wrapping_mul(0xc2b2_ae35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

fn shade(c: [f32; 3], k: f32) -> [u8; 4] {
    [(c[0] * k).clamp(0.0, 255.0) as u8, (c[1] * k).clamp(0.0, 255.0) as u8, (c[2] * k).clamp(0.0, 255.0) as u8, 255]
}

fn pixel(tile: usize, x: usize, y: usize) -> [u8; 4] {
    let n = noise(tile, x, y);
    let speckle = 0.85 + 0.3 * n;
    match tile {
        GRASS_TOP => shade([95.0, 159.0, 53.0], speckle),
        GRASS_SIDE => {
            // a ragged green fringe over dirt
            let fringe = 3 + (noise(GRASS_SIDE, x, 0) * 3.0) as usize;
            if y < fringe { shade([95.0, 159.0, 53.0], speckle) } else { shade([134.0, 96.0, 67.0], speckle) }
        }
        DIRT => shade([134.0, 96.0, 67.0], speckle),
        STONE => {
            let crack = if noise(STONE, x / 4, y / 4) > 0.82 { 0.75 } else { 1.0 };
            shade([125.0, 125.0, 125.0], (0.88 + 0.24 * n) * crack)
        }
        SAND => shade([219.0, 207.0, 163.0], 0.92 + 0.16 * n),
        WATER => {
            let wave = 0.9 + 0.12 * ((x as f32 * 0.7).sin() * (y as f32 * 0.5).cos());
            shade([46.0, 92.0, 200.0], wave * (0.97 + 0.06 * n))
        }
        SNOW => shade([245.0, 250.0, 250.0], 0.96 + 0.06 * n),
        LOG_TOP => {
            let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
            let ring = ((dx * dx + dy * dy).sqrt() * 1.4).sin() * 0.5 + 0.5;
            shade([165.0, 130.0, 80.0], 0.75 + 0.3 * ring)
        }
        LOG_SIDE => {
            let stripe = if x % 4 == 0 { 0.78 } else { 1.0 };
            shade([102.0, 81.0, 49.0], stripe * (0.9 + 0.2 * n))
        }
        LEAVES => shade([56.0, 118.0, 29.0], 0.65 + 0.6 * n),
        _ => [255, 0, 255, 255],
    }
}

/// The atlas as RGBA bytes, row-major, `SIZE` x `SIZE`.
pub fn build() -> Vec<u8> {
    let mut out = vec![0u8; SIZE * SIZE * 4];
    for tile in 0..TILES_PER_ROW * TILES_PER_ROW {
        let (tx, ty) = (tile % TILES_PER_ROW, tile / TILES_PER_ROW);
        for y in 0..TILE {
            for x in 0..TILE {
                let p = pixel(tile, x, y);
                let o = ((ty * TILE + y) * SIZE + tx * TILE + x) * 4;
                out[o..o + 4].copy_from_slice(&p);
            }
        }
    }
    out
}

/// Texture coordinates of tile `tile`: (u0, v0, size), inset by half a texel against bleeding.
pub fn uv_rect(tile: usize) -> (f32, f32, f32) {
    let (tx, ty) = (tile % TILES_PER_ROW, tile / TILES_PER_ROW);
    let texel = 1.0 / SIZE as f32;
    let size = 1.0 / TILES_PER_ROW as f32;
    (tx as f32 * size + texel * 0.5, ty as f32 * size + texel * 0.5, size - texel)
}
