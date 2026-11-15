/*!
# GX2 (Wii U) backend

Horizon's port of wgpu to the Wii U GPU (Latte, an R700 derivative), through Nintendo's GX2 API as wut
exposes it. It takes the place of the `Noop` backend on `target_vendor = "wiiu"`, so wgpu-core needs no
changes: the game asks for `Backends::NOOP` there.

- Shaders: naga modules compile at pipeline creation, on the console, with `naga-latte` (wiiu/naga-latte).
- Memory is shared between CPU and GPU (MEM2). Command buffers record a list of operations that replay at
  submit: GPU work through GX2 calls, copies on the CPU after waiting for the GPU (GX2DrawDone) if it may
  still be using what they overwrite.
- Byte order: the CPU is big-endian. Vertex data, index data and storage buffers stay big-endian and the GPU
  swaps them as it reads (fetch shaders, index type, `naga-latte`'s fetches). Uniform buffers are read through
  the constant cache, which cannot swap, so their contents are swapped (32-bit words) as they are copied in.
- The surface is WHBGfx's TV colour buffer, copied to the TV and the GamePad at present.

Not done yet: mipmaps (textures keep level 0), sampling depth textures, compute, queries, MSAA.
*/
#![allow(unused_variables, clippy::missing_safety_doc)]

use alloc::{boxed::Box, string::String, sync::Arc, vec, vec::Vec};
use core::{alloc::Layout, ptr, sync::atomic::Ordering, time::Duration};
use std::sync::Mutex;

#[cfg(supports_64bit_atomics)]
use core::sync::atomic::AtomicU64;
#[cfg(not(supports_64bit_atomics))]
use portable_atomic::AtomicU64;

use wut_sys::*;

use crate::TlasInstance;

mod command;
mod conv;
mod shader;

/// Print what the backend does (to the console / Cemu's log), while it is brought up.
pub(crate) const TRACE: bool = true;

macro_rules! trace {
    ($($t:tt)*) => {
        if $crate::gx2::TRACE {
            std::println!($($t)*);
        }
    };
}
pub(crate) use trace;

pub use command::CommandBuffer;

#[derive(Clone, Debug)]
pub struct Api;

type DeviceResult<T> = Result<T, crate::DeviceError>;

/// GPU-visible memory (MEM2, through the default heap).
#[derive(Debug)]
pub struct GpuMem {
    pub ptr: *mut u8,
    pub size: usize,
    layout: Layout,
}

unsafe impl Send for GpuMem {}
unsafe impl Sync for GpuMem {}

impl GpuMem {
    pub fn new(size: usize, align: usize) -> Self {
        let layout = Layout::from_size_align(size.max(1).next_multiple_of(align.max(4)), align.max(4)).unwrap();
        // SAFETY: non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "GX2: out of memory ({size} bytes)");
        trace!("gx2: alloc {:p} +{:#x}", ptr, layout.size());
        // the zeroes reach memory before the GPU reads it
        unsafe { GX2Invalidate(GX2InvalidateMode_GX2_INVALIDATE_MODE_CPU, ptr.cast(), layout.size() as u32) };
        GpuMem { ptr, size: layout.size(), layout }
    }
}

impl Drop for GpuMem {
    fn drop(&mut self) {
        trace!("gx2: free {:p}", self.ptr);
        // SAFETY: allocated with this layout.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

/// CPU writes reach memory, and the GPU's caches forget what they held there.
pub(crate) fn cpu_wrote(ptr: *const u8, size: usize) {
    let mode = GX2InvalidateMode_GX2_INVALIDATE_MODE_CPU
        | GX2InvalidateMode_GX2_INVALIDATE_MODE_ATTRIBUTE_BUFFER
        | GX2InvalidateMode_GX2_INVALIDATE_MODE_TEXTURE
        | GX2InvalidateMode_GX2_INVALIDATE_MODE_UNIFORM_BLOCK;
    unsafe { GX2Invalidate(mode, ptr as *mut _, size as u32) };
}

unsafe extern "C" {
    fn DCInvalidateRange(addr: *mut core::ffi::c_void, size: u32);
}

/// The CPU forgets cached lines, before reading what the GPU wrote.
pub(crate) fn cpu_will_read(ptr: *const u8, size: usize) {
    unsafe { DCInvalidateRange(ptr as *mut _, size as u32) };
}

macro_rules! unit_resources {
    ($($name:ident),*) => {$(
        #[derive(Debug)]
        pub struct $name;
    )*};
}
unit_resources!(QuerySet, PipelineCache, ComputePipeline, AccelerationStructure, RayTracingPipeline);

pub struct Instance;
pub struct Adapter;

/// What the queue's replay shares across submits.
pub struct Shared {
    /// The GX2 context state every pass starts from (GX2 clears overwrite state).
    context: *mut GX2ContextState,
    _context_mem: GpuMem,
    /// GPU work may still be running: wait before the CPU writes memory it could read.
    gpu_busy: bool,
}

unsafe impl Send for Shared {}

impl core::fmt::Debug for Shared {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Shared")
    }
}

impl Shared {
    fn new() -> Self {
        let mem = GpuMem::new(core::mem::size_of::<GX2ContextState>(), 256);
        let context = mem.ptr.cast::<GX2ContextState>();
        // SAFETY: zeroed, aligned context state memory.
        unsafe { GX2SetupContextStateEx(context, 1) };
        Shared { context, _context_mem: mem, gpu_busy: true }
    }

