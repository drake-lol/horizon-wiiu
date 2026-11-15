//! usage: mkatlas <prepared 26.3 folder> <textures/block folder> <output folder>
//! Reads the player's own prepared data (`facts.json`) and the block textures extracted from their Minecraft, and
//! writes what the Wii U client loads from the SD card (nothing here is shipped by the repo):
//!   atlas.rgba  "HZAT", u16 BE width, u16 BE height, RGBA bytes (16x16 tiles, 64 per row; tile 0 is white)
//!   types.bin   "HZTY", u32 BE count, then per type: u16 top, u16 side, u16 bottom (tile ids), u8 flags, u8 0
//!   states.bin  "HZS2", u32 BE count, then per block state id: u16 BE type id (0 = not drawn)
//!   hotbar.bin  "HZHB", then nine u16 BE type ids for the hotbar
//! flags: 1 opaque cube, 2 tint the top (grass), 4 tint all faces (foliage), 8 tint all faces (water), 16 liquid.

#[path = "../../horizon-wiiu/src/atlas.rs"]
#[allow(dead_code)]
mod atlas;
#[path = "../../horizon-wiiu/src/loader.rs"]
#[allow(dead_code)]
mod loader;
#[path = "../../horizon-wiiu/src/types.rs"]
#[allow(dead_code)]
mod types;
#[path = "../../horizon-wiiu/src/world.rs"]
#[allow(dead_code)]
mod world;

use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::collections::HashMap;
use std::path::Path;

const TILE: usize = 16;
const PER_ROW: usize = 64;

struct Atlas {
    tiles: Vec<[u8; TILE * TILE * 4]>,
    by_name: HashMap<String, u16>,
    dir: String,
}

