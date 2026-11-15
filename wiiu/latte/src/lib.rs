//! Latte (Wii U GPU7, an R700 derivative) shader machine code, built from instructions instead of text, so shaders
//! can be compiled on the console itself. Layout and encodings follow decaf-emu's `latte_instructions.h` and
//! `latte_enum_sq.h`, and its assembler's clause placement: control flow first, ALU clauses from a 256-byte
//! boundary, then texture and vertex-fetch clauses from the next 128-byte boundary. The tests compare the output
//! with decaf's `latte-assembler` byte for byte.

pub mod op;

/// An ALU operand source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    /// R0..R127, channel 0..3 (x, y, z, w).
    Gpr(u8, u8),
    /// C0..C255 (uniform registers, UniformRegister mode), channel.
    Const(u16, u8),
    /// Uniform block constant through the clause's kcache lock: bank 0..1, index 0..31 inside the lock, channel.
    Kcache(u8, u8, u8),
    /// The previous group's vector result for this channel (PV.x..w).
    Pv(u8),
    /// The previous group's scalar (trans) result.
    Ps,
    Zero,
    One,
    Half,
    IntOne,
    IntMinusOne,
    /// A 32-bit literal, stored after the group (at most 4 different ones per group).
    Lit(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operand {
    pub src: Src,
    pub neg: bool,
    pub abs: bool,
    /// Address the GPR or constant relative to an index register (AR.x after MOVA, or the loop index).
    pub rel: bool,
}

impl From<Src> for Operand {
    fn from(src: Src) -> Self {
        Operand { src, neg: false, abs: false, rel: false }
    }
}

impl Operand {
    pub fn neg(self) -> Self {
        Operand { neg: !self.neg, ..self }
    }
    pub fn abs(self) -> Self {
        Operand { abs: true, neg: false, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AluOp {
    /// SQ_OP2_INST opcode (see [`op`]); unary ops ignore the second operand.
    Op2(u32, [Operand; 2]),
    /// SQ_OP3_INST opcode (no abs on any operand).
    Op3(u32, [Operand; 3]),
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
    /// For PRED_SET*: update the execute mask / predicate.
    pub update_exec: bool,
    pub update_pred: bool,
    /// Bank swizzle (VEC_012.. for slots 0-3, SCL_210.. for trans); 0 is the default.
    pub bank_swizzle: u8,
    /// Index mode for relative operands: 0..3 = AR.x..w, 4 = loop index.
    pub index_mode: u8,
}

impl Alu {
    pub fn new(slot: u8, op: AluOp, dst: (u8, u8)) -> Self {
        Alu {
            slot,
            op,
            dst,
            write: true,
            clamp: false,
            update_exec: false,
            update_pred: false,
            bank_swizzle: 0,
            index_mode: 0,
        }
    }
}

/// A group of up to five instructions issued together.
pub type Group = Vec<Alu>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sel {
    X = 0,
    Y = 1,
    Z = 2,
    W = 3,
    Zero = 4,
    One = 5,
    Mask = 7,
}

pub const XYZW: [Sel; 4] = [Sel::X, Sel::Y, Sel::Z, Sel::W];

/// A texture-clause instruction: `SAMPLE dst.sel, src.sel, tN, sN` and its relatives (see [`op::tex`]).
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
    /// Texel offsets, in 1.3.1 fixed point halves (`textureSample(.., offset)`), x/y/z.
    pub offset: [i8; 3],
    pub lod_bias: i8,
}

/// A vertex-fetch instruction: read `dst.sel` from buffer `buffer` at the element index in `src.src_x` (or the
/// vertex/instance index when that is how the buffer is bound), plus `offset` bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vtx {
    pub buffer: u8,
    pub src: u8,
    pub src_x: u8,
    /// 0 vertex data, 1 instance data, 2 no index offset.
    pub fetch_type: u8,
    pub dst: u8,
    pub dst_sel: [Sel; 4],
    /// SQ_DATA_FORMAT (13 = 32, 29 = 32_32, 47 = 32_32_32, 34 = 32_32_32_32, 30/48/35 the FLOAT ones...).
    pub format: u8,
    /// SQ_NUM_FORMAT: 0 norm, 1 int, 2 scaled.
    pub num_format: u8,
    pub signed: bool,
    pub offset: u16,
    /// SQ_ENDIAN: 0 none, 1 8in16, 2 8in32.
    pub endian: u8,
    /// Bytes fetched at once (the mega-fetch count is this minus one).
    pub mega_fetch: u8,
}

/// Export targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// PIX0..: pixel colour outputs; with `Z`, depth.
    Pixel(u8),
    /// POS0..: vertex position.
    Pos(u8),
    /// PARAM0..: interpolated parameters.
    Param(u8),
}

/// The kcache lock of an ALU clause: which uniform block (bank) and which 16-constant line, two lines locked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Kcache {
    pub bank: u8,
    /// Address in 16-constant lines; locks this line and the next (LOCK_2).
    pub line: u8,
    pub on: bool,
}

