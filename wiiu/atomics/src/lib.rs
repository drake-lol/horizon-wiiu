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

// ---- libc functions std references that newlib on the Wii U does not provide ----
// The Wii U has no users, groups, chroot or fifos: the calls fail with ENOSYS.

use core::ffi::{c_char, c_int, c_long, c_uint};

extern "C" {
    fn __errno() -> *mut c_int;
}

const ENOSYS: c_int = 88;

unsafe fn fail() -> c_int {
    unsafe { *__errno() = ENOSYS };
    -1
}

#[no_mangle]
pub unsafe extern "C" fn chown(_path: *const c_char, _owner: c_uint, _group: c_uint) -> c_int {
    unsafe { fail() }
}

#[no_mangle]
pub unsafe extern "C" fn lchown(_path: *const c_char, _owner: c_uint, _group: c_uint) -> c_int {
    unsafe { fail() }
}

#[no_mangle]
pub unsafe extern "C" fn chroot(_path: *const c_char) -> c_int {
    unsafe { fail() }
}

#[no_mangle]
pub unsafe extern "C" fn mkfifo(_path: *const c_char, _mode: c_uint) -> c_int {
    unsafe { fail() }
}

/// newlib's `_SC_*` numbers: `_SC_PAGESIZE` 8, `_SC_NPROCESSORS_CONF` 57 and `_SC_NPROCESSORS_ONLN` 58.
/// The Espresso has three cores and the console's pages are 4 KiB.
#[no_mangle]
pub unsafe extern "C" fn sysconf(name: c_int) -> c_long {
    match name {
        8 => 4096,
        57 | 58 => 3,
        _ => unsafe { fail() as c_long },
    }
}

// ---- Cemu has no nsysnet.set_multicast_state, which wut's socket init calls: skip that init ----
// (Enable the `socket-init` behaviour again for real hardware by removing these two.)
#[cfg(feature = "skip-socket-init")]
#[no_mangle]
pub extern "C" fn __init_wut_socket() {}

#[cfg(feature = "skip-socket-init")]
#[no_mangle]
pub extern "C" fn __fini_wut_socket() {}

// ---- debugging aid: show the message of a fatal error before the console stops ----
// Link with `-Wl,--wrap=OSFatal` (wiiu/build.sh does, with the `trace-fatal` feature).
#[cfg(feature = "trace-fatal")]
extern "C" {
    fn OSReport(fmt: *const c_char, ...);
    fn __real_OSFatal(msg: *const c_char) -> !;
}

#[cfg(feature = "trace-fatal")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_OSFatal(msg: *const c_char) -> ! {
    unsafe {
        OSReport(c"FATAL: %s\n".as_ptr(), msg);
        __real_OSFatal(msg)
    }
}
