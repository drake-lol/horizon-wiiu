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

extern "C" {
    fn __real_ftruncate(fd: c_int, len: i64) -> c_int;
}

/// Linked with `--wrap=ftruncate` (wiiu/tools/bldrpx.sh). Cemu never answers wut's FSA truncate, so a
/// truncate that would not change the file's size (setting a new, empty file to 0: logs, locks) is done
/// without asking the filesystem.
#[no_mangle]
pub unsafe extern "C" fn __wrap_ftruncate(fd: c_int, len: i64) -> c_int {
    unsafe {
        let at = lseek(fd, 0, 1);
        let end = lseek(fd, 0, 2);
        if at >= 0 {
            lseek(fd, at, 0);
        }
        if end >= 0 && end == len {
            return 0;
        }
        __real_ftruncate(fd, len)
    }
}

/// No pipes on the Wii U (mio's waker asks for one): fail with ENOSYS.
#[no_mangle]
pub unsafe extern "C" fn pipe2(_fds: *mut c_int, _flags: c_int) -> c_int {
    set_errno(88);
    -1
}

/// File times cannot be set on the SD card through newlib: fail with ENOSYS.
#[no_mangle]
pub unsafe extern "C" fn futimens(_fd: c_int, _times: *const u8) -> c_int {
    set_errno(88);
    -1
}

/// No file descriptors behind directory streams here (newlib's `DIR` has none).
#[no_mangle]
pub unsafe extern "C" fn dirfd(_dirp: *mut u8) -> c_int {
    -1
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

// Link with `-Wl,--wrap=abort` too: report who aborted (the return address, to look up with `nm`).
#[cfg(feature = "trace-fatal")]
extern "C" {
    fn __real_abort() -> !;
}

#[cfg(feature = "trace-fatal")]
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn __wrap_abort() -> ! {
    let lr: usize;
    unsafe { core::arch::asm!("mflr {0}", out(reg) lr) };
    unsafe {
        OSReport(c"ABORT from %p\n".as_ptr(), lr as *const u8);
        __real_abort()
    }
}

// Link with `-Wl,--wrap=pthread_mutex_lock` to report a failing lock's error code.
#[cfg(feature = "trace-fatal")]
extern "C" {
    fn __real_pthread_mutex_lock(m: *mut u8) -> c_int;
}

#[cfg(feature = "trace-fatal")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_pthread_mutex_lock(m: *mut u8) -> c_int {
    let rc = unsafe { __real_pthread_mutex_lock(m) };
    unsafe { OSReport(c"LOCK rc=%d\n".as_ptr(), rc) };
    rc
}

// ---- pthread keys: devkitPPC's newlib stubs them out with ENOSYS, but std's thread-locals need them ----
// Values live in a global table keyed by (OS thread, key), under the same spinlock as the atomics.
// Destructors are recorded but not run at thread exit (the leak is bounded by the thread count).

use core::cell::UnsafeCell;

extern "C" {
    fn OSGetCurrentThread() -> *mut u8;
}

const MAX_KEYS: usize = 256;
const MAX_ENTRIES: usize = 4096;

struct Keys {
    used: [bool; MAX_KEYS],
    dtor: [usize; MAX_KEYS],
    // (thread pointer, key, value); thread == 0 marks a free slot.
    entries: [(usize, c_uint, usize); MAX_ENTRIES],
}

struct KeyTable(UnsafeCell<Keys>);
unsafe impl Sync for KeyTable {}

static KEYS: KeyTable = KeyTable(UnsafeCell::new(Keys {
    used: [false; MAX_KEYS],
    dtor: [0; MAX_KEYS],
    entries: [(0, 0, 0); MAX_ENTRIES],
}));

const EAGAIN: c_int = 11;
const EINVAL: c_int = 22;

#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(key: *mut c_uint, dtor: usize) -> c_int {
    with(|| {
        let t = unsafe { &mut *KEYS.0.get() };
        for k in 0..MAX_KEYS {
            if !t.used[k] {
                t.used[k] = true;
                t.dtor[k] = dtor;
                unsafe { key.write(k as c_uint) };
                return 0;
            }
        }
        EAGAIN
    })
}

#[no_mangle]
pub unsafe extern "C" fn pthread_key_delete(key: c_uint) -> c_int {
    with(|| {
        let t = unsafe { &mut *KEYS.0.get() };
        let k = key as usize;
        if k >= MAX_KEYS || !t.used[k] {
            return EINVAL;
        }
        t.used[k] = false;
        for e in t.entries.iter_mut() {
            if e.0 != 0 && e.1 == key {
                e.0 = 0;
            }
        }
        0
    })
}

#[no_mangle]
pub unsafe extern "C" fn pthread_setspecific(key: c_uint, value: usize) -> c_int {
    let me = unsafe { OSGetCurrentThread() } as usize;
    with(|| {
        let t = unsafe { &mut *KEYS.0.get() };
        let k = key as usize;
        if k >= MAX_KEYS || !t.used[k] {
            return EINVAL;
        }
        let mut free = None;
        for (i, e) in t.entries.iter_mut().enumerate() {
            if e.0 == me && e.1 == key {
                e.2 = value;
                return 0;
            }
            if e.0 == 0 && free.is_none() {
                free = Some(i);
            }
        }
        match free {
            Some(i) => {
                t.entries[i] = (me, key, value);
                0
            }
            None => EAGAIN,
        }
    })
}

