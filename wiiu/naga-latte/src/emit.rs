//! Allocated IR to Latte control flow and clauses.
//!
//! `if` follows the pattern Mesa's r600 driver uses: ALU_PUSH_BEFORE with PRED_SETNE_INT, JUMP to the ELSE (or
//! past the POP) when no pixel takes the branch, ELSE, POP. Loops are LOOP_START_DX10 / LOOP_END with
//! LOOP_BREAK / LOOP_CONTINUE pointing at the LOOP_END.

use std::collections::HashSet;

use latte::op::{cf as cfop, op2};
use latte::{Alu, AluKind, AluOp, Cf, Group, Kcache, Operand, Sel, Src};

use crate::ir::*;
use crate::regalloc::Alloc;

enum Pending {
    None,
    Tex(Vec<latte::Tex>, HashSet<u8>),
    Vtx(Vec<latte::Vtx>, HashSet<u8>),
}

pub struct Emitter<'a> {
    a: &'a Alloc,
    pub cf: Vec<Cf>,
    groups: Vec<Group>,
    slots: usize,
    kc: [Option<(u8, u8)>; 2],
    fetch: Pending,
    loops: Vec<Vec<usize>>,
    depth: u8,
    pub max_depth: u8,
}

const SEL: [Sel; 4] = [Sel::X, Sel::Y, Sel::Z, Sel::W];

impl<'a> Emitter<'a> {
    pub fn new(a: &'a Alloc) -> Self {
        Emitter {
            a,
            cf: Vec::new(),
            groups: Vec::new(),
            slots: 0,
            kc: [None; 2],
            fetch: Pending::None,
            loops: Vec::new(),
            depth: 0,
            max_depth: 0,
        }
    }

    fn r(&self, v: V) -> (u8, u8) {
        self.a.reg[&v]
    }

    fn flush_alu(&mut self, kind: AluKind) {
        if self.groups.is_empty() {
            return;
        }
        let mut kcache = [Kcache::default(); 2];
        for (i, k) in self.kc.iter().enumerate() {
            if let Some((bank, line)) = k {
                kcache[i] = Kcache { bank: *bank, line: *line, on: true };
            }
        }
        self.cf.push(Cf::Alu { kind, groups: std::mem::take(&mut self.groups), kcache });
        self.slots = 0;
        self.kc = [None; 2];
    }

    fn flush_fetch(&mut self) {
        match std::mem::replace(&mut self.fetch, Pending::None) {
            Pending::None => {}
            Pending::Tex(fetches, _) => self.cf.push(Cf::Tex { fetches, valid_pixel: false }),
            Pending::Vtx(fetches, _) => self.cf.push(Cf::Vtx { fetches, valid_pixel: false }),
        }
    }

    pub fn flush(&mut self) {
        self.flush_alu(AluKind::Alu);
        self.flush_fetch();
    }

    /// The kcache bank for a uniform constant, locking a bank if one is free.
    fn kcache(kc: &mut [Option<(u8, u8)>; 2], block: u8, index: u16) -> Option<(u8, u8)> {
        let line = (index / 16) as u8;
        for (b, k) in kc.iter().enumerate() {
            if let Some((blk, l)) = k {
                if *blk == block && (*l == line || *l + 1 == line) {
                    return Some((b as u8, (index - *l as u16 * 16) as u8));
                }
            }
        }
        for (b, k) in kc.iter_mut().enumerate() {
            if k.is_none() {
                *k = Some((block, line));
                return Some((b as u8, (index % 16) as u8));
            }
        }
        None
    }

    fn operands(&self, args: &[Arg], kc: &mut [Option<(u8, u8)>; 2]) -> Option<Vec<Operand>> {
        let mut out = Vec::new();
        for x in args {
            let src = match x.a {
                A::V(v) => {
                    let (g, c) = self.r(v);
                    Src::Gpr(g, c)
                }
                A::Imm(0) => Src::Zero,
                A::Imm(0x3f80_0000) => Src::One,
                A::Imm(0x3f00_0000) => Src::Half,
                A::Imm(1) => Src::IntOne,
                A::Imm(0xFFFF_FFFF) => Src::IntMinusOne,
                A::Imm(b) => Src::Lit(b),
                A::Ub { block, index, chan } => {
                    let (bank, i) = Self::kcache(kc, block, index)?;
                    Src::Kcache(bank, i, chan)
                }
            };
            out.push(Operand { src, neg: x.neg, abs: x.abs, rel: false });
        }
        Some(out)
    }