/// ALU clause flavours (CF_ALU_INST).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluKind {
    Alu = 8,
    PushBefore = 9,
    PopAfter = 10,
    Pop2After = 11,
    Continue = 13,
    Break = 14,
    ElseAfter = 15,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cf {
    CallFs { barrier: bool },
    Alu { kind: AluKind, groups: Vec<Group>, kcache: [Kcache; 2] },
    /// A texture clause; `valid_pixel` restricts it to covered pixels (VALID_PIX, for derivatives in pixel shaders).
    Tex { fetches: Vec<Tex>, valid_pixel: bool },
    Vtx { fetches: Vec<Vtx>, valid_pixel: bool },
    /// EXP or EXP_DONE of a register with a swizzle.
    Export { target: Target, gpr: u8, sel: [Sel; 4], done: bool, barrier: bool },
    /// Flow control with a CF address (an index into this list) and the stack entries to pop when taken.
    Flow { inst: u32, addr: u32, pop: u8 },
    Nop,
}

const ALU_CLAUSE_ALIGN_SLOTS: usize = 256 / 8;
const FETCH_CLAUSE_ALIGN_SLOTS: usize = 128 / 8;

fn align_up(v: usize, to: usize) -> usize {
    v.div_ceil(to) * to
}

fn src_sel(s: Src) -> (u32, u32) {
    match s {
        Src::Gpr(r, c) => (r as u32, c as u32),
        Src::Const(n, c) => (256 + n as u32, c as u32),
        Src::Kcache(b, i, c) => (128 + 32 * b as u32 + i as u32, c as u32),
        Src::Pv(c) => (254, c as u32),
        Src::Ps => (255, 0),
        Src::Zero => (248, 0),
        Src::One => (249, 0),
        Src::IntOne => (250, 0),
        Src::IntMinusOne => (251, 0),
        Src::Half => (252, 0),
        Src::Lit(_) => (253, 0),
    }
}

