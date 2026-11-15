//! Prints one hash per kind of computation. Run natively and on the Wii U (Cemu) and diff the output:
//! a differing line names the operations to look at.
#![cfg_attr(target_vendor = "wiiu", feature(asm_experimental_arch))]
#[cfg(target_vendor = "wiiu")]
extern crate wiiu_atomics;

/// Condition-register instructions LLVM uses to keep comparison results across calls.
#[cfg(target_vendor = "wiiu")]
fn cr_tests() {
    use core::arch::asm;
    for &start in &[0x0000_0000u32, 0xFFFF_FFFF, 0x1234_5678, 0x8421_8421] {
        for &val in &[0x0000_0000u32, 0xFFFF_FFFF, 0xA5A5_A5A5, 0x0F0F_F0F0] {
            // mtcrf 0xFF then mtcrf on one field (2, mask 0x20) then mfcr
            let (all, f2, f3, f4, f24): (u32, u32, u32, u32, u32);
            unsafe {
                asm!("mtcrf 255, {s}", "mfcr {o}", s = in(reg) start, o = out(reg) all);
                asm!("mtcrf 255, {s}", "mtcrf 32, {v}", "mfcr {o}", s = in(reg) start, v = in(reg) val, o = out(reg) f2);
                asm!("mtcrf 255, {s}", "mtcrf 16, {v}", "mfcr {o}", s = in(reg) start, v = in(reg) val, o = out(reg) f3);
                asm!("mtcrf 255, {s}", "mtcrf 8, {v}", "mfcr {o}", s = in(reg) start, v = in(reg) val, o = out(reg) f4);
                asm!("mtcrf 255, {s}", "mtcrf 56, {v}", "mfcr {o}", s = in(reg) start, v = in(reg) val, o = out(reg) f24);
            }
            let field = |m: u32| (start & !m) | (val & m);
            let want = [start, field(0x00F0_0000), field(0x000F_0000), field(0x0000_F000), field(0x00FF_F000)];
            let got = [all, f2, f3, f4, f24];
            for k in 0..5 {
                if got[k] != want[k] {
                    println!("CR BAD mtcrf case {k} start {start:#010x} val {val:#010x}: got {:#010x} want {:#010x}", got[k], want[k]);
                }
            }
            // mcrf: copy field 0 into field 2, field 5 into field 3
            let (m1, m2): (u32, u32);
            unsafe {
                asm!("mtcrf 255, {s}", "mcrf 2, 0", "mfcr {o}", s = in(reg) start, o = out(reg) m1);
                asm!("mtcrf 255, {s}", "mcrf 3, 5", "mfcr {o}", s = in(reg) start, o = out(reg) m2);
            }
            let cf = |x: u32, f: u32| (x >> (28 - 4 * f)) & 0xF;
            let set = |x: u32, f: u32, v: u32| (x & !(0xF << (28 - 4 * f))) | (v << (28 - 4 * f));
            let w1 = set(start, 2, cf(start, 0));
            let w2 = set(start, 3, cf(start, 5));
            if m1 != w1 || m2 != w2 {
                println!("CR BAD mcrf start {start:#010x}: got {m1:#010x} {m2:#010x} want {w1:#010x} {w2:#010x}");
            }
        }
    }
    // a comparison into cr2, kept across a call, then branched on
    #[inline(never)]
    extern "C" fn clobber() -> u32 {
        bb(7)
    }
    let mut bad = 0;
    for (a, b) in [(1u32, 2u32), (2, 1), (3, 3)] {
        let r: u32;
        unsafe {
            asm!(
                "cmplw 2, {a}, {b}",
                "mfcr {t}",
                "mtcrf 32, {t}",
                "li {r}, 0",
                "blt 2, 1f",
                "li {r}, 1",
                "1:",
                a = in(reg) a, b = in(reg) b, t = out(reg) _, r = out(reg) r,
            );
        }
        let _ = clobber();
        if (r == 0) != (a < b) {
            bad += 1;
            println!("CR BAD cmplw cr2 {a} {b} -> {r}");
        }
    }
    println!("cr tests done ({bad} bad branch cases)");
}

