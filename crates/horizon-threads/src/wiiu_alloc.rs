//! Stand-ins for `mimalloc` and `libmimalloc-sys` on the Wii U, which has no `mmap` for them: the
//! game's allocator is newlib's heap through Rust's `System`, and the mimalloc statistics
//! calls report nothing.
#![allow(non_camel_case_types)]

pub mod mimalloc {
    use std::alloc::{GlobalAlloc, Layout, System};

    /// The same unit struct as `mimalloc::MiMalloc`, backed by the system heap.
    #[derive(Clone, Copy, Default)]
    pub struct MiMalloc;

    unsafe impl GlobalAlloc for MiMalloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

pub mod libmimalloc_sys {
    use std::ffi::{c_char, c_long, c_void};

    pub type mi_option_t = i32;

    pub unsafe fn mi_option_set(_option: mi_option_t, _value: c_long) {}
    pub unsafe fn mi_option_get(_option: mi_option_t) -> c_long {
        0
    }
    pub unsafe fn mi_collect(_force: bool) {}
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn mi_process_info(
        _elapsed: &mut usize,
        _user: &mut usize,
        _system: &mut usize,
        _rss: &mut usize,
        _peak_rss: &mut usize,
        _commit: &mut usize,
        _peak_commit: &mut usize,
        _faults: &mut usize,
    ) {
    }
    pub unsafe fn mi_stats_get_json(_size: usize, _buf: *mut c_char) -> *mut c_char {
        std::ptr::null_mut()
    }
    pub unsafe fn mi_malloc(_size: usize) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub unsafe fn mi_free(_p: *mut c_void) {}
}
