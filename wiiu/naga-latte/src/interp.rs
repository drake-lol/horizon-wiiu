//! A one-pixel Latte interpreter for tests: runs assembled program words (so it checks the encoding too) with
//! the control-flow stack semantics the compiler relies on. Each pixel runs alone, so divergence between pixels
//! is not modelled; derivatives are zero.

use latte::op::{cf, op2, op3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Active,
    Branch,
    Break,
    Cont,
}

/// What a run produced.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// (export type: 0 pixel / 1 pos / 2 param, array base, value)
    pub exports: Vec<(u8, u32, [u32; 4])>,
    pub killed: bool,
    pub steps: usize,
}

impl Output {
    pub fn get(&self, ty: u8, base: u32) -> Option<[f32; 4]> {
        self.exports.iter().find(|e| e.0 == ty && e.1 == base).map(|e| e.2.map(f32::from_bits))
    }
}

pub type TexFn<'a> = dyn Fn(u8, u8, u8, [u32; 4]) -> [u32; 4] + 'a;

pub struct Machine<'a> {
    pub regs: Vec<[u32; 4]>,
    /// Uniform blocks: vec4 constants per block.
    pub blocks: Vec<Vec<[u32; 4]>>,
    /// Fetch buffers by id, as dwords.
    pub buffers: std::collections::HashMap<u8, Vec<u32>>,
    /// (instruction, resource, sampler, source register) -> four channels
    pub tex: Box<TexFn<'a>>,
    pub step_limit: usize,
}

fn f(x: u32) -> f32 {
    f32::from_bits(x)
}

fn b(x: f32) -> u32 {
    x.to_bits()
}

fn mask(c: bool) -> u32 {
    if c { 0xFFFF_FFFF } else { 0 }
}

impl<'a> Machine<'a> {
    pub fn new() -> Self {
        Machine {
            regs: vec![[0; 4]; 128],
            blocks: Vec::new(),
            buffers: Default::default(),
            tex: Box::new(|_, _, _, c| c),
            step_limit: 1_000_000,
        }
    }

    pub fn set(&mut self, gpr: u8, v: [f32; 4]) {
        self.regs[gpr as usize] = v.map(b);
    }

    pub fn run(&mut self, code: &[u32]) -> Output {
        let mut out = Output::default();
        let mut s = State::Active;
        let mut stack: Vec<(State, bool)> = Vec::new();
        let mut pc = 0usize;
        loop {
            out.steps += 1;
            assert!(out.steps < self.step_limit, "program did not finish");
            let w0 = code[pc * 2];
            let w1 = code[pc * 2 + 1];
            // ALU clause words have no end bit: bit 21 is part of their count
            let end = (w1 >> 29) & 1 == 0 && (w1 >> 21) & 1 == 1;
            let mut next = pc + 1;
            if (w1 >> 29) & 1 == 1 {
                // ALU clause
                let kind = (w1 >> 26) & 0xF;
                let addr = (w0 & 0x3F_FFFF) as usize;
                let count = (((w1 >> 18) & 0x7F) + 1) as usize;
                let kc = [
                    ((w0 >> 22) & 0xF, (w1 >> 2) & 0xFF, (w0 >> 30) & 3),
                    ((w0 >> 26) & 0xF, (w1 >> 10) & 0xFF, w1 & 3),
                ];
                if kind == 9 {
                    stack.push((s, false));
                }
                if s == State::Active {
                    match self.alu_clause(&code[addr * 2..(addr + count) * 2], kc, &mut out) {
                        Some(false) => s = State::Branch,
                        _ => {}
                    }
                    if out.killed {
                        return out;
                    }
                }
                if kind == 10 {
                    s = stack.pop().unwrap().0;
                }
            } else {
                let inst = (w1 >> 23) & 0x7F;
                let addr = w0 as usize;
                let pop = (w1 & 7) as usize;
                let do_pop = |stack: &mut Vec<(State, bool)>, s: &mut State, n: usize| {
                    for _ in 0..n {
                        *s = stack.pop().unwrap().0;
                    }
                };
                match inst {
                    cf::NOP | cf::CALL_FS => {}
                    cf::TEX | cf::VTX => {
                        if s == State::Active {
                            let count = (((w1 >> 10) & 7) | (((w1 >> 19) & 1) << 3)) as usize + 1;
                            for i in 0..count {
                                let at = (addr + i * 2) * 2;
                                let w = &code[at..at + 4];
                                if inst == cf::TEX {
                                    self.tex_inst(w);
                                } else {
                                    self.vtx_inst(w);
                                }
                            }
                        }
                    }
                    cf::EXP | cf::EXP_DONE => {
                        let base = w0 & 0x1FFF;
                        let ty = ((w0 >> 13) & 3) as u8;
                        let gpr = ((w0 >> 15) & 0x7F) as usize;
                        let mut v = [0u32; 4];
                        for (c, item) in v.iter_mut().enumerate() {
                            let sel = (w1 >> (3 * c)) & 7;
                            *item = match sel {
                                0..=3 => self.regs[gpr][sel as usize],
                                4 => 0,
                                5 => b(1.0),
                                _ => 0,
                            };
                        }
                        out.exports.push((ty, base, v));
                    }
                    cf::JUMP => {
                        if s != State::Active {
                            do_pop(&mut stack, &mut s, pop);
                            next = addr;
                        }
                    }
                    cf::ELSE => {
                        if stack.last().map(|e| e.0) == Some(State::Active) {
                            s = match s {
                                State::Active => State::Branch,
                                State::Branch => State::Active,
                                o => o,
                            };
                        }
                        if s != State::Active {
                            do_pop(&mut stack, &mut s, pop);
                            next = addr;
                        }
                    }
                    cf::POP => do_pop(&mut stack, &mut s, pop),
                    cf::LOOP_START_DX10 => {
                        stack.push((s, true));
                        if s != State::Active {
                            s = stack.pop().unwrap().0;
                            next = addr;
                        }
                    }
                    cf::LOOP_END => {
                        if s == State::Cont {
                            s = State::Active;
                        }
                        if s == State::Active {
                            next = addr;
                        } else {
                            s = stack.pop().unwrap().0;
                        }
                    }
                    cf::LOOP_BREAK | cf::LOOP_CONTINUE => {
                        if s == State::Active {
                            s = if inst == cf::LOOP_BREAK { State::Break } else { State::Cont };
                            while let Some(&(_, marker)) = stack.last() {
                                if marker {
                                    break;
                                }
                                stack.pop();
                            }
                            next = addr;
                        }
                    }
                    other => panic!("CF instruction {other:#x} not interpreted"),
                }
            }
            if end {
                return out;
            }
            pc = next;
        }
    }