    /// Back to the pass-starting state, in uniform-block shader mode with GPRs split between stages.
    pub(crate) fn reset_state(&self) {
        unsafe {
            GX2SetContextState(self.context);
            GX2SetShaderModeEx(GX2ShaderMode_GX2_SHADER_MODE_UNIFORM_BLOCK, 48, 64, 0, 0, 200, 192);
            // wgpu's clip space: z in 0..1
            GX2SetRasterizerClipControlEx(1, 1, 1);
        }
    }

    /// Wait for the GPU before touching memory it may still read or write.
    pub(crate) fn sync(&mut self) {
        if self.gpu_busy {
            unsafe { GX2DrawDone() };
            self.gpu_busy = false;
        }
    }
}

#[derive(Debug)]
pub struct Device {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Debug)]
pub struct Queue {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Debug)]
pub struct Surface {
    configured: Mutex<Option<wgt::TextureFormat>>,
}

#[derive(Debug)]
pub struct Buffer {
    pub(crate) mem: GpuMem,
    pub(crate) size: u64,
    /// Contents kept as 32-bit little-endian words (uniform buffers).
    pub(crate) swapped: bool,
}

pub struct Texture {
    pub(crate) surface: GX2Surface,
    pub(crate) mem: Option<GpuMem>,
    pub(crate) format: wgt::TextureFormat,
    pub(crate) bpe: u32,
    pub(crate) mips: u32,
    pub(crate) linear: bool,
}

impl core::fmt::Debug for Texture {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = &self.surface;
        write!(f, "Texture({}x{}x{} {:?})", s.width, s.height, s.depth, self.format)
    }
}

unsafe impl Send for Texture {}
unsafe impl Sync for Texture {}


pub struct TextureView {
    pub(crate) texture: Box<GX2Texture>,
    pub(crate) color: Option<Box<GX2ColorBuffer>>,
    pub(crate) depth: Option<Box<GX2DepthBuffer>>,
    /// Views of mip levels the texture does not have: drawing to them is skipped.
    pub(crate) dead: bool,
    pub(crate) surface_size: usize,
}

impl core::fmt::Debug for TextureView {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TextureView(color {}, depth {}, dead {})", self.color.is_some(), self.depth.is_some(), self.dead)
    }
}

unsafe impl Send for TextureView {}
unsafe impl Sync for TextureView {}


#[derive(Debug)]
pub struct Sampler {
    pub(crate) sampler: Box<GX2Sampler>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Uniform,
    Storage,
    Texture,
    Sampler,
}

#[derive(Debug)]
pub struct BindGroupLayout {
    entries: Vec<wgt::BindGroupLayoutEntry>,
}

/// Where one binding goes in each stage.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    pub binding: u32,
    pub kind: Kind,
    pub vs: Option<u8>,
    pub ps: Option<u8>,
    /// Index among the group's dynamic offsets.
    pub dynamic: Option<usize>,
}

#[derive(Debug)]
pub struct PipelineLayout {
    pub(crate) groups: Vec<Vec<Slot>>,
}

impl PipelineLayout {
    fn options(&self, vertex: bool) -> naga_latte::Options {
        let mut o = naga_latte::Options::default();
        for (g, slots) in self.groups.iter().enumerate() {
            for s in slots {
                let Some(slot) = (if vertex { s.vs } else { s.ps }) else { continue };
                let rb = naga::ResourceBinding { group: g as u32, binding: s.binding };
                match s.kind {
                    Kind::Uniform => o.uniform_blocks.insert(rb, slot),
                    Kind::Storage => o.storage_buffers.insert(rb, slot),
                    Kind::Texture => o.textures.insert(rb, slot),
                    Kind::Sampler => o.samplers.insert(rb, slot),
                };
            }
        }
        o
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Res {
    Buffer { ptr: *const u8, size: u32 },
    Texture(*const GX2Texture),
    Sampler(*const GX2Sampler),
}

#[derive(Debug)]
pub struct BindGroup {
    pub(crate) entries: Vec<(u32, Res)>,
}

unsafe impl Send for BindGroup {}
unsafe impl Sync for BindGroup {}

#[derive(Debug)]
pub struct ShaderModule {
    module: naga::Module,
    info: naga::valid::ModuleInfo,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Blend {
    pub color: (GX2BlendMode, GX2BlendMode, GX2BlendCombineMode),
    pub alpha: (GX2BlendMode, GX2BlendMode, GX2BlendCombineMode),
}

pub struct RenderPipeline {
    pub(crate) vs: shader::VertexProgram,
    pub(crate) ps: Option<shader::PixelProgram>,
    pub(crate) fetch: shader::FetchProgram,
    pub(crate) groups: Vec<Vec<Slot>>,
    pub(crate) strides: Vec<u32>,
    pub(crate) mode: GX2PrimitiveMode,
    pub(crate) front_face: GX2FrontFace,
    pub(crate) cull_front: bool,
    pub(crate) cull_back: bool,
    pub(crate) depth: Option<wgt::DepthStencilState>,
    pub(crate) blends: [Option<Blend>; 8],
    pub(crate) masks: [GX2ChannelMask; 8],
    pub(crate) targets: u32,
}

unsafe impl Send for RenderPipeline {}
unsafe impl Sync for RenderPipeline {}

impl core::fmt::Debug for RenderPipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RenderPipeline")
    }
}

#[derive(Debug)]
pub struct Fence {
    value: AtomicU64,
    /// (fence value, GX2 timestamp of the submit that signals it)
    pending: Mutex<Vec<(crate::FenceValue, i64)>>,
}

impl Fence {
    fn update(&self) -> crate::FenceValue {
        let retired = unsafe { GX2GetRetiredTimeStamp() };
        let mut pending = self.pending.lock().unwrap();
        let mut done = self.value.load(Ordering::Acquire);
        pending.retain(|(v, ts)| {
            if *ts <= retired {
                done = done.max(*v);
                false
            } else {
                true
            }
        });
        self.value.store(done, Ordering::Release);
        done
    }
}

impl crate::Api for Api {
    const VARIANT: wgt::Backend = wgt::Backend::Noop;

