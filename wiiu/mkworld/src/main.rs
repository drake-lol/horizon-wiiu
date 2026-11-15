//! Writes `r.0.0.mca` from the Wii U client's procedural terrain, as real chunk NBT (sections with paletted
//! block states), so the region reader can be tested without a Minecraft install.
//! usage: mkworld <output-dir>

#[path = "../../horizon-wiiu/src/atlas.rs"]
#[allow(dead_code)]
mod atlas;
#[path = "../../horizon-wiiu/src/loader.rs"]
#[allow(dead_code)]
mod loader;
#[path = "../../horizon-wiiu/src/world.rs"]
#[allow(dead_code)]
mod world;

use horizon_core::position::{BlockPos, ChunkPos};
use horizon_world::chunk::{BlockState, PalettedContainer};
use horizon_world::compression::Compression;
use horizon_world::nbt::{Limits, NamedTag, Tag};
use horizon_world::region::{ChunkRecord, RegionWriter};

fn state(name: &str, props: &[(&str, &str)]) -> Tag {
    BlockState { name: name.to_string(), properties: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }.to_nbt()
}

fn block_tag(b: world::Block) -> Tag {
    use world::Block::*;
    match b {
        Air => state("minecraft:air", &[]),
        Stone => state("minecraft:stone", &[]),
        Dirt => state("minecraft:dirt", &[]),
        Grass => state("minecraft:grass_block", &[("snowy", "false")]),
        Sand => state("minecraft:sand", &[]),
        Water => state("minecraft:water", &[("level", "0")]),
        Snow => state("minecraft:snow_block", &[]),
        Log => state("minecraft:oak_log", &[("axis", "y")]),
        Leaves => state("minecraft:oak_leaves", &[("distance", "1"), ("persistent", "false"), ("waterlogged", "false")]),
        Planks => state("minecraft:oak_planks", &[]),
        Gravel => state("minecraft:gravel", &[]),
        Sandstone => state("minecraft:sandstone", &[]),
        Ice => state("minecraft:ice", &[]),
        Lava => state("minecraft:lava", &[("level", "0")]),
        Bedrock => state("minecraft:bedrock", &[]),
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: mkworld <output-dir>");
    if dir == "--mesh" { mesh_check(); return; }
    if dir == "--load" {
        let path = std::env::args().nth(2).expect("--load <r.X.Z.mca>");
        let w = loader::load_region(&path, 0, 0).expect("load failed");
        let mut counts = std::collections::BTreeMap::new();
        for y in 0..w.sy as i32 { for z in 0..w.sz as i32 { for x in 0..w.sx as i32 {
            *counts.entry(format!("{}", w.get(x, y, z) as u8)).or_insert(0usize) += 1;
        } } }
        println!("loaded {}x{}x{} y0={}: block-id counts {:?}", w.sx, w.sy, w.sz, w.y0, counts);
        return;
    }
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::path::Path::new(&dir).join("r.0.0.mca");
    let _ = std::fs::remove_file(&path);
    let world = world::World::generate();
    let mut writer = RegionWriter::create(&path).unwrap();
    for cz in 0..8i32 {
        for cx in 0..8i32 {
            let mut sections = Vec::new();
            for sy in 0..(world.sy / 16) as i32 {
                let mut container = PalettedContainer::uniform(state("minecraft:air", &[]), 4096, 4).unwrap();
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let b = world.get(cx * 16 + x, sy * 16 + y, cz * 16 + z);
                            if b != world::Block::Air {
                                let idx = BlockPos::new(x, y, z).section_index();
                                container.set(idx, block_tag(b)).unwrap();
                            }
                        }
                    }
                }
                container.compact().unwrap();
                sections.push(Tag::compound([
                    ("Y", Tag::Byte(sy as i8)),
                    ("block_states", container.to_nbt()),
                    ("biomes", Tag::compound([("palette", Tag::List(vec![Tag::String("minecraft:plains".into())]))])),
                ]));
            }
            let root = NamedTag {
                name: "".into(),
                tag: Tag::compound([
                    ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
                    ("xPos", Tag::Int(cx)),
                    ("zPos", Tag::Int(cz)),
                    ("yPos", Tag::Int(0)),
                    ("Status", Tag::String("minecraft:full".into())),
                    ("sections", Tag::List(sections)),
                ]),
            };
            let record = ChunkRecord::from_nbt(&root, Compression::Zlib, 0, Limits::default()).unwrap();
            writer.write(ChunkPos::new(cx, cz), &record).unwrap();
        }
    }
    writer.finish().unwrap();
    println!("wrote {}", path.display());
}

#[allow(dead_code)]
fn mesh_check() {
    let world = world::World::generate();
    let t = std::time::Instant::now();
    let v = world.mesh_chunk(0, 0);
    println!("mesh 0,0: {} verts in {:?}", v.len(), t.elapsed());
}
