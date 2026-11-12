//! Video frames to the screen: an iced `shader` primitive that owns one wgpu texture and
//! rewrites it in place (`queue.write_texture`) whenever the player publishes a new frame.
//! The texture is recreated only when the frame size changes. Drawing is one 4-vertex strip
//! sampling that texture inside iced's own render pass (no extra pass, no atlas, no copy on
//! the UI thread: the primitive holds an `Arc` of the player's frame buffer).

use iced::advanced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;
use nt_core::VideoFrame;
use std::sync::Arc;

const SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let uv = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    var o: VOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}
@fragment fn fs(v: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, v.uv).rgb, 1.0);
}
"#;

/// The shader program: draws the newest frame the app took from the player.
pub struct VideoProgram {
    pub frame: Option<Arc<VideoFrame>>,
}

impl<M> shader::Program<M> for VideoProgram {
    type State = ();
    type Primitive = VideoPrimitive;
    fn draw(&self, _: &(), _: mouse::Cursor, _: Rectangle) -> VideoPrimitive {
        VideoPrimitive { frame: self.frame.clone() }
    }
}

#[derive(Debug)]
pub struct VideoPrimitive {
    frame: Option<Arc<VideoFrame>>,
}

pub struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    texture: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
    uploaded: u64,
}

impl shader::Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nt video"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nt video"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nt video"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nt video"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nt video"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        VideoPipeline { pipeline, layout, sampler, format, texture: None, uploaded: u64::MAX }
    }
}

impl shader::Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        p: &mut VideoPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let Some(frame) = &self.frame else { return };
        if frame.generation == p.uploaded && p.texture.is_some() {
            return;
        }
        let (w, h) = (frame.width, frame.height);
        if w == 0 || h == 0 {
            return;
        }
        if !p.texture.as_ref().is_some_and(|(_, _, tw, th)| *tw == w && *th == h) {
            let tex_format = if p.format.is_srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("nt video frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: tex_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nt video"),
                layout: &p.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&p.sampler) },
                ],
            });
            p.texture = Some((texture, bind, w, h));
        }
        if let Some((texture, _, _, _)) = &p.texture {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                frame.pixels(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            p.uploaded = frame.generation;
        }
    }

    fn draw(&self, p: &VideoPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if self.frame.is_none() {
            return true;
        }
        if let Some((_, bind, _, _)) = &p.texture {
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, 0..1);
        }
        true
    }
}
