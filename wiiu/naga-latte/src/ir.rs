//! The compiler's middle form: scalar operations on virtual registers, with structured control flow.
//!
//! A virtual register ([`V`]) is one 32-bit channel. Most are written once; local variables are written by every
//! store, so allocation works on live intervals over the linear order (see `regalloc.rs`), not on SSA.

/// A virtual scalar register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct V(pub u32);

/// Where an operand comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum A {
    V(V),
    /// A 32-bit constant (float, int or bool bits).
    Imm(u32),
    /// A uniform block constant read through the kcache: block (kcache bank), vec4 index, channel.
    Ub { block: u8, index: u16, chan: u8 },
}

/// An operand: a source and its float modifiers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arg {
    pub a: A,
    pub neg: bool,
    pub abs: bool,
}

impl Arg {
    pub fn v(v: V) -> Arg {
        Arg { a: A::V(v), neg: false, abs: false }
    }
    pub fn imm(bits: u32) -> Arg {
        Arg { a: A::Imm(bits), neg: false, abs: false }
    }
    pub fn f(x: f32) -> Arg {
        Arg::imm(x.to_bits())
    }
    pub fn neg(self) -> Arg {
        if let A::Imm(b) = self.a {
            if !self.abs {
                // fold, so literals stay shareable
                return Arg::imm(b ^ 0x8000_0000);
            }
        }
        Arg { neg: !self.neg, ..self }
    }
    pub fn abs(self) -> Arg {
        Arg { abs: true, neg: false, ..self }
    }
    pub fn as_v(&self) -> Option<V> {
        match (self.a, self.neg, self.abs) {
            (A::V(v), false, false) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    /// `dst = op(a, b)` with a SQ_OP2 opcode; `clamp` saturates to [0, 1].
    Op2 { op: u32, dst: V, a: [Arg; 2], clamp: bool },
    /// `dst = op(a, b, c)` with a SQ_OP3 opcode.
    Op3 { op: u32, dst: V, a: [Arg; 3] },
    /// Kill the active pixels for which `op(a, b)` holds (KILLGT etc.).
    Kill { op: u32, a: [Arg; 2] },
    /// A texture instruction: coordinates from one register group, results into another (`None` = not written).
    Tex { inst: u8, coord: [Option<V>; 4], dst: [Option<V>; 4], resource: u8, sampler: u8, offset: [i8; 3], normalized: bool },
    /// A buffer read: `count` consecutive dwords at dword `index` (+ `offset` bytes) of buffer `buffer`.
    Vtx { buffer: u8, index: V, offset: u16, dst: [Option<V>; 4], count: u8, swap: bool },
    If { cond: V, then: Vec<Inst>, els: Vec<Inst> },
    Loop { body: Vec<Inst> },
    Break,
    Continue,
}

/// Where the shader's results go.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub target: latte::Target,
    pub src: [Option<V>; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub body: Vec<Inst>,
    pub exports: Vec<Export>,
    /// Registers the hardware fills before the shader runs: (register, GPR, channel).
    pub inputs: Vec<(V, u8, u8)>,
    /// Registers whose four channels must share a GPR (texture coordinates and results, exports).
    pub groups: Vec<[Option<V>; 4]>,
    pub next: u32,
}

impl Program {
    pub fn fresh(&mut self) -> V {
        self.next += 1;
        V(self.next - 1)
    }
}

/// Opcodes that run only in the transcendental slot (R700).
pub fn is_trans(op: u32) -> bool {
    (0x61..=0x6F).contains(&op) || (0x73..=0x79).contains(&op)
}