impl Atlas {
    fn tile(&mut self, name: &str) -> Option<u16> {
        if let Some(&t) = self.by_name.get(name) {
            return Some(t);
        }
        let path = format!("{}/{name}.png", self.dir);
        if !Path::new(&path).exists() {
            return None;
        }
        let mut decoder = png::Decoder::new(std::fs::File::open(&path).ok()?);
        // palette and 16-bit images become plain 8-bit RGB(A)/grey
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let channels = match info.color_type {
            png::ColorType::Rgba => 4,
            png::ColorType::Rgb => 3,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Grayscale => 1,
            _ => return None, // indexed: not used by block textures
        };
        let frame = w.min(h); // an animation strip: the first square frame
        let px = |x: usize, y: usize| -> [u8; 4] {
            let o = (y * w + x) * channels;
            match channels {
                4 => [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]],
                3 => [buf[o], buf[o + 1], buf[o + 2], 255],
                2 => [buf[o], buf[o], buf[o], buf[o + 1]],
                _ => [buf[o], buf[o], buf[o], 255],
            }
        };
        let mut tile = [0u8; TILE * TILE * 4];
        let (mut sum, mut n) = ([0u32; 3], 0u32);
        for y in 0..TILE {
            for x in 0..TILE {
                let p = px(x * frame / TILE, y * frame / TILE);
                tile[(y * TILE + x) * 4..][..4].copy_from_slice(&p);
                if p[3] >= 128 {
                    for c in 0..3 {
                        sum[c] += p[c] as u32;
                    }
                    n += 1;
                }
            }
        }
        // no alpha test in the console shader: see-through pixels become a dark version of the tile's own colour
        let fill = if name.contains("leaves") { [28, 66, 14] } else if n > 0 { [(sum[0] / n / 2) as u8, (sum[1] / n / 2) as u8, (sum[2] / n / 2) as u8] } else { [0, 0, 0] };
        for i in 0..TILE * TILE {
            if tile[i * 4 + 3] < 128 {
                tile[i * 4..i * 4 + 3].copy_from_slice(&fill);
            }
            tile[i * 4 + 3] = 255;
        }
        let id = self.tiles.len() as u16;
        self.tiles.push(tile);
        self.by_name.insert(name.to_string(), id);
        Some(id)
    }

    fn first(&mut self, names: &[String]) -> Option<u16> {
        names.iter().find_map(|n| self.tile(n))
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (prepared, textures, out) = (args.next().expect("prepared"), args.next().expect("textures"), args.next().expect("out"));
    std::fs::create_dir_all(&out).unwrap();
    let (registry, _) = BlockRegistry::from_facts_json_bytes(&std::fs::read(format!("{prepared}/facts.json")).unwrap()).unwrap();

    let mut atlas = Atlas { tiles: vec![[255u8; TILE * TILE * 4]], by_name: HashMap::new(), dir: textures };
    let mut types: Vec<(u16, u16, u16, u8)> = vec![(0, 0, 0, 0)];
    let mut type_of: HashMap<(u16, u16, u16, u8), u16> = HashMap::new();
    let mut states = Vec::with_capacity(registry.len());
    let mut by_block: HashMap<String, u16> = HashMap::new();

    for id in 0..registry.len() {
        let name = registry.state(BlockStateId(id as u32)).map(|s| s.name.clone()).unwrap_or_default();
        let key = name.strip_prefix("minecraft:").unwrap_or(&name).to_string();
        if let Some(&t) = by_block.get(&key) {
            states.push(t);
            continue;
        }
        let n = key.as_str();
        let s = |x: &str| vec![x.to_string()];
        let t: u16 = if n == "air" || n == "cave_air" || n == "void_air" || loader::not_a_cube(n) {
            0
        } else {
            let (top, side, bottom, flags): (Vec<String>, Vec<String>, Vec<String>, u8) = if n == "water" || n == "bubble_column" {
                (s("water_still"), s("water_still"), s("water_still"), 8 | 16)
            } else if n == "lava" {
                (s("lava_still"), s("lava_still"), s("lava_still"), 16)
            } else if n == "grass_block" {
                (s("grass_block_top"), s("grass_block_side"), s("dirt"), 1 | 2)
            } else if n == "podzol" || n == "mycelium" {
                (vec![format!("{n}_top")], vec![format!("{n}_side")], s("dirt"), 1)
            } else if n.ends_with("_leaves") {
                (s(n), s(n), s(n), 1 | 4)
            } else if let Some(base) = n.strip_suffix("_wood").map(|b| format!("{b}_log")) {
                (vec![base.clone()], vec![base.clone()], vec![base], 1)
            } else {
                (
                    vec![format!("{n}_top"), n.to_string(), format!("{n}_side"), format!("{n}_front")],
                    vec![format!("{n}_side"), n.to_string(), format!("{n}_front"), format!("{n}_top")],
                    vec![format!("{n}_bottom"), n.to_string(), format!("{n}_side"), format!("{n}_top")],
                    1,
                )
            };
            match (atlas.first(&top), atlas.first(&side), atlas.first(&bottom)) {
                (Some(a), Some(b), Some(c)) => {
                    let k = (a, b, c, flags);
                    let next = types.len() as u16;
                    *type_of.entry(k).or_insert_with(|| {
                        types.push(k);
                        next
                    })
                }
                _ => 0,
            }
        };
        by_block.insert(key, t);
        states.push(t);
    }
    // the per-state table repeats a block's type for each of its states (blocks are numbered contiguously)
    let hot = ["grass_block", "dirt", "stone", "sand", "oak_planks", "oak_log", "oak_leaves", "ice", "gravel"];
    let hotbar: Vec<u16> = hot.iter().map(|h| by_block.get(*h).copied().unwrap_or(0)).collect();

    let rows = atlas.tiles.len().div_ceil(PER_ROW);
    let (w, h) = (PER_ROW * TILE, rows * TILE);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, t) in atlas.tiles.iter().enumerate() {
        let (tx, ty) = (i % PER_ROW, i / PER_ROW);
        for y in 0..TILE {
            let dst = ((ty * TILE + y) * w + tx * TILE) * 4;
            rgba[dst..dst + TILE * 4].copy_from_slice(&t[y * TILE * 4..(y + 1) * TILE * 4]);
        }
    }
    let mut f = b"HZAT".to_vec();
    f.extend((w as u16).to_be_bytes());
    f.extend((h as u16).to_be_bytes());
    f.extend(&rgba);
    std::fs::write(format!("{out}/atlas.rgba"), f).unwrap();

    let mut f = b"HZTY".to_vec();
    f.extend((types.len() as u32).to_be_bytes());
    for (a, b, c, fl) in &types {
        for v in [a, b, c] {
            f.extend(v.to_be_bytes());
        }
        f.extend([*fl, 0]);
    }
    std::fs::write(format!("{out}/types.bin"), f).unwrap();

    let mut f = b"HZS2".to_vec();
    f.extend((states.len() as u32).to_be_bytes());
    for t in &states {
        f.extend(t.to_be_bytes());
    }
    std::fs::write(format!("{out}/states.bin"), f).unwrap();

    let mut f = b"HZHB".to_vec();
    for t in &hotbar {
        f.extend(t.to_be_bytes());
    }
    std::fs::write(format!("{out}/hotbar.bin"), f).unwrap();
    println!("{} states, {} block types, {} textures ({}x{} atlas), hotbar {:?}", states.len(), types.len(), atlas.tiles.len(), w, h, hotbar);
}
