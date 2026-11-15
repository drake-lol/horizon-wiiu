//! naga IR to the scalar IR: every function call inlined, every value split into 32-bit channels.
//!
//! Control flow keeps its structure. `return` from the middle of a function, `continue` inside a `switch` and
//! `continue` with a continuing block become breaks out of one-trip loops plus per-pixel flags, so the emitter
//! only ever sees `if`, `loop`, `break` and `continue`.

use std::collections::HashMap;

use latte::op::{op2, op3, tex as texop};
use naga::valid::{FunctionInfo, ModuleInfo};
use naga::{
    AddressSpace, ArraySize, BinaryOperator, Binding, Block, BuiltIn, Expression, Function, GlobalVariable, Handle,
    ImageClass, ImageDimension, ImageQuery, Literal, LocalVariable, MathFunction, Module, RelationalFunction,
    SampleLevel, ScalarKind, ShaderStage, Statement, SwitchValue, TypeInner, UnaryOperator, VectorSize,
};

use crate::ir::*;
use crate::{Error, Options, PsInput, ShaderInfo};

type R<T> = Result<T, Error>;

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(Error::Unsupported(what.into()))
}

#[derive(Clone, Debug)]
enum Lv {
    Val(Vec<Arg>),
    Ptr(Ptr),
    Image(Handle<GlobalVariable>),
    Sampler(Handle<GlobalVariable>),
}

#[derive(Clone, Debug)]
enum Root {
    Local(usize, Handle<LocalVariable>),
    Private(Handle<GlobalVariable>),
    /// A uniform block location: kcache bank, and fetch buffer 128 + location.
    Uniform(u8),
    /// A storage buffer, bound as a uniform block at this location (data left in the CPU's byte order).
    Storage(u8),
}

#[derive(Clone, Debug)]
enum Dyn {
    /// Element `idx` of `count`, each `stride` scalars, inside a register variable.
    Scalars { idx: V, stride: u32, count: u32 },
    /// A byte offset into a buffer; `aligned16` when every dynamic step was a multiple of 16 bytes.
    Bytes { v: V, aligned16: bool },
}

#[derive(Clone, Debug)]
struct Ptr {
    root: Root,
    ty: TypeInner,
    /// Scalars into a register variable, or bytes into a buffer.
    off: u32,
    dynamic: Option<Dyn>,
}

