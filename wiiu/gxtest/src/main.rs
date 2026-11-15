//! GX2 bring-up: initialise the GPU through WHB, then clear the TV and GamePad with a moving colour.
extern crate wiiu_atomics;

use wut_sys::*;

fn main() {
    println!("gxtest: start");
    unsafe {
        WHBProcInit();
        let ok = WHBGfxInit();
        println!("gxtest: WHBGfxInit -> {ok}");
        let mut frame = 0u32;
        while WHBProcIsRunning() != 0 {
            let t = frame as f32 / 60.0;
            WHBGfxBeginRender();

            WHBGfxBeginRenderTV();
            WHBGfxClearColor(0.5 + 0.5 * t.sin(), 0.3, 0.5 + 0.5 * (t * 0.7).cos(), 1.0);
            WHBGfxFinishRenderTV();

            WHBGfxBeginRenderDRC();
            WHBGfxClearColor(0.1, 0.5 + 0.5 * t.sin(), 0.3, 1.0);
            WHBGfxFinishRenderDRC();

            WHBGfxFinishRender();
            frame += 1;
            if frame == 30 {
                println!("gxtest: 30 frames drawn");
            }
        }
        WHBGfxShutdown();
        WHBProcShutdown();
    }
}
