extern crate wiiu_atomics;
use std::collections::HashMap;

#[no_mangle]
pub extern "C" fn stdtest() -> u32 {
    let mut m = HashMap::new();
    m.insert(1u32, vec![1u8; 4]);
    m.len() as u32 + String::from("x").len() as u32
}

use std::sync::atomic::{AtomicU64, Ordering};
static N: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn stdtest_atomic() -> u64 {
    N.fetch_add(5, Ordering::SeqCst);
    N.load(Ordering::SeqCst)
}