use std::hint::black_box as bb;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*, written with plain ops so the generator is itself under test only lightly
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

struct H(u64);
impl H {
    fn add(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x100_0000_01B3).rotate_left(7);
    }
}

fn values(r: &mut Rng) -> Vec<u64> {
    let mut v: Vec<u64> = vec![0, 1, 2, 3, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0x1_0000_0000, u64::MAX, u64::MAX - 1, 1 << 63, (1 << 63) - 1, 0xFFFF_FFFF_0000_0000, 0x0000_0001_FFFF_FFFF];
    for _ in 0..200 {
        let x = r.next();
        v.push(x);
        v.push(x & 0xFFFF_FFFF);
        v.push(x >> (r.next() % 64));
    }
    v
}

fn run(name: &str, f: impl Fn(u64, u64) -> u64, vals: &[u64]) {
    let mut h = H(0xcbf2_9ce4_8422_2325);
    for &a in vals {
        for &b in vals.iter().step_by(7) {
            h.add(f(bb(a), bb(b)));
        }
    }
    println!("{name:24} {:016x}", h.0);
}

#[inline(never)]
fn cmp_u32(x: u32, y: u32) -> i8 {
    x.cmp(&y) as i8
}
#[inline(never)]
fn cmp_i32(x: i32, y: i32) -> i8 {
    x.cmp(&y) as i8
}
#[inline(never)]
fn cmp_u16(x: u16, y: u16) -> i8 {
    x.cmp(&y) as i8
}
#[inline(never)]
fn sat_sub(x: u64, y: u64) -> u64 {
    x.saturating_sub(y)
}
#[inline(never)]
fn sat_add(x: u64, y: u64) -> u64 {
    x.saturating_add(y)
}
#[inline(never)]
fn sat_sub32(x: u32, y: u32) -> u32 {
    x.saturating_sub(y)
}
#[inline(never)]
fn fma(x: f64, y: f64) -> f64 {
    x.mul_add(y, 1.0)
}
#[inline(never)]
fn fsqrt(x: f64) -> f64 {
    x.sqrt()
}
#[inline(never)]
fn fdiv(x: f64, y: f64) -> f64 {
    x / y
}

#[inline(never)]
fn combo_u32(a: u64, b: u64) -> u64 {
    let (x, y) = (a as u32, b as u32);
    ((x < y) as u64) | (((x as i32) < (y as i32)) as u64) << 1 | ((x == y) as u64) << 2 | (x.cmp(&y) as i8 as u8 as u64) << 3
}
#[inline(never)]
fn combo_u16(a: u64, b: u64) -> u64 {
    let (x, y) = (a as u16, b as u16);
    ((x < y) as u64) | (x.cmp(&y) as i8 as u8 as u64) << 1
}
#[inline(never)]
fn combo_sat(a: u64, b: u64) -> u64 {
    a.saturating_sub(b) ^ a.saturating_add(b)
}

fn combos() {
    let mut r = Rng(77);
    let mut bad = 0;
    for i in 0..4000u32 {
        let a = if i % 3 == 0 { r.next() & 0xFFFF } else { r.next() };
        let b = if i % 5 == 0 { a } else if i % 3 == 0 { r.next() & 0xFFFF } else { r.next() };
        let got = [combo_u32(bb(a), bb(b)), combo_u16(bb(a), bb(b)), combo_sat(bb(a), bb(b))];
        let (x, y) = (a as u32, b as u32);
        let o32 = if x < y { 0xFFu64 } else if x == y { 0 } else { 1 };
        let want32 = ((x < y) as u64) | (((x as i32) < (y as i32)) as u64) << 1 | ((x == y) as u64) << 2 | o32 << 3;
        let (p, q) = (a as u16, b as u16);
        let o16 = if p < q { 0xFFu64 } else if p == q { 0 } else { 1 };
        let want16 = ((p < q) as u64) | o16 << 1;
        let ss = if a > b { a - b } else { 0 };
        let sa = if a > u64::MAX - b { u64::MAX } else { a + b };
        let want = [want32, want16, ss ^ sa];
        for k in 0..3 {
            if got[k] != want[k] && bad < 12 {
                println!("combo{k} a={a:#x} b={b:#x} got {:#x} want {:#x}", got[k], want[k]);
                bad += 1;
            }
        }
    }
    println!("combos done, {bad} bad");
}