    /// Runs a clause; returns the last predicate result, if the clause set one.
    fn alu_clause(&mut self, words: &[u32], kc: [(u32, u32, u32); 2], out: &mut Output) -> Option<bool> {
        let mut pv = [0u32; 4];
        let mut ps = 0u32;
        let mut i = 0;
        let mut pred = None;
        let n = words.len() / 2;
        while i < n {
            // one group: instructions up to LAST, then literals
            let start = i;
            while (words[i * 2] >> 31) & 1 == 0 {
                i += 1;
            }
            let last = i;
            i += 1;
            let mut max_lit = 0usize;
            let mut has_lit = false;
            for k in start..=last {
                let (w0, w1) = (words[k * 2], words[k * 2 + 1]);
                let op3 = (w1 >> 15) & 7 != 0;
                let mut sels = vec![(w0 & 0x1FF, (w0 >> 10) & 3), ((w0 >> 13) & 0x1FF, (w0 >> 23) & 3)];
                if op3 {
                    sels.push((w1 & 0x1FF, (w1 >> 10) & 3));
                }
                for (sel, chan) in sels {
                    if sel == 253 {
                        has_lit = true;
                        max_lit = max_lit.max(chan as usize);
                    }
                }
            }
            let lits: Vec<u32> = if has_lit {
                let slots = (max_lit + 2) / 2;
                let l = words[i * 2..(i + slots) * 2].to_vec();
                i += slots;
                l
            } else {
                Vec::new()
            };
            let mut writes = Vec::new();
            let mut used = [false; 5];
            let (mut npv, mut nps) = (pv, ps);
            for k in start..=last {
                let (w0, w1) = (words[k * 2], words[k * 2 + 1]);
                let is_op3 = (w1 >> 15) & 7 != 0;
                let read = |sel: u32, chan: u32, neg: bool, abs: bool, m: &Self| -> u32 {
                    let mut v = match sel {
                        0..=127 => m.regs[sel as usize][chan as usize],
                        128..=191 => {
                            let bank = ((sel - 128) / 32) as usize;
                            let idx = (sel - 128) % 32;
                            let (block, line, _) = kc[bank];
                            m.blocks[block as usize][(line * 16 + idx) as usize][chan as usize]
                        }
                        248 => 0,
                        249 => b(1.0),
                        250 => 1,
                        251 => 0xFFFF_FFFF,
                        252 => b(0.5),
                        253 => lits[chan as usize],
                        254 => pv[chan as usize],
                        255 => ps,
                        _ => panic!("source {sel}"),
                    };
                    if abs {
                        v &= 0x7FFF_FFFF;
                    }
                    if neg {
                        v ^= 0x8000_0000;
                    }
                    v
                };
                let s0 = read(w0 & 0x1FF, (w0 >> 10) & 3, (w0 >> 12) & 1 == 1, !is_op3 && w1 & 1 == 1, self);
                let s1 = read((w0 >> 13) & 0x1FF, (w0 >> 23) & 3, (w0 >> 25) & 1 == 1, !is_op3 && (w1 >> 1) & 1 == 1, self);
                let dst = ((w1 >> 21) & 0x7F) as usize;
                let chan = ((w1 >> 29) & 3) as usize;
                let clamp = (w1 >> 31) & 1 == 1;
                let (result, write) = if is_op3 {
                    let s2 = read(w1 & 0x1FF, (w1 >> 10) & 3, (w1 >> 12) & 1 == 1, false, self);
                    let op = (w1 >> 13) & 0x1F;
                    let r = match op {
                        op3::MULADD | op3::MULADD_IEEE => b(f(s0) * f(s1) + f(s2)),
                        op3::CNDE => if f(s0) == 0.0 { s1 } else { s2 },
                        op3::CNDGT => if f(s0) > 0.0 { s1 } else { s2 },
                        op3::CNDGE => if f(s0) >= 0.0 { s1 } else { s2 },
                        op3::CNDE_INT => if s0 == 0 { s1 } else { s2 },
                        op3::CNDGT_INT => if (s0 as i32) > 0 { s1 } else { s2 },
                        op3::CNDGE_INT => if (s0 as i32) >= 0 { s1 } else { s2 },
                        other => panic!("OP3 {other:#x}"),
                    };
                    (r, true)
                } else {
                    let op = (w1 >> 7) & 0x7FF;
                    let write = (w1 >> 4) & 1 == 1;
                    let (x, y) = (f(s0), f(s1));
                    let r = match op {
                        op2::ADD => b(x + y),
                        op2::MUL | op2::MUL_IEEE => b(x * y),
                        op2::MAX | op2::MAX_DX10 => b(x.max(y)),
                        op2::MIN | op2::MIN_DX10 => b(x.min(y)),
                        op2::SETE => b(if x == y { 1.0 } else { 0.0 }),
                        op2::SETGT => b(if x > y { 1.0 } else { 0.0 }),
                        op2::SETGE => b(if x >= y { 1.0 } else { 0.0 }),
                        op2::SETNE => b(if x != y { 1.0 } else { 0.0 }),
                        op2::SETE_DX10 => mask(x == y),
                        op2::SETGT_DX10 => mask(x > y),
                        op2::SETGE_DX10 => mask(x >= y),
                        op2::SETNE_DX10 => mask(x != y),
                        op2::FRACT => b(x - x.floor()),
                        op2::TRUNC => b(x.trunc()),
                        op2::CEIL => b(x.ceil()),
                        op2::RNDNE => b(x.round_ties_even()),
                        op2::FLOOR => b(x.floor()),
                        op2::MOV => s0,
                        op2::AND_INT => s0 & s1,
                        op2::OR_INT => s0 | s1,
                        op2::XOR_INT => s0 ^ s1,
                        op2::NOT_INT => !s0,
                        op2::ADD_INT => s0.wrapping_add(s1),
                        op2::SUB_INT => s0.wrapping_sub(s1),
                        op2::MAX_INT => (s0 as i32).max(s1 as i32) as u32,
                        op2::MIN_INT => (s0 as i32).min(s1 as i32) as u32,
                        op2::MAX_UINT => s0.max(s1),
                        op2::MIN_UINT => s0.min(s1),
                        op2::SETE_INT => mask(s0 == s1),
                        op2::SETNE_INT => mask(s0 != s1),
                        op2::SETGT_INT => mask((s0 as i32) > (s1 as i32)),
                        op2::SETGE_INT => mask((s0 as i32) >= (s1 as i32)),
                        op2::SETGT_UINT => mask(s0 > s1),
                        op2::SETGE_UINT => mask(s0 >= s1),
                        op2::LSHL_INT => s0.wrapping_shl(s1 & 31),
                        op2::LSHR_INT => s0.wrapping_shr(s1 & 31),
                        op2::ASHR_INT => ((s0 as i32) >> (s1 & 31)) as u32,
                        op2::MULLO_INT | op2::MULLO_UINT => s0.wrapping_mul(s1),
                        op2::INT_TO_FLT => b(s0 as i32 as f32),
                        op2::UINT_TO_FLT => b(s0 as f32),
                        op2::FLT_TO_INT => x as i32 as u32,
                        op2::FLT_TO_UINT => x as u32,
                        op2::RECIP_IEEE => b(1.0 / x),
                        op2::RECIPSQRT_IEEE => b(1.0 / x.sqrt()),
                        op2::SQRT_IEEE => b(x.sqrt()),
                        op2::EXP_IEEE => b(x.exp2()),
                        op2::LOG_IEEE => b(x.log2()),
                        op2::SIN => b((x * 2.0 * std::f32::consts::PI).sin()),
                        op2::COS => b((x * 2.0 * std::f32::consts::PI).cos()),
                        op2::KILLGT | op2::KILLGE | op2::KILLE | op2::KILLNE => {
                            let kill = match op {
                                op2::KILLGT => x > y,
                                op2::KILLGE => x >= y,
                                op2::KILLE => x == y,
                                _ => x != y,
                            };
                            if kill {
                                out.killed = true;
                            }
                            0
                        }
                        op2::PRED_SETNE_INT | op2::PRED_SETE_INT => {
                            let c = if op == op2::PRED_SETNE_INT { s0 != s1 } else { s0 == s1 };
                            if (w1 >> 2) & 1 == 1 {
                                pred = Some(c);
                            }
                            0
                        }
                        other => panic!("OP2 {other:#x}"),
                    };
                    (r, write)
                };
                let result = if clamp { b(f(result).clamp(0.0, 1.0)) } else { result };
                let slot = if !used[chan] && !is_trans_op(w1, is_op3) { chan } else { 4 };
                used[slot] = true;
                if slot == 4 {
                    nps = result;
                } else {
                    npv[chan] = result;
                }
                if write {
                    writes.push((dst, chan, result));
                }
            }
            for (d, c, v) in writes {
                self.regs[d][c] = v;
            }
            pv = npv;
            ps = nps;
            if out.killed {
                return pred;
            }
        }
        pred
    }