/// Forget every value `thread` set: a new thread may be created at a dead one's address.
fn forget_thread(thread: usize) {
    with(|| {
        let t = unsafe { &mut *KEYS.0.get() };
        for e in t.entries.iter_mut() {
            if e.0 == thread {
                e.0 = 0;
            }
        }
    })
}

type ThreadStart = unsafe extern "C" fn(*mut u8) -> *mut u8;

extern "C" {
    fn __real_pthread_create(thread: *mut u8, attr: *const u8, start: ThreadStart, arg: *mut u8) -> c_int;
    fn malloc(size: usize) -> *mut u8;
    fn free(p: *mut u8);
}

unsafe extern "C" fn thread_trampoline(pair: *mut u8) -> *mut u8 {
    unsafe {
        let start: ThreadStart = core::mem::transmute((pair as *const usize).read());
        let arg = (pair as *const usize).add(1).read() as *mut u8;
        free(pair);
        let me = OSGetCurrentThread() as usize;
        forget_thread(me);
        let r = start(arg);
        forget_thread(me);
        r
    }
}

/// Linked with `--wrap=pthread_create` (wiiu/tools/bldrpx.sh): each thread starts and ends with no
/// thread-specific values, so one created where a finished thread's OSThread was does not inherit them
/// (std aborted: "thread::set_current should only be called once per thread").
#[no_mangle]
pub unsafe extern "C" fn __wrap_pthread_create(thread: *mut u8, attr: *const u8, start: ThreadStart, arg: *mut u8) -> c_int {
    unsafe {
        let pair = malloc(2 * core::mem::size_of::<usize>());
        if pair.is_null() {
            return EAGAIN;
        }
        (pair as *mut usize).write(start as usize);
        (pair as *mut usize).add(1).write(arg as usize);
        let r = __real_pthread_create(thread, attr, thread_trampoline, pair);
        if r != 0 {
            free(pair);
        }
        r
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_getspecific(key: c_uint) -> usize {
    let me = unsafe { OSGetCurrentThread() } as usize;
    with(|| {
        let t = unsafe { &*KEYS.0.get() };
        for e in t.entries.iter() {
            if e.0 == me && e.1 == key {
                return e.2;
            }
        }
        0
    })
}

// ---- stdout/stderr: wut leaves fds 1 and 2 unconnected; send them to the OS console (OSReport) ----
// Link with `-Wl,--wrap=_write_r` (wiiu/build.sh does). Other fds fall through to newlib.
#[cfg(feature = "console-stdout")]
extern "C" {
    fn OSConsoleWrite(buf: *const c_char, size: c_uint) -> c_int;
    fn __real__write_r(reent: *mut u8, fd: c_int, buf: *const u8, len: usize) -> isize;
}

#[cfg(feature = "console-stdout")]
#[no_mangle]
pub unsafe extern "C" fn __wrap__write_r(reent: *mut u8, fd: c_int, buf: *const u8, len: usize) -> isize {
    if fd == 1 || fd == 2 {
        unsafe {
            OSConsoleWrite(buf as *const c_char, len as c_uint);
            console_file(reent, buf, len);
        }
        return len as isize;
    }
    unsafe { __real__write_r(reent, fd, buf, len) }
}

/// stdout and stderr also go to `horizon/console.txt` on the SD card: an emulator's log may not keep the
/// OS console, and a real console has no console to read.
#[cfg(feature = "console-stdout")]
unsafe fn console_file(reent: *mut u8, buf: *const u8, len: usize) {
    use core::sync::atomic::AtomicI32;
    extern "C" {
        fn open(path: *const c_char, flags: c_int, mode: c_int) -> c_int;
    }
    static FD: AtomicI32 = AtomicI32::new(-2);
    let mut fd = FD.load(Ordering::Acquire);
    if fd == -2 {
        // O_WRONLY | O_CREAT | O_TRUNC (newlib)
        fd = unsafe { open(c"fs:/vol/external01/horizon/console.txt".as_ptr(), 0x0001 | 0x0200 | 0x0400, 0o644) };
        FD.store(fd, Ordering::Release);
    }
    if fd >= 0 {
        unsafe { __real__write_r(reent, fd, buf, len) };
    }
}

// ---- more libc gaps the engine crates reach ----

extern "C" {
    fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
    fn OSGetSystemTime() -> i64;
}

/// `pread`: newlib on the Wii U lacks it. Not atomic against other users of the same fd, which std's
/// file reads here are not (one thread per open file).
#[no_mangle]
pub unsafe extern "C" fn pread(fd: c_int, buf: *mut u8, count: usize, offset: i64) -> isize {
    unsafe {
        let here = lseek(fd, 0, 1);
        if here < 0 || lseek(fd, offset, 0) < 0 {
            return -1;
        }
        let n = read(fd, buf, count);
        lseek(fd, here, 0);
        n
    }
}

/// Entropy for hash-map seeds (not cryptographic): xorshift64* seeded from the system clock.
#[no_mangle]
pub unsafe extern "C" fn _getentropy_r(_reent: *mut u8, buf: *mut u8, len: usize) -> c_int {
    let mut x = (unsafe { OSGetSystemTime() } as u64) ^ 0x9e37_79b9_7f4a_7c15;
    for i in 0..len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        unsafe { *buf.add(i) = (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8 };
    }
    0
}

fn set_errno(e: c_int) {
    unsafe { *__errno() = e };
}
