//! usage: mkstates <prepared 26.3 folder> <output states.bin>
//! Reads `facts.json` (the player's own prepared data, derived from their Minecraft) and writes one byte per block
//! state id: the viewer's `Block` for it (0 = not drawn as a cube). 190,000 states become ~190 KB, so the console
//! never loads the registry itself. Layout: "HZST", u32 big-endian count, then the bytes.

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

fn main() {
    let mut args = std::env::args().skip(1);
    let (dir, out) = (args.next().expect("prepared folder"), args.next().expect("output file"));
    let bytes = std::fs::read(format!("{dir}/facts.json")).expect("facts.json");
    let (registry, _) = BlockRegistry::from_facts_json_bytes(&bytes).expect("parse facts.json");
    let n = registry.len();
    let mut table = Vec::with_capacity(n);
    let mut counts = [0usize; 32];
    for id in 0..n {
        let name = registry.state(BlockStateId(id as u32)).map(|s| s.name.clone()).unwrap_or_default();
        let b = loader::block_for(&name) as u8;
        counts[b as usize] += 1;
        table.push(b);
    }
    let mut file = b"HZST".to_vec();
    file.extend_from_slice(&(n as u32).to_be_bytes());
    file.extend_from_slice(&table);
    std::fs::write(&out, &file).expect("write");
    println!("{n} states -> {} bytes; first states: {:?}", file.len(), (0..4).map(|i| registry.state(BlockStateId(i)).map(|s| s.name.clone())).collect::<Vec<_>>());
    println!("by block id: {:?}", &counts[..16]);
}
