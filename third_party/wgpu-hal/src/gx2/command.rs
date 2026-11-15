//! Command recording and replay. Encoders record a list; the queue replays it at submit, issuing GX2 calls
//! for GPU work and doing copies on the CPU (after waiting for the GPU when it may still use the memory).

use alloc::{vec, vec::Vec};
use core::mem;
use core::ops::Range;

use wut_sys::*;

use super::{cpu_wrote, Api, BindGroup, Buffer, Kind, PipelineLayout, QuerySet, RenderPipeline, Res, Shared, Texture, TextureView};

type DeviceResult<T> = Result<T, crate::DeviceError>;

#[derive(Debug)]
pub struct CommandBuffer {
    commands: Vec<Cmd>,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    src: usize,
    dst: usize,
    size: usize,
}

#[derive(Clone, Copy, Debug)]
struct TexCopy {
    /// Linear surface start, row pitch in bytes, texel size, slice size in bytes.
    image: *mut u8,
    pitch: usize,
    bpe: usize,
    slice: usize,
    linear: bool,
    swap: usize,
}

#[derive(Debug)]
enum Cmd {
    ClearBuffer { ptr: *mut u8, len: usize },
    CopyBuffer { src: *const u8, dst: *mut u8, swap: bool, regions: Vec<Region> },
    BufferToTexture { src: *const u8, dst: TexCopy, regions: Vec<crate::BufferTextureCopy> },
    TextureToBuffer { src: TexCopy, dst: *mut u8, regions: Vec<crate::BufferTextureCopy> },
    TextureToTexture { src: *const GX2Surface, dst: *mut GX2Surface, src_layer: u32, dst_layer: u32 },
    BeginPass { colors: Vec<(*mut GX2ColorBuffer, Option<[f32; 4]>, usize)>, depth: Option<(*mut GX2DepthBuffer, Option<f32>, Option<u8>, usize)>, extent: (u32, u32), dead: bool },
    EndPass,
    Pipeline(*const RenderPipeline),
    BindGroup { index: u32, group: *const BindGroup, offsets: Vec<u32> },
    VertexBuffer { slot: u32, ptr: *const u8, size: u32 },
    IndexBuffer { ptr: *const u8, u32_indices: bool },
    Viewport { x: f32, y: f32, w: f32, h: f32, near: f32, far: f32 },
    Scissor { x: u32, y: u32, w: u32, h: u32 },
    StencilRef(u8),
    BlendConstants([f32; 4]),
    Draw { first_vertex: u32, vertex_count: u32, first_instance: u32, instance_count: u32 },
    DrawIndexed { first_index: u32, index_count: u32, base_vertex: i32, first_instance: u32, instance_count: u32 },
}

unsafe impl Send for CommandBuffer {}
unsafe impl Sync for CommandBuffer {}

impl CommandBuffer {
    pub(crate) fn new() -> Self {
        Self { commands: Vec::new() }
    }

    /// Run the recorded commands.
    ///
    /// # Safety
    /// Every resource they refer to must still be alive.
    pub(crate) unsafe fn replay(&self, shared: &mut Shared) {
        let mut st = State::default();
        for c in &self.commands {
            super::trace!("gx2: {c:?}");
            unsafe { st.run(c, shared) };
        }
    }
}

fn tex_copy(t: &Texture) -> TexCopy {
    let s = &t.surface;
    let bpe = t.bpe as usize;
    TexCopy {
        image: s.image.cast(),
        pitch: s.pitch as usize * bpe,
        bpe,
        slice: (s.imageSize / s.depth.max(1)) as usize,
        linear: t.linear,
        swap: super::conv::swap_unit(t.format),
    }
}

/// Copy `len` bytes, reversing each `unit`-byte group (big-endian CPU data to the GPU's order, or back).
unsafe fn copy_swapped(src: *const u8, dst: *mut u8, len: usize, unit: usize) {
    unsafe {
        if unit <= 1 {
            core::ptr::copy(src, dst, len);
            return;
        }
        let mut i = 0;
        while i + unit <= len {
            for k in 0..unit {
                *dst.add(i + k) = *src.add(i + unit - 1 - k);
            }
            i += unit;
        }
    }
}