    fn literal_slots(ops: &[Operand]) -> usize {
        let mut lits: Vec<u32> = Vec::new();
        for o in ops {
            if let Src::Lit(v) = o.src {
                if !lits.contains(&v) {
                    lits.push(v);
                }
            }
        }
        lits.len().div_ceil(2)
    }

    /// Add one instruction as its own group; `make` builds it from the resolved operands.
    fn alu(&mut self, args: &[Arg], make: impl Fn(Vec<Operand>) -> Alu) {
        self.flush_fetch();
        let mut kc = self.kc;
        let ops = match self.operands(args, &mut kc) {
            Some(o) => o,
            None => {
                self.flush_alu(AluKind::Alu);
                kc = self.kc;
                self.operands(args, &mut kc).expect("two kcache lines at most per instruction")
            }
        };
        let need = 1 + Self::literal_slots(&ops);
        if self.slots + need > latte::MAX_ALU_SLOTS {
            self.flush_alu(AluKind::Alu);
            return self.alu(args, make);
        }
        self.kc = kc;
        self.slots += need;
        self.groups.push(vec![make(ops)]);
    }

    fn add_fetch(&mut self, tex: Option<latte::Tex>, vtx: Option<latte::Vtx>, reads: u8, writes: u8) {
        self.flush_alu(AluKind::Alu);
        let fits = match (&self.fetch, &tex) {
            (Pending::Tex(f, w), Some(_)) => f.len() < latte::MAX_FETCHES && !w.contains(&reads),
            (Pending::Vtx(f, w), None) => f.len() < latte::MAX_FETCHES && !w.contains(&reads),
            _ => false,
        };
        if !fits {
            self.flush_fetch();
            self.fetch = if tex.is_some() { Pending::Tex(Vec::new(), HashSet::new()) } else { Pending::Vtx(Vec::new(), HashSet::new()) };
        }
        match (&mut self.fetch, tex, vtx) {
            (Pending::Tex(f, w), Some(t), _) => {
                f.push(t);
                w.insert(writes);
            }
            (Pending::Vtx(f, w), _, Some(v)) => {
                f.push(v);
                w.insert(writes);
            }
            _ => unreachable!(),
        }
    }

    fn flow(&mut self, inst: u32) -> usize {
        self.cf.push(Cf::Flow { inst, addr: 0, pop: 0 });
        self.cf.len() - 1
    }

    fn patch(&mut self, at: usize, to: usize, pop_count: u8) {
        if let Cf::Flow { addr, pop, .. } = &mut self.cf[at] {
            *addr = to as u32;
            *pop = pop_count;
        }
    }

    fn enter(&mut self) {
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
    }

    pub fn block(&mut self, insts: &[Inst]) {
        for i in insts {
            self.inst(i);
        }
    }