    fn tex_inst(&mut self, w: &[u32]) {
        let inst = (w[0] & 0x1F) as u8;
        let resource = ((w[0] >> 8) & 0xFF) as u8;
        let src = ((w[0] >> 16) & 0x7F) as usize;
        let dst = (w[1] & 0x7F) as usize;
        let sampler = ((w[2] >> 15) & 0x1F) as u8;
        let mut coord = [0u32; 4];
        for (c, item) in coord.iter_mut().enumerate() {
            let sel = (w[2] >> (20 + 3 * c)) & 7;
            *item = match sel {
                0..=3 => self.regs[src][sel as usize],
                5 => b(1.0),
                _ => 0,
            };
        }
        let r = (self.tex)(inst, resource, sampler, coord);
        for c in 0..4 {
            let sel = (w[1] >> (9 + 3 * c)) & 7;
            match sel {
                0..=3 => self.regs[dst][c] = r[sel as usize],
                4 => self.regs[dst][c] = 0,
                5 => self.regs[dst][c] = b(1.0),
                _ => {}
            }
        }
    }

    fn vtx_inst(&mut self, w: &[u32]) {
        let buffer = ((w[0] >> 8) & 0xFF) as u8;
        let src = ((w[0] >> 16) & 0x7F) as usize;
        let src_x = ((w[0] >> 24) & 3) as usize;
        let dst = (w[1] & 0x7F) as usize;
        let format = (w[1] >> 22) & 0x3F;
        let offset = w[2] & 0xFFFF;
        let count = match format {
            13 => 1,
            29 => 2,
            47 => 3,
            _ => 4,
        };
        // buffers are bound with a 16-byte stride (GX2 uniform blocks)
        let base = self.regs[src][src_x] as usize * 4 + (offset / 4) as usize;
        let data = &self.buffers[&buffer];
        let mut r = [0u32; 4];
        for (k, item) in r.iter_mut().enumerate().take(count) {
            *item = data.get(base + k).copied().unwrap_or(0);
        }
        for c in 0..4 {
            let sel = (w[1] >> (9 + 3 * c)) & 7;
            if sel <= 3 {
                self.regs[dst][c] = r[sel as usize];
            }
        }
    }
}

impl Default for Machine<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn is_trans_op(w1: u32, op3: bool) -> bool {
    !op3 && crate::ir::is_trans((w1 >> 7) & 0x7FF)
}
