extern crate wiiu_atomics;

use std::cmp::Ordering;
use std::hint::black_box;

#[inline(never)]
fn cmp_u16(a: &[u16], b: &[u16]) -> Ordering {
    a.cmp(b)
}

#[inline(never)]
fn cmp_u32(a: u32, b: u32) -> Ordering {
    a.cmp(&b)
}

#[inline(never)]
fn cmp_u16_scalar(a: u16, b: u16) -> Ordering {
    a.cmp(&b)
}

#[inline(never)]
fn manual(a: &[u16], b: &[u16]) -> i32 {
    for i in 0..a.len().min(b.len()) {
        if a[i] < b[i] {
            return -1;
        }
        if a[i] > b[i] {
            return 1;
        }
    }
    (a.len() as i32 - b.len() as i32).signum()
}

fn main() {
    let data = black_box(vec![68u16, 97, 116, 97, 86]);
    let want = black_box(vec![115u16, 101, 99, 116]);
    let same = black_box(vec![115u16, 101, 99, 116]);
    println!("slice cmp data vs want  = {:?} (expect Less)", cmp_u16(&data, &want));
    println!("slice cmp want vs data  = {:?} (expect Greater)", cmp_u16(&want, &data));
    println!("slice cmp same vs want  = {:?} (expect Equal)", cmp_u16(&same, &want));
    println!("u32 cmp 3 vs 5          = {:?} (expect Less)", cmp_u32(black_box(3), black_box(5)));
    println!("u32 cmp 5 vs 3          = {:?} (expect Greater)", cmp_u32(black_box(5), black_box(3)));
    println!("u16 cmp 68 vs 115       = {:?} (expect Less)", cmp_u16_scalar(black_box(68), black_box(115)));
    println!("manual data vs want     = {} (expect -1)", manual(&data, &want));
    println!("manual same vs want     = {} (expect 0)", manual(&same, &want));
    println!("Vec eq same             = {} (expect true)", same == want);
}