#[derive(Default)]
struct State {
    pipeline: Option<*const RenderPipeline>,
    groups: [Option<(*const BindGroup, Vec<u32>)>; 4],
    vertex: [(usize, u32); 16],
    index: Option<(*const u8, bool)>,
    stencil_ref: u8,
    /// Draws are skipped in a pass whose target is a mip level the texture does not have.
    dead: bool,
    in_pass: bool,
    /// The pass's colour and depth images (start, bytes), whose caches are flushed when it ends.
    pass_targets: Vec<(*mut u8, usize)>,
    pass_depth: Option<(*mut u8, usize)>,
}

impl State {
    unsafe fn run(&mut self, c: &Cmd, shared: &mut Shared) {
        unsafe {
            match c {
                Cmd::ClearBuffer { ptr, len } => {
                    shared.sync();
                    core::ptr::write_bytes(*ptr, 0, *len);
                    cpu_wrote(*ptr, *len);
                }
                Cmd::CopyBuffer { src, dst, swap, regions } => {
                    shared.sync();
                    for r in regions {
                        copy_swapped(src.add(r.src), dst.add(r.dst), r.size, if *swap { 4 } else { 1 });
                        cpu_wrote(dst.add(r.dst), r.size);
                    }
                }
                Cmd::BufferToTexture { src, dst, regions } => {
                    shared.sync();
                    shared.stats.uploads += 1;
                    if !dst.linear {
                        std::println!("GX2: upload to a render target texture skipped");
                        return;
                    }
                    for r in regions {
                        if r.texture_base.mip_level > 0 {
                            continue;
                        }
                        let o = r.texture_base.origin;
                        let row_bytes = r.size.width as usize * dst.bpe;
                        let bpr = r.buffer_layout.bytes_per_row.map_or(row_bytes, |b| b as usize);
                        let rows = r.buffer_layout.rows_per_image.unwrap_or(r.size.height) as usize;
                        for layer in 0..r.size.depth as usize {
                            let slice = r.texture_base.array_layer as usize + o.z as usize + layer;
                            for y in 0..r.size.height as usize {
                                let s = src.add(r.buffer_layout.offset as usize + (layer * rows + y) * bpr);
                                let d = dst.image.add(slice * dst.slice + (o.y as usize + y) * dst.pitch + o.x as usize * dst.bpe);
                                copy_swapped(s, d, row_bytes, dst.swap);
                            }
                            cpu_wrote(dst.image.add(slice * dst.slice), dst.slice);
                        }
                    }
                }
                Cmd::TextureToBuffer { src, dst, regions } => {
                    shared.sync();
                    if !src.linear {
                        std::println!("GX2: reading back a render target texture is not supported yet");
                        return;
                    }
                    for r in regions {
                        let o = r.texture_base.origin;
                        let row_bytes = r.size.width as usize * src.bpe;
                        let bpr = r.buffer_layout.bytes_per_row.map_or(row_bytes, |b| b as usize);
                        let rows = r.buffer_layout.rows_per_image.unwrap_or(r.size.height) as usize;
                        for layer in 0..r.size.depth as usize {
                            let slice = r.texture_base.array_layer as usize + o.z as usize + layer;
                            super::cpu_will_read(src.image.add(slice * src.slice), src.slice);
                            for y in 0..r.size.height as usize {
                                let s = src.image.add(slice * src.slice + (o.y as usize + y) * src.pitch + o.x as usize * src.bpe);
                                let d = dst.add(r.buffer_layout.offset as usize + (layer * rows + y) * bpr);
                                copy_swapped(s, d, row_bytes, src.swap);
                            }
                        }
                    }
                }
                Cmd::TextureToTexture { src, dst, src_layer, dst_layer } => {
                    GX2CopySurface(*src, 0, *src_layer, *dst, 0, *dst_layer);
                    shared.gpu_busy = true;
                }
                Cmd::BeginPass { colors, depth, extent, dead } => {
                    self.in_pass = true;
                    self.dead = *dead;
                    self.pipeline = None;
                    shared.stats.passes += 1;
                    if *dead {
                        shared.stats.dead_passes += 1;
                        return;
                    }
                    shared.stats.clears += colors.iter().filter(|c| c.1.is_some()).count() as u32;
                    // clears first: they overwrite state, which the reset below restores
                    for (cb, clear, _) in colors {
                        if let Some([r, g, b, a]) = clear {
                            GX2ClearColor(*cb, *r, *g, *b, *a);
                        }
                    }
                    if let Some((db, d, s, _)) = depth {
                        let mut flags = 0;
                        if d.is_some() {
                            flags |= GX2ClearFlags_GX2_CLEAR_FLAGS_DEPTH;
                        }
                        if s.is_some() {
                            flags |= GX2ClearFlags_GX2_CLEAR_FLAGS_STENCIL;
                        }
                        if flags != 0 {
                            GX2ClearDepthStencilEx(*db, d.unwrap_or(1.0), s.unwrap_or(0), flags);
                        }
                    }
                    shared.reset_state();
                    for (i, (cb, _, _)) in colors.iter().enumerate() {
                        GX2SetColorBuffer(*cb, i as GX2RenderTarget);
                    }
                    match depth {
                        Some((db, ..)) => GX2SetDepthBuffer(*db),
                        None => GX2SetDepthOnlyControl(0, 0, GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS),
                    }
                    GX2SetViewport(0.0, 0.0, extent.0 as f32, extent.1 as f32, 0.0, 1.0);
                    GX2SetScissor(0, 0, extent.0, extent.1);
                    shared.gpu_busy = true;
                    self.pass_targets = colors.iter().map(|(cb, _, size)| ((**cb).surface.image as *mut u8, *size)).collect();
                    self.pass_depth = depth.as_ref().map(|(db, _, _, size)| ((**db).surface.image as *mut u8, *size));
                }
                Cmd::EndPass => {
                    self.in_pass = false;
                    // what was drawn becomes visible to later sampling
                    for (image, size) in self.pass_targets.drain(..) {
                        GX2Invalidate(
                            GX2InvalidateMode_GX2_INVALIDATE_MODE_COLOR_BUFFER | GX2InvalidateMode_GX2_INVALIDATE_MODE_TEXTURE,
                            image.cast(),
                            size as u32,
                        );
                    }
                    if let Some((image, size)) = self.pass_depth.take() {
                        GX2Invalidate(
                            GX2InvalidateMode_GX2_INVALIDATE_MODE_DEPTH_BUFFER | GX2InvalidateMode_GX2_INVALIDATE_MODE_TEXTURE,
                            image.cast(),
                            size as u32,
                        );
                    }
                }
                Cmd::Pipeline(p) => {
                    self.pipeline = Some(*p);
                    if !self.dead {
                        self.apply_pipeline(&**p);
                    }
                }
                Cmd::BindGroup { index, group, offsets } => {
                    if let Some(g) = self.groups.get_mut(*index as usize) {
                        *g = Some((*group, offsets.clone()));
                    }
                }
                Cmd::VertexBuffer { slot, ptr, size } => {
                    if let Some(v) = self.vertex.get_mut(*slot as usize) {
                        *v = (*ptr as usize, *size);
                    }
                }
                Cmd::IndexBuffer { ptr, u32_indices } => self.index = Some((*ptr, *u32_indices)),
                Cmd::Viewport { x, y, w, h, near, far } => GX2SetViewport(*x, *y, *w, *h, *near, *far),
                Cmd::Scissor { x, y, w, h } => GX2SetScissor(*x, *y, *w, *h),
                Cmd::StencilRef(r) => {
                    self.stencil_ref = *r;
                    if let Some(p) = self.pipeline {
                        self.apply_stencil(&*p);
                    }
                }
                Cmd::BlendConstants([r, g, b, a]) => GX2SetBlendConstantColor(*r, *g, *b, *a),
                Cmd::Draw { first_vertex, vertex_count, first_instance, instance_count } => {
                    if self.ready().is_none() {
                        shared.stats.skipped_draws += 1;
                    }
                    if let Some(p) = self.ready() {
                        shared.stats.draws += 1;
                        let p = &*p;
                        self.bind(p);
                        GX2DrawEx2(p.mode, *vertex_count, *first_vertex, *instance_count, *first_instance);
                    }
                }
                Cmd::DrawIndexed { first_index, index_count, base_vertex, first_instance, instance_count } => {
                    if self.ready().is_none() || self.index.is_none() {
                        shared.stats.skipped_draws += 1;
                    }
                    if let (Some(p), Some((ptr, wide))) = (self.ready(), self.index) {
                        shared.stats.draws += 1;
                        let p = &*p;
                        self.bind(p);
                        let (ty, size) = if wide {
                            (GX2IndexType_GX2_INDEX_TYPE_U32, 4)
                        } else {
                            (GX2IndexType_GX2_INDEX_TYPE_U16, 2)
                        };
                        GX2DrawIndexedEx2(
                            p.mode,
                            *index_count,
                            ty,
                            ptr.add(*first_index as usize * size).cast(),
                            *base_vertex as u32,
                            *instance_count,
                            *first_instance,
                        );
                    }
                }
            }
        }
    }

