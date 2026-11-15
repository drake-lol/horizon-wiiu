//! Loads a saved Minecraft world (Anvil `region/r.X.Z.mca` files) into the viewer's dense block array,
//! through horizon-world's region and chunk readers. Looks for `HORIZON_WORLD`, then the SD card's
//! `/vol/external01/horizon/world/region`, then a region embedded at build time.

use crate::world::{Block, World, CHUNK};
use horizon_core::position::ChunkPos;
use horizon_world::chunk::{BlockState, PalettedContainer};
use horizon_world::nbt::{Limits, Tag};
use horizon_world::region::RegionFile;
use std::collections::HashMap;

/// A region baked into the build, so the viewer has a world with no SD card (made by `wiiu/mkworld`).
static EMBEDDED: Option<&[u8]> = None;

const MAX_CHUNKS: i32 = 8;
const MAX_HEIGHT: usize = 128;

/// Plants, partial blocks and other things a plain cube would draw wrongly.
fn not_a_cube(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "air", "grass", "fern", "flower", "sapling", "vine", "kelp", "seagrass", "bush", "torch", "rail", "carpet", "door", "fence",
        "wall", "pane", "bars", "ladder", "sign", "button", "plate", "lever", "slab", "stairs", "cobweb", "fire", "banner", "bed",
        "tulip", "rose", "poppy", "dandelion", "mushroom", "lantern", "chain", "rod", "head", "skull", "pot", "candle", "coral_fan",
        "lily", "snow_layer", "moss_carpet", "hanging", "dripstone", "amethyst_cluster", "bud", "wire", "repeater", "comparator",
    ];
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    // "grass_block" and "snow_block" are cubes; everything else matching the list is not
    if n == "grass_block" || n == "snow_block" || n == "mushroom_block" || n.ends_with("_mushroom_block") || n == "bone_block" {
        return false;
    }
    SKIP.iter().any(|s| n.contains(s))
}

pub fn block_for(name: &str) -> Block {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if not_a_cube(n) {
        return Block::Air;
    }
    if n.contains("water") || n == "bubble_column" {
        Block::Water
    } else if n.contains("lava") {
        Block::Lava
    } else if n == "grass_block" {
        Block::Grass
    } else if n == "bedrock" {
        Block::Bedrock
    } else if n.contains("sandstone") {
        Block::Sandstone
    } else if n.ends_with("sand") {
        Block::Sand
    } else if n == "gravel" {
        Block::Gravel
    } else if n.contains("snow") {
        Block::Snow
    } else if n.contains("ice") {
        Block::Ice
    } else if n.ends_with("_log") || n.ends_with("_wood") || n.ends_with("_stem") || n.ends_with("_hyphae") {
        Block::Log
    } else if n.ends_with("_leaves") {
        Block::Leaves
    } else if n.ends_with("_planks") {
        Block::Planks
    } else if matches!(n, "dirt" | "coarse_dirt" | "rooted_dirt" | "mud" | "podzol" | "mycelium" | "farmland" | "dirt_path" | "clay") {
        Block::Dirt
    } else {
        Block::Stone
    }
}

fn region_files(dir: &str) -> Vec<(i32, i32, String)> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else { return out };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 4 && parts[0] == "r" && parts[3] == "mca" {
            if let (Ok(x), Ok(z)) = (parts[1].parse(), parts[2].parse()) {
                out.push((x, z, format!("{dir}/{name}")));
            }
        }
    }
    out.sort();
    out
}

pub fn load() -> Option<World> {
    let dirs = [std::env::var("HORIZON_WORLD").ok(), Some("/vol/external01/horizon/world/region".to_string())];
    for dir in dirs.into_iter().flatten() {
        let files = region_files(&dir);
        println!("loader: {dir}: {} region file(s) ({:?})", files.len(), std::fs::read_dir(&dir).map(|_| "ok").map_err(|e| e.to_string()));
        if let Some((rx, rz, path)) = files.first() {
            println!("loader: reading {path}");
            return load_region(path, *rx, *rz);
        }
    }
    let _ = EMBEDDED;
    None
}

pub fn load_region(path: &str, rx: i32, rz: i32) -> Option<World> {
    let mut region = RegionFile::open(path).ok()?;
    // pass 1: find which chunks exist and the lowest / highest non-air sections
    let mut chunks = Vec::new();
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    for cz in 0..MAX_CHUNKS {
        for cx in 0..MAX_CHUNKS {
            let pos = ChunkPos::new(rx * 32 + cx, rz * 32 + cz);
            let record = match region.read(pos) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    println!("loader: chunk {cx},{cz}: absent");
                    continue;
                }
                Err(e) => {
                    println!("loader: chunk {cx},{cz}: read error {e}");
                    continue;
                }
            };
            let root = match record.nbt(Limits::default()) {
                Ok(r) => r,
                Err(e) => {
                    println!("loader: chunk {cx},{cz}: nbt error {e} ({} compressed bytes)", record.compressed.len());
                    continue;
                }
            };
            let Some(Tag::List(sections)) = root.tag.get("sections") else {
                println!("loader: chunk {cx},{cz}: no sections list");
                continue;
            };
            for s in sections {
                let Some(Tag::Byte(y)) = s.get("Y") else { continue };
                let air_only = match s.get("block_states").and_then(|c| PalettedContainer::from_nbt(c, 4096, 4).ok()) {
                    Some(c) => c.palette_len() == 1,
                    None => true,
                };
                if !air_only {
                    lo = lo.min(*y as i32);
                    hi = hi.max(*y as i32);
                }
            }
            chunks.push((cx, cz, root));
        }
    }
    println!("loader: {} chunk(s), sections {lo}..={hi}", chunks.len());
    if chunks.is_empty() || lo > hi {
        return None;
    }
    let sy = (((hi - lo + 1) * 16) as usize).min(MAX_HEIGHT);
    let mut world = World::empty(MAX_CHUNKS as usize * CHUNK, sy, MAX_CHUNKS as usize * CHUNK, lo * 16);
    let mut names: HashMap<usize, Block> = HashMap::new();
    for (cx, cz, root) in &chunks {
        let Some(Tag::List(sections)) = root.tag.get("sections") else { continue };
        for s in sections {
            let Some(Tag::Byte(y)) = s.get("Y") else { continue };
            let base_y = (*y as i32 - lo) * 16;
            if base_y < 0 || base_y as usize >= sy {
                continue;
            }
            let Some(container) = s.get("block_states").and_then(|c| PalettedContainer::from_nbt(c, 4096, 4).ok()) else { continue };
            names.clear();
            for i in 0..4096 {
                let Ok(tag) = container.get(i) else { continue };
                let key = tag as *const Tag as usize;
                let block = *names.entry(key).or_insert_with(|| BlockState::from_nbt(tag).map(|b| block_for(&b.name)).unwrap_or(Block::Air));
                if block == Block::Air {
                    continue;
                }
                let (x, yy, z) = (i & 15, (i >> 8) & 15, (i >> 4) & 15);
                let wy = base_y as usize + yy;
                if wy < sy {
                    world.set(*cx as usize * CHUNK + x, wy, *cz as usize * CHUNK + z, block);
                }
            }
        }
    }
    Some(world)
}