fn micro() {
    let pairs: [(u64, u64); 10] = [
        (0, 0), (1, 2), (2, 1), (0x8000_0000, 1), (1, 0x8000_0000), (0xFFFF_FFFF, 0), (5, 5),
        (0x1_0000_0001, 0x1_0000_0000), (0x0000_0001_0000_0000, 0x0000_0000_FFFF_FFFF), (u64::MAX, 1),
    ];
    for (a, b) in pairs {
        println!(
            "micro {a:x} {b:x}: cmpu32 {} cmpi32 {} cmpu16 {} satsub {:x} satadd {:x} satsub32 {:x}",
            cmp_u32(bb(a as u32), bb(b as u32)),
            cmp_i32(bb(a as i32), bb(b as i32)),
            cmp_u16(bb(a as u16), bb(b as u16)),
            sat_sub(bb(a), bb(b)),
            sat_add(bb(a), bb(b)),
            sat_sub32(bb(a as u32), bb(b as u32)),
        );
    }
    for (x, y) in [(2.0f64, 3.0f64), (1e10, 1e-10), (0.1, 0.7), (123456.789, 0.000321)] {
        println!(
            "microf {x} {y}: fma {:x} sqrt {:x} div {:x}",
            fma(bb(x), bb(y)).to_bits(),
            fsqrt(bb(x)).to_bits(),
            fdiv(bb(x), bb(y)).to_bits()
        );
    }
}

