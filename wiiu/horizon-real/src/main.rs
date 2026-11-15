//! Horizon on the Wii U: the real client. GX2 and the GamePad start here; everything else is the game's own
//! `entry::main`, with its arguments in `horizon/args.txt` on the SD card (see wiiu/README.md).
extern crate wiiu_atomics;

use wut_sys::*;

fn main() {
    println!("horizon-real: start");
    unsafe {
        WHBProcInit();
        WHBGfxInit();
        VPADInit();
    }
    horizon_client::entry::main();
    println!("horizon-real: exit");
    unsafe {
        VPADShutdown();
        WHBGfxShutdown();
        WHBProcShutdown();
    }
}