    type Instance = Instance;
    type Surface = Surface;
    type Adapter = Adapter;
    type Device = Device;

    type Queue = Queue;
    type CommandEncoder = CommandBuffer;
    type CommandBuffer = CommandBuffer;

    type Buffer = Buffer;
    type Texture = Texture;
    type SurfaceTexture = Texture;
    type TextureView = TextureView;
    type Sampler = Sampler;
    type QuerySet = QuerySet;
    type Fence = Fence;
    type AccelerationStructure = AccelerationStructure;
    type PipelineCache = PipelineCache;

    type BindGroupLayout = BindGroupLayout;
    type BindGroup = BindGroup;
    type PipelineLayout = PipelineLayout;
    type ShaderModule = ShaderModule;
    type RenderPipeline = RenderPipeline;
    type RayTracingPipeline = RayTracingPipeline;
    type ComputePipeline = ComputePipeline;
}

crate::impl_dyn_resource!(
    AccelerationStructure,
    Adapter,
    BindGroup,
    BindGroupLayout,
    Buffer,
    CommandBuffer,
    ComputePipeline,
    Device,
    Fence,
    Instance,
    PipelineCache,
    PipelineLayout,
    QuerySet,
    Queue,
    RayTracingPipeline,
    RenderPipeline,
    Sampler,
    ShaderModule,
    Surface,
    Texture,
    TextureView
);

impl core::fmt::Debug for Instance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("GX2 Instance")
    }
}
impl core::fmt::Debug for Adapter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("GX2 Adapter")
    }
}

impl crate::DynAccelerationStructure for AccelerationStructure {}
impl crate::DynBindGroup for BindGroup {}
impl crate::DynBindGroupLayout for BindGroupLayout {}
impl crate::DynBuffer for Buffer {}
impl crate::DynCommandBuffer for CommandBuffer {}
impl crate::DynComputePipeline for ComputePipeline {}
impl crate::DynFence for Fence {}
impl crate::DynPipelineCache for PipelineCache {}
impl crate::DynPipelineLayout for PipelineLayout {}
impl crate::DynQuerySet for QuerySet {}
impl crate::DynRenderPipeline for RenderPipeline {}
impl crate::DynRayTracingPipeline for RayTracingPipeline {}
impl crate::DynSampler for Sampler {}
impl crate::DynShaderModule for ShaderModule {}
impl crate::DynSurfaceTexture for Texture {}
impl crate::DynTexture for Texture {}
impl crate::DynTextureView for TextureView {}

impl core::borrow::Borrow<dyn crate::DynTexture> for Texture {
    fn borrow(&self) -> &dyn crate::DynTexture {
        self
    }
}

/// The TV picture WHBGfx renders (720p).
const TV: (u32, u32) = (1280, 720);

impl crate::Instance for Instance {
    type A = Api;

    /// GX2 must already be up: the Wii U client calls WHBProcInit and WHBGfxInit before wgpu.
    unsafe fn init(_desc: &crate::InstanceDescriptor<'_>) -> Result<Self, crate::InstanceError> {
        Ok(Instance)
    }

    unsafe fn create_surface(
        &self,
        _display_handle: raw_window_handle::RawDisplayHandle,
        _window_handle: raw_window_handle::RawWindowHandle,
    ) -> Result<Surface, crate::InstanceError> {
        Ok(Surface { configured: Mutex::new(None) })
    }

    unsafe fn enumerate_adapters(&self, _surface_hint: Option<&Surface>) -> Vec<crate::ExposedAdapter<Api>> {
        let mut limits = wgt::Limits::downlevel_webgl2_defaults();
        limits.max_texture_dimension_1d = 8192;
        limits.max_texture_dimension_2d = 8192;
        limits.max_texture_dimension_3d = 2048;
        limits.max_texture_array_layers = 2048;
        limits.max_bind_groups = 4;
        limits.max_sampled_textures_per_shader_stage = 16;
        limits.max_samplers_per_shader_stage = 16;
        limits.max_uniform_buffers_per_shader_stage = 12;
        limits.max_storage_buffers_per_shader_stage = 4;
        limits.max_storage_buffer_binding_size = 1 << 26;
        limits.max_uniform_buffer_binding_size = 1 << 16;
        limits.max_vertex_buffers = 16;
        limits.max_vertex_attributes = 16;
        limits.max_vertex_buffer_array_stride = 2048;
        limits.min_uniform_buffer_offset_alignment = 256;
        limits.min_storage_buffer_offset_alignment = 256;
        limits.max_inter_stage_shader_variables = 31;
        let mut flags = wgt::DownlevelFlags::all();
        flags.remove(
            wgt::DownlevelFlags::COMPUTE_SHADERS
                | wgt::DownlevelFlags::INDIRECT_EXECUTION
                | wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE
                | wgt::DownlevelFlags::DEPTH_TEXTURE_AND_BUFFER_COPIES
                | wgt::DownlevelFlags::MULTISAMPLED_SHADING
                | wgt::DownlevelFlags::ANISOTROPIC_FILTERING,
        );
        let capabilities = crate::Capabilities {
            limits,
            alignments: crate::Alignments {
                buffer_copy_offset: wgt::BufferSize::new(4).unwrap(),
                buffer_copy_pitch: wgt::BufferSize::new(256).unwrap(),
                uniform_bounds_check_alignment: wgt::BufferSize::new(16).unwrap(),
                raw_tlas_instance_size: 0,
                ray_tracing_scratch_buffer_alignment: 1,
                ray_tracing_pipeline_group_data_size: 1,
                ray_tracing_pipeline_group_data_alignment: 1,
                ray_tracing_pipeline_data_offset_alignment: 1,
            },
            downlevel: wgt::DownlevelCapabilities {
                flags,
                limits: wgt::DownlevelLimits {},
                shader_model: wgt::ShaderModel::Sm4,
            },
            cooperative_matrix_properties: Vec::new(),
        };
        vec![crate::ExposedAdapter {
            adapter: Adapter,
            info: wgt::AdapterInfo {
                name: String::from("Latte (Wii U GPU7) via GX2"),
                driver: String::from("Horizon GX2"),
                ..wgt::AdapterInfo::new(wgt::DeviceType::IntegratedGpu, wgt::Backend::Noop)
            },
            features: wgt::Features::empty(),
            capabilities,
        }]
    }
}