impl Ptr {
    fn in_buffer(&self) -> bool {
        matches!(self.root, Root::Uniform(_) | Root::Storage(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CtxKind {
    /// The one-trip loop around a function with an early return.
    Func,
    /// A WGSL loop.
    Real,
    /// The one-trip loop around a loop body that has `continue` and a continuing block.
    Body,
    /// The one-trip loop around a switch.
    Switch,
}

struct Ctx {
    kind: CtxKind,
    saw_ret: bool,
    saw_cont: bool,
    flag: Option<V>,
}

struct Frame<'a> {
    id: usize,
    f: &'a Function,
    fi: &'a FunctionInfo,
    args: Vec<Lv>,
    exprs: HashMap<Handle<Expression>, Lv>,
    ret: Option<Vec<V>>,
    ret_flag: Option<V>,
    ctx: Vec<Ctx>,
}

pub(crate) struct Lower<'a> {
    m: &'a Module,
    info: &'a ModuleInfo,
    opts: &'a Options,
    pub p: Program,
    blocks: Vec<Vec<Inst>>,
    vars: HashMap<(usize, Handle<LocalVariable>), Vec<V>>,
    privates: HashMap<Handle<GlobalVariable>, Vec<V>>,
    frames: usize,
    pub out: ShaderInfo,
}

const TRUE: u32 = 0xFFFF_FFFF;

fn lit(l: &Literal) -> R<u32> {
    Ok(match *l {
        Literal::F32(x) => x.to_bits(),
        Literal::F64(x) => (x as f32).to_bits(),
        Literal::AbstractFloat(x) => (x as f32).to_bits(),
        Literal::U32(x) => x,
        Literal::I32(x) => x as u32,
        Literal::AbstractInt(x) => x as u32,
        Literal::Bool(b) => if b { TRUE } else { 0 },
        ref other => return unsupported(format!("literal {other:?}")),
    })
}

fn size(s: VectorSize) -> u32 {
    s as u32
}

fn col_stride(rows: VectorSize) -> u32 {
    if rows == VectorSize::Bi { 8 } else { 16 }
}

impl<'a> Lower<'a> {
    pub fn new(m: &'a Module, info: &'a ModuleInfo, opts: &'a Options) -> R<Self> {
        Ok(Lower {
            m,
            info,
            opts,
            p: Program::default(),
            blocks: vec![Vec::new()],
            vars: HashMap::new(),
            privates: HashMap::new(),
            frames: 0,
            out: ShaderInfo::default(),
        })
    }

    // ---- building blocks ----

    fn push(&mut self, i: Inst) {
        self.blocks.last_mut().unwrap().push(i);
    }

    fn fresh(&mut self) -> V {
        self.p.fresh()
    }

    fn assign(&mut self, dst: V, a: Arg) {
        self.push(Inst::Op2 { op: op2::MOV, dst, a: [a, Arg::imm(0)], clamp: false });
    }

    fn mov(&mut self, a: Arg) -> V {
        let d = self.fresh();
        self.assign(d, a);
        d
    }

    fn to_v(&mut self, a: Arg) -> V {
        a.as_v().unwrap_or_else(|| self.mov(a))
    }

    /// An operand without float modifiers, for integer instructions.
    fn plain(&mut self, a: Arg) -> Arg {
        if a.neg || a.abs { Arg::v(self.mov(a)) } else { a }
    }

    fn op2(&mut self, op: u32, a: Arg, b: Arg) -> Arg {
        let (mut a, mut b) = (a, b);
        if is_trans(op) {
            // keep constants out of the trans slot's read ports
            if matches!(a.a, A::Ub { .. }) {
                a = Arg::v(self.mov(a));
            }
            if matches!(b.a, A::Ub { .. }) {
                b = Arg::v(self.mov(b));
            }
        }
        let (a, b) = self.limit_kcache2(a, b);
        let dst = self.fresh();
        self.push(Inst::Op2 { op, dst, a: [a, b], clamp: false });
        Arg::v(dst)
    }

    fn op1(&mut self, op: u32, a: Arg) -> Arg {
        self.op2(op, a, Arg::imm(0))
    }

    fn sat(&mut self, a: Arg) -> Arg {
        let dst = self.fresh();
        self.push(Inst::Op2 { op: op2::MOV, dst, a: [a, Arg::imm(0)], clamp: true });
        Arg::v(dst)
    }

    fn op3(&mut self, op: u32, a: Arg, b: Arg, c: Arg) -> Arg {
        let mut x = [a, b, c];
        for o in x.iter_mut() {
            if o.abs {
                *o = Arg::v(self.mov(*o));
            }
        }
        // at most two kcache lines per instruction (two banks per clause)
        let mut lines: Vec<(u8, u16)> = Vec::new();
        for o in x.iter_mut() {
            if let A::Ub { block, index, .. } = o.a {
                let l = (block, index / 16);
                if !lines.contains(&l) {
                    if lines.len() == 2 {
                        *o = Arg::v(self.mov(*o));
                        continue;
                    }
                    lines.push(l);
                }
            }
        }
        let dst = self.fresh();
        self.push(Inst::Op3 { op, dst, a: x });
        Arg::v(dst)
    }

    fn limit_kcache2(&mut self, a: Arg, b: Arg) -> (Arg, Arg) {
        (a, b)
    }

    fn fadd(&mut self, a: Arg, b: Arg) -> Arg {
        if let (A::Imm(x), A::Imm(y)) = (a.a, b.a) {
            if !a.abs && !b.abs && !a.neg && !b.neg {
                return Arg::f(f32::from_bits(x) + f32::from_bits(y));
            }
        }
        self.op2(op2::ADD, a, b)
    }

    fn fmul(&mut self, a: Arg, b: Arg) -> Arg {
        if a.a == A::Imm(1f32.to_bits()) && !a.neg {
            return b;
        }
        if b.a == A::Imm(1f32.to_bits()) && !b.neg {
            return a;
        }
        self.op2(op2::MUL_IEEE, a, b)
    }

    fn fmad(&mut self, a: Arg, b: Arg, c: Arg) -> Arg {
        self.op3(op3::MULADD_IEEE, a, b, c)
    }

    fn iop(&mut self, op: u32, a: Arg, b: Arg) -> Arg {
        if let (A::Imm(x), A::Imm(y)) = (a.a, b.a) {
            let folded = match op {
                op2::ADD_INT => Some(x.wrapping_add(y)),
                op2::SUB_INT => Some(x.wrapping_sub(y)),
                op2::MULLO_INT | op2::MULLO_UINT => Some(x.wrapping_mul(y)),
                op2::AND_INT => Some(x & y),
                op2::OR_INT => Some(x | y),
                op2::XOR_INT => Some(x ^ y),
                op2::LSHL_INT => Some(x.wrapping_shl(y)),
                op2::LSHR_INT => Some(x.wrapping_shr(y)),
                _ => None,
            };
            if let Some(v) = folded {
                return Arg::imm(v);
            }
        }
        if op == op2::ADD_INT && b.a == A::Imm(0) {
            return a;
        }
        let a = self.plain(a);
        let b = self.plain(b);
        self.op2(op, a, b)
    }

    /// `c != 0 ? t : f` on int/bool masks.
    fn select(&mut self, c: Arg, t: Arg, f: Arg) -> Arg {
        if let A::Imm(x) = c.a {
            return if x != 0 { t } else { f };
        }
        if t == f {
            return t;
        }
        let c = self.plain(c);
        self.op3(op3::CNDE_INT, c, f, t)
    }

    // ---- types ----

    fn ty_of(&self, fr: &Frame<'a>, h: Handle<Expression>) -> &'a TypeInner {
        let m: &'a Module = self.m;
        fr.fi[h].ty.inner_with(&m.types)
    }

    fn inner(&self, t: Handle<naga::Type>) -> &'a TypeInner {
        let m: &'a Module = self.m;
        &m.types[t].inner
    }

    fn nscalars(&self, t: &TypeInner) -> R<u32> {
        Ok(match *t {
            TypeInner::Scalar(_) | TypeInner::Atomic(_) => 1,
            TypeInner::Vector { size: s, .. } => size(s),
            TypeInner::Matrix { columns, rows, .. } => size(columns) * size(rows),
            TypeInner::Array { base, size: ArraySize::Constant(n), .. } => n.get() * self.nscalars(self.inner(base))?,
            TypeInner::Struct { ref members, .. } => {
                let mut n = 0;
                for mb in members {
                    n += self.nscalars(self.inner(mb.ty))?;
                }
                n
            }
            ref other => return unsupported(format!("value of type {other:?}")),
        })
    }

    /// Byte offset of each scalar of a host-shareable type.
    fn byte_offsets(&self, t: &TypeInner) -> R<Vec<u32>> {
        Ok(match *t {
            TypeInner::Scalar(_) | TypeInner::Atomic(_) => vec![0],
            TypeInner::Vector { size: s, .. } => (0..size(s)).map(|i| 4 * i).collect(),
            TypeInner::Matrix { columns, rows, .. } => {
                let cs = col_stride(rows);
                (0..size(columns)).flat_map(|c| (0..size(rows)).map(move |r| c * cs + 4 * r)).collect()
            }
            TypeInner::Array { base, size: ArraySize::Constant(n), stride } => {
                let inner = self.byte_offsets(self.inner(base))?;
                (0..n.get()).flat_map(|i| inner.iter().map(move |o| i * stride + o)).collect()
            }
            TypeInner::Struct { ref members, .. } => {
                let mut out = Vec::new();
                for mb in members {
                    out.extend(self.byte_offsets(self.inner(mb.ty))?.into_iter().map(|o| o + mb.offset));
                }
                out
            }
            ref other => return unsupported(format!("buffer data of type {other:?}")),
        })
    }

    fn kind(t: &TypeInner) -> ScalarKind {
        t.scalar_kind().unwrap_or(ScalarKind::Float)
    }

    // ---- constants ----

    fn const_val(&self, h: Handle<Expression>) -> R<Vec<u32>> {
        Ok(match self.m.global_expressions[h] {
            Expression::Literal(ref l) => vec![lit(l)?],
            Expression::Constant(c) => self.const_val(self.m.constants[c].init)?,
            Expression::ZeroValue(ty) => vec![0; self.nscalars(self.inner(ty))? as usize],
            Expression::Compose { ref components, .. } => {
                let mut v = Vec::new();
                for c in components {
                    v.extend(self.const_val(*c)?);
                }
                v
            }
            Expression::Splat { size: s, value } => {
                let x = self.const_val(value)?;
                (0..size(s)).map(|_| x[0]).collect()
            }
            ref other => return unsupported(format!("constant expression {other:?} (process overrides first)")),
        })
    }

    // ---- functions ----

    fn has_nontail_return(b: &Block) -> bool {
        fn any_return(b: &Block) -> bool {
            b.iter().any(|s| match s {
                Statement::Return { .. } => true,
                Statement::Block(b) => any_return(b),
                Statement::If { accept, reject, .. } => any_return(accept) || any_return(reject),
                Statement::Loop { body, continuing, .. } => any_return(body) || any_return(continuing),
                Statement::Switch { cases, .. } => cases.iter().any(|c| any_return(&c.body)),
                _ => false,
            })
        }
        let n = b.len();
        b.iter().enumerate().any(|(i, s)| match s {
            Statement::Return { .. } => i + 1 != n,
            Statement::Block(inner) => any_return(inner),
            Statement::If { accept, reject, .. } => any_return(accept) || any_return(reject),
            Statement::Loop { body, continuing, .. } => any_return(body) || any_return(continuing),
            Statement::Switch { cases, .. } => cases.iter().any(|c| any_return(&c.body)),
            _ => false,
        })
    }

    fn has_continue(b: &Block) -> bool {
        b.iter().any(|s| match s {
            Statement::Continue => true,
            Statement::Block(b) => Self::has_continue(b),
            Statement::If { accept, reject, .. } => Self::has_continue(accept) || Self::has_continue(reject),
            Statement::Switch { cases, .. } => cases.iter().any(|c| Self::has_continue(&c.body)),
            _ => false,
        })
    }

    fn run_function(&mut self, f: &'a Function, fi: &'a FunctionInfo, args: Vec<Lv>) -> R<Option<Vec<V>>> {
        let id = self.frames;
        self.frames += 1;
        let ret = match &f.result {
            Some(r) => {
                let n = self.nscalars(self.inner(r.ty))?;
                Some((0..n).map(|_| self.fresh()).collect::<Vec<_>>())
            }
            None => None,
        };
        let mut fr = Frame { id, f, fi, args, exprs: HashMap::new(), ret, ret_flag: None, ctx: Vec::new() };
        for (h, var) in f.local_variables.iter() {
            let n = self.nscalars(self.inner(var.ty))?;
            let init = match var.init {
                Some(e) => self.val(&mut fr, e)?,
                None => vec![Arg::imm(0); n as usize],
            };
            let regs: Vec<V> = (0..n).map(|_| self.fresh()).collect();
            for (r, a) in regs.iter().zip(init) {
                self.assign(*r, a);
            }
            self.vars.insert((id, h), regs);
        }
        if Self::has_nontail_return(&f.body) {
            let flag = self.fresh();
            self.assign(flag, Arg::imm(0));
            fr.ret_flag = Some(flag);
            fr.ctx.push(Ctx { kind: CtxKind::Func, saw_ret: false, saw_cont: false, flag: None });
            self.blocks.push(Vec::new());
            self.block(&mut fr, &f.body)?;
            self.push(Inst::Break);
            let body = self.blocks.pop().unwrap();
            self.push(Inst::Loop { body });
        } else {
            self.block(&mut fr, &f.body)?;
        }
        Ok(fr.ret)
    }

    fn ret_check(&mut self, fr: &Frame<'a>) {
        let flag = fr.ret_flag.expect("return flag");
        self.push(Inst::If { cond: flag, then: vec![Inst::Break], els: vec![] });
    }

    fn emit_continue(&mut self, fr: &mut Frame<'a>, i: usize) -> R<()> {
        match fr.ctx.get(i).map(|c| c.kind) {
            Some(CtxKind::Real) => self.push(Inst::Continue),
            Some(CtxKind::Body) => self.push(Inst::Break),
            Some(CtxKind::Switch) => {
                let flag = fr.ctx[i].flag.unwrap();
                fr.ctx[i].saw_cont = true;
                self.assign(flag, Arg::imm(TRUE));
                self.push(Inst::Break);
            }
            _ => return Err(Error::Invalid("continue outside a loop".into())),
        }
        Ok(())
    }

    fn block(&mut self, fr: &mut Frame<'a>, b: &'a Block) -> R<()> {
        for s in b.iter() {
            self.statement(fr, s)?;
        }
        Ok(())
    }

    fn sub_block(&mut self, fr: &mut Frame<'a>, b: &'a Block) -> R<Vec<Inst>> {
        self.blocks.push(Vec::new());
        self.block(fr, b)?;
        Ok(self.blocks.pop().unwrap())
    }

    fn statement(&mut self, fr: &mut Frame<'a>, s: &'a Statement) -> R<()> {
        match s {
            Statement::Emit(range) => {
                for h in range.clone() {
                    self.eval(fr, h)?;
                }
            }
            Statement::Block(b) => self.block(fr, b)?,
            Statement::If { condition, accept, reject } => {
                let c = self.val(fr, *condition)?[0];
                if let A::Imm(x) = c.a {
                    return self.block(fr, if x != 0 { accept } else { reject });
                }
                let cond = self.to_v(c);
                let then = self.sub_block(fr, accept)?;
                let els = self.sub_block(fr, reject)?;
                if !then.is_empty() || !els.is_empty() {
                    self.push(Inst::If { cond, then, els });
                }
            }
            Statement::Loop { body, continuing, break_if } => {
                let wrap = !continuing.is_empty() && Self::has_continue(body);
                self.blocks.push(Vec::new());
                fr.ctx.push(Ctx { kind: CtxKind::Real, saw_ret: false, saw_cont: false, flag: None });
                if wrap {
                    let brk = self.fresh();
                    self.assign(brk, Arg::imm(0));
                    fr.ctx.push(Ctx { kind: CtxKind::Body, saw_ret: false, saw_cont: false, flag: Some(brk) });
                    let mut inner = self.sub_block(fr, body)?;
                    inner.push(Inst::Break);
                    let c = fr.ctx.pop().unwrap();
                    self.push(Inst::Loop { body: inner });
                    if c.saw_ret {
                        self.ret_check(fr);
                    }
                    self.push(Inst::If { cond: brk, then: vec![Inst::Break], els: vec![] });
                } else {
                    self.block(fr, body)?;
                }
                self.block(fr, continuing)?;
                if let Some(bi) = break_if {
                    let c = self.val(fr, *bi)?[0];
                    match c.a {
                        A::Imm(0) => {}
                        A::Imm(_) => self.push(Inst::Break),
                        _ => {
                            let cond = self.to_v(c);
                            self.push(Inst::If { cond, then: vec![Inst::Break], els: vec![] });
                        }
                    }
                }
                let c = fr.ctx.pop().unwrap();
                let body = self.blocks.pop().unwrap();
                self.push(Inst::Loop { body });
                if c.saw_ret {
                    self.ret_check(fr);
                }
            }
            Statement::Switch { selector, cases } => {
                let sel = self.val(fr, *selector)?[0];
                let sel = self.plain(sel);
                let flag = self.fresh();
                self.assign(flag, Arg::imm(0));
                self.blocks.push(Vec::new());
                fr.ctx.push(Ctx { kind: CtxKind::Switch, saw_ret: false, saw_cont: false, flag: Some(flag) });
                let mut conds: Vec<Arg> = Vec::new();
                let mut has_default = false;
                let mut default_body = None;
                for case in cases {
                    match case.value {
                        SwitchValue::I32(v) => {
                            let c = self.op2(op2::SETE_INT, sel, Arg::imm(v as u32));
                            conds.push(c);
                        }
                        SwitchValue::U32(v) => {
                            let c = self.op2(op2::SETE_INT, sel, Arg::imm(v));
                            conds.push(c);
                        }
                        SwitchValue::Default => has_default = true,
                    }
                    if case.fall_through && case.body.is_empty() {
                        continue;
                    }
                    if case.fall_through {
                        return unsupported("switch fallthrough into a non-empty case");
                    }
                    if has_default {
                        default_body = Some(&case.body);
                    } else {
                        let mut any = conds[0];
                        for c in conds[1..].to_vec() {
                            any = self.iop(op2::OR_INT, any, c);
                        }
                        let cond = self.to_v(any);
                        let mut then = self.sub_block(fr, &case.body)?;
                        then.push(Inst::Break);
                        self.push(Inst::If { cond, then, els: vec![] });
                    }
                    conds.clear();
                    has_default = false;
                }
                if let Some(b) = default_body {
                    self.block(fr, b)?;
                }
                self.push(Inst::Break);
                let c = fr.ctx.pop().unwrap();
                let body = self.blocks.pop().unwrap();
                self.push(Inst::Loop { body });
                if c.saw_ret {
                    self.ret_check(fr);
                }
                if c.saw_cont {
                    self.blocks.push(Vec::new());
                    let below = fr.ctx.len() - 1;
                    self.emit_continue(fr, below)?;
                    let then = self.blocks.pop().unwrap();
                    self.push(Inst::If { cond: flag, then, els: vec![] });
                }
            }
            Statement::Break => match fr.ctx.last().map(|c| (c.kind, c.flag)) {
                Some((CtxKind::Real | CtxKind::Switch, _)) => self.push(Inst::Break),
                Some((CtxKind::Body, Some(flag))) => {
                    self.assign(flag, Arg::imm(TRUE));
                    self.push(Inst::Break);
                }
                _ => return Err(Error::Invalid("break outside a loop".into())),
            },
            Statement::Continue => {
                let i = fr.ctx.len().wrapping_sub(1);
                self.emit_continue(fr, i)?;
            }
            Statement::Return { value } => {
                if let Some(v) = value {
                    let vals = self.val(fr, *v)?;
                    let regs = fr.ret.clone().ok_or_else(|| Error::Invalid("return value from a void function".into()))?;
                    for (r, a) in regs.iter().zip(vals) {
                        self.assign(*r, a);
                    }
                }
                if let Some(flag) = fr.ret_flag {
                    self.assign(flag, Arg::imm(TRUE));
                    for c in fr.ctx.iter_mut().skip(1) {
                        c.saw_ret = true;
                    }
                    self.push(Inst::Break);
                }
            }
            Statement::Kill => {
                self.out.kills = true;
                self.push(Inst::Kill { op: op2::KILLGT, a: [Arg::f(1.0), Arg::imm(0)] });
            }
            Statement::Store { pointer, value } => {
                let p = self.ptr(fr, *pointer)?;
                let v = self.val(fr, *value)?;
                self.write(&p, v)?;
            }
            Statement::Call { function, arguments, result } => {
                let mut args = Vec::new();
                for a in arguments {
                    args.push(self.eval(fr, *a)?);
                }
                let m: &'a Module = self.m;
                let info: &'a ModuleInfo = self.info;
                let r = self.run_function(&m.functions[*function], &info[*function], args)?;
                if let (Some(res), Some(regs)) = (result, r) {
                    fr.exprs.insert(*res, Lv::Val(regs.into_iter().map(Arg::v).collect()));
                }
            }
            Statement::ControlBarrier(_) | Statement::MemoryBarrier(_) => {}
            other => return unsupported(format!("statement {other:?}")),
        }
        Ok(())
    }

    // ---- memory ----

    fn ptr(&mut self, fr: &mut Frame<'a>, h: Handle<Expression>) -> R<Ptr> {
        match self.eval(fr, h)? {
            Lv::Ptr(p) => Ok(p),
            other => Err(Error::Invalid(format!("expected a pointer, found {other:?}"))),
        }
    }

    fn var_regs(&self, root: &Root) -> R<Vec<V>> {
        Ok(match root {
            Root::Local(f, h) => self.vars[&(*f, *h)].clone(),
            Root::Private(g) => self.privates[g].clone(),
            _ => return Err(Error::Invalid("not a register variable".into())),
        })
    }

    fn write(&mut self, p: &Ptr, vals: Vec<Arg>) -> R<()> {
        if p.in_buffer() {
            return unsupported("writes to buffers");
        }
        let regs = self.var_regs(&p.root)?;
        match p.dynamic {
            None => {
                for (i, a) in vals.into_iter().enumerate() {
                    self.assign(regs[p.off as usize + i], a);
                }
            }
            Some(Dyn::Scalars { idx, stride, count }) => {
                for e in 0..count {
                    let c = self.op2(op2::SETE_INT, Arg::v(idx), Arg::imm(e));
                    for (i, a) in vals.iter().enumerate() {
                        let r = regs[(p.off + e * stride) as usize + i];
                        let s = self.select(c, *a, Arg::v(r));
                        self.assign(r, s);
                    }
                }
            }
            Some(Dyn::Bytes { .. }) => unreachable!(),
        }
        Ok(())
    }

    fn read(&mut self, p: &Ptr) -> R<Vec<Arg>> {
        match p.root {
            Root::Uniform(block) if p.dynamic.is_none() => {
                let offs = self.byte_offsets(&p.ty)?;
                Ok(offs
                    .iter()
                    .map(|o| {
                        let b = p.off + o;
                        Arg { a: A::Ub { block, index: (b / 16) as u16, chan: ((b % 16) / 4) as u8 }, neg: false, abs: false }
                    })
                    .collect())
            }
            Root::Uniform(loc) | Root::Storage(loc) => {
                let offs = self.byte_offsets(&p.ty)?;
                let swap = matches!(p.root, Root::Storage(_));
                let buffer = 128 + loc;
                match p.dynamic {
                    None => {
                        let idx = self.mov(Arg::imm(0));
                        self.fetch(buffer, idx, p.off, &offs, swap)
                    }
                    Some(Dyn::Bytes { v, aligned16: true }) => {
                        let idx = self.iop(op2::LSHR_INT, Arg::v(v), Arg::imm(4));
                        let idx = self.to_v(idx);
                        self.fetch(buffer, idx, p.off, &offs, swap)
                    }
                    Some(Dyn::Bytes { v, aligned16: false }) => {
                        // the 16-byte element holding each dword, then the dword inside it
                        let mut out = Vec::new();
                        for o in offs {
                            let t = self.iop(op2::ADD_INT, Arg::v(v), Arg::imm(p.off + o));
                            let idx = self.iop(op2::LSHR_INT, t, Arg::imm(4));
                            let idx = self.to_v(idx);
                            let ch = self.iop(op2::LSHR_INT, t, Arg::imm(2));
                            let ch = self.iop(op2::AND_INT, ch, Arg::imm(3));
                            let q = self.fetch(buffer, idx, 0, &[0, 4, 8, 12], swap)?;
                            let mut r = q[0];
                            for (c, x) in q.iter().enumerate().skip(1) {
                                let hit = self.iop(op2::SETE_INT, ch, Arg::imm(c as u32));
                                r = self.select(hit, *x, r);
                            }
                            out.push(r);
                        }
                        Ok(out)
                    }
                    Some(Dyn::Scalars { .. }) => unreachable!(),
                }
            }
            Root::Local(..) | Root::Private(_) => {
                let regs = self.var_regs(&p.root)?;
                let n = self.nscalars(&p.ty)?;
                let mut out = Vec::new();
                match p.dynamic {
                    None => {
                        for i in 0..n {
                            let v = self.mov(Arg::v(regs[(p.off + i) as usize]));
                            out.push(Arg::v(v));
                        }
                    }
                    Some(Dyn::Scalars { idx, stride, count }) => {
                        for i in 0..n {
                            let mut r = Arg::v(regs[(p.off + i) as usize]);
                            if count == 1 {
                                r = Arg::v(self.mov(r));
                            }
                            for e in 1..count {
                                let c = self.op2(op2::SETE_INT, Arg::v(idx), Arg::imm(e));
                                let other = Arg::v(regs[(p.off + e * stride + i) as usize]);
                                r = self.select(c, other, r);
                            }
                            out.push(r);
                        }
                    }
                    Some(Dyn::Bytes { .. }) => unreachable!(),
                }
                Ok(out)
            }
        }
    }

    /// Read dwords at `idx * 16 + base + offs[i]` bytes (buffers are fetched as 16-byte elements), merging runs
    /// of up to four consecutive dwords.
    fn fetch(&mut self, buffer: u8, idx: V, base: u32, offs: &[u32], swap: bool) -> R<Vec<Arg>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < offs.len() {
            let mut n = 1;
            while n < 4 && i + n < offs.len() && offs[i + n] == offs[i] + 4 * n as u32 {
                n += 1;
            }
            let mut offset = base + offs[i];
            if offset % 4 != 0 {
                return unsupported(format!("buffer offset {offset}"));
            }
            let mut idx = idx;
            if offset > 0xFFF0 {
                // past the offset field: move whole elements into the index
                let moved = self.iop(op2::ADD_INT, Arg::v(idx), Arg::imm(offset / 16));
                idx = self.to_v(moved);
                offset %= 16;
            }
            let mut dst = [None; 4];
            for d in dst.iter_mut().take(n) {
                *d = Some(self.fresh());
            }
            self.p.groups.push(dst);
            self.push(Inst::Vtx { buffer, index: idx, offset: offset as u16, dst, count: n as u8, swap });
            out.extend(dst.iter().take(n).map(|d| Arg::v(d.unwrap())));
            i += n;
        }
        Ok(out)
    }

    fn ptr_index(&self, p: &Ptr, k: u32) -> R<Ptr> {
        let bytes = p.in_buffer();
        let (ty, step) = self.element(&p.ty, k, bytes)?;
        Ok(Ptr { root: p.root.clone(), ty, off: p.off + step, dynamic: p.dynamic.clone() })
    }

    /// The type of element `k` of an aggregate, and its offset (bytes or scalars).
    fn element(&self, t: &TypeInner, k: u32, bytes: bool) -> R<(TypeInner, u32)> {
        Ok(match *t {
            TypeInner::Vector { scalar, .. } => (TypeInner::Scalar(scalar), if bytes { 4 * k } else { k }),
            TypeInner::Matrix { rows, scalar, .. } => (
                TypeInner::Vector { size: rows, scalar },
                if bytes { k * col_stride(rows) } else { k * size(rows) },
            ),
            TypeInner::Array { base, stride, .. } => {
                let inner = self.inner(base).clone();
                let step = if bytes { k * stride } else { k * self.nscalars(&inner)? };
                (inner, step)
            }
            TypeInner::Struct { ref members, .. } => {
                let mb = &members[k as usize];
                let mut off = 0;
                if !bytes {
                    for prev in &members[..k as usize] {
                        off += self.nscalars(self.inner(prev.ty))?;
                    }
                } else {
                    off = mb.offset;
                }
                (self.inner(mb.ty).clone(), off)
            }
            ref other => return unsupported(format!("indexing into {other:?}")),
        })
    }

    /// Element count and element size (bytes or scalars) for dynamic indexing.
    fn dyn_shape(&self, t: &TypeInner, bytes: bool) -> R<(TypeInner, u32, u32)> {
        Ok(match *t {
            TypeInner::Vector { size: s, scalar } => (TypeInner::Scalar(scalar), if bytes { 4 } else { 1 }, size(s)),
            TypeInner::Matrix { columns, rows, scalar } => (
                TypeInner::Vector { size: rows, scalar },
                if bytes { col_stride(rows) } else { size(rows) },
                size(columns),
            ),
            TypeInner::Array { base, stride, size: sz } => {
                let inner = self.inner(base).clone();
                let count = match sz {
                    ArraySize::Constant(n) => n.get(),
                    _ => u32::MAX,
                };
                let step = if bytes { stride } else { self.nscalars(&inner)? };
                (inner, step, count)
            }
            ref other => return unsupported(format!("dynamic index into {other:?}")),
        })
    }

    fn ptr_dyn(&mut self, p: &Ptr, idx: Arg) -> R<Ptr> {
        let bytes = p.in_buffer();
        let (ty, step, count) = self.dyn_shape(&p.ty, bytes)?;
        let idx = self.plain(idx);
        if bytes {
            if step % 4 != 0 {
                return unsupported("buffer element stride not a multiple of 4");
            }
            let scaled = if step.is_power_of_two() {
                self.iop(op2::LSHL_INT, idx, Arg::imm(step.trailing_zeros()))
            } else {
                self.iop(op2::MULLO_INT, idx, Arg::imm(step))
            };
            let (total, aligned) = match p.dynamic {
                Some(Dyn::Bytes { v, aligned16 }) => (self.iop(op2::ADD_INT, scaled, Arg::v(v)), aligned16),
                _ => (scaled, true),
            };
            let v = self.to_v(total);
            let aligned16 = aligned && step % 16 == 0;
            Ok(Ptr { root: p.root.clone(), ty, off: p.off, dynamic: Some(Dyn::Bytes { v, aligned16 }) })
        } else {
            if p.dynamic.is_some() {
                return unsupported("two dynamic indices into one local variable");
            }
            let v = self.to_v(idx);
            Ok(Ptr { root: p.root.clone(), ty, off: p.off, dynamic: Some(Dyn::Scalars { idx: v, stride: step, count }) })
        }
    }

    // ---- expressions ----

    fn eval(&mut self, fr: &mut Frame<'a>, h: Handle<Expression>) -> R<Lv> {
        if let Some(v) = fr.exprs.get(&h) {
            return Ok(v.clone());
        }
        let lv = self.eval_new(fr, h)?;
        fr.exprs.insert(h, lv.clone());
        Ok(lv)
    }

    fn val(&mut self, fr: &mut Frame<'a>, h: Handle<Expression>) -> R<Vec<Arg>> {
        match self.eval(fr, h)? {
            Lv::Val(v) => Ok(v),
            other => Err(Error::Invalid(format!("expected a value, found {other:?}"))),
        }
    }

    fn global_lv(&mut self, g: Handle<GlobalVariable>) -> R<Lv> {
        let gv = &self.m.global_variables[g];
        let ty = self.inner(gv.ty).clone();
        let binding = || gv.binding.ok_or_else(|| Error::Invalid("resource without a binding".into()));
        Ok(match gv.space {
            AddressSpace::Handle => match ty {
                TypeInner::Image { .. } => Lv::Image(g),
                TypeInner::Sampler { .. } => Lv::Sampler(g),
                other => return unsupported(format!("handle of type {other:?}")),
            },
            AddressSpace::Uniform => {
                let b = binding()?;
                let block = *self.opts.uniform_blocks.get(&b).ok_or(Error::Unbound(b))?;
                Lv::Ptr(Ptr { root: Root::Uniform(block), ty, off: 0, dynamic: None })
            }
            AddressSpace::Storage { .. } => {
                let b = binding()?;
                let buf = *self.opts.storage_buffers.get(&b).ok_or(Error::Unbound(b))?;
                Lv::Ptr(Ptr { root: Root::Storage(buf), ty, off: 0, dynamic: None })
            }
            AddressSpace::Private => Lv::Ptr(Ptr { root: Root::Private(g), ty, off: 0, dynamic: None }),
            other => return unsupported(format!("address space {other:?}")),
        })
    }

    fn eval_new(&mut self, fr: &mut Frame<'a>, h: Handle<Expression>) -> R<Lv> {
        let e: &'a Expression = &fr.f.expressions[h];
        Ok(match *e {
            Expression::Literal(ref l) => Lv::Val(vec![Arg::imm(lit(l)?)]),
            Expression::Constant(c) => {
                Lv::Val(self.const_val(self.m.constants[c].init)?.into_iter().map(Arg::imm).collect())
            }
            Expression::Override(_) => return unsupported("override (process overrides first)"),
            Expression::ZeroValue(ty) => Lv::Val(vec![Arg::imm(0); self.nscalars(self.inner(ty))? as usize]),
            Expression::Compose { ref components, .. } => {
                let mut v = Vec::new();
                for c in components {
                    v.extend(self.val(fr, *c)?);
                }
                Lv::Val(v)
            }
            Expression::AccessIndex { base, index } => self.access(fr, base, index)?,
            Expression::Access { base, index } => {
                let i = self.val(fr, index)?[0];
                if let (A::Imm(k), false, false) = (i.a, i.neg, i.abs) {
                    // a runtime index that folded to a constant: clamp it the way WGSL clamps indices
                    let t = match self.eval(fr, base)? {
                        Lv::Ptr(p) => p.ty,
                        _ => self.ty_of(fr, base).clone(),
                    };
                    let count = self.dyn_shape(&t, false).map(|s| s.2).unwrap_or(u32::MAX);
                    let k = if (k as i32) < 0 { 0 } else { k.min(count.saturating_sub(1)) };
                    return self.access(fr, base, k);
                }
                match self.eval(fr, base)? {
                    Lv::Ptr(p) => Lv::Ptr(self.ptr_dyn(&p, i)?),
                    Lv::Val(v) => {
                        let t = self.ty_of(fr, base).clone();
                        let (et, step, count) = self.dyn_shape(&t, false)?;
                        let n = self.nscalars(&et)?;
                        let idx = self.plain(i);
                        let mut out = Vec::new();
                        for c in 0..n {
                            let mut r = v[c as usize];
                            for e in 1..count {
                                let hit = self.op2(op2::SETE_INT, idx, Arg::imm(e));
                                r = self.select(hit, v[(e * step + c) as usize], r);
                            }
                            out.push(r);
                        }
                        Lv::Val(out)
                    }
                    other => return unsupported(format!("dynamic index into {other:?}")),
                }
            }
            Expression::Splat { size: s, value } => {
                let x = self.val(fr, value)?[0];
                Lv::Val(vec![x; size(s) as usize])
            }
            Expression::Swizzle { size: s, vector, pattern } => {
                let v = self.val(fr, vector)?;
                Lv::Val((0..size(s) as usize).map(|i| v[pattern[i] as usize]).collect())
            }
            Expression::FunctionArgument(i) => fr.args[i as usize].clone(),
            Expression::GlobalVariable(g) => self.global_lv(g)?,
            Expression::LocalVariable(l) => {
                let ty = self.inner(fr.f.local_variables[l].ty).clone();
                Lv::Ptr(Ptr { root: Root::Local(fr.id, l), ty, off: 0, dynamic: None })
            }
            Expression::Load { pointer } => {
                let p = self.ptr(fr, pointer)?;
                Lv::Val(self.read(&p)?)
            }
            Expression::ImageSample {
                image,
                sampler,
                gather,
                coordinate,
                array_index,
                offset,
                level,
                depth_ref,
                clamp_to_edge: _,
            } => {
                if gather.is_some() {
                    return unsupported("textureGather");
                }
                let (resource, dim, depth) = self.image(fr, image)?;
                let sampler = match self.eval(fr, sampler)? {
                    Lv::Sampler(g) => self.binding_of(g, &self.opts.samplers)?,
                    other => return Err(Error::Invalid(format!("not a sampler: {other:?}"))),
                };
                let mut coord: Vec<Arg> = self.val(fr, coordinate)?;
                if let Some(ai) = array_index {
                    let layer = self.val(fr, ai)?[0];
                    let layer = self.op1(op2::INT_TO_FLT, layer);
                    coord.push(layer);
                }
                let _ = dim;
                while coord.len() < 3 {
                    coord.push(Arg::imm(0));
                }
                let mut w: Option<Arg> = None;
                let inst = match (level, depth_ref) {
                    (SampleLevel::Auto, None) => texop::SAMPLE,
                    (SampleLevel::Zero, None) => texop::SAMPLE_LZ,
                    (SampleLevel::Exact(l), None) => {
                        w = Some(self.val(fr, l)?[0]);
                        texop::SAMPLE_L
                    }
                    (SampleLevel::Bias(b), None) => {
                        w = Some(self.val(fr, b)?[0]);
                        texop::SAMPLE_LB
                    }
                    (SampleLevel::Auto, Some(r)) => {
                        w = Some(self.val(fr, r)?[0]);
                        texop::SAMPLE_C
                    }
                    (SampleLevel::Zero, Some(r)) => {
                        w = Some(self.val(fr, r)?[0]);
                        texop::SAMPLE_C_LZ
                    }
                    (SampleLevel::Gradient { x, y }, None) => {
                        let gx = self.val(fr, x)?;
                        let gy = self.val(fr, y)?;
                        let mut off = [0i8; 3];
                        if let Some(o) = offset {
                            off = self.texel_offset(fr, o)?;
                        }
                        let pad = |g: Vec<Arg>| [g.first().copied(), g.get(1).copied(), g.get(2).copied(), None];
                        let srcs = [Some(coord[0]), Some(coord[1]), Some(coord[2]), None];
                        let r = self.tex_seq(
                            &[
                                (texop::SET_GRADIENTS_H as u8, pad(gx), false),
                                (texop::SET_GRADIENTS_V as u8, pad(gy), false),
                                (texop::SAMPLE_G as u8, srcs, true),
                            ],
                            resource,
                            sampler,
                            off,
                        )?;
                        return Ok(Lv::Val(if depth { vec![r[0]] } else { r }));
                    }
                    (other, _) => return unsupported(format!("sample level {other:?}")),
                };
                let mut off = [0i8; 3];
                if let Some(o) = offset {
                    off = self.texel_offset(fr, o)?;
                }
                let srcs = [Some(coord[0]), Some(coord[1]), Some(coord[2]), w];
                let r = self.tex(inst as u8, srcs, resource, sampler, off, true)?;
                Lv::Val(if depth { vec![r[0]] } else { r })
            }
            Expression::ImageLoad { image, coordinate, array_index, sample, level } => {
                if sample.is_some() {
                    return unsupported("multisampled textureLoad");
                }
                let (resource, _, depth) = self.image(fr, image)?;
                let mut coord = self.val(fr, coordinate)?;
                if let Some(ai) = array_index {
                    coord.push(self.val(fr, ai)?[0]);
                }
                while coord.len() < 3 {
                    coord.push(Arg::imm(0));
                }
                let lod = match level {
                    Some(l) => self.val(fr, l)?[0],
                    None => Arg::imm(0),
                };
                let srcs = [Some(coord[0]), Some(coord[1]), Some(coord[2]), Some(lod)];
                let r = self.tex(texop::LD as u8, srcs, resource, 0, [0; 3], false)?;
                Lv::Val(if depth { vec![r[0]] } else { r })
            }
            Expression::ImageQuery { image, query } => {
                let (resource, dim, _) = self.image(fr, image)?;
                let lod = match query {
                    ImageQuery::Size { level: Some(l) } => self.val(fr, l)?[0],
                    _ => Arg::imm(0),
                };
                let r = self.tex(
                    texop::GET_TEXTURE_INFO as u8,
                    [Some(lod), None, None, None],
                    resource,
                    0,
                    [0; 3],
                    false,
                )?;
                Lv::Val(match query {
                    ImageQuery::Size { .. } => match dim {
                        ImageDimension::D1 => vec![r[0]],
                        ImageDimension::D2 | ImageDimension::Cube => vec![r[0], r[1]],
                        ImageDimension::D3 => vec![r[0], r[1], r[2]],
                    },
                    ImageQuery::NumLevels => vec![r[3]],
                    ImageQuery::NumLayers => vec![r[2]],
                    ImageQuery::NumSamples => vec![Arg::imm(1)],
                })
            }
            Expression::Unary { op, expr } => {
                let kind = Self::kind(self.ty_of(fr, expr));
                let v = self.val(fr, expr)?;
                Lv::Val(
                    v.into_iter()
                        .map(|x| match (op, kind) {
                            (UnaryOperator::Negate, ScalarKind::Float) => x.neg(),
                            (UnaryOperator::Negate, _) => self.iop(op2::SUB_INT, Arg::imm(0), x),
                            (UnaryOperator::LogicalNot | UnaryOperator::BitwiseNot, _) => {
                                if let A::Imm(b) = x.a {
                                    return Arg::imm(!b);
                                }
                                let x = self.plain(x);
                                self.op1(op2::NOT_INT, x)
                            }
                        })
                        .collect(),
                )
            }
            Expression::Binary { op, left, right } => Lv::Val(self.binary(fr, op, left, right)?),
            Expression::Select { condition, accept, reject } => {
                let c = self.val(fr, condition)?;
                let t = self.val(fr, accept)?;
                let f = self.val(fr, reject)?;
                Lv::Val((0..t.len()).map(|i| self.select(c[if c.len() == 1 { 0 } else { i }], t[i], f[i])).collect())
            }
            Expression::Derivative { axis, expr, .. } => {
                let v = self.val(fr, expr)?;
                let mut out = Vec::new();
                for chunk in v.chunks(4) {
                    let mut srcs = [None; 4];
                    for (i, a) in chunk.iter().enumerate() {
                        srcs[i] = Some(*a);
                    }
                    let gx = self.tex(texop::GET_GRADIENTS_H as u8, srcs, 0, 0, [0; 3], false)?;
                    let gy = self.tex(texop::GET_GRADIENTS_V as u8, srcs, 0, 0, [0; 3], false)?;
                    for i in 0..chunk.len() {
                        out.push(match axis {
                            naga::DerivativeAxis::X => gx[i],
                            naga::DerivativeAxis::Y => gy[i],
                            naga::DerivativeAxis::Width => self.fadd(gx[i].abs(), gy[i].abs()),
                        });
                    }
                }
                Lv::Val(out)
            }
            Expression::Relational { fun, argument } => {
                let v = self.val(fr, argument)?;
                Lv::Val(match fun {
                    RelationalFunction::All | RelationalFunction::Any => {
                        let op = if fun == RelationalFunction::All { op2::AND_INT } else { op2::OR_INT };
                        let mut acc = v[0];
                        for x in &v[1..] {
                            acc = self.iop(op, acc, *x);
                        }
                        vec![acc]
                    }
                    RelationalFunction::IsNan => v.iter().map(|x| self.op2(op2::SETNE_DX10, *x, *x)).collect(),
                    RelationalFunction::IsInf => v
                        .iter()
                        .map(|x| self.op2(op2::SETE_DX10, x.abs(), Arg::f(f32::INFINITY)))
                        .collect(),
                })
            }
            Expression::Math { fun, arg, arg1, arg2, arg3 } => {
                let t = self.ty_of(fr, arg).clone();
                let a = self.val(fr, arg)?;
                let b = match arg1 {
                    Some(x) => Some(self.val(fr, x)?),
                    None => None,
                };
                let c = match arg2 {
                    Some(x) => Some(self.val(fr, x)?),
                    None => None,
                };
                let d = match arg3 {
                    Some(x) => Some(self.val(fr, x)?),
                    None => None,
                };
                Lv::Val(self.math(fun, &t, a, b, c, d)?)
            }
            Expression::As { expr, kind, convert } => {
                let from = Self::kind(self.ty_of(fr, expr));
                let v = self.val(fr, expr)?;
                if convert.is_none() || from == kind {
                    return Ok(Lv::Val(v));
                }
                Lv::Val(v.into_iter().map(|x| self.convert(x, from, kind)).collect::<R<Vec<_>>>()?)
            }
            Expression::CallResult(_) => {
                return Err(Error::Invalid("call result used before its call".into()));
            }
            ref other => return unsupported(format!("expression {other:?}")),
        })
    }

    fn binding_of(&self, g: Handle<GlobalVariable>, map: &HashMap<naga::ResourceBinding, u8>) -> R<u8> {
        let b = self.m.global_variables[g].binding.ok_or_else(|| Error::Invalid("resource without a binding".into()))?;
        map.get(&b).copied().ok_or(Error::Unbound(b))
    }

    /// (resource id, dimension, depth texture)
    fn image(&mut self, fr: &mut Frame<'a>, image: Handle<Expression>) -> R<(u8, ImageDimension, bool)> {
        let g = match self.eval(fr, image)? {
            Lv::Image(g) => g,
            other => return Err(Error::Invalid(format!("not a texture: {other:?}"))),
        };
        let res = self.binding_of(g, &self.opts.textures)?;
        match *self.inner(self.m.global_variables[g].ty) {
            TypeInner::Image { dim, class, .. } => Ok((res, dim, matches!(class, ImageClass::Depth { .. }))),
            _ => unreachable!(),
        }
    }

    fn texel_offset(&mut self, fr: &mut Frame<'a>, o: Handle<Expression>) -> R<[i8; 3]> {
        let mut off = [0i8; 3];
        for (i, a) in self.val(fr, o)?.iter().enumerate() {
            match a.a {
                // 1.3.1 fixed point: whole texels are twice the integer
                A::Imm(x) => off[i] = (x as i32 * 2) as i8,
                _ => return unsupported("non-constant texel offset"),
            }
        }
        Ok(off)
    }

    /// Several texture instructions back to back (one clause), their sources all copied first; the results of
    /// the ones marked `true`, concatenated.
    fn tex_seq(&mut self, items: &[(u8, [Option<Arg>; 4], bool)], resource: u8, sampler: u8, offset: [i8; 3]) -> R<Vec<Arg>> {
        let mut coords = Vec::new();
        for (_, srcs, _) in items {
            let mut coord = [None; 4];
            for (i, s) in srcs.iter().enumerate() {
                if let Some(a) = s {
                    coord[i] = Some(self.mov(*a));
                }
            }
            self.p.groups.push(coord);
            coords.push(coord);
        }
        let mut out = Vec::new();
        for ((inst, _, want), coord) in items.iter().zip(coords) {
            let dst = if *want {
                [Some(self.fresh()), Some(self.fresh()), Some(self.fresh()), Some(self.fresh())]
            } else {
                [None; 4]
            };
            if *want {
                self.p.groups.push(dst);
                out.extend(dst.iter().map(|d| Arg::v(d.unwrap())));
            }
            self.push(Inst::Tex { inst: *inst, coord, dst, resource, sampler, offset, normalized: true });
        }
        Ok(out)
    }

    /// A texture-clause instruction: sources copied into one register, four results in another.
    fn tex(
        &mut self,
        inst: u8,
        srcs: [Option<Arg>; 4],
        resource: u8,
        sampler: u8,
        offset: [i8; 3],
        normalized: bool,
    ) -> R<Vec<Arg>> {
        let mut coord = [None; 4];
        for (i, s) in srcs.iter().enumerate() {
            if let Some(a) = s {
                coord[i] = Some(self.mov(*a));
            }
        }
        self.p.groups.push(coord);
        let dst = [Some(self.fresh()), Some(self.fresh()), Some(self.fresh()), Some(self.fresh())];
        self.p.groups.push(dst);
        self.push(Inst::Tex { inst, coord, dst, resource, sampler, offset, normalized });
        Ok(dst.iter().map(|d| Arg::v(d.unwrap())).collect())
    }

    fn access(&mut self, fr: &mut Frame<'a>, base: Handle<Expression>, k: u32) -> R<Lv> {
        Ok(match self.eval(fr, base)? {
            Lv::Ptr(p) => Lv::Ptr(self.ptr_index(&p, k)?),
            Lv::Val(v) => {
                let t = self.ty_of(fr, base).clone();
                let (et, off) = self.element(&t, k, false)?;
                let n = self.nscalars(&et)?;
                Lv::Val(v[off as usize..(off + n) as usize].to_vec())
            }
            other => return unsupported(format!("index into {other:?}")),
        })
    }

    fn convert(&mut self, x: Arg, from: ScalarKind, to: ScalarKind) -> R<Arg> {
        use ScalarKind::*;
        Ok(match (from, to) {
            (Sint, Float) => self.op1(op2::INT_TO_FLT, x),
            (Uint, Float) => self.op1(op2::UINT_TO_FLT, x),
            (Float, Sint) => self.op1(op2::FLT_TO_INT, x),
            (Float, Uint) => self.op1(op2::FLT_TO_UINT, x),
            (Bool, Float) => self.select(x, Arg::f(1.0), Arg::imm(0)),
            (Bool, Sint | Uint) => self.iop(op2::AND_INT, x, Arg::imm(1)),
            (Float, Bool) => self.op2(op2::SETNE_DX10, x, Arg::imm(0)),
            (Sint | Uint, Bool) => {
                let x = self.plain(x);
                self.op2(op2::SETNE_INT, x, Arg::imm(0))
            }
            (Sint, Uint) | (Uint, Sint) => x,
            (a, b) => return unsupported(format!("conversion {a:?} -> {b:?}")),
        })
    }

    fn dot(&mut self, a: &[Arg], b: &[Arg]) -> Arg {
        let mut acc = self.fmul(a[0], b[0]);
        for i in 1..a.len() {
            acc = self.fmad(a[i], b[i], acc);
        }
        acc
    }

    fn binary(&mut self, fr: &mut Frame<'a>, op: BinaryOperator, l: Handle<Expression>, r: Handle<Expression>) -> R<Vec<Arg>> {
        let lt = self.ty_of(fr, l).clone();
        let rt = self.ty_of(fr, r).clone();
        let a = self.val(fr, l)?;
        let b = self.val(fr, r)?;
        if op == BinaryOperator::Multiply {
            match (&lt, &rt) {
                (TypeInner::Matrix { columns, rows, .. }, TypeInner::Vector { .. }) => {
                    let (c, rw) = (size(*columns) as usize, size(*rows) as usize);
                    return Ok((0..rw)
                        .map(|j| {
                            let col: Vec<Arg> = (0..c).map(|i| a[i * rw + j]).collect();
                            self.dot(&col, &b)
                        })
                        .collect());
                }
                (TypeInner::Vector { .. }, TypeInner::Matrix { columns, rows, .. }) => {
                    let (c, rw) = (size(*columns) as usize, size(*rows) as usize);
                    return Ok((0..c)
                        .map(|i| {
                            let col = b[i * rw..(i + 1) * rw].to_vec();
                            self.dot(&a, &col)
                        })
                        .collect());
                }
                (TypeInner::Matrix { columns: ac, rows: ar, .. }, TypeInner::Matrix { columns: bc, rows: br, .. }) => {
                    let (ac, ar, bc, br) = (size(*ac) as usize, size(*ar) as usize, size(*bc) as usize, size(*br) as usize);
                    let mut out = Vec::new();
                    for k in 0..bc {
                        for j in 0..ar {
                            let row: Vec<Arg> = (0..ac).map(|i| a[i * ar + j]).collect();
                            let col = b[k * br..(k + 1) * br].to_vec();
                            out.push(self.dot(&row, &col));
                        }
                    }
                    return Ok(out);
                }
                _ => {}
            }
        }
        let kind = Self::kind(&lt);
        let n = a.len().max(b.len());
        let mut out = Vec::new();
        for i in 0..n {
            let x = a[if a.len() == 1 { 0 } else { i }];
            let y = b[if b.len() == 1 { 0 } else { i }];
            out.push(self.scalar_binary(op, kind, x, y)?);
        }
        Ok(out)
    }

    fn scalar_binary(&mut self, op: BinaryOperator, kind: ScalarKind, x: Arg, y: Arg) -> R<Arg> {
        use BinaryOperator as B;
        Ok(match kind {
            ScalarKind::Float => match op {
                B::Add => self.fadd(x, y),
                B::Subtract => self.fadd(x, y.neg()),
                B::Multiply => self.fmul(x, y),
                B::Divide => {
                    let r = self.op1(op2::RECIP_IEEE, y);
                    self.fmul(x, r)
                }
                B::Modulo => {
                    let r = self.op1(op2::RECIP_IEEE, y);
                    let q = self.fmul(x, r);
                    let t = self.op1(op2::TRUNC, q);
                    self.fmad(t.neg(), y, x)
                }
                B::Equal => self.op2(op2::SETE_DX10, x, y),
                B::NotEqual => self.op2(op2::SETNE_DX10, x, y),
                B::Less => self.op2(op2::SETGT_DX10, y, x),
                B::LessEqual => self.op2(op2::SETGE_DX10, y, x),
                B::Greater => self.op2(op2::SETGT_DX10, x, y),
                B::GreaterEqual => self.op2(op2::SETGE_DX10, x, y),
                other => return unsupported(format!("float {other:?}")),
            },
            ScalarKind::Sint | ScalarKind::Uint | ScalarKind::Bool | ScalarKind::AbstractInt => {
                let signed = kind != ScalarKind::Uint;
                match op {
                    B::Add => self.iop(op2::ADD_INT, x, y),
                    B::Subtract => self.iop(op2::SUB_INT, x, y),
                    B::Multiply => self.iop(if signed { op2::MULLO_INT } else { op2::MULLO_UINT }, x, y),
                    B::Divide => self.idiv(x, y, signed)?.0,
                    B::Modulo => self.idiv(x, y, signed)?.1,
                    B::Equal => self.iop(op2::SETE_INT, x, y),
                    B::NotEqual => self.iop(op2::SETNE_INT, x, y),
                    B::Less => self.iop(if signed { op2::SETGT_INT } else { op2::SETGT_UINT }, y, x),
                    B::LessEqual => self.iop(if signed { op2::SETGE_INT } else { op2::SETGE_UINT }, y, x),
                    B::Greater => self.iop(if signed { op2::SETGT_INT } else { op2::SETGT_UINT }, x, y),
                    B::GreaterEqual => self.iop(if signed { op2::SETGE_INT } else { op2::SETGE_UINT }, x, y),
                    B::And | B::LogicalAnd => self.iop(op2::AND_INT, x, y),
                    B::InclusiveOr | B::LogicalOr => self.iop(op2::OR_INT, x, y),
                    B::ExclusiveOr => self.iop(op2::XOR_INT, x, y),
                    B::ShiftLeft => self.iop(op2::LSHL_INT, x, y),
                    B::ShiftRight => self.iop(if kind == ScalarKind::Sint { op2::ASHR_INT } else { op2::LSHR_INT }, x, y),
                }
            }
            other => return unsupported(format!("{other:?} arithmetic")),
        })
    }

    /// Integer division and remainder (truncating). Powers of two are exact; otherwise the quotient comes from a
    /// float reciprocal corrected by one step either way, exact for dividends below 2^31.
    fn idiv(&mut self, x: Arg, y: Arg, signed: bool) -> R<(Arg, Arg)> {
        if let A::Imm(d) = y.a {
            if !signed && d.is_power_of_two() {
                let q = self.iop(op2::LSHR_INT, x, Arg::imm(d.trailing_zeros()));
                let r = self.iop(op2::AND_INT, x, Arg::imm(d - 1));
                return Ok((q, r));
            }
        }
        let x = self.plain(x);
        let y = self.plain(y);
        if signed {
            let nx = self.iop(op2::SUB_INT, Arg::imm(0), x);
            let ax = self.iop(op2::MAX_INT, x, nx);
            let ny = self.iop(op2::SUB_INT, Arg::imm(0), y);
            let ay = self.iop(op2::MAX_INT, y, ny);
            let (q, _) = self.udiv(ax, ay);
            let s = self.iop(op2::XOR_INT, x, y);
            let neg = self.iop(op2::SETGT_INT, Arg::imm(0), s);
            let nq = self.iop(op2::SUB_INT, Arg::imm(0), q);
            let q = self.select(neg, nq, q);
            let qy = self.iop(op2::MULLO_INT, q, y);
            let r = self.iop(op2::SUB_INT, x, qy);
            Ok((q, r))
        } else {
            Ok(self.udiv(x, y))
        }
    }

    fn udiv(&mut self, x: Arg, y: Arg) -> (Arg, Arg) {
        let fx = self.op1(op2::UINT_TO_FLT, x);
        let fy = self.op1(op2::UINT_TO_FLT, y);
        let ry = self.op1(op2::RECIP_IEEE, fy);
        let fq = self.fmul(fx, ry);
        let fq = self.op1(op2::TRUNC, fq);
        let q = self.op1(op2::FLT_TO_UINT, fq);
        let qy = self.iop(op2::MULLO_UINT, q, y);
        let r = self.iop(op2::SUB_INT, x, qy);
        // one too many: r went negative
        let neg = self.iop(op2::SETGT_INT, Arg::imm(0), r);
        let q = self.iop(op2::ADD_INT, q, neg);
        let back = self.iop(op2::AND_INT, neg, y);
        let r = self.iop(op2::ADD_INT, r, back);
        // one too few
        let ge = self.iop(op2::SETGE_UINT, r, y);
        let q = self.iop(op2::SUB_INT, q, ge);
        let take = self.iop(op2::AND_INT, ge, y);
        let r = self.iop(op2::SUB_INT, r, take);
        (q, r)
    }

    fn sin_cos(&mut self, x: Arg, cos: bool) -> Arg {
        // R700's SIN/COS take the angle in turns, in [-0.5, 0.5]
        let t = self.fmad(x, Arg::f(1.0 / (2.0 * std::f32::consts::PI)), Arg::f(0.5));
        let t = self.op1(op2::FRACT, t);
        let t = self.fadd(t, Arg::f(-0.5));
        self.op1(if cos { op2::COS } else { op2::SIN }, t)
    }

    fn exp2(&mut self, x: Arg) -> Arg {
        self.op1(op2::EXP_IEEE, x)
    }

    fn log2(&mut self, x: Arg) -> Arg {
        self.op1(op2::LOG_IEEE, x)
    }

    fn recip(&mut self, x: Arg) -> Arg {
        self.op1(op2::RECIP_IEEE, x)
    }

    /// atan of a value in [0, 1].
    fn atan01(&mut self, x: Arg) -> Arg {
        let x2 = self.fmul(x, x);
        let mut p = Arg::f(-0.011_721_2);
        for c in [0.052_653_32, -0.116_432_87, 0.193_543_46, -0.332_623_47, 0.999_977_26] {
            p = self.fmad(p, x2, Arg::f(c));
        }
        self.fmul(p, x)
    }

    fn atan2(&mut self, y: Arg, x: Arg) -> Arg {
        use std::f32::consts::{FRAC_PI_2, PI};
        let ax = x.abs();
        let ay = y.abs();
        let mx = self.op2(op2::MAX_DX10, ax, ay);
        let mn = self.op2(op2::MIN_DX10, ax, ay);
        let r = self.recip(mx);
        let a = self.fmul(mn, r);
        let mut t = self.atan01(a);
        let swap = self.op2(op2::SETGT_DX10, ay, ax);
        let alt = self.fadd(Arg::f(FRAC_PI_2), t.neg());
        t = self.select(swap, alt, t);
        let xneg = self.op2(op2::SETGT_DX10, Arg::imm(0), x);
        let alt = self.fadd(Arg::f(PI), t.neg());
        t = self.select(xneg, alt, t);
        let yneg = self.op2(op2::SETGT_DX10, Arg::imm(0), y);
        self.select(yneg, t.neg(), t)
    }

    #[allow(clippy::too_many_arguments)]
    fn math(
        &mut self,
        fun: MathFunction,
        t: &TypeInner,
        a: Vec<Arg>,
        b: Option<Vec<Arg>>,
        c: Option<Vec<Arg>>,
        _d: Option<Vec<Arg>>,
    ) -> R<Vec<Arg>> {
        use MathFunction as M;
        let kind = Self::kind(t);
        let n = a.len();
        let bi = |v: &Option<Vec<Arg>>, i: usize| -> Arg {
            let v = v.as_ref().unwrap();
            v[if v.len() == 1 { 0 } else { i }]
        };
        let mut out = Vec::new();
        match fun {
            M::Dot => return Ok(vec![if kind == ScalarKind::Float {
                self.dot(&a, b.as_ref().unwrap())
            } else {
                let b = b.unwrap();
                let mut acc = self.iop(op2::MULLO_INT, a[0], b[0]);
                for i in 1..n {
                    let p = self.iop(op2::MULLO_INT, a[i], b[i]);
                    acc = self.iop(op2::ADD_INT, acc, p);
                }
                acc
            }]),
            M::Length => {
                let d = self.dot(&a, &a);
                return Ok(vec![self.op1(op2::SQRT_IEEE, d)]);
            }
            M::Distance => {
                let b = b.unwrap();
                let diff: Vec<Arg> = (0..n).map(|i| self.fadd(a[i], b[i].neg())).collect();
                let d = self.dot(&diff, &diff);
                return Ok(vec![self.op1(op2::SQRT_IEEE, d)]);
            }
            M::Normalize => {
                let d = self.dot(&a, &a);
                let r = self.op1(op2::RECIPSQRT_IEEE, d);
                return Ok(a.iter().map(|x| self.fmul(*x, r)).collect());
            }
            M::Cross => {
                let b = b.unwrap();
                let mut out = Vec::new();
                for (i, j) in [(1, 2), (2, 0), (0, 1)] {
                    let p = self.fmul(a[j], b[i]);
                    out.push(self.fmad(a[i], b[j], p.neg()));
                }
                return Ok(out);
            }
            M::Reflect => {
                let nrm = b.unwrap();
                let d = self.dot(&nrm, &a);
                let d2 = self.fmul(d, Arg::f(2.0));
                return Ok((0..n).map(|i| self.fmad(d2.neg(), nrm[i], a[i])).collect());
            }
            M::FaceForward => {
                let (i_, nref) = (b.unwrap(), c.unwrap());
                let d = self.dot(&nref, &i_);
                let neg = self.op2(op2::SETGT_DX10, Arg::imm(0), d);
                return Ok(a.iter().map(|x| self.select(neg, *x, x.neg())).collect());
            }
            M::Transpose => {
                if let TypeInner::Matrix { columns, rows, .. } = *t {
                    let (cn, rn) = (size(columns) as usize, size(rows) as usize);
                    return Ok((0..rn).flat_map(|r| (0..cn).map(move |cc| (cc, r))).map(|(cc, r)| a[cc * rn + r]).collect());
                }
                return unsupported("transpose of a non-matrix");
            }
            M::Determinant => {
                let m = |i: usize, j: usize| a[i * (n as f64).sqrt() as usize + j];
                let det2 = |s: &mut Self, p: Arg, q: Arg, r: Arg, w: Arg| {
                    let t = s.fmul(q, r);
                    s.fmad(p, w, t.neg())
                };
                return Ok(vec![match n {
                    4 => det2(self, m(0, 0), m(0, 1), m(1, 0), m(1, 1)),
                    9 => {
                        let c0 = det2(self, m(1, 1), m(1, 2), m(2, 1), m(2, 2));
                        let c1 = det2(self, m(1, 0), m(1, 2), m(2, 0), m(2, 2));
                        let c2 = det2(self, m(1, 0), m(1, 1), m(2, 0), m(2, 1));
                        let x = self.fmul(m(0, 0), c0);
                        let x = self.fmad(m(0, 1).neg(), c1, x);
                        self.fmad(m(0, 2), c2, x)
                    }
                    _ => return unsupported("4x4 determinant"),
                }]);
            }
            _ => {}
        }
        for i in 0..n {
            let x = a[i];
            out.push(match (fun, kind) {
                (M::Abs, ScalarKind::Float) => x.abs(),
                (M::Abs, _) => {
                    let nx = self.iop(op2::SUB_INT, Arg::imm(0), x);
                    self.iop(op2::MAX_INT, x, nx)
                }
                (M::Min, ScalarKind::Float) => self.op2(op2::MIN_DX10, x, bi(&b, i)),
                (M::Max, ScalarKind::Float) => self.op2(op2::MAX_DX10, x, bi(&b, i)),
                (M::Min, ScalarKind::Sint) => self.iop(op2::MIN_INT, x, bi(&b, i)),
                (M::Max, ScalarKind::Sint) => self.iop(op2::MAX_INT, x, bi(&b, i)),
                (M::Min, _) => self.iop(op2::MIN_UINT, x, bi(&b, i)),
                (M::Max, _) => self.iop(op2::MAX_UINT, x, bi(&b, i)),
                (M::Clamp, ScalarKind::Float) => {
                    let (lo, hi) = (bi(&b, i), bi(&c, i));
                    if lo.a == A::Imm(0) && hi.a == A::Imm(1f32.to_bits()) && !hi.neg {
                        self.sat(x)
                    } else {
                        let m = self.op2(op2::MAX_DX10, x, lo);
                        self.op2(op2::MIN_DX10, m, hi)
                    }
                }
                (M::Clamp, ScalarKind::Sint) => {
                    let m = self.iop(op2::MAX_INT, x, bi(&b, i));
                    self.iop(op2::MIN_INT, m, bi(&c, i))
                }
                (M::Clamp, _) => {
                    let m = self.iop(op2::MAX_UINT, x, bi(&b, i));
                    self.iop(op2::MIN_UINT, m, bi(&c, i))
                }
                (M::Saturate, _) => self.sat(x),
                (M::Sin, _) => self.sin_cos(x, false),
                (M::Cos, _) => self.sin_cos(x, true),
                (M::Tan, _) => {
                    let s = self.sin_cos(x, false);
                    let co = self.sin_cos(x, true);
                    let r = self.recip(co);
                    self.fmul(s, r)
                }
                (M::Atan, _) => self.atan2(x, Arg::f(1.0)),
                (M::Atan2, _) => self.atan2(x, bi(&b, i)),
                (M::Asin | M::Acos, _) => {
                    let one_minus = self.fmad(x.neg(), x, Arg::f(1.0));
                    let s = self.op1(op2::SQRT_IEEE, one_minus);
                    if fun == M::Asin { self.atan2(x, s) } else { self.atan2(s, x) }
                }
                (M::Sinh | M::Cosh | M::Tanh, _) => {
                    let e = self.fmul(x, Arg::f(std::f32::consts::LOG2_E));
                    let ep = self.exp2(e);
                    let en = self.exp2(e.neg());
                    match fun {
                        M::Sinh => {
                            let d = self.fadd(ep, en.neg());
                            self.fmul(d, Arg::f(0.5))
                        }
                        M::Cosh => {
                            let d = self.fadd(ep, en);
                            self.fmul(d, Arg::f(0.5))
                        }
                        _ => {
                            let num = self.fadd(ep, en.neg());
                            let den = self.fadd(ep, en);
                            let r = self.recip(den);
                            self.fmul(num, r)
                        }
                    }
                }
                (M::Radians, _) => self.fmul(x, Arg::f(std::f32::consts::PI / 180.0)),
                (M::Degrees, _) => self.fmul(x, Arg::f(180.0 / std::f32::consts::PI)),
                (M::Ceil, _) => self.op1(op2::CEIL, x),
                (M::Floor, _) => self.op1(op2::FLOOR, x),
                (M::Round, _) => self.op1(op2::RNDNE, x),
                (M::Fract, _) => self.op1(op2::FRACT, x),
                (M::Trunc, _) => self.op1(op2::TRUNC, x),
                (M::Exp, _) => {
                    let e = self.fmul(x, Arg::f(std::f32::consts::LOG2_E));
                    self.exp2(e)
                }
                (M::Exp2, _) => self.exp2(x),
                (M::Log, _) => {
                    let l = self.log2(x);
                    self.fmul(l, Arg::f(std::f32::consts::LN_2))
                }
                (M::Log2, _) => self.log2(x),
                (M::Pow, _) => {
                    let l = self.log2(x);
                    let p = self.fmul(l, bi(&b, i));
                    self.exp2(p)
                }
                (M::Sign, ScalarKind::Float) => {
                    let t = self.op3(op3::CNDGT, x.neg(), Arg::f(-1.0), Arg::imm(0));
                    self.op3(op3::CNDGT, x, Arg::f(1.0), t)
                }
                (M::Sign, _) => {
                    let lt = self.iop(op2::SETGT_INT, Arg::imm(0), x);
                    let gt = self.iop(op2::SETGT_INT, x, Arg::imm(0));
                    self.iop(op2::SUB_INT, lt, gt)
                }
                (M::Fma, _) => self.fmad(x, bi(&b, i), bi(&c, i)),
                (M::Mix, _) => {
                    let (y, tt) = (bi(&b, i), bi(&c, i));
                    let d = self.fadd(y, x.neg());
                    self.fmad(d, tt, x)
                }
                // step(edge, x): the edge is the first argument
                (M::Step, _) => self.op2(op2::SETGE, bi(&b, i), x),
                (M::SmoothStep, _) => {
                    let (e1, v) = (bi(&b, i), bi(&c, i));
                    let w = self.fadd(e1, x.neg());
                    let r = self.recip(w);
                    let d = self.fadd(v, x.neg());
                    let tt = self.fmul(d, r);
                    let tt = self.sat(tt);
                    let k = self.fmad(tt, Arg::f(-2.0), Arg::f(3.0));
                    let t2 = self.fmul(tt, tt);
                    self.fmul(t2, k)
                }
                (M::Sqrt, _) => self.op1(op2::SQRT_IEEE, x),
                (M::InverseSqrt, _) => self.op1(op2::RECIPSQRT_IEEE, x),
                (M::ExtractBits, _) => {
                    // (x >> offset) & ((1 << count) - 1), sign-extended for signed types
                    let (off, cnt) = (bi(&b, i), bi(&c, i));
                    let sh = self.iop(op2::SUB_INT, Arg::imm(32), cnt);
                    let l = self.iop(op2::SUB_INT, sh, off);
                    let up = self.iop(op2::LSHL_INT, x, l);
                    let r = self.iop(if kind == ScalarKind::Sint { op2::ASHR_INT } else { op2::LSHR_INT }, up, sh);
                    let zero = self.iop(op2::SETE_INT, cnt, Arg::imm(0));
                    self.select(zero, Arg::imm(0), r)
                }
                (M::InsertBits, _) => {
                    let (newb, off, cnt) = (bi(&b, i), bi(&c, i), bi(&_d, i));
                    let one = self.iop(op2::LSHL_INT, Arg::imm(1), cnt);
                    let mask = self.iop(op2::SUB_INT, one, Arg::imm(1));
                    let full = self.iop(op2::SETGE_UINT, cnt, Arg::imm(32));
                    let mask = self.select(full, Arg::imm(TRUE), mask);
                    let mask = self.iop(op2::LSHL_INT, mask, off);
                    let ins = self.iop(op2::LSHL_INT, newb, off);
                    let ins = self.iop(op2::AND_INT, ins, mask);
                    let keep = self.iop(op2::NOT_INT, mask, Arg::imm(0));
                    let kept = self.iop(op2::AND_INT, x, keep);
                    self.iop(op2::OR_INT, kept, ins)
                }
                (M::Unpack4x8unorm | M::Unpack4x8snorm | M::Unpack2x16unorm | M::Unpack2x16snorm, _) => {
                    let (count, bits) = if matches!(fun, M::Unpack4x8unorm | M::Unpack4x8snorm) { (4, 8) } else { (2, 16) };
                    let signed = matches!(fun, M::Unpack4x8snorm | M::Unpack2x16snorm);
                    let max = ((1u32 << (bits - signed as u32)) - 1) as f32;
                    let mut v = Vec::new();
                    for k in 0..count {
                        let comp = if signed {
                            let up = self.iop(op2::LSHL_INT, x, Arg::imm(32 - bits * (k + 1)));
                            let s = self.iop(op2::ASHR_INT, up, Arg::imm(32 - bits));
                            let f = self.op1(op2::INT_TO_FLT, s);
                            let f = self.fmul(f, Arg::f(1.0 / max));
                            self.op2(op2::MAX_DX10, f, Arg::f(-1.0))
                        } else {
                            let s = self.iop(op2::LSHR_INT, x, Arg::imm(bits * k));
                            let s = self.iop(op2::AND_INT, s, Arg::imm((1 << bits) - 1));
                            let f = self.op1(op2::UINT_TO_FLT, s);
                            self.fmul(f, Arg::f(1.0 / max))
                        };
                        v.push(comp);
                    }
                    return Ok(v);
                }
                (M::Pack4x8unorm | M::Pack2x16unorm, _) => {
                    let bits = if fun == M::Pack4x8unorm { 8 } else { 16 };
                    let max = ((1u32 << bits) - 1) as f32;
                    let mut acc = Arg::imm(0);
                    for (k, comp) in a.iter().enumerate() {
                        let s = self.sat(*comp);
                        let f = self.fmad(s, Arg::f(max), Arg::f(0.5));
                        let u = self.op1(op2::FLT_TO_UINT, f);
                        let sh = self.iop(op2::LSHL_INT, u, Arg::imm(bits * k as u32));
                        acc = self.iop(op2::OR_INT, acc, sh);
                    }
                    return Ok(vec![acc]);
                }
                (M::CountLeadingZeros, _) => {
                    // binary search: 16, 8, 4, 2, 1 leading zero bits at a time
                    let mut v = self.plain(x);
                    let mut n = Arg::imm(0);
                    for k in [16u32, 8, 4, 2, 1] {
                        let limit = if k == 32 { 0 } else { (1u32 << (32 - k)) - 1 };
                        let small = self.iop(op2::SETGE_UINT, Arg::imm(limit), v);
                        let add = self.iop(op2::AND_INT, small, Arg::imm(k));
                        n = self.iop(op2::ADD_INT, n, add);
                        let shifted = self.iop(op2::LSHL_INT, v, Arg::imm(k));
                        v = self.select(small, shifted, v);
                    }
                    let zero = self.iop(op2::SETE_INT, x, Arg::imm(0));
                    self.select(zero, Arg::imm(32), n)
                }
                (M::CountTrailingZeros, _) => {
                    let mut v = self.plain(x);
                    let mut n = Arg::imm(0);
                    for k in [16u32, 8, 4, 2, 1] {
                        let low = self.iop(op2::AND_INT, v, Arg::imm((1u32 << k) - 1));
                        let none = self.iop(op2::SETE_INT, low, Arg::imm(0));
                        let add = self.iop(op2::AND_INT, none, Arg::imm(k));
                        n = self.iop(op2::ADD_INT, n, add);
                        let shifted = self.iop(op2::LSHR_INT, v, Arg::imm(k));
                        v = self.select(none, shifted, v);
                    }
                    let zero = self.iop(op2::SETE_INT, x, Arg::imm(0));
                    self.select(zero, Arg::imm(32), n)
                }
                (M::CountOneBits, _) => {
                    let v = self.plain(x);
                    let h = self.iop(op2::LSHR_INT, v, Arg::imm(1));
                    let h = self.iop(op2::AND_INT, h, Arg::imm(0x5555_5555));
                    let v = self.iop(op2::SUB_INT, v, h);
                    let lo = self.iop(op2::AND_INT, v, Arg::imm(0x3333_3333));
                    let hi = self.iop(op2::LSHR_INT, v, Arg::imm(2));
                    let hi = self.iop(op2::AND_INT, hi, Arg::imm(0x3333_3333));
                    let v = self.iop(op2::ADD_INT, lo, hi);
                    let h = self.iop(op2::LSHR_INT, v, Arg::imm(4));
                    let v = self.iop(op2::ADD_INT, v, h);
                    let v = self.iop(op2::AND_INT, v, Arg::imm(0x0F0F_0F0F));
                    let v = self.iop(op2::MULLO_UINT, v, Arg::imm(0x0101_0101));
                    self.iop(op2::LSHR_INT, v, Arg::imm(24))
                }
                (other, k) => return unsupported(format!("{other:?} on {k:?}")),
            });
        }
        Ok(out)
    }

    // ---- entry points ----

    pub fn entry(&mut self, index: usize) -> R<()> {
        let m: &'a Module = self.m;
        let ep = &m.entry_points[index];
        let fi = self.info.get_entry_point(index);
        self.out.stage = Some(ep.stage);
        if !matches!(ep.stage, ShaderStage::Vertex | ShaderStage::Fragment) {
            return unsupported(format!("{:?} shaders", ep.stage));
        }
        for (g, gv) in m.global_variables.iter() {
            if gv.space == AddressSpace::Private {
                let n = self.nscalars(self.inner(gv.ty))?;
                let init = match gv.init {
                    Some(e) => self.const_val(e)?,
                    None => vec![0; n as usize],
                };
                let regs: Vec<V> = (0..n).map(|_| self.fresh()).collect();
                for (r, x) in regs.iter().zip(init) {
                    self.assign(*r, Arg::imm(x));
                }
                self.privates.insert(g, regs);
            }
        }
        // every input binding, flattened, in argument order
        let mut bindings: Vec<(Binding, u32)> = Vec::new();
        for arg in &ep.function.arguments {
            self.flatten_bindings(arg.binding.as_ref(), arg.ty, &mut bindings)?;
        }
        let mut locations: Vec<u32> = bindings
            .iter()
            .filter_map(|(b, _)| if let Binding::Location { location, .. } = b { Some(*location) } else { None })
            .collect();
        locations.sort();
        locations.dedup();
        let mut values: Vec<Arg> = Vec::new();
        let ps_base = locations.len() as u8;
        let mut next_special = ps_base;
        for (b, n) in &bindings {
            match (ep.stage, b) {
                (ShaderStage::Vertex, Binding::Location { location, .. }) => {
                    let gpr = 1 + locations.iter().position(|l| l == location).unwrap() as u8;
                    self.out.attribs.push((*location, gpr));
                    for c in 0..*n {
                        values.push(Arg::v(self.input(gpr, c as u8)));
                    }
                }
                (ShaderStage::Vertex, Binding::BuiltIn(BuiltIn::VertexIndex)) => {
                    self.out.uses_vertex_index = true;
                    values.push(Arg::v(self.input(0, 0)));
                }
                (ShaderStage::Vertex, Binding::BuiltIn(BuiltIn::InstanceIndex)) => {
                    self.out.uses_instance_index = true;
                    values.push(Arg::v(self.input(0, crate::INSTANCE_ID_CHANNEL)));
                }
                (ShaderStage::Fragment, Binding::Location { location, interpolation, .. }) => {
                    let gpr = locations.iter().position(|l| l == location).unwrap() as u8;
                    self.out.ps_inputs.push(PsInput {
                        semantic: *location,
                        gpr,
                        flat: *interpolation == Some(naga::Interpolation::Flat),
                        linear: *interpolation == Some(naga::Interpolation::Linear),
                    });
                    for c in 0..*n {
                        values.push(Arg::v(self.input(gpr, c as u8)));
                    }
                }
                (ShaderStage::Fragment, Binding::BuiltIn(BuiltIn::Position { .. })) => {
                    let gpr = next_special;
                    next_special += 1;
                    self.out.position_gpr = Some(gpr);
                    for c in 0..4 {
                        values.push(Arg::v(self.input(gpr, c)));
                    }
                }
                (ShaderStage::Fragment, Binding::BuiltIn(BuiltIn::FrontFacing)) => {
                    let gpr = next_special;
                    next_special += 1;
                    self.out.front_face_gpr = Some(gpr);
                    let raw = self.input(gpr, 0);
                    values.push(self.op2(op2::SETGT_DX10, Arg::v(raw), Arg::imm(0)));
                }
                (_, other) => return unsupported(format!("input {other:?}")),
            }
        }
        // split the flattened values back into one Lv per argument
        let mut args = Vec::new();
        let mut at = 0;
        for arg in &ep.function.arguments {
            let n = self.nscalars(self.inner(arg.ty))? as usize;
            args.push(Lv::Val(values[at..at + n].to_vec()));
            at += n;
        }
        let ret = self.run_function(&ep.function, fi, args)?;
        let mut outs: Vec<(Binding, u32)> = Vec::new();
        if let Some(r) = &ep.function.result {
            self.flatten_bindings(r.binding.as_ref(), r.ty, &mut outs)?;
        }
        let ret = ret.unwrap_or_default();
        let mut params: Vec<u32> = outs
            .iter()
            .filter_map(|(b, _)| if let Binding::Location { location, .. } = b { Some(*location) } else { None })
            .collect();
        params.sort();
        let mut at = 0;
        let mut exports: Vec<(latte::Target, Vec<Arg>)> = Vec::new();
        for (b, n) in &outs {
            let vals: Vec<Arg> = ret[at..at + *n as usize].iter().map(|v| Arg::v(*v)).collect();
            at += *n as usize;
            match (ep.stage, b) {
                (ShaderStage::Vertex, Binding::BuiltIn(BuiltIn::Position { .. })) => {
                    exports.push((latte::Target::Pos(0), vals));
                }
                (ShaderStage::Vertex, Binding::Location { location, .. }) => {
                    let i = params.iter().position(|l| l == location).unwrap() as u8;
                    exports.push((latte::Target::Param(i), vals));
                }
                (ShaderStage::Fragment, Binding::Location { location, .. }) => {
                    self.out.color_outputs.push(*location);
                    exports.push((latte::Target::Pixel(*location as u8), vals));
                }
                (ShaderStage::Fragment, Binding::BuiltIn(BuiltIn::FragDepth)) => {
                    self.out.writes_depth = true;
                    exports.push((latte::Target::Pixel(61), vals));
                }
                (ShaderStage::Vertex, Binding::BuiltIn(BuiltIn::PointSize)) => {}
                (_, other) => return unsupported(format!("output {other:?}")),
            }
        }
        self.out.params = params;
        // the hardware wants at least one parameter / colour export
        if ep.stage == ShaderStage::Vertex && self.out.params.is_empty() {
            exports.push((latte::Target::Param(0), vec![Arg::imm(0); 4]));
        }
        if ep.stage == ShaderStage::Fragment && self.out.color_outputs.is_empty() && !self.out.writes_depth {
            exports.push((latte::Target::Pixel(0), vec![Arg::imm(0); 4]));
        }
        // vertex: position first; then by target
        exports.sort_by_key(|(t, _)| match t {
            latte::Target::Pos(n) => (0, *n),
            latte::Target::Param(n) => (1, *n),
            latte::Target::Pixel(61) => (3, 0),
            latte::Target::Pixel(n) => (2, *n),
        });
        for (target, vals) in exports {
            let mut src = [None; 4];
            for (i, a) in vals.iter().enumerate().take(4) {
                src[i] = Some(self.mov(*a));
            }
            if vals.len() < 4 {
                // unwritten channels: 0, and w = 1 for colours
                for (i, s) in src.iter_mut().enumerate().skip(vals.len()) {
                    *s = Some(self.mov(Arg::f(if i == 3 { 1.0 } else { 0.0 })));
                }
            }
            self.p.groups.push(src);
            self.p.exports.push(Export { target, src });
        }
        self.p.body = std::mem::take(&mut self.blocks[0]);
        Ok(())
    }

    fn input(&mut self, gpr: u8, chan: u8) -> V {
        let v = self.fresh();
        self.p.inputs.push((v, gpr, chan));
        v
    }

    fn flatten_bindings(&self, b: Option<&Binding>, ty: Handle<naga::Type>, out: &mut Vec<(Binding, u32)>) -> R<()> {
        match (b, self.inner(ty)) {
            (Some(b), t) => out.push((b.clone(), self.nscalars(t)?)),
            (None, TypeInner::Struct { members, .. }) => {
                for mb in members {
                    self.flatten_bindings(mb.binding.as_ref(), mb.ty, out)?;
                }
            }
            (None, other) => return unsupported(format!("unbound entry point value of type {other:?}")),
        }
        Ok(())
    }
}
