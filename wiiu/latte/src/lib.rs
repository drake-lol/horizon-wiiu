//! Latte (Wii U GPU7, an R700 derivative) shader machine code, built from instructions instead of text, so shaders
//! can be compiled on the console itself. Layout and encodings follow decaf-emu's `latte_instructions.h` and its
//! assembler's clause placement (`assembler_cf.cpp`): control flow first, ALU clauses from a 256-byte boundary, then
//! texture clauses from the next 128-byte boundary. Words are emitted in the byte order GX2 programs use (see tests).

/// An ALU operand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Src {
    /// R0..R127, channel 0..3 (x, y, z, w).
    Gpr(u8, u8),
    /// C0..C255 (uniform registers), channel.
    Const(u16, u8),
    /// The previous group's vector result for this channel (PV.x..w).
    Pv(u8),
    /// The previous group's scalar (trans) result.
    Ps,
    /// 0.0, 1.0, 0.5 as inline constants.
    Zero,
    One,
    Half,
    /// A 32-bit literal, stored after the group (at most 4 per group).
    Lit(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operand {
    pub src: Src,
    pub neg: bool,
    pub abs: bool,
}

impl From<Src> for Operand {
    fn from(src: Src) -> Self {
        Operand { src, neg: false, abs: false }
    }
}

/// Two-operand ALU opcodes (SQ_OP2_INST).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op2 {
    Add = 0x00,
    Mul = 0x01,
    MulIeee = 0x02,
    Max = 0x03,
    Min = 0x04,
    SetE = 0x08,
    SetGt = 0x09,
    SetGe = 0x0A,
    SetNe = 0x0B,
    Fract = 0x10,
    Trunc = 0x11,
    Ceil = 0x12,
    Floor = 0x14,
    Mov = 0x19,
    Nop = 0x1A,
    KillE = 0x2C,
    KillGt = 0x2D,
    KillGe = 0x2E,
    KillNe = 0x2F,
    Dot4 = 0x50,
    Dot4Ieee = 0x51,
    ExpIeee = 0x61,
    LogIeee = 0x63,
    RecipIeee = 0x66,
    RecipSqrtIeee = 0x69,
    SqrtIeee = 0x6A,
    Sin = 0x6E,
    Cos = 0x6F,
}

/// Three-operand ALU opcodes (SQ_OP3_INST).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op3 {
    MulAdd = 0x10,
    CndE = 0x18,
    CndGt = 0x19,
    CndGe = 0x1A,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AluOp {
    Op2(Op2, [Operand; 2]),
    Op3(Op3, [Operand; 3]),
}

/// One ALU instruction in a group: which slot (x, y, z, w = 0..3, trans = 4), what, and where it writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alu {
    pub slot: u8,
    pub op: AluOp,
    /// Destination register and channel; `write: false` keeps the result in PV/PS only (the `____` mask).
    pub dst: (u8, u8),
    pub write: bool,
    pub clamp: bool,
}

/// A group of up to five instructions issued together.
pub type Group = Vec<Alu>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sel {
    X = 0,
    Y = 1,
    Z = 2,
    W = 3,
    Zero = 4,
    One = 5,
    Mask = 7,
}

/// A texture fetch: `SAMPLE dst.sel, src.sel, tN, sN`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tex {
    pub inst: u8,
    pub dst: u8,
    pub dst_sel: [Sel; 4],
    pub src: u8,
    pub src_sel: [Sel; 4],
    pub resource: u8,
    pub sampler: u8,
    /// Coordinates in 0..1 (COORD_TYPE normalized on all four), as for every `textureSample`.
    pub normalized: bool,
}

pub const TEX_SAMPLE: u8 = 0x10;
pub const TEX_SAMPLE_L: u8 = 0x11;
pub const TEX_SAMPLE_LZ: u8 = 0x13;

