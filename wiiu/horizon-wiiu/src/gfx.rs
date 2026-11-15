//! GX2 rendering: the position + colour shader (`uModelViewProj * aPosition`, colour passed through), vertex
//! buffers per chunk, and the fixed pipeline state the world needs.

use crate::math::Mat4;
use crate::world::Vertex;
use core::ffi::c_void;
use core::mem::{size_of, zeroed};
use wut_sys::*;

static SHADER: &[u8] = include_bytes!("../assets/tex_mvp.gsh");

const VERTEX_BUFFER_FLAGS: u32 = GX2RResourceFlags_GX2R_RESOURCE_BIND_VERTEX_BUFFER
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_READ
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_CPU_WRITE
    | GX2RResourceFlags_GX2R_RESOURCE_USAGE_GPU_READ;

pub struct Gfx {
    group: Box<WHBGfxShaderGroup>,
    texture: Option<Box<GX2Texture>>,
    sampler: GX2Sampler,
    hud: Option<Mesh>,
    hud_count: u32,
}

/// Vertices the HUD buffer holds (two quads' worth per element, a few dozen elements).
pub const HUD_CAPACITY: usize = 4096;

unsafe extern "C" {
    fn memalign(align: usize, size: usize) -> *mut c_void;
}

pub struct Mesh {
    buffer: GX2RBuffer,
    pub vertices: u32,
}

impl Drop for Mesh {
    fn drop(&mut self) {
        unsafe { GX2RDestroyBufferEx(&mut self.buffer, 0) };
    }
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
            WHBGfxInitShaderAttribute(&mut *group, c"aTexCoord".as_ptr(), 0, 20, GX2AttribFormat_GX2_ATTRIB_FORMAT_FLOAT_32_32);
            WHBGfxInitFetchShader(&mut *group);

            let mut sampler: GX2Sampler = zeroed();
            GX2InitSampler(&mut sampler, GX2TexClampMode_GX2_TEX_CLAMP_MODE_CLAMP, GX2TexXYFilterMode_GX2_TEX_XY_FILTER_MODE_POINT);
            let mut gfx = Gfx { group, texture: None, sampler, hud: None, hud_count: 0 };
            gfx.hud = gfx.alloc(HUD_CAPACITY);
            Some(gfx)
        }
    }

    /// Create the block texture from RGBA bytes (`width * height * 4`).
    pub fn set_texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> bool {
        unsafe {
            let mut tex: Box<GX2Texture> = Box::new(zeroed());
            tex.surface.dim = GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_2D;
            tex.surface.width = width;
            tex.surface.height = height;
            tex.surface.depth = 1;
            tex.surface.mipLevels = 1;
            tex.surface.format = GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R8_G8_B8_A8;
            tex.surface.aa = GX2AAMode_GX2_AA_MODE1X;
            tex.surface.__bindgen_anon_1.use_ = GX2SurfaceUse_GX2_SURFACE_USE_TEXTURE;
            tex.surface.tileMode = GX2TileMode_GX2_TILE_MODE_LINEAR_ALIGNED;
            tex.viewNumMips = 1;
            tex.viewNumSlices = 1;
            // component map R, G, B, A = selects 0, 1, 2, 3
            tex.compMap = 0x0001_0203;
            GX2CalcSurfaceSizeAndAlignment(&mut tex.surface);
            GX2InitTextureRegs(&mut *tex);
            let image = memalign(tex.surface.alignment as usize, tex.surface.imageSize as usize) as *mut u8;
            if image.is_null() {
                return false;
            }
            core::ptr::write_bytes(image, 0, tex.surface.imageSize as usize);
            let pitch = tex.surface.pitch as usize;
            for y in 0..height as usize {
                let src = &rgba[y * width as usize * 4..(y + 1) * width as usize * 4];
                core::ptr::copy_nonoverlapping(src.as_ptr(), image.add(y * pitch * 4), src.len());
            }
            tex.surface.image = image as *mut c_void;
            GX2Invalidate(GX2InvalidateMode_GX2_INVALIDATE_MODE_CPU_TEXTURE, image as *mut c_void, tex.surface.imageSize);
            self.texture = Some(tex);
            true
        }
    }

    fn alloc(&self, capacity: usize) -> Option<Mesh> {
        unsafe {
            let mut buffer: GX2RBuffer = zeroed();
            buffer.flags = VERTEX_BUFFER_FLAGS;
            buffer.elemSize = size_of::<Vertex>() as u32;
            buffer.elemCount = capacity as u32;
            if GX2RCreateBuffer(&mut buffer) == 0 {
                return None;
            }
            Some(Mesh { buffer, vertices: 0 })
        }
    }

    pub fn upload(&self, verts: &[Vertex]) -> Option<Mesh> {
        if verts.is_empty() {
            return None;
        }
        let mut mesh = self.alloc(verts.len())?;
        unsafe {
            let p = GX2RLockBufferEx(&mut mesh.buffer, 0) as *mut Vertex;
            core::ptr::copy_nonoverlapping(verts.as_ptr(), p, verts.len());
            GX2RUnlockBufferEx(&mut mesh.buffer, 0);
        }
        mesh.vertices = verts.len() as u32;
        Some(mesh)
    }

    /// Replace the HUD's quads (at most `HUD_CAPACITY` vertices); drawn on top of both screens.
    pub fn set_hud(&mut self, verts: &[Vertex]) {
        let n = verts.len().min(HUD_CAPACITY);
        if let Some(h) = &mut self.hud {
            unsafe {
                let p = GX2RLockBufferEx(&mut h.buffer, 0) as *mut Vertex;
                core::ptr::copy_nonoverlapping(verts.as_ptr(), p, n);
                GX2RUnlockBufferEx(&mut h.buffer, 0);
            }
        }
        self.hud_count = n as u32;
    }

    pub fn begin_frame(&self) {
        unsafe { WHBGfxBeginRender() };
    }

    pub fn end_frame(&self) {
        unsafe { WHBGfxFinishRender() };
    }

    /// Draw one screen. `tv` picks the TV or the GamePad buffers.
    pub fn draw_screen(&mut self, tv: bool, sky: [f32; 3], mvp: &Mat4, ortho: &Mat4, meshes: &mut [Option<Mesh>]) {
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
            if let Some(tex) = &self.texture {
                GX2SetPixelTexture(&**tex, 0);
                GX2SetPixelSampler(&self.sampler, 0);
            }

            for mesh in meshes.iter_mut().flatten() {
                GX2RSetAttributeBuffer(&mut mesh.buffer, 0, size_of::<Vertex>() as u32, 0);
                GX2DrawEx(GX2PrimitiveMode_GX2_PRIMITIVE_MODE_QUADS, mesh.vertices, 0, 1);
            }

            // HUD: no depth, no culling, orthographic
            if self.hud_count > 0 {
                if let Some(h) = &mut self.hud {
                    GX2SetDepthOnlyControl(0, 0, GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS);
                    GX2SetCullOnlyControl(GX2FrontFace_GX2_FRONT_FACE_CCW, 0, 0);
                    GX2SetVertexUniformReg(0, 16, ortho.as_ptr() as *const c_void);
                    GX2RSetAttributeBuffer(&mut h.buffer, 0, size_of::<Vertex>() as u32, 0);
                    GX2DrawEx(GX2PrimitiveMode_GX2_PRIMITIVE_MODE_QUADS, self.hud_count, 0, 1);
                }
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