    fn ready(&self) -> Option<*const RenderPipeline> {
        if self.dead || !self.in_pass {
            return None;
        }
        self.pipeline
    }

    unsafe fn apply_pipeline(&self, p: &RenderPipeline) {
        unsafe {
            GX2SetFetchShader(&*p.fetch.shader);
            GX2SetVertexShader(&*p.vs.shader);
            if let Some(ps) = &p.ps {
                GX2SetPixelShader(&*ps.shader);
            }
            GX2SetPolygonControl(
                p.front_face,
                p.cull_front as BOOL,
                p.cull_back as BOOL,
                0,
                GX2PolygonMode_GX2_POLYGON_MODE_TRIANGLE,
                GX2PolygonMode_GX2_POLYGON_MODE_TRIANGLE,
                0,
                0,
                0,
            );
            let mut enable = 0u8;
            for (i, b) in p.blends.iter().enumerate() {
                if let Some(b) = b {
                    enable |= 1 << i;
                    GX2SetBlendControl(
                        i as GX2RenderTarget,
                        b.color.0,
                        b.color.1,
                        b.color.2,
                        1,
                        b.alpha.0,
                        b.alpha.1,
                        b.alpha.2,
                    );
                }
            }
            GX2SetColorControl(GX2LogicOp_GX2_LOGIC_OP_COPY, enable, 0, (p.targets != 0) as BOOL);
            let m = &p.masks;
            GX2SetTargetChannelMasks(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7]);
            GX2SetAlphaTest(0, GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS, 0.0);
            self.apply_stencil(p);
        }
    }

    unsafe fn apply_stencil(&self, p: &RenderPipeline) {
        use super::conv::{compare, stencil_op};
        unsafe {
            match &p.depth {
                Some(d) => {
                    let s = &d.stencil;
                    let stencil = s.is_enabled();
                    GX2SetDepthStencilControl(
                        (d.depth_compare != Some(wgt::CompareFunction::Always) || d.depth_write_enabled == Some(true)) as BOOL,
                        (d.depth_write_enabled == Some(true)) as BOOL,
                        compare(d.depth_compare.unwrap_or(wgt::CompareFunction::Always)),
                        stencil as BOOL,
                        stencil as BOOL,
                        compare(s.front.compare),
                        stencil_op(s.front.pass_op),
                        stencil_op(s.front.depth_fail_op),
                        stencil_op(s.front.fail_op),
                        compare(s.back.compare),
                        stencil_op(s.back.pass_op),
                        stencil_op(s.back.depth_fail_op),
                        stencil_op(s.back.fail_op),
                    );
                    if stencil {
                        let (rm, wm, r) = (s.read_mask as u8, s.write_mask as u8, self.stencil_ref);
                        GX2SetStencilMask(rm, wm, r, rm, wm, r);
                    }
                }
                None => GX2SetDepthOnlyControl(0, 0, GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS),
            }
        }
    }

    /// Vertex buffers and bind groups, as the pipeline lays them out.
    unsafe fn bind(&self, p: &RenderPipeline) {
        unsafe {
            for (i, stride) in p.strides.iter().enumerate() {
                let (ptr, size) = self.vertex[i];
                if ptr != 0 {
                    GX2SetAttribBuffer(i as u32, size, *stride, ptr as *const _);
                }
            }
            for (gi, slots) in p.groups.iter().enumerate() {
                let Some((group, offsets)) = self.groups.get(gi).and_then(|g| g.as_ref()) else { continue };
                let group = &**group;
                for s in slots {
                    let Some((_, res)) = group.entries.iter().find(|(b, _)| *b == s.binding) else { continue };
                    match (s.kind, res) {
                        (Kind::Uniform | Kind::Storage, Res::Buffer { ptr, size }) => {
                            let off = s.dynamic.and_then(|d| offsets.get(d)).copied().unwrap_or(0) as usize;
                            let ptr = ptr.add(off).cast();
                            if let Some(slot) = s.vs {
                                GX2SetVertexUniformBlock(slot as u32, *size, ptr);
                            }
                            if let Some(slot) = s.ps {
                                GX2SetPixelUniformBlock(slot as u32, *size, ptr);
                            }
                        }
                        (Kind::Texture, Res::Texture(t)) => {
                            if let Some(slot) = s.vs {
                                GX2SetVertexTexture(*t, slot as u32);
                            }
                            if let Some(slot) = s.ps {
                                GX2SetPixelTexture(*t, slot as u32);
                            }
                        }
                        (Kind::Sampler, Res::Sampler(smp)) => {
                            if let Some(slot) = s.vs {
                                GX2SetVertexSampler(*smp, slot as u32);
                            }
                            if let Some(slot) = s.ps {
                                GX2SetPixelSampler(*smp, slot as u32);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn color_of(c: wgt::Color) -> [f32; 4] {
    [c.r as f32, c.g as f32, c.b as f32, c.a as f32]
}

impl crate::CommandEncoder for CommandBuffer {
    type A = Api;

    unsafe fn begin_encoding(&mut self, _label: crate::Label) -> DeviceResult<()> {
        self.commands.clear();
        Ok(())
    }

    unsafe fn discard_encoding(&mut self) {
        self.commands.clear();
    }

    unsafe fn end_encoding(&mut self) -> DeviceResult<CommandBuffer> {
        Ok(CommandBuffer { commands: mem::take(&mut self.commands) })
    }

    unsafe fn reset_all<I>(&mut self, _command_buffers: I) {}

    unsafe fn transition_buffers<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::BufferBarrier<'a, Buffer>>,
    {
    }

    unsafe fn transition_textures<'a, T>(&mut self, _barriers: T)
    where
        T: Iterator<Item = crate::TextureBarrier<'a, Texture>>,
    {
    }

    unsafe fn clear_buffer(&mut self, buffer: &Buffer, range: crate::MemoryRange) {
        self.commands.push(Cmd::ClearBuffer {
            ptr: unsafe { buffer.mem.ptr.add(range.start as usize) },
            len: (range.end - range.start) as usize,
        });
    }

    unsafe fn copy_buffer_to_buffer<T>(&mut self, src: &Buffer, dst: &Buffer, regions: T)
    where
        T: Iterator<Item = crate::BufferCopy>,
    {
        self.commands.push(Cmd::CopyBuffer {
            src: src.mem.ptr,
            dst: dst.mem.ptr,
            // a uniform buffer holds swapped words: swap on the way in, and on the way out
            swap: src.swapped != dst.swapped,
            regions: regions
                .map(|r| Region { src: r.src_offset as usize, dst: r.dst_offset as usize, size: r.size.get() as usize })
                .collect(),
        });
    }

    unsafe fn copy_texture_to_texture<T>(&mut self, src: &Texture, _src_usage: wgt::TextureUses, dst: &Texture, regions: T)
    where
        T: Iterator<Item = crate::TextureCopy>,
    {
        for r in regions {
            if r.src_base.mip_level > 0 || r.dst_base.mip_level > 0 {
                continue;
            }
            self.commands.push(Cmd::TextureToTexture {
                src: &src.surface,
                dst: &dst.surface as *const _ as *mut _,
                src_layer: r.src_base.array_layer,
                dst_layer: r.dst_base.array_layer,
            });
        }
    }

    unsafe fn copy_buffer_to_texture<T>(&mut self, src: &Buffer, dst: &Texture, regions: T)
    where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
        self.commands.push(Cmd::BufferToTexture { src: src.mem.ptr, dst: tex_copy(dst), regions: regions.collect() });
    }

    unsafe fn copy_texture_to_buffer<T>(&mut self, src: &Texture, _src_usage: wgt::TextureUses, dst: &Buffer, regions: T)
    where
        T: Iterator<Item = crate::BufferTextureCopy>,
    {
        self.commands.push(Cmd::TextureToBuffer { src: tex_copy(src), dst: dst.mem.ptr, regions: regions.collect() });
    }

    unsafe fn copy_acceleration_structure_to_acceleration_structure(
        &mut self,
        _src: &super::AccelerationStructure,
        _dst: &super::AccelerationStructure,
        _copy: wgt::AccelerationStructureCopy,
    ) {
    }

    unsafe fn begin_query(&mut self, _set: &QuerySet, _index: u32) {}
    unsafe fn end_query(&mut self, _set: &QuerySet, _index: u32) {}
    unsafe fn write_timestamp(&mut self, _set: &QuerySet, _index: u32) {}
    unsafe fn read_acceleration_structure_compact_size(&mut self, _acceleration_structure: &super::AccelerationStructure, _buf: &Buffer) {}
    unsafe fn reset_queries(&mut self, _set: &QuerySet, _range: Range<u32>) {}
    unsafe fn copy_query_results(
        &mut self,
        _set: &QuerySet,
        _range: Range<u32>,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _stride: wgt::BufferSize,
    ) {
    }

    // render

    unsafe fn begin_render_pass(&mut self, desc: &crate::RenderPassDescriptor<QuerySet, TextureView>) -> DeviceResult<()> {
        let mut dead = false;
        let mut colors = vec![];
        for a in desc.color_attachments.iter().flatten() {
            let view = a.target.view;
            dead |= view.dead;
            let Some(cb) = &view.color else {
                dead = true;
                continue;
            };
            let clear = a.ops.contains(crate::AttachmentOps::LOAD_CLEAR).then(|| color_of(a.clear_value));
            colors.push((&**cb as *const GX2ColorBuffer as *mut GX2ColorBuffer, clear, view.surface_size));
        }
        let depth = desc.depth_stencil_attachment.as_ref().and_then(|d| {
            let view = d.target.view;
            let db = view.depth.as_ref()?;
            let clear_d = d.depth_ops.contains(crate::AttachmentOps::LOAD_CLEAR).then_some(d.clear_value.0);
            let clear_s = d.stencil_ops.contains(crate::AttachmentOps::LOAD_CLEAR).then_some(d.clear_value.1 as u8);
            Some((&**db as *const GX2DepthBuffer as *mut GX2DepthBuffer, clear_d, clear_s, view.surface_size))
        });
        self.commands.push(Cmd::BeginPass { colors, depth, extent: (desc.extent.width, desc.extent.height), dead });
        Ok(())
    }

    unsafe fn end_render_pass(&mut self) {
        self.commands.push(Cmd::EndPass);
    }

    unsafe fn set_bind_group(&mut self, _layout: &PipelineLayout, index: u32, group: &BindGroup, dynamic_offsets: &[wgt::DynamicOffset]) {
        self.commands.push(Cmd::BindGroup { index, group, offsets: dynamic_offsets.to_vec() });
    }

    unsafe fn set_immediates(&mut self, _layout: &PipelineLayout, _offset_bytes: u32, _data: &[u32]) {
        std::println!("GX2: immediates are not supported");
    }

    unsafe fn insert_debug_marker(&mut self, _label: &str) {}
    unsafe fn begin_debug_marker(&mut self, _group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, pipeline: &RenderPipeline) {
        self.commands.push(Cmd::Pipeline(pipeline));
    }

    unsafe fn set_index_buffer<'a>(&mut self, binding: crate::BufferBinding<'a, Buffer>, format: wgt::IndexFormat) {
        self.commands.push(Cmd::IndexBuffer {
            ptr: unsafe { binding.buffer.mem.ptr.add(binding.offset as usize) },
            u32_indices: format == wgt::IndexFormat::Uint32,
        });
    }

    unsafe fn set_vertex_buffer<'a>(&mut self, index: u32, binding: crate::BufferBinding<'a, Buffer>) {
        let size = binding.size.map(|s| s.get()).unwrap_or(binding.buffer.size - binding.offset);
        self.commands.push(Cmd::VertexBuffer {
            slot: index,
            ptr: unsafe { binding.buffer.mem.ptr.add(binding.offset as usize) },
            size: size as u32,
        });
    }

    unsafe fn set_viewport(&mut self, rect: &crate::Rect<f32>, depth_range: Range<f32>) {
        self.commands.push(Cmd::Viewport {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            near: depth_range.start,
            far: depth_range.end,
        });
    }

    unsafe fn set_scissor_rect(&mut self, rect: &crate::Rect<u32>) {
        self.commands.push(Cmd::Scissor { x: rect.x, y: rect.y, w: rect.w, h: rect.h });
    }

    unsafe fn set_stencil_reference(&mut self, value: u32) {
        self.commands.push(Cmd::StencilRef(value as u8));
    }

    unsafe fn set_blend_constants(&mut self, color: &[f32; 4]) {
        self.commands.push(Cmd::BlendConstants(*color));
    }

    unsafe fn draw(&mut self, first_vertex: u32, vertex_count: u32, first_instance: u32, instance_count: u32) {
        self.commands.push(Cmd::Draw { first_vertex, vertex_count, first_instance, instance_count });
    }

    unsafe fn draw_indexed(&mut self, first_index: u32, index_count: u32, base_vertex: i32, first_instance: u32, instance_count: u32) {
        self.commands.push(Cmd::DrawIndexed { first_index, index_count, base_vertex, first_instance, instance_count });
    }

    unsafe fn draw_mesh_tasks(&mut self, _x: u32, _y: u32, _z: u32) {}
    unsafe fn draw_indirect(&mut self, _buffer: &Buffer, _offset: wgt::BufferAddress, _draw_count: u32) {}
    unsafe fn draw_indexed_indirect(&mut self, _buffer: &Buffer, _offset: wgt::BufferAddress, _draw_count: u32) {}
    unsafe fn draw_mesh_tasks_indirect(&mut self, _buffer: &Buffer, _offset: wgt::BufferAddress, _draw_count: u32) {}
    unsafe fn draw_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }
    unsafe fn draw_indexed_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }
    unsafe fn draw_mesh_tasks_indirect_count(
        &mut self,
        _buffer: &Buffer,
        _offset: wgt::BufferAddress,
        _count_buffer: &Buffer,
        _count_offset: wgt::BufferAddress,
        _max_count: u32,
    ) {
    }

    // compute: not on GX2

    unsafe fn begin_compute_pass(&mut self, _desc: &crate::ComputePassDescriptor<QuerySet>) {}
    unsafe fn end_compute_pass(&mut self) {}
    unsafe fn set_compute_pipeline(&mut self, _pipeline: &super::ComputePipeline) {}
    unsafe fn dispatch_workgroups(&mut self, _count: [u32; 3]) {}
    unsafe fn dispatch_workgroups_indirect(&mut self, _buffer: &Buffer, _offset: wgt::BufferAddress) {}

    unsafe fn build_acceleration_structures<'a, T>(&mut self, _descriptor_count: u32, _descriptors: T)
    where
        Api: 'a,
        T: IntoIterator<Item = crate::BuildAccelerationStructureDescriptor<'a, Buffer, super::AccelerationStructure>>,
    {
    }

    unsafe fn place_acceleration_structure_barrier(&mut self, _barriers: crate::AccelerationStructureBarrier) {}

    unsafe fn set_acceleration_structure_dependencies(_command_buffers: &[&CommandBuffer], _dependencies: &[&super::AccelerationStructure]) {}

    unsafe fn begin_ray_tracing_pass(&mut self, _desc: &crate::RayTracingPassDescriptor) {}
    unsafe fn end_ray_tracing_pass(&mut self) {}
    unsafe fn set_ray_tracing_pipeline(&mut self, _pipeline: &super::RayTracingPipeline) {}
    unsafe fn trace_rays(
        &mut self,
        _count: [u32; 3],
        _ray_generation_group_data: crate::PipelineGroupData<Buffer>,
        _miss_group_data: crate::PipelineGroupData<Buffer>,
        _intersection_group_data: crate::PipelineGroupData<Buffer>,
    ) {
    }
}