impl crate::Surface for Surface {
    type A = Api;

    unsafe fn configure(&self, _device: &Device, config: &crate::SurfaceConfiguration) -> Result<(), crate::SurfaceError> {
        *self.configured.lock().unwrap() = Some(config.format);
        Ok(())
    }

    unsafe fn unconfigure(&self, _device: &Device) {
        *self.configured.lock().unwrap() = None;
    }

    unsafe fn acquire_texture(
        &self,
        _timeout: Option<Duration>,
        _fence: &Fence,
    ) -> Result<crate::AcquiredSurfaceTexture<Api>, crate::SurfaceError> {
        // SAFETY: WHBGfx is initialised (Instance::init's contract).
        unsafe {
            WHBGfxBeginRender();
            let cb = &*WHBGfxGetTVColourBuffer();
            Ok(crate::AcquiredSurfaceTexture {
                texture: Texture {
                    surface: cb.surface,
                    mem: None,
                    format: wgt::TextureFormat::Rgba8Unorm,
                    bpe: 4,
                    mips: 1,
                    linear: false,
                },
                suboptimal: false,
            })
        }
    }

    unsafe fn discard_texture(&self, _texture: Texture) {}
}

impl crate::Adapter for Adapter {
    type A = Api;

    unsafe fn open(
        &self,
        _features: wgt::Features,
        _limits: &wgt::Limits,
        _memory_hints: &wgt::MemoryHints,
    ) -> DeviceResult<crate::OpenDevice<Api>> {
        let shared = Arc::new(Mutex::new(Shared::new()));
        Ok(crate::OpenDevice { device: Device { shared: shared.clone() }, queue: Queue { shared } })
    }

    unsafe fn texture_format_capabilities(&self, format: wgt::TextureFormat) -> crate::TextureFormatCapabilities {
        use crate::TextureFormatCapabilities as C;
        if conv::surface_format(format).is_none() {
            return C::empty();
        }
        let mut caps = C::SAMPLED | C::COPY_SRC | C::COPY_DST;
        if format.is_depth_stencil_format() {
            caps |= C::DEPTH_STENCIL_ATTACHMENT;
        } else {
            caps |= C::SAMPLED_LINEAR | C::COLOR_ATTACHMENT;
            if format.sample_type(None, None) == Some(wgt::TextureSampleType::Float { filterable: true }) {
                caps |= C::COLOR_ATTACHMENT_BLEND;
            }
        }
        caps
    }

    unsafe fn surface_capabilities(&self, _surface: &Surface) -> Option<crate::SurfaceCapabilities> {
        Some(crate::SurfaceCapabilities {
            formats: vec![wgt::SurfaceFormatCapabilities {
                format: wgt::TextureFormat::Rgba8Unorm,
                color_spaces: wgt::SurfaceColorSpaces::SRGB,
            }],
            maximum_frame_latency: 1..=2,
            current_extent: Some(wgt::Extent3d { width: TV.0, height: TV.1, depth_or_array_layers: 1 }),
            usage: wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::COPY_SRC | wgt::TextureUses::COPY_DST,
            present_modes: vec![wgt::PresentMode::Fifo],
            composite_alpha_modes: vec![wgt::CompositeAlphaMode::Opaque],
        })
    }

    unsafe fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        wgt::PresentationTimestamp::INVALID_TIMESTAMP
    }

    fn get_ordered_buffer_usages(&self) -> wgt::BufferUses {
        wgt::BufferUses::INCLUSIVE | wgt::BufferUses::MAP_WRITE
    }

    fn get_ordered_texture_usages(&self) -> wgt::TextureUses {
        wgt::TextureUses::INCLUSIVE | wgt::TextureUses::COLOR_TARGET | wgt::TextureUses::DEPTH_STENCIL_WRITE
    }
}

impl crate::Queue for Queue {
    type A = Api;