    fn inst(&mut self, i: &Inst) {
        match i {
            Inst::Op2 { op, dst, a, clamp } => {
                let (g, c) = self.r(*dst);
                let slot = if is_trans(*op) { 4 } else { c };
                let (op, clamp) = (*op, *clamp);
                self.alu(a, |o| Alu { clamp, ..Alu::new(slot, AluOp::Op2(op, [o[0], o[1]]), (g, c)) });
            }
            Inst::Op3 { op, dst, a } => {
                let (g, c) = self.r(*dst);
                let op = *op;
                self.alu(a, |o| Alu::new(c, AluOp::Op3(op, [o[0], o[1], o[2]]), (g, c)));
            }
            Inst::Kill { op, a } => {
                let op = *op;
                self.alu(a, |o| Alu { write: false, ..Alu::new(0, AluOp::Op2(op, [o[0], o[1]]), (0, 0)) });
            }
            Inst::Tex { inst, coord, dst, resource, sampler, offset, normalized } => {
                let src = coord.iter().flatten().next().map(|v| self.r(*v).0).unwrap_or(0);
                let dstg = dst.iter().flatten().next().map(|v| self.r(*v).0).unwrap_or(0);
                let mut src_sel = [Sel::Zero; 4];
                let mut dst_sel = [Sel::Mask; 4];
                for c in 0..4 {
                    if coord[c].is_some() {
                        src_sel[c] = SEL[c];
                    }
                    if dst[c].is_some() {
                        dst_sel[c] = SEL[c];
                    }
                }
                let t = latte::Tex {
                    inst: *inst,
                    dst: dstg,
                    dst_sel,
                    src,
                    src_sel,
                    resource: *resource,
                    sampler: *sampler,
                    normalized: *normalized,
                    offset: *offset,
                    lod_bias: 0,
                };
                self.add_fetch(Some(t), None, src, dstg);
            }
            Inst::Vtx { buffer, index, offset, dst, count, swap } => {
                let (ig, ic) = self.r(*index);
                let dstg = dst.iter().flatten().next().map(|v| self.r(*v).0).unwrap_or(0);
                let mut dst_sel = [Sel::Mask; 4];
                for c in 0..4 {
                    if dst[c].is_some() {
                        dst_sel[c] = SEL[c];
                    }
                }
                let format = match count {
                    1 => 13,
                    2 => 29,
                    3 => 47,
                    _ => 34,
                };
                let v = latte::Vtx {
                    buffer: *buffer,
                    src: ig,
                    src_x: ic,
                    fetch_type: 2,
                    dst: dstg,
                    dst_sel,
                    format,
                    num_format: 1,
                    signed: false,
                    offset: *offset,
                    endian: if *swap { 2 } else { 0 },
                    mega_fetch: 4 * count,
                };
                self.add_fetch(None, Some(v), ig, dstg);
            }
            Inst::If { cond, then, els } => {
                self.flush();
                let (g, c) = self.r(*cond);
                let pred = Alu {
                    write: false,
                    update_exec: true,
                    update_pred: true,
                    ..Alu::new(
                        0,
                        AluOp::Op2(op2::PRED_SETNE_INT, [Src::Gpr(g, c).into(), Src::Zero.into()]),
                        (0, 0),
                    )
                };
                self.cf.push(Cf::Alu { kind: AluKind::PushBefore, groups: vec![vec![pred]], kcache: Default::default() });
                self.enter();
                let jump = self.flow(cfop::JUMP);
                self.block(then);
                self.flush();
                if els.is_empty() {
                    let pop = self.flow(cfop::POP);
                    self.patch(pop, pop + 1, 1);
                    self.patch(jump, pop + 1, 1);
                } else {
                    let el = self.flow(cfop::ELSE);
                    self.block(els);
                    self.flush();
                    let pop = self.flow(cfop::POP);
                    self.patch(pop, pop + 1, 1);
                    self.patch(jump, el, 0);
                    self.patch(el, pop + 1, 1);
                }
                self.depth -= 1;
            }
            Inst::Loop { body } => {
                self.flush();
                self.enter();
                let start = self.flow(cfop::LOOP_START_DX10);
                self.loops.push(Vec::new());
                self.block(body);
                self.flush();
                let end = self.flow(cfop::LOOP_END);
                self.patch(end, start + 1, 0);
                self.patch(start, end + 1, 0);
                for at in self.loops.pop().unwrap() {
                    self.patch(at, end, 0);
                }
                self.depth -= 1;
            }
            Inst::Break | Inst::Continue => {
                self.flush();
                let inst = if matches!(i, Inst::Break) { cfop::LOOP_BREAK } else { cfop::LOOP_CONTINUE };
                let at = self.flow(inst);
                self.loops.last_mut().expect("break outside a loop").push(at);
            }
        }
    }

    /// Vertex shaders start by calling the fetch shader, which loads the attributes.
    pub fn call_fs(&mut self) {
        self.cf.push(Cf::CallFs { barrier: false });
    }

    pub fn exports(&mut self, exports: &[Export]) {
        self.flush();
        for (i, e) in exports.iter().enumerate() {
            let gpr = e.src.iter().flatten().next().map(|v| self.r(*v).0).unwrap_or(0);
            let same_kind = |t: &latte::Target| std::mem::discriminant(t) == std::mem::discriminant(&e.target);
            let last = !exports[i + 1..].iter().any(|o| same_kind(&o.target));
            self.cf.push(Cf::Export { target: e.target, gpr, sel: latte::XYZW, done: last, barrier: true });
        }
    }
}
