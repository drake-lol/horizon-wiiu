//! The Espresso (PowerPC 750 family) has 32-bit `lwarx`/`stwcx.` only. The workspace uses
//! `AtomicU64`/`AtomicI64` in ~200 places, so the target spec claims 64-bit atomics and LLVM
//! lowers them to the `__atomic_*_8` libcalls defined here, serialised by one global spinlock.
#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

static LOCK: AtomicU32 = AtomicU32::new(0);

#[inline]
fn with<R>(f: impl FnOnce() -> R) -> R {
    while LOCK
        .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let r = f();
    LOCK.store(0, Ordering::Release);
    r
}

macro_rules! rmw {
    ($name:ident, |$old:ident, $val:ident| $new:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(ptr: *mut u64, $val: u64, _order: i32) -> u64 {
            with(|| {
                let $old = ptr.read();
                ptr.write($new);
                $old
            })
        }
    };
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_load_8(ptr: *const u64, _order: i32) -> u64 {
    with(|| ptr.read())
}

#[no_mangle]
pub unsafe extern "C" fn __atomic_store_8(ptr: *mut u64, val: u64, _order: i32) {
    with(|| ptr.write(val))
}

rmw!(__atomic_exchange_8, |_old, val| val);
rmw!(__atomic_fetch_add_8, |old, val| old.wrapping_add(val));
rmw!(__atomic_fetch_sub_8, |old, val| old.wrapping_sub(val));
rmw!(__atomic_fetch_and_8, |old, val| old & val);
rmw!(__atomic_fetch_or_8, |old, val| old | val);
rmw!(__atomic_fetch_xor_8, |old, val| old ^ val);
rmw!(__atomic_fetch_nand_8, |old, val| !(old & val));

#[no_mangle]
pub unsafe extern "C" fn __atomic_compare_exchange_8(
    ptr: *mut u64,
    expected: *mut u64,
    desired: u64,
    _weak: bool,
    _success: i32,
    _failure: i32,
) -> bool {
    with(|| {
        let cur = ptr.read();
        if cur == expected.read() {
            ptr.write(desired);
            true
        } else {
            expected.write(cur);
            false
        }
    })
}

#[no_mangle]
pub extern "C" fn __atomic_is_lock_free(_size: usize, _ptr: *const u8) -> bool {
    false
}