/// Encode one ALU group to 64-bit slots (pairs of words), literals appended as (lo, hi) pairs.
pub fn encode_group(group: &[Alu]) -> Vec<[u32; 2]> {
    let mut literals: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    let mut sorted: Vec<&Alu> = group.iter().collect();
    sorted.sort_by_key(|a| a.slot);
    let lit_chan = |o: &Operand, literals: &mut Vec<u32>| -> (u32, u32) {
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
    for (i, a) in sorted.iter().enumerate() {
        let last = i == sorted.len() - 1;
        let ops: Vec<Operand> = match a.op {
            AluOp::Op2(_, o) => o.to_vec(),
            AluOp::Op3(_, o) => o.to_vec(),
        };
        let (s0, c0) = lit_chan(&ops[0], &mut literals);
        let (s1, c1) = lit_chan(&ops[1], &mut literals);
        let mut w0 = s0
            | ((ops[0].rel as u32) << 9)
            | (c0 << 10)
            | ((ops[0].neg as u32) << 12)
            | (s1 << 13)
            | ((ops[1].rel as u32) << 22)
            | (c1 << 23)
            | ((ops[1].neg as u32) << 25)
            | ((a.index_mode as u32) << 26);
        if last {
            w0 |= 1 << 31;
        }
        let mut w1 = ((a.bank_swizzle as u32) << 18)
            | ((a.dst.0 as u32) << 21)
            | ((a.dst.1 as u32) << 29)
            | ((a.clamp as u32) << 31);
        match a.op {
            AluOp::Op2(op, o) => {
                w1 |= (o[0].abs as u32)
                    | ((o[1].abs as u32) << 1)
                    | ((a.update_exec as u32) << 2)
                    | ((a.update_pred as u32) << 3)
                    | ((a.write as u32) << 4)
                    | (op << 7);
            }
            AluOp::Op3(op, o) => {
                let (s2, c2) = lit_chan(&o[2], &mut literals);
                w1 |= s2 | ((o[2].rel as u32) << 9) | (c2 << 10) | ((o[2].neg as u32) << 12) | (op << 13);
            }
        }
        out.push([w0, w1]);
    }
    assert!(literals.len() <= 4, "more than four literals in one ALU group");
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

fn encode_tex(f: &Tex) -> [u32; 4] {
    let w0 = (f.inst as u32) | ((f.resource as u32) << 8) | ((f.src as u32) << 16);
    let w1 = (f.dst as u32)
        | (sel_bits(&f.dst_sel) << 9)
        | (((f.lod_bias as u32) & 0x7F) << 21)
        | if f.normalized { 0xF << 28 } else { 0 };
    let w2 = ((f.offset[0] as u32) & 0x1F)
        | (((f.offset[1] as u32) & 0x1F) << 5)
        | (((f.offset[2] as u32) & 0x1F) << 10)
        | ((f.sampler as u32) << 15)
        | (sel_bits(&f.src_sel) << 20);
    [w0, w1, w2, 0]
}

fn encode_vtx(f: &Vtx) -> [u32; 4] {
    let w0 = ((f.fetch_type as u32) << 5)
        | ((f.buffer as u32) << 8)
        | ((f.src as u32) << 16)
        | ((f.src_x as u32) << 24)
        | (((f.mega_fetch.max(1) - 1) as u32) << 26);
    let w1 = (f.dst as u32)
        | (sel_bits(&f.dst_sel) << 9)
        | ((f.format as u32) << 22)
        | ((f.num_format as u32) << 28)
        | ((f.signed as u32) << 30);
    let w2 = (f.offset as u32) | ((f.endian as u32) << 16) | (1 << 19);
    [w0, w1, w2, 0]
}

/// Program size limits of one clause.
pub const MAX_ALU_SLOTS: usize = 128;
pub const MAX_FETCHES: usize = 8;

/// Assemble a program to its 32-bit words, in program order.
pub fn assemble(cf: &[Cf]) -> Vec<u32> {
    // clause bodies first, to know their sizes and addresses
    let mut alu_data: Vec<[u32; 2]> = Vec::new();
    let mut fetch_data: Vec<[u32; 4]> = Vec::new();
    let mut at = Vec::new();
    for c in cf {
        match c {
            Cf::Alu { groups, .. } => {
                let start = alu_data.len();
                for g in groups {
                    alu_data.extend(encode_group(g));
                }
                assert!(alu_data.len() - start <= MAX_ALU_SLOTS, "ALU clause over 128 slots");
                at.push((start, alu_data.len() - start));
            }
            Cf::Tex { fetches, .. } => {
                let start = fetch_data.len();
                fetch_data.extend(fetches.iter().map(encode_tex));
                at.push((start, fetch_data.len() - start));
            }
            Cf::Vtx { fetches, .. } => {
                let start = fetch_data.len();
                fetch_data.extend(fetches.iter().map(encode_vtx));
                at.push((start, fetch_data.len() - start));
            }
            _ => at.push((0, 0)),
        }
    }
    let alu_base = align_up(cf.len(), ALU_CLAUSE_ALIGN_SLOTS);
    let fetch_base = align_up(alu_base + alu_data.len(), FETCH_CLAUSE_ALIGN_SLOTS);
    let mut words = Vec::new();
    for (i, c) in cf.iter().enumerate() {
        let end = ((i == cf.len() - 1) as u32) << 21;
        let (start, n) = at[i];
        let (w0, w1) = match c {
            Cf::CallFs { barrier } => (0, (op::cf::CALL_FS << 23) | end | ((*barrier as u32) << 31)),
            Cf::Nop => (0, (op::cf::NOP << 23) | end | (1 << 31)),
            Cf::Alu { kind, kcache, .. } => {
                let mode = |k: &Kcache| if k.on { 2u32 } else { 0 };
                let w0 = (alu_base + start) as u32
                    | ((kcache[0].bank as u32) << 22)
                    | ((kcache[1].bank as u32) << 26)
                    | (mode(&kcache[0]) << 30);
                let w1 = (mode(&kcache[1]) >> 0)
                    | ((kcache[0].line as u32) << 2)
                    | ((kcache[1].line as u32) << 10)
                    | (((n - 1) as u32) << 18)
                    | ((*kind as u32) << 26)
                    | (1 << 31);
                (w0, w1)
            }
            Cf::Tex { valid_pixel, .. } | Cf::Vtx { valid_pixel, .. } => {
                let inst = if matches!(c, Cf::Tex { .. }) { op::cf::TEX } else { op::cf::VTX };
                let w0 = (fetch_base + start * 2) as u32;
                let w1 = (((n - 1) as u32) << 10) | (inst << 23) | (1 << 31) | end | ((*valid_pixel as u32) << 22);
                (w0, w1)
            }
            Cf::Export { target, gpr, sel, done, barrier } => {
                let (ty, base) = match target {
                    Target::Pixel(n) => (0u32, *n as u32),
                    Target::Pos(n) => (1u32, 60 + *n as u32),
                    Target::Param(n) => (2u32, *n as u32),
                };
                let w0 = base | (ty << 13) | ((*gpr as u32) << 15);
                let inst = if *done { op::cf::EXP_DONE } else { op::cf::EXP };
                let w1 = sel_bits(sel) | (inst << 23) | ((*barrier as u32) << 31) | end;
                (w0, w1)
            }
            Cf::Flow { inst, addr, pop } => (*addr, (*pop as u32) | (inst << 23) | (1 << 31) | end),
        };
        words.push(w0);
        words.push(w1);
    }
    words.resize(alu_base * 2, 0);
    for [a, b] in &alu_data {
        words.push(*a);
        words.push(*b);
    }
    if !fetch_data.is_empty() {
        words.resize(fetch_base * 2, 0);
        for t in &fetch_data {
            words.extend_from_slice(t);
        }
    }
    words
}

/// Program words as GX2 stores them (little-endian bytes).
pub fn to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
