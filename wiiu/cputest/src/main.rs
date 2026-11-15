//! Prints one hash per kind of computation. Run natively and on the Wii U (Cemu) and diff the output:
//! a differing line names the operations to look at.
#[cfg(target_vendor = "wiiu")]
extern crate wiiu_atomics;

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

fn main() {
    #[cfg(target_vendor = "wiiu")]
    unsafe {
        wut_sys::WHBProcInit();
    }
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