/// Export targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// PIX0..: pixel colour outputs.
    Pixel(u8),
    /// POS0..: vertex position.
    Pos(u8),
    /// PARAM0..: interpolated parameters.
    Param(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cf {
    CallFs { barrier: bool },
    Alu(Vec<Group>),
    /// A texture clause; `valid_pixel` restricts it to covered pixels (VALID_PIX, for derivatives in pixel shaders).
    Tex { fetches: Vec<Tex>, valid_pixel: bool },
    /// EXP or EXP_DONE of a register with a swizzle.
    Export { target: Target, gpr: u8, sel: [Sel; 4], done: bool, barrier: bool },
}

const ALU_CLAUSE_ALIGN_SLOTS: usize = 256 / 8;
const TEX_CLAUSE_ALIGN_SLOTS: usize = 128 / 8;

fn align_up(v: usize, to: usize) -> usize {
    v.div_ceil(to) * to
}

fn src_sel(s: Src) -> (u32, u32) {
    match s {
        Src::Gpr(r, c) => (r as u32, c as u32),
        Src::Const(n, c) => (256 + n as u32, c as u32),
        Src::Pv(c) => (254, c as u32),
        Src::Ps => (255, 0),
        Src::Zero => (248, 0),
        Src::One => (249, 0),
        Src::Half => (252, 0),
        Src::Lit(_) => (253, 0),
    }
}

/// Encode one ALU group to 64-bit slots (pairs of words), literals appended as (lo, hi) pairs.
fn encode_group(group: &[Alu]) -> Vec<[u32; 2]> {
    let mut literals: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    let mut sorted: Vec<&Alu> = group.iter().collect();
    sorted.sort_by_key(|a| a.slot);
    for (i, a) in sorted.iter().enumerate() {
        let last = i == sorted.len() - 1;
        let mut lit_chan = |o: &Operand| -> (u32, u32) {
            let (sel, chan) = src_sel(o.src);
            if let Src::Lit(v) = o.src {
                let idx = literals.iter().position(|&l| l == v).unwrap_or_else(|| {
                    literals.push(v);
                    literals.len() - 1
                });
                return (sel, idx as u32);
            }
            (sel, chan)
        };
        let ops: Vec<Operand> = match a.op {
            AluOp::Op2(_, o) => o.to_vec(),
            AluOp::Op3(_, o) => o.to_vec(),
        };
        let (s0, c0) = lit_chan(&ops[0]);
        let (s1, c1) = if ops.len() > 1 { lit_chan(&ops[1]) } else { (0, 0) };
        let mut w0 = s0 | (c0 << 10) | ((ops[0].neg as u32) << 12) | (s1 << 13);
        if ops.len() > 1 {
            w0 |= (c1 << 23) | ((ops[1].neg as u32) << 25);
        }
        if last {
            w0 |= 1 << 31;
        }
        let mut w1 = ((a.dst.0 as u32) << 21) | ((a.dst.1 as u32) << 29) | ((a.clamp as u32) << 31);
        match a.op {
            AluOp::Op2(op, o) => {
                w1 |= (o[0].abs as u32) | ((o[1].abs as u32) << 1) | ((a.write as u32) << 4) | ((op as u32) << 7);
            }
            AluOp::Op3(op, o) => {
                let (s2, c2) = lit_chan(&o[2]);
                w1 |= s2 | (c2 << 10) | ((o[2].neg as u32) << 12) | ((op as u32) << 13);
            }
        }
        out.push([w0, w1]);
    }
    // literals fill whole 64-bit slots
    while literals.len() % 2 != 0 {
        literals.push(0);
    }
    for pair in literals.chunks(2) {
        out.push([pair[0], pair[1]]);
    }
    out
}

fn sel_bits(s: &[Sel; 4]) -> u32 {
    (s[0] as u32) | ((s[1] as u32) << 3) | ((s[2] as u32) << 6) | ((s[3] as u32) << 9)
}

/// Assemble a program to its 32-bit words, in program order.
pub fn assemble(cf: &[Cf]) -> Vec<u32> {
    // clause bodies first, to know their sizes and addresses
    let mut alu_data: Vec<[u32; 2]> = Vec::new();
    let mut tex_data: Vec<[u32; 4]> = Vec::new();
    let mut alu_at = Vec::new();
    let mut tex_at = Vec::new();
    for c in cf {
        match c {
            Cf::Alu(groups) => {
                let start = alu_data.len();
                for g in groups {
                    alu_data.extend(encode_group(g));
                }
                alu_at.push((start, alu_data.len() - start));
            }
            Cf::Tex { fetches: t, .. } => {
                let start = tex_data.len();
                for f in t {
                    let w0 = (f.inst as u32) | ((f.resource as u32) << 8) | ((f.src as u32) << 16);
                    let w1 = (f.dst as u32) | (sel_bits(&f.dst_sel) << 9) | if f.normalized { 0xF << 28 } else { 0 };
                    let w2 = ((f.sampler as u32) << 15) | (sel_bits(&f.src_sel) << 20);
                    tex_data.push([w0, w1, w2, 0]);
                }
                tex_at.push((start, tex_data.len() - start));
            }
            _ => {}
        }
    }
    let alu_base = align_up(cf.len(), ALU_CLAUSE_ALIGN_SLOTS);
    let tex_base = align_up(alu_base + alu_data.len(), TEX_CLAUSE_ALIGN_SLOTS);
    let (mut ai, mut ti) = (0, 0);
    let mut words = Vec::new();
    for (i, c) in cf.iter().enumerate() {
        let end = i == cf.len() - 1;
        let (w0, w1) = match c {
            Cf::CallFs { barrier } => (0, (0x13 << 23) | ((end as u32) << 21) | ((*barrier as u32) << 31)),
            Cf::Alu(_) => {
                let (start, n) = alu_at[ai];
                ai += 1;
                let w0 = (alu_base + start) as u32;
                let w1 = (((n - 1) as u32) << 18) | (0x8 << 26) | (1 << 31);
                (w0, w1)
            }
            Cf::Tex { valid_pixel, .. } => {
                let (start, n) = tex_at[ti];
                ti += 1;
                let w0 = (tex_base + start * 2) as u32;
                let w1 = (((n - 1) as u32) << 10)
                    | (0x1 << 23)
                    | (1 << 31)
                    | ((end as u32) << 21)
                    | ((*valid_pixel as u32) << 22);
                (w0, w1)
            }
            Cf::Export { target, gpr, sel, done, barrier } => {
                let (ty, base) = match target {
                    Target::Pixel(n) => (0u32, *n as u32),
                    Target::Pos(n) => (1u32, 60 + *n as u32),
                    Target::Param(n) => (2u32, *n as u32),
                };
                let w0 = base | (ty << 13) | ((*gpr as u32) << 15);
                let inst = if *done { 0x28 } else { 0x27 };
                let w1 = sel_bits(sel) | (inst << 23) | ((*barrier as u32) << 31) | ((end as u32) << 21);
                (w0, w1)
            }
        };
        words.push(w0);
        words.push(w1);
    }
    words.resize(alu_base * 2, 0);
    for [a, b] in &alu_data {
        words.push(*a);
        words.push(*b);
    }
    if !tex_data.is_empty() {
        words.resize(tex_base * 2, 0);
        for t in &tex_data {
            words.extend_from_slice(t);
        }
    }
    words
}

/// Program words as GX2 stores them (little-endian bytes).
pub fn to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