    unsafe fn submit(
        &self,
        command_buffers: &[&CommandBuffer],
        _surface_textures: &[&Texture],
        (fence, fence_value): (&Fence, crate::FenceValue),
    ) -> DeviceResult<()> {
        let mut shared = self.shared.lock().unwrap();
        trace!("gx2: submit {} buffers, fence {}", command_buffers.len(), fence_value);
        for cb in command_buffers {
            // SAFETY: wgpu-core keeps every resource the commands use alive until the fence passes.
            unsafe { cb.replay(&mut shared) };
        }
        unsafe { GX2Flush() };
        let ts = unsafe { GX2GetLastSubmittedTimeStamp() };
        fence.pending.lock().unwrap().push((fence_value, ts));
        Ok(())
    }

    unsafe fn present(&self, _surface: &Surface, _texture: Texture) -> Result<(), crate::SurfaceError> {
        let mut shared = self.shared.lock().unwrap();
        unsafe {
            let tv = WHBGfxGetTVColourBuffer();
            GX2CopyColorBufferToScanBuffer(tv, GX2ScanTarget_GX2_SCAN_TARGET_TV);
            GX2CopyColorBufferToScanBuffer(tv, GX2ScanTarget_GX2_SCAN_TARGET_DRC);
            // swaps, flushes and waits for the GPU
            WHBGfxFinishRender();
        }
        shared.gpu_busy = false;
        Ok(())
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        1.0
    }

    unsafe fn wait_for_idle(&self) -> Result<(), crate::DeviceError> {
        self.shared.lock().unwrap().sync();
        Ok(())
    }
}

/// One stage's next free slot of each kind, as bindings are given slots.
#[derive(Default)]
struct Counters {
    blocks: u8,
    textures: u8,
    samplers: u8,
}

impl Counters {
    fn next(&mut self, kind: Kind) -> u8 {
        let c = match kind {
            Kind::Uniform | Kind::Storage => &mut self.blocks,
            Kind::Texture => &mut self.textures,
            Kind::Sampler => &mut self.samplers,
        };
        *c += 1;
        *c - 1
    }
}

fn kind_of(ty: &wgt::BindingType) -> Option<Kind> {
    Some(match ty {
        wgt::BindingType::Buffer { ty: wgt::BufferBindingType::Uniform, .. } => Kind::Uniform,
        wgt::BindingType::Buffer { ty: wgt::BufferBindingType::Storage { .. }, .. } => Kind::Storage,
        wgt::BindingType::Texture { .. } => Kind::Texture,
        wgt::BindingType::Sampler(_) => Kind::Sampler,
        _ => return None,
    })
}

static WARNED_MIPS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

impl crate::Device for Device {
    type A = Api;

    unsafe fn create_buffer(&self, desc: &crate::BufferDescriptor) -> DeviceResult<Buffer> {
        Ok(Buffer {
            mem: GpuMem::new(desc.size as usize, 256),
            size: desc.size,
            swapped: desc.usage.contains(wgt::BufferUses::UNIFORM),
        })
    }

    unsafe fn destroy_buffer(&self, _buffer: Buffer) {}
    unsafe fn add_raw_buffer(&self, _buffer: &Buffer) {}

    unsafe fn map_buffer(&self, buffer: &Buffer, range: crate::MemoryRange) -> DeviceResult<crate::BufferMapping> {
        // reading back what the GPU wrote: let it finish first
        self.shared.lock().unwrap().sync();
        let p = unsafe { buffer.mem.ptr.add(range.start as usize) };
        cpu_will_read(p, (range.end - range.start) as usize);
        Ok(crate::BufferMapping { ptr: ptr::NonNull::new(p).unwrap(), is_coherent: false })
    }

    unsafe fn unmap_buffer(&self, buffer: &Buffer) {
        cpu_wrote(buffer.mem.ptr, buffer.size as usize);
    }

    unsafe fn flush_mapped_ranges<I>(&self, buffer: &Buffer, ranges: I)
    where
        I: Iterator<Item = crate::MemoryRange>,
    {
        for r in ranges {
            cpu_wrote(unsafe { buffer.mem.ptr.add(r.start as usize) }, (r.end - r.start) as usize);
        }
    }

    unsafe fn invalidate_mapped_ranges<I>(&self, buffer: &Buffer, ranges: I)
    where
        I: Iterator<Item = crate::MemoryRange>,
    {
        for r in ranges {
            cpu_will_read(unsafe { buffer.mem.ptr.add(r.start as usize) }, (r.end - r.start) as usize);
        }
    }

    unsafe fn create_texture(&self, desc: &crate::TextureDescriptor) -> DeviceResult<Texture> {
        let (format, bpe) = conv::surface_format(desc.format).ok_or(crate::DeviceError::Unexpected)?;
        if desc.mip_level_count > 1 && !WARNED_MIPS.swap(true, Ordering::Relaxed) {
            log::warn!("GX2: mipmaps not supported yet; textures keep level 0");
        }
        let target = desc.usage.intersects(
            wgt::TextureUses::COLOR_TARGET
                | wgt::TextureUses::DEPTH_STENCIL_WRITE
                | wgt::TextureUses::DEPTH_STENCIL_READ,
        );
        let depth = desc.format.is_depth_stencil_format();
        let layers = desc.size.depth_or_array_layers;
        // SAFETY: plain data, filled by GX2CalcSurfaceSizeAndAlignment.
        let mut s: GX2Surface = unsafe { core::mem::zeroed() };
        s.dim = match desc.dimension {
            wgt::TextureDimension::D1 => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_2D,
            wgt::TextureDimension::D2 if layers > 1 => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_2D_ARRAY,
            wgt::TextureDimension::D2 => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_2D,
            wgt::TextureDimension::D3 => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_3D,
        };
        s.width = desc.size.width;
        s.height = desc.size.height;
        s.depth = layers;
        s.mipLevels = 1;
        s.format = format;
        s.aa = GX2AAMode_GX2_AA_MODE1X;
        let mut use_ = GX2SurfaceUse_GX2_SURFACE_USE_TEXTURE;
        if target {
            use_ |= if depth { GX2SurfaceUse_GX2_SURFACE_USE_DEPTH_BUFFER } else { GX2SurfaceUse_GX2_SURFACE_USE_COLOR_BUFFER };
        }
        s.__bindgen_anon_1.use_ = use_;
        // render targets tiled (the GPU's own layout); others linear, so copies are plain row writes
        s.tileMode = if target { GX2TileMode_GX2_TILE_MODE_DEFAULT } else { GX2TileMode_GX2_TILE_MODE_LINEAR_ALIGNED };
        unsafe { GX2CalcSurfaceSizeAndAlignment(&mut s) };
        let mem = GpuMem::new(s.imageSize as usize, s.alignment.max(256) as usize);
        s.image = mem.ptr.cast();
        Ok(Texture { surface: s, mem: Some(mem), format: desc.format, bpe, mips: 1, linear: !target })
    }

