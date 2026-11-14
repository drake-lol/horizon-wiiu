#![no_std]
#![no_main]

use core::ffi::{c_char, c_int};

extern "C" {
    fn WHBProcInit();
    fn WHBProcIsRunning() -> c_int;
    fn WHBProcShutdown();
    fn WHBLogConsoleInit() -> c_int;
    fn WHBLogConsoleDraw();
    fn WHBLogConsoleFree();
    fn WHBLogPrint(msg: *const c_char) -> c_int;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    unsafe {
        WHBProcInit();
        WHBLogConsoleInit();
        WHBLogPrint(c"Horizon for Wii U: hello from Rust".as_ptr());
        while WHBProcIsRunning() != 0 {
            WHBLogConsoleDraw();
        }
        WHBLogConsoleFree();
        WHBProcShutdown();
    }
    0
}
