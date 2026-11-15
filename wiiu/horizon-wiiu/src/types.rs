//! Block types: what each id stored in the world looks like. With the player's own prepared data on the SD card
//! (`atlas.rgba`, `types.bin`, `hotbar.bin`, made by `wiiu/mkatlas`) there are hundreds of types with real textures;
//! without it, the 15 placeholder blocks and their generated atlas.

use crate::atlas;
use crate::world::Block;

pub const OPAQUE: u8 = 1;
pub const TINT_TOP: u8 = 2;
pub const TINT_FOLIAGE: u8 = 4;
pub const TINT_WATER: u8 = 8;
pub const LIQUID: u8 = 16;
/// A full cube with see-through texels (leaves, glass): drawn, solid to walk on, but it hides no neighbour.
pub const CUTOUT: u8 = 32;
/// Two crossed quads (grass, flowers, saplings).
pub const CROSS: u8 = 64;
/// Tinted with the grass colour on every face (short grass, ferns).
pub const TINT_GRASS: u8 = 128;

#[derive(Clone, Copy, Default)]
pub struct TypeInfo {
    pub top: u16,
    pub side: u16,
    pub bottom: u16,
    pub flags: u8,
}

#[derive(Clone)]
pub struct Types {
    pub list: Vec<TypeInfo>,
    pub hotbar: [u16; 9],
}

pub struct Loaded {
    pub types: Types,
    pub atlas_width: usize,
    pub atlas_height: usize,
    pub per_row: usize,
    pub rgba: Vec<u8>,
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

impl Types {
    /// The placeholder blocks, ids equal to `Block`'s discriminants.
    pub fn builtin() -> Loaded {
        let mut list = vec![TypeInfo::default(); 15];
        for v in 1..15u8 {
            let b = Block::from_u8(v);
            let mut flags = 0;
            if b.solid() {
                flags |= OPAQUE;
            }
            if b == Block::Water {
                flags |= LIQUID;
            }
            if b == Block::Lava {
                flags |= LIQUID;
            }
            list[v as usize] = TypeInfo { top: b.tile_for(true, false) as u16, side: b.tile_for(false, false) as u16, bottom: b.tile_for(false, true) as u16, flags };
        }
        let hot = [Block::Grass, Block::Dirt, Block::Stone, Block::Sand, Block::Planks, Block::Log, Block::Leaves, Block::Ice, Block::Gravel];
        let mut hotbar = [0u16; 9];
        for (i, b) in hot.iter().enumerate() {
            hotbar[i] = *b as u16;
        }
        atlas::set_geometry(atlas::TILES_PER_ROW, atlas::WIDTH, atlas::HEIGHT, atlas::WHITE);
        Loaded { types: Types { list, hotbar }, atlas_width: atlas::WIDTH, atlas_height: atlas::HEIGHT, per_row: atlas::TILES_PER_ROW, rgba: atlas::build() }
    }

    /// The real atlas and types from the SD card, if all four files are there and agree.
    pub fn from_sd() -> Option<Loaded> {
        let dir = "/vol/external01/horizon";
        let atlas_file = std::fs::read(format!("{dir}/atlas.rgba")).ok()?;
        let types_file = std::fs::read(format!("{dir}/types.bin")).ok()?;
        let hot_file = std::fs::read(format!("{dir}/hotbar.bin")).ok()?;
        if atlas_file.len() < 8 || &atlas_file[..4] != b"HZAT" || types_file.len() < 8 || &types_file[..4] != b"HZTY" || hot_file.len() < 22 || &hot_file[..4] != b"HZHB" {
            return None;
        }
        let (w, h) = (be16(&atlas_file, 4) as usize, be16(&atlas_file, 6) as usize);
        let rgba = atlas_file[8..].to_vec();
        if rgba.len() != w * h * 4 {
            return None;
        }
        let n = u32::from_be_bytes([types_file[4], types_file[5], types_file[6], types_file[7]]) as usize;
        if types_file.len() < 8 + n * 8 {
            return None;
        }
        let list = (0..n)
            .map(|i| {
                let o = 8 + i * 8;
                TypeInfo { top: be16(&types_file, o), side: be16(&types_file, o + 2), bottom: be16(&types_file, o + 4), flags: types_file[o + 6] }
            })
            .collect();
        let mut hotbar = [0u16; 9];
        for (i, slot) in hotbar.iter_mut().enumerate() {
            *slot = be16(&hot_file, 4 + i * 2);
        }
        let per_row = 64;
        atlas::set_geometry(per_row, w, h, 0);
        Some(Loaded { types: Types { list, hotbar }, atlas_width: w, atlas_height: h, per_row, rgba })
    }

    pub fn info(&self, t: u16) -> TypeInfo {
        self.list.get(t as usize).copied().unwrap_or_default()
    }

    pub fn opaque(&self, t: u16) -> bool {
        t != 0 && self.info(t).flags & OPAQUE != 0
    }

    /// Blocks the player stands on and cannot walk through.
    pub fn solid(&self, t: u16) -> bool {
        t != 0 && self.info(t).flags & (OPAQUE | CUTOUT) != 0
    }

    pub fn liquid(&self, t: u16) -> bool {
        t != 0 && self.info(t).flags & LIQUID != 0
    }
}