    unsafe fn destroy_texture(&self, _texture: Texture) {}
    unsafe fn add_raw_texture(&self, _texture: &Texture) {}

    unsafe fn create_texture_view(&self, texture: &Texture, desc: &crate::TextureViewDescriptor) -> DeviceResult<TextureView> {
        let r = &desc.range;
        let dead = r.base_mip_level >= texture.mips;
        let first_slice = r.base_array_layer;
        let slices = r.array_layer_count.unwrap_or(texture.surface.depth - first_slice).max(1);
        // SAFETY: plain data, then GX2's own register setup.
        let mut t: Box<GX2Texture> = Box::new(unsafe { core::mem::zeroed() });
        t.surface = texture.surface;
        if let Some((f, _)) = conv::surface_format(desc.format) {
            if !desc.format.is_depth_stencil_format() {
                t.surface.format = f;
            }
        }
        t.surface.dim = match desc.dimension {
            wgt::TextureViewDimension::Cube => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_CUBE,
            wgt::TextureViewDimension::CubeArray => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_CUBE,
            wgt::TextureViewDimension::D2Array => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_2D_ARRAY,
            wgt::TextureViewDimension::D3 => GX2SurfaceDim_GX2_SURFACE_DIM_TEXTURE_3D,
            _ => texture.surface.dim,
        };
        t.viewFirstMip = 0;
        t.viewNumMips = 1;
        t.viewFirstSlice = first_slice;
        t.viewNumSlices = slices;
        t.compMap = conv::comp_map(desc.format);
        unsafe { GX2InitTextureRegs(&mut *t) };
        let use_ = unsafe { texture.surface.__bindgen_anon_1.use_ };
        let mut color = None;
        let mut depth = None;
        if use_ & GX2SurfaceUse_GX2_SURFACE_USE_COLOR_BUFFER != 0 {
            let mut cb: Box<GX2ColorBuffer> = Box::new(unsafe { core::mem::zeroed() });
            cb.surface = texture.surface;
            cb.surface.format = t.surface.format;
            cb.viewMip = 0;
            cb.viewFirstSlice = first_slice;
            cb.viewNumSlices = 1;
            unsafe { GX2InitColorBufferRegs(&mut *cb) };
            color = Some(cb);
        }
        if use_ & GX2SurfaceUse_GX2_SURFACE_USE_DEPTH_BUFFER != 0 {
            let mut db: Box<GX2DepthBuffer> = Box::new(unsafe { core::mem::zeroed() });
            db.surface = texture.surface;
            db.viewMip = 0;
            db.viewFirstSlice = first_slice;
            db.viewNumSlices = 1;
            db.depthClear = 1.0;
            unsafe {
                GX2InitDepthBufferHiZEnable(&mut *db, 0);
                GX2InitDepthBufferRegs(&mut *db);
            }
            depth = Some(db);
        }
        Ok(TextureView { texture: t, color, depth, dead, surface_size: texture.surface.imageSize as usize })
    }

    unsafe fn destroy_texture_view(&self, _view: TextureView) {}

    unsafe fn create_sampler(&self, desc: &crate::SamplerDescriptor) -> DeviceResult<Sampler> {
        let mut s: Box<GX2Sampler> = Box::new(unsafe { core::mem::zeroed() });
        unsafe {
            GX2InitSampler(&mut *s, conv::address(desc.address_modes[0]), conv::filter(desc.min_filter));
            GX2InitSamplerClamping(
                &mut *s,
                conv::address(desc.address_modes[0]),
                conv::address(desc.address_modes[1]),
                conv::address(desc.address_modes[2]),
            );
            GX2InitSamplerXYFilter(
                &mut *s,
                conv::filter(desc.mag_filter),
                conv::filter(desc.min_filter),
                GX2TexAnisoRatio_GX2_TEX_ANISO_RATIO_NONE,
            );
            GX2InitSamplerZMFilter(&mut *s, GX2TexZFilterMode_GX2_TEX_Z_FILTER_MODE_POINT, conv::mip_filter(desc.mipmap_filter));
            GX2InitSamplerLOD(&mut *s, desc.lod_clamp.start, desc.lod_clamp.end.min(15.0), 0.0);
            if let Some(c) = desc.compare {
                GX2InitSamplerDepthCompare(&mut *s, conv::compare(c));
            }
            if let Some(b) = desc.border_color {
                GX2InitSamplerBorderType(
                    &mut *s,
                    match b {
                        wgt::SamplerBorderColor::OpaqueWhite => GX2TexBorderType_GX2_TEX_BORDER_TYPE_WHITE,
                        wgt::SamplerBorderColor::OpaqueBlack => GX2TexBorderType_GX2_TEX_BORDER_TYPE_BLACK,
                        _ => GX2TexBorderType_GX2_TEX_BORDER_TYPE_TRANSPARENT_BLACK,
                    },
                );
            }
        }
        Ok(Sampler { sampler: s })
    }

