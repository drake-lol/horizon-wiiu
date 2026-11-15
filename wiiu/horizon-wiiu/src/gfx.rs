//! GX2 rendering: the position + colour shader (`uModelViewProj * aPosition`, colour passed through), vertex
//! buffers per chunk, and the fixed pipeline state the world needs.

use crate::math::Mat4;
use crate::world::Vertex;
use core::ffi::c_void;
use core::mem::{size_of, zeroed};
use wut_sys::*;

static SHADER: &[u8] = include_bytes!("../assets/color_mvp.gsh");

const VERTEX_BUFFER_FLAGS: u32 = GX2RResourceFlags_GX2R_RESOURCE_BIND_VERTEX_BUFFER
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_READ
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_WRITE
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_GPU_READ;

pub struct Gfx {
    group: Box<WHBGfxShaderGroup>,
}

pub struct Mesh {
    buffer: GX2RBuffer,
    pub vertices: u32,
}

impl Gfx {
    /// Initialise GX2 (through WHB) and load the shader. `None` if either step fails.
    pub fn new() -> Option<Gfx> {
        unsafe {
            if WHBGfxInit() == 0 {
                return None;
            }
            let mut group: Box<WHBGfxShaderGroup> = Box::new(zeroed());
            if WHBGfxLoadGFDShaderGroup(&mut *group, 0, SHADER.as_ptr() as *const c_void) == 0 {
                return None;
            }
            WHBGfxInitShaderAttribute(&mut *group, c"aPosition".as_ptr(), 0, 0, GX2AttribFormat_GX2_ATTRIB_FORMAT_FLOAT_32_32_32_32);
            WHBGfxInitShaderAttribute(&mut *group, c"aColor".as_ptr(), 0, 16, GX2AttribFormat_GX2_ATTRIB_FORMAT_UNORM_8_8_8_8);
            WHBGfxInitFetchShader(&mut *group);
            Some(Gfx { group })
        }
    }

    pub fn upload(&self, verts: &[Vertex]) -> Option<Mesh> {
        if verts.is_empty() {
            return None;
        }
        unsafe {
            let mut buffer: GX2RBuffer = zeroed();
            buffer.flags = VERTEX_BUFFER_FLAGS;
            buffer.elemSize = size_of::<Vertex>() as u32;
            buffer.elemCount = verts.len() as u32;
            if GX2RCreateBuffer(&mut buffer) == 0 {
                return None;
            }
            let p = GX2RLockBufferEx(&mut buffer, 0) as *mut Vertex;
            core::ptr::copy_nonoverlapping(verts.as_ptr(), p, verts.len());
            GX2RUnlockBufferEx(&mut buffer, 0);
            Some(Mesh { buffer, vertices: verts.len() as u32 })
        }
    }

    pub fn begin_frame(&self) {
        unsafe { WHBGfxBeginRender() };
    }

    pub fn end_frame(&self) {
        unsafe { WHBGfxFinishRender() };
    }

    /// Draw one screen. `tv` picks the TV or the GamePad buffers.
    pub fn draw_screen(&self, tv: bool, sky: [f32; 3], mvp: &Mat4, meshes: &mut [Mesh]) {
        unsafe {
            if tv {
                WHBGfxBeginRenderTV();
            } else {
                WHBGfxBeginRenderDRC();
            }
            WHBGfxClearColor(sky[0], sky[1], sky[2], 1.0);

            GX2SetDepthStencilControl(
                1,
                1,
                GX2CompareFunction_GX2_COMPARE_FUNC_LESS,
                0,
                0,
                GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
                GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
                GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
            );
            GX2SetCullOnlyControl(GX2FrontFace_GX2_FRONT_FACE_CCW, 0, 1);

            let group = &*self.group as *const WHBGfxShaderGroup as *mut WHBGfxShaderGroup;
            GX2SetFetchShader(&mut (*group).fetchShader);
            GX2SetVertexShader((*group).vertexShader);
            GX2SetPixelShader((*group).pixelShader);
            GX2SetVertexUniformReg(0, 16, mvp.as_ptr() as *const c_void);

            for mesh in meshes.iter_mut() {
                GX2RSetAttributeBuffer(&mut mesh.buffer, 0, size_of::<Vertex>() as u32, 0);
                GX2DrawEx(GX2PrimitiveMode_GX2_PRIMITIVE_MODE_QUADS, mesh.vertices, 0, 1);
            }

            if tv {
                WHBGfxFinishRenderTV();
            } else {
                WHBGfxFinishRenderDRC();
            }
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe { WHBGfxShutdown() };
    }
}
