//! Register allocation: live intervals over the program's linear order, widened to whole loops, packed into GPR
//! channels first-fit. Groups (texture operands, fetch results, exports) take the same GPR with channel i for
//! member i; inputs keep the registers the hardware puts them in.

use std::collections::HashMap;

use crate::ir::*;
use crate::Error;

/// GPRs R0..R119; R120..R127 are clause temporaries.
pub const MAX_GPRS: usize = 120;

pub struct Alloc {
    pub reg: HashMap<V, (u8, u8)>,
    pub gprs: u8,
}

#[derive(Clone, Copy)]
struct Iv {
    start: u32,
    end: u32,
}

fn touch(iv: &mut HashMap<V, Iv>, v: V, p: u32) {
    let e = iv.entry(v).or_insert(Iv { start: p, end: p });
    e.start = e.start.min(p);
    e.end = e.end.max(p);
}

fn touch_arg(iv: &mut HashMap<V, Iv>, a: &Arg, p: u32) {
    if let A::V(v) = a.a {
        touch(iv, v, p);
    }
}

fn walk(insts: &[Inst], pos: &mut u32, iv: &mut HashMap<V, Iv>, loops: &mut Vec<(u32, u32)>) {
    for i in insts {
        *pos += 1;
        let p = *pos;
        match i {
            Inst::Op2 { dst, a, .. } => {
                a.iter().for_each(|x| touch_arg(iv, x, p));
                touch(iv, *dst, p);
            }
            Inst::Op3 { dst, a, .. } => {
                a.iter().for_each(|x| touch_arg(iv, x, p));
                touch(iv, *dst, p);
            }
            Inst::Kill { a, .. } => a.iter().for_each(|x| touch_arg(iv, x, p)),
            Inst::Tex { coord, dst, .. } => {
                coord.iter().flatten().for_each(|v| touch(iv, *v, p));
                dst.iter().flatten().for_each(|v| touch(iv, *v, p));
            }
            Inst::Vtx { index, dst, .. } => {
                touch(iv, *index, p);
                dst.iter().flatten().for_each(|v| touch(iv, *v, p));
            }
            Inst::If { cond, then, els } => {
                touch(iv, *cond, p);
                walk(then, pos, iv, loops);
                walk(els, pos, iv, loops);
                *pos += 1;
            }
            Inst::Loop { body } => {
                walk(body, pos, iv, loops);
                *pos += 1;
                loops.push((p, *pos));
            }
            Inst::Break | Inst::Continue => {}
        }
    }
}

pub fn allocate(p: &Program) -> Result<Alloc, Error> {
    let mut iv: HashMap<V, Iv> = HashMap::new();
    for (v, _, _) in &p.inputs {
        touch(&mut iv, *v, 0);
    }
    let mut pos = 0;
    let mut loops = Vec::new();
    walk(&p.body, &mut pos, &mut iv, &mut loops);
    let end = pos + 1;
    for e in &p.exports {
        e.src.iter().flatten().for_each(|v| touch(&mut iv, *v, end));
    }
    // a value live across a loop boundary stays live for the whole loop: other pixels keep iterating
    loops.sort_by_key(|(s, e)| e - s);
    for (s, e) in &loops {
        for i in iv.values_mut() {
            if i.start <= *e && i.end >= *s && (i.start < *s || i.end > *e) {
                i.start = i.start.min(*s);
                i.end = i.end.max(*e);
            }
        }
    }

    let mut reg: HashMap<V, (u8, u8)> = HashMap::new();
    let mut busy = vec![[-1i64; 4]; MAX_GPRS];
    let mut top = 0u8;
    for (v, g, c) in &p.inputs {
        let i = iv[v];
        let b = &mut busy[*g as usize][*c as usize];
        *b = (*b).max(i.end as i64);
        reg.insert(*v, (*g, *c));
        top = top.max(g + 1);
    }

    // units: groups, then the remaining single registers
    enum Unit {
        Group([Option<V>; 4]),
        One(V),
    }
    let mut in_group: HashMap<V, ()> = HashMap::new();
    let mut units: Vec<(Iv, Unit)> = Vec::new();
    for g in &p.groups {
        let mut u: Option<Iv> = None;
        for v in g.iter().flatten() {
            in_group.insert(*v, ());
            if let Some(i) = iv.get(v) {
                u = Some(match u {
                    None => *i,
                    Some(x) => Iv { start: x.start.min(i.start), end: x.end.max(i.end) },
                });
            }
        }
        if let Some(u) = u {
            units.push((u, Unit::Group(*g)));
        }
    }
    let input_set: HashMap<V, ()> = p.inputs.iter().map(|(v, _, _)| (*v, ())).collect();
    for (v, i) in &iv {
        if !in_group.contains_key(v) && !input_set.contains_key(v) {
            units.push((*i, Unit::One(*v)));
        }
    }
    units.sort_by_key(|(i, u)| (i.start, match u {
        Unit::Group(g) => g.iter().flatten().next().map(|v| v.0).unwrap_or(0),
        Unit::One(v) => v.0,
    }));
    for (i, u) in units {
        let free = |b: &[i64; 4], c: usize| b[c] < i.start as i64;
        match u {
            Unit::One(v) => {
                let mut placed = false;
                'find: for (g, b) in busy.iter_mut().enumerate() {
                    for c in 0..4 {
                        if free(b, c) {
                            b[c] = i.end as i64;
                            reg.insert(v, (g as u8, c as u8));
                            top = top.max(g as u8 + 1);
                            placed = true;
                            break 'find;
                        }
                    }
                }
                if !placed {
                    return Err(Error::TooManyRegisters);
                }
            }
            Unit::Group(members) => {
                let mut placed = false;
                for (g, b) in busy.iter_mut().enumerate() {
                    if members.iter().enumerate().all(|(c, m)| m.is_none() || free(b, c)) {
                        for (c, m) in members.iter().enumerate() {
                            if let Some(v) = m {
                                b[c] = i.end as i64;
                                reg.insert(*v, (g as u8, c as u8));
                            }
                        }
                        top = top.max(g as u8 + 1);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    return Err(Error::TooManyRegisters);
                }
            }
        }
    }
    Ok(Alloc { reg, gprs: top })
}