    unsafe fn destroy_sampler(&self, _sampler: Sampler) {}

    unsafe fn create_command_encoder(&self, _desc: &crate::CommandEncoderDescriptor<Queue>) -> DeviceResult<CommandBuffer> {
        Ok(CommandBuffer::new())
    }

    unsafe fn create_bind_group_layout(&self, desc: &crate::BindGroupLayoutDescriptor) -> DeviceResult<BindGroupLayout> {
        Ok(BindGroupLayout { entries: desc.entries.to_vec() })
    }

    unsafe fn destroy_bind_group_layout(&self, _bg_layout: BindGroupLayout) {}

    unsafe fn create_pipeline_layout(
        &self,
        desc: &crate::PipelineLayoutDescriptor<BindGroupLayout>,
    ) -> DeviceResult<PipelineLayout> {
        let (mut vs, mut ps) = (Counters::default(), Counters::default());
        let mut groups = Vec::new();
        for bgl in desc.bind_group_layouts {
            let mut slots = Vec::new();
            let mut dynamic = 0;
            if let Some(bgl) = bgl {
                for e in &bgl.entries {
                    let Some(kind) = kind_of(&e.ty) else { continue };
                    let dyn_index = match e.ty {
                        wgt::BindingType::Buffer { has_dynamic_offset: true, .. } => {
                            dynamic += 1;
                            Some(dynamic - 1)
                        }
                        _ => None,
                    };
                    slots.push(Slot {
                        binding: e.binding,
                        kind,
                        vs: e.visibility.contains(wgt::ShaderStages::VERTEX).then(|| vs.next(kind)),
                        ps: e.visibility.contains(wgt::ShaderStages::FRAGMENT).then(|| ps.next(kind)),
                        dynamic: dyn_index,
                    });
                }
            }
            groups.push(slots);
        }
        Ok(PipelineLayout { groups })
    }

    unsafe fn destroy_pipeline_layout(&self, _pipeline_layout: PipelineLayout) {}

    unsafe fn create_bind_group(
        &self,
        desc: &crate::BindGroupDescriptor<BindGroupLayout, Buffer, Sampler, TextureView, AccelerationStructure>,
    ) -> DeviceResult<BindGroup> {
        let mut entries = Vec::new();
        for e in desc.entries {
            let layout = desc.layout.entries.iter().find(|l| l.binding == e.binding);
            let Some(layout) = layout else { continue };
            let i = e.resource_index as usize;
            let res = match kind_of(&layout.ty) {
                Some(Kind::Uniform | Kind::Storage) => {
                    let b = &desc.buffers[i];
                    let size = b.size.map(|s| s.get()).unwrap_or(b.buffer.size - b.offset);
                    Res::Buffer { ptr: unsafe { b.buffer.mem.ptr.add(b.offset as usize) }, size: size as u32 }
                }
                Some(Kind::Texture) => Res::Texture(&*desc.textures[i].view.texture),
                Some(Kind::Sampler) => Res::Sampler(&*desc.samplers[i].sampler),
                None => continue,
            };
            entries.push((e.binding, res));
        }
        Ok(BindGroup { entries })
    }

    unsafe fn destroy_bind_group(&self, _group: BindGroup) {}

    unsafe fn create_shader_module(
        &self,
        _desc: &crate::ShaderModuleDescriptor,
        shader: crate::ShaderInput,
    ) -> Result<ShaderModule, crate::ShaderError> {
        match shader {
            crate::ShaderInput::Naga(n) => Ok(ShaderModule { module: n.module.into_owned(), info: n.info }),
            _ => Err(crate::ShaderError::Compilation(String::from("GX2 compiles naga modules only"))),
        }
    }

    unsafe fn destroy_shader_module(&self, _module: ShaderModule) {}