fn main() {
    #[cfg(target_vendor = "wiiu")]
    unsafe {
        wut_sys::WHBProcInit();
    }
    #[cfg(target_vendor = "wiiu")]
    cr_tests();
    micro();
    combos();
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    let v = values(&mut r);
    run("u64 add", |a, b| a.wrapping_add(b), &v);
    run("u64 sub", |a, b| a.wrapping_sub(b), &v);
    run("u64 mul", |a, b| a.wrapping_mul(b), &v);
    run("u64 div", |a, b| if b == 0 { 0 } else { a / b }, &v);
    run("u64 rem", |a, b| if b == 0 { 0 } else { a % b }, &v);
    run("i64 div", |a, b| if b == 0 || (a as i64 == i64::MIN && b as i64 == -1) { 0 } else { (a as i64 / b as i64) as u64 }, &v);
    run("u64 lt", |a, b| (a < b) as u64, &v);
    run("u64 le", |a, b| (a <= b) as u64, &v);
    run("u64 eq", |a, b| (a == b) as u64, &v);
    run("i64 lt", |a, b| ((a as i64) < (b as i64)) as u64, &v);
    run("u64 cmp", |a, b| a.cmp(&b) as i8 as u64, &v);
    run("i64 cmp", |a, b| (a as i64).cmp(&(b as i64)) as i8 as u64, &v);
    run("u64 shl", |a, b| a.wrapping_shl(b as u32), &v);
    run("u64 shr", |a, b| a.wrapping_shr(b as u32), &v);
    run("i64 shr", |a, b| ((a as i64).wrapping_shr(b as u32)) as u64, &v);
    run("u64 min max", |a, b| a.min(b) ^ a.max(b).rotate_left(3), &v);
    run("u64 overflow", |a, b| { let (s, o) = a.overflowing_add(b); s ^ o as u64 }, &v);
    run("u64 sat", |a, b| a.saturating_sub(b) ^ a.saturating_add(b), &v);
    run("u64 clz ctz", |a, b| (a.leading_zeros() + b.trailing_zeros() * 64 + a.count_ones() * 4096) as u64, &v);
    run("u32 add sub", |a, b| ((a as u32).wrapping_add(b as u32) ^ (a as u32).wrapping_sub(b as u32).rotate_left(5)) as u64, &v);
    run("u32 mul div", |a, b| { let (x, y) = (a as u32, b as u32); (x.wrapping_mul(y) ^ if y == 0 { 0 } else { x / y }) as u64 }, &v);
    run("u32 cmp", |a, b| { let (x, y) = (a as u32, b as u32); ((x < y) as u64) | (((x as i32) < (y as i32)) as u64) << 1 | ((x == y) as u64) << 2 | (x.cmp(&y) as i8 as u8 as u64) << 3 }, &v);
    run("u16 cmp", |a, b| { let (x, y) = (a as u16, b as u16); ((x < y) as u64) | (x.cmp(&y) as i8 as u8 as u64) << 1 }, &v);
    run("u8 i8", |a, b| { let (x, y) = (a as u8, b as i8); (x.wrapping_add(y as u8) as u64) | ((y < 0) as u64) << 9 | ((x as i8 > y) as u64) << 10 }, &v);
    run("bool logic", |a, b| { let (p, q) = (a & 1 == 1, b & 2 == 2); let r = (p && !q) || (q ^ (a > b)); (r as u64) | ((p == q) as u64) << 1 | ((a > 5 && b < 9 || a == b) as u64) << 2 }, &v);
    run("u64 to f64", |a, _| (a as f64).to_bits(), &v);
    run("i64 to f64", |a, _| (a as i64 as f64).to_bits(), &v);
    run("f64 to u64", |a, b| { let f = (a >> 11) as f64 / ((b >> 40) as f64 + 1.0); f as u64 ^ (f as i64 as u64).rotate_left(1) }, &v);
    run("f32 ops", |a, b| { let (x, y) = (a as u32 as f32 / 7.0, b as u32 as f32 + 0.5); ((x * y + x / y - x.sqrt()).to_bits() as u64) ^ ((x < y) as u64) << 40 }, &v);
    run("f64 ops", |a, b| { let (x, y) = (a as f64 / 3.0, b as f64 + 0.25); (x * y + x / y - x.abs().sqrt() + x.mul_add(y, 1.0)).to_bits() ^ ((x.partial_cmp(&y).map_or(9, |o| o as i8)) as u64) }, &v);
    run("f32 to int", |a, _| { let f = f32::from_bits(a as u32); (f as i32 as u32 as u64) | (f as u32 as u64) << 32 }, &v);
    run("u128", |a, b| { let p = (a as u128) * (b as u128); (p >> 64) as u64 ^ p as u64 }, &v);
    // slices, sorting, tuples (where earlier miscompiles showed up)
    run("u16 slice ord", |a, b| { let x: Vec<u16> = (0..(a % 6)).map(|i| (a >> (i * 8)) as u16).collect(); let y: Vec<u16> = (0..(b % 6)).map(|i| (b >> (i * 8)) as u16).collect(); x.cmp(&y) as i8 as u64 }, &v);
    run("tuple ord", |a, b| ((a as u32, (a >> 32) as u16) < (b as u32, (b >> 32) as u16)) as u64, &v);
    run("sort", |a, b| { let mut x: Vec<u32> = (0..16).map(|i| (a.rotate_left(i * 5) ^ b) as u32).collect(); x.sort_unstable(); x.iter().fold(0u64, |s, &e| s.wrapping_mul(31).wrapping_add(e as u64)) }, &v);
    run("duration", |a, b| { use std::time::Duration; let (x, y) = (Duration::from_nanos(a >> 2), Duration::from_nanos(b >> 2)); ((x < y) as u64) ^ (x.saturating_sub(y).as_nanos() as u64) }, &v);
    run("usize arith", |a, b| { let (x, y) = (a as usize, (b as usize) | 1); (x.wrapping_mul(y) ^ (x / y) ^ x.wrapping_sub(y)) as u64 }, &v);
    run("hash", |a, b| { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); (a, b as u32).hash(&mut h); h.finish() }, &v[..60]);
    println!("cputest done");
    #[cfg(target_vendor = "wiiu")]
    unsafe {
        while wut_sys::WHBProcIsRunning() != 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        wut_sys::WHBProcShutdown();
    }
}
