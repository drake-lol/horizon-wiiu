//! A textured, tinted quad drawn with wgpu through the Wii U GX2 backend. The WGSL below compiles on the
//! console (naga, then naga-latte); a uniform buffer spins and tints it, a checker texture covers it.
extern crate wiiu_atomics;

use std::ptr::NonNull;

use wgpu::util::DeviceExt;
use wut_sys::*;

const SHADER: &str = r#"
struct U { angle: vec4f, tint: vec4f }
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

struct Out { @builtin(position) pos: vec4f, @location(0) uv: vec2f, @location(1) color: vec4f }

@vertex fn vs(@location(0) p: vec2f, @location(1) uv: vec2f, @location(2) color: vec4f) -> Out {
    let c = cos(u.angle.x);
    let s = sin(u.angle.x);
    var o: Out;
    o.pos = vec4(p.x * c - p.y * s, p.x * s + p.y * c, 0.0, 1.0);
    o.uv = uv;
    o.color = color * u.tint;
    return o;
}

@fragment fn fs(i: Out) -> @location(0) vec4f {
    let t = textureSample(tex, smp, i.uv);
    if t.a < 0.5 { discard; }
    return t * i.color;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    p: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

fn main() {
    println!("wgpu-tri: start");
    unsafe {
        WHBProcInit();
        WHBGfxInit();
    }
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::NOOP;
    desc.backend_options.noop.enable = true;
    let instance = wgpu::Instance::new(desc);
    // No window system: the GX2 backend presents to the TV whatever handle it gets.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_window_handle::RawDisplayHandle::Orbital(raw_window_handle::OrbitalDisplayHandle::new())),
            raw_window_handle: raw_window_handle::RawWindowHandle::Orbital(raw_window_handle::OrbitalWindowHandle::new(NonNull::dangling())),
        })
    }
    .expect("surface");
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .expect("adapter");
    println!("wgpu-tri: adapter {:?}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("device");
    let config = surface.get_default_config(&adapter, 1280, 720).expect("surface config");
    surface.configure(&device, &config);

    let t0 = std::time::Instant::now();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(config.format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    println!("wgpu-tri: pipeline compiled in {} ms", t0.elapsed().as_millis());

    let v = |x: f32, y: f32, u: f32, w: f32, c: [f32; 4]| Vertex { p: [x, y], uv: [u, w], color: c };
    let vertices = [
        v(-0.5, -0.5, 0.0, 1.0, [1.0, 0.3, 0.3, 1.0]),
        v(0.5, -0.5, 1.0, 1.0, [0.3, 1.0, 0.3, 1.0]),
        v(0.5, 0.5, 1.0, 0.0, [0.3, 0.3, 1.0, 1.0]),
        v(-0.5, 0.5, 0.0, 0.0, [1.0, 1.0, 0.3, 1.0]),
    ];
    let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 32,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // an 8x8 checker, with transparent squares the fragment shader discards
    let mut pixels = Vec::new();
    for y in 0..8 {
        for x in 0..8 {
            pixels.extend_from_slice(if (x + y) % 2 == 0 { &[255u8, 255, 255, 255] } else if x % 3 == 0 { &[0, 0, 0, 0] } else { &[64, 64, 64, 255] });
        }
    }
    let texture = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &pixels,
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });

    let mut frame = 0u32;
    while unsafe { WHBProcIsRunning() } != 0 {
        let angle = frame as f32 * 0.02;
        let pulse = 0.75 + 0.25 * (frame as f32 * 0.05).sin();
        let data: [f32; 8] = [angle, 0.0, 0.0, 0.0, pulse, pulse, 1.0, 1.0];
        queue.write_buffer(&ubuf, 0, bytemuck::cast_slice(&data));
        let target = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => {
                println!("wgpu-tri: no frame: {other:?}");
                continue;
            }
        };
        let target_view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.35, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..6, 0, 0..1);
        }
        queue.submit([enc.finish()]);
        queue.present(target);
        if frame % 60 == 0 {
            println!("wgpu-tri: frame {frame}");
        }
        frame += 1;
    }
    unsafe {
        WHBGfxShutdown();
        WHBProcShutdown();
    }
}