    unsafe fn create_render_pipeline(
        &self,
        desc: &crate::RenderPipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<RenderPipeline, crate::PipelineError> {
        let crate::VertexProcessor::Standard { vertex_buffers, vertex_stage } = &desc.vertex_processor else {
            return Err(crate::PipelineError::Linkage(wgt::ShaderStages::MESH, String::from("no mesh shaders on GX2")));
        };
        let err = |stage: wgt::ShaderStages| move |e: String| crate::PipelineError::Linkage(stage, e);
        let vs = shader::compile(
            &vertex_stage.module.module,
            &vertex_stage.module.info,
            naga::ShaderStage::Vertex,
            vertex_stage.entry_point,
            vertex_stage.constants,
            &desc.layout.options(true),
        )
        .map_err(err(wgt::ShaderStages::VERTEX))?;
        let ps = match &desc.fragment_stage {
            Some(fs) => Some(
                shader::compile(
                    &fs.module.module,
                    &fs.module.info,
                    naga::ShaderStage::Fragment,
                    fs.entry_point,
                    fs.constants,
                    &desc.layout.options(false),
                )
                .map_err(err(wgt::ShaderStages::FRAGMENT))?,
            ),
            None => None,
        };
        let mut blends = [None; 8];
        let mut masks = [0; 8];
        let mut targets = 0;
        for (i, t) in desc.color_targets.iter().enumerate().take(8) {
            let Some(t) = t else { continue };
            targets |= 1 << i;
            masks[i] = conv::channel_mask(t.write_mask);
            blends[i] = t.blend.map(|b| Blend {
                color: (conv::blend_factor(b.color.src_factor), conv::blend_factor(b.color.dst_factor), conv::blend_op(b.color.operation)),
                alpha: (conv::blend_factor(b.alpha.src_factor), conv::blend_factor(b.alpha.dst_factor), conv::blend_op(b.alpha.operation)),
            });
        }
        Ok(RenderPipeline {
            vs: shader::vertex(&vs),
            ps: ps.as_ref().map(shader::pixel),
            fetch: shader::fetch(vertex_buffers),
            groups: desc.layout.groups.clone(),
            strides: vertex_buffers.iter().map(|b| b.as_ref().map_or(0, |b| b.array_stride as u32)).collect(),
            mode: conv::primitive(desc.primitive.topology),
            front_face: match desc.primitive.front_face {
                wgt::FrontFace::Ccw => GX2FrontFace_GX2_FRONT_FACE_CCW,
                wgt::FrontFace::Cw => GX2FrontFace_GX2_FRONT_FACE_CW,
            },
            cull_front: desc.primitive.cull_mode == Some(wgt::Face::Front),
            cull_back: desc.primitive.cull_mode == Some(wgt::Face::Back),
            depth: desc.depth_stencil.clone(),
            blends,
            masks,
            targets,
        })
    }

    unsafe fn destroy_render_pipeline(&self, _pipeline: RenderPipeline) {}

    unsafe fn create_compute_pipeline(
        &self,
        _desc: &crate::ComputePipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<ComputePipeline, crate::PipelineError> {
        Err(crate::PipelineError::Linkage(wgt::ShaderStages::COMPUTE, String::from("no compute shaders on GX2")))
    }

    unsafe fn destroy_compute_pipeline(&self, _pipeline: ComputePipeline) {}

    unsafe fn create_ray_tracing_pipeline(
        &self,
        _desc: &crate::RayTracingPipelineDescriptor<PipelineLayout, ShaderModule, PipelineCache>,
    ) -> Result<RayTracingPipeline, crate::PipelineError> {
        Err(crate::PipelineError::Linkage(wgt::ShaderStages::empty(), String::from("no ray tracing on GX2")))
    }

    unsafe fn destroy_ray_tracing_pipeline(&self, _pipeline: RayTracingPipeline) {}

    unsafe fn get_raytracing_pipeline_group_data(
        &self,
        _pipeline: &RayTracingPipeline,
        groups: core::ops::Range<u32>,
    ) -> Result<Vec<u8>, crate::DeviceError> {
        Ok(vec![0; groups.count()])
    }

    unsafe fn create_pipeline_cache(&self, _desc: &crate::PipelineCacheDescriptor<'_>) -> Result<PipelineCache, crate::PipelineCacheError> {
        Ok(PipelineCache)
    }

    unsafe fn destroy_pipeline_cache(&self, _cache: PipelineCache) {}

    unsafe fn create_query_set(&self, _desc: &wgt::QuerySetDescriptor<crate::Label>) -> DeviceResult<QuerySet> {
        Ok(QuerySet)
    }

    unsafe fn destroy_query_set(&self, _set: QuerySet) {}

    unsafe fn create_fence(&self) -> DeviceResult<Fence> {
        Ok(Fence { value: AtomicU64::new(0), pending: Mutex::new(Vec::new()) })
    }

    unsafe fn destroy_fence(&self, _fence: Fence) {}

    unsafe fn get_fence_value(&self, fence: &Fence) -> DeviceResult<crate::FenceValue> {
        Ok(fence.update())
    }

    unsafe fn wait(&self, fence: &Fence, value: crate::FenceValue, _timeout: Option<Duration>) -> DeviceResult<bool> {
        if fence.update() >= value {
            return Ok(true);
        }
        let ts = fence.pending.lock().unwrap().iter().filter(|(v, _)| *v >= value).map(|(_, ts)| *ts).min();
        match ts {
            Some(ts) => {
                unsafe { GX2WaitTimeStamp(ts) };
                Ok(fence.update() >= value)
            }
            None => Ok(false),
        }
    }

    unsafe fn start_graphics_debugger_capture(&self) -> bool {
        false
    }

    unsafe fn stop_graphics_debugger_capture(&self) {}

    unsafe fn create_acceleration_structure(&self, _desc: &crate::AccelerationStructureDescriptor) -> DeviceResult<AccelerationStructure> {
        Err(crate::DeviceError::Unexpected)
    }

    unsafe fn get_acceleration_structure_build_sizes<'a>(
        &self,
        _desc: &crate::GetAccelerationStructureBuildSizesDescriptor<'a, Buffer>,
    ) -> crate::AccelerationStructureBuildSizes {
        Default::default()
    }

    unsafe fn get_acceleration_structure_device_address(&self, _acceleration_structure: &AccelerationStructure) -> wgt::BufferAddress {
        0
    }

    unsafe fn destroy_acceleration_structure(&self, _acceleration_structure: AccelerationStructure) {}

    fn tlas_instance_to_bytes(&self, _instance: TlasInstance) -> Vec<u8> {
        vec![]
    }

    fn get_internal_counters(&self) -> wgt::HalCounters {
        Default::default()
    }

    fn check_if_oom(&self) -> DeviceResult<()> {
        Ok(())
    }
}
