//! GX2 bring-up: a vertex-coloured triangle, drawn with the pos_col_shader.gsh from
//! devkitPro/wiiu-examples (graphics/gx2/gx2_triangle), through WHB and GX2R.
extern crate wiiu_atomics;

use core::ffi::c_void;
use core::mem::zeroed;
use wut_sys::*;

static GSH: &[u8] = include_bytes!("../pos_col_shader.gsh");

const POSITIONS: [f32; 12] = [1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, -1.0, -1.0, 1.0, 1.0];
const COLOURS: [f32; 12] = [1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0];

const VERTEX_BUFFER_FLAGS: u32 = GX2RResourceFlags_GX2R_RESOURCE_BIND_VERTEX_BUFFER
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_READ
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_WRITE
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_GPU_READ;

unsafe fn vertex_buffer(data: &[f32; 12]) -> GX2RBuffer {
    let mut b: GX2RBuffer = unsafe { zeroed() };
    b.flags = VERTEX_BUFFER_FLAGS;
    b.elemSize = 16;
    b.elemCount = 3;
    unsafe {
        GX2RCreateBuffer(&mut b);
        let p = GX2RLockBufferEx(&mut b, 0) as *mut f32;
        core::ptr::copy_nonoverlapping(data.as_ptr(), p, 12);
        GX2RUnlockBufferEx(&mut b, 0);
    }
    b
}

unsafe fn draw(group: &mut WHBGfxShaderGroup, pos: &mut GX2RBuffer, col: &mut GX2RBuffer) {
    unsafe {
        GX2SetFetchShader(&mut group.fetchShader);
        GX2SetVertexShader(group.vertexShader);
        GX2SetPixelShader(group.pixelShader);
        GX2RSetAttributeBuffer(pos, 0, pos.elemSize, 0);
        GX2RSetAttributeBuffer(col, 1, col.elemSize, 0);
        GX2DrawEx(GX2PrimitiveMode_GX2_PRIMITIVE_MODE_TRIANGLES, 3, 0, 1);
    }
}

fn main() {
    println!("gxtest: start");
    unsafe {
        WHBProcInit();
        println!("gxtest: WHBGfxInit -> {}", WHBGfxInit());

        let mut group: WHBGfxShaderGroup = zeroed();
        let ok = WHBGfxLoadGFDShaderGroup(&mut group, 0, GSH.as_ptr() as *const c_void);
        println!("gxtest: shader group -> {ok}");
        WHBGfxInitShaderAttribute(&mut group, c"aPosition".as_ptr(), 0, 0, GX2AttribFormat_GX2_ATTRIB_FORMAT_FLOAT_32_32_32_32);
        WHBGfxInitShaderAttribute(&mut group, c"aColour".as_ptr(), 1, 0, GX2AttribFormat_GX2_ATTRIB_FORMAT_FLOAT_32_32_32_32);
        WHBGfxInitFetchShader(&mut group);

        let mut pos = vertex_buffer(&POSITIONS);
        let mut col = vertex_buffer(&COLOURS);

        let mut frame = 0u32;
        while WHBProcIsRunning() != 0 {
            WHBGfxBeginRender();

            WHBGfxBeginRenderTV();
            WHBGfxClearColor(0.0, 0.0, 0.3, 1.0);
            draw(&mut group, &mut pos, &mut col);
            WHBGfxFinishRenderTV();

            WHBGfxBeginRenderDRC();
            WHBGfxClearColor(0.3, 0.0, 0.3, 1.0);
            draw(&mut group, &mut pos, &mut col);
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
