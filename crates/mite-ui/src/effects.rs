//! Glow (bloom) and frosted glass: the design's blur effects, as WGSL passes around Vello.
//!
//! Views draw what glows into [`DrawCx::glow`](crate::DrawCx::glow) as well as into the scene,
//! and call [`DrawCx::glass`](crate::DrawCx::glass) before drawing a glass panel. Each glass
//! panel starts a new layer: the layers below it are composited first, then blurred into the
//! panel's rounded rect, so the panel frosts exactly what lies under it. Each layer's glow is
//! blurred twice (a tight and a wide Gaussian, so halos fall off like light) and composited
//! before the next layer, so a panel covers the glow of the curve under it.
//!
//! Each layer is one Vello render: its glow is drawn into the same texture at half size, in a
//! strip under the frame. A frame with one layer and nothing glowing renders straight into the
//! frame, as before.

use vello::kurbo::{Affine, RoundedRect};
use vello::peniko::Color;
use vello::wgpu;
use vello::{AaConfig, RenderParams, Renderer, Scene};

use crate::present::Offscreen;
use crate::tokens::{Theme, ThemeKind};

/// The bloom's two Gaussians (σ, logical px) and their weights.
const BLOOM_TIGHT: f64 = 2.0;
const BLOOM_WIDE: f64 = 6.0;
const BLOOM_WEIGHTS: (f32, f32) = (0.9, 1.4);
/// `blur.saturate` in tokens.json.
const GLASS_SATURATE: f32 = 1.2;
/// Largest Gaussian the passes run at quarter size (σ in quarter-size texels).
const MAX_SIGMA: f64 = 24.0;
const PARAM_STRIDE: u64 = 256;
const PARAM_SLOTS: u64 = 128;
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Empty rows between a layer and its glow strip, so neither spills into the other.
const GLOW_GAP: u32 = 64;

/// Glow sources and the layers below glass panels, recorded while a view draws.
#[derive(Default)]
pub struct Effects {
    pub(crate) glow: Scene,
    layers: Vec<Layer>,
    /// The glass panel the current layer starts with.
    glass: Option<Glass>,
    pool: Vec<Scene>,
}

struct Layer {
    scene: Scene,
    glow: Scene,
    glass: Option<Glass>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Glass {
    pub shape: RoundedRect,
    /// Backdrop blur σ, logical px (`blur.*` in tokens.json).
    pub blur: f64,
}

impl Effects {
    fn spare(&mut self) -> Scene {
        self.pool.pop().unwrap_or_default()
    }

    /// Ends the current layer with what `scene` holds (leaving `scene` empty) and starts the
    /// next one, behind `glass`.
    pub(crate) fn split(&mut self, scene: &mut Scene, glass: Option<Glass>) {
        let mut content = self.spare();
        std::mem::swap(scene, &mut content);
        let fresh = self.spare();
        let glow = std::mem::replace(&mut self.glow, fresh);
        self.layers.push(Layer { scene: content, glow, glass: self.glass.take() });
        self.glass = glass;
    }

    fn recycle(&mut self) {
        for mut layer in self.layers.drain(..) {
            layer.scene.reset();
            layer.glow.reset();
            self.pool.push(layer.scene);
            self.pool.push(layer.glow);
        }
        self.glow.reset();
        self.glass = None;
    }

    /// Layers recorded so far (the one being drawn not included).
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

struct Tex {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Tex {
    fn new(device: &wgpu::Device, label: &str, (width, height): (u32, u32), format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { _texture: texture, view }
    }
}

/// Intermediate textures for one frame size.
struct Targets {
    size: (u32, u32),
    /// Vello's render of a layer, with its glow at half size in a strip [`GLOW_GAP`] rows under it.
    work: Tex,
    work_size: (u32, u32),
    half: [Tex; 2],
    quarter: [Tex; 2],
    groups: Groups,
}

struct Groups {
    work: wgpu::BindGroup,
    half: [wgpu::BindGroup; 2],
    quarter: [wgpu::BindGroup; 2],
    bloom: wgpu::BindGroup,
}

struct Pipelines {
    prep: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    bloom_screen: wgpu::RenderPipeline,
    bloom_over: wgpu::RenderPipeline,
    glass: wgpu::RenderPipeline,
    over: wgpu::RenderPipeline,
}

/// Uniforms of one pass (`Params` in effects.wgsl).
#[derive(Clone, Copy, Default)]
struct Params {
    map: f32,
    dir: [f32; 2],
    sigma: f32,
    k: (f32, f32),
    rect: [f32; 4],
    corner: f32,
    saturate: f32,
}

impl Params {
    fn bytes(&self) -> [u8; 64] {
        let radius = (3.0 * self.sigma).ceil().min(72.0);
        let v = [
            self.map, self.map, self.dir[0], self.dir[1], self.sigma, radius, self.k.0, self.k.1,
            self.rect[0], self.rect[1], self.rect[2], self.rect[3], self.corner, self.saturate, 0.0, 0.0,
        ];
        let mut out = [0u8; 64];
        for (chunk, x) in out.chunks_exact_mut(4).zip(v) {
            chunk.copy_from_slice(&x.to_ne_bytes());
        }
        out
    }

    fn blur(sigma: f64, dir: [f32; 2]) -> Self {
        Self { map: 1.0, dir, sigma: sigma.clamp(0.3, MAX_SIGMA) as f32, ..Self::default() }
    }

    fn map(map: f32) -> Self {
        Self { map, ..Self::default() }
    }
}

/// Renders a view's layers and effects into a frame.
pub(crate) struct Compositor {
    pipelines: Pipelines,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    slot: u64,
    targets: Option<Targets>,
    /// Each layer's scene and glow strip in physical pixels (reused between frames).
    physical: Vec<Scene>,
}

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mite-ui effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mite-ui effects"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mite-ui effects"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let over = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let screen = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: over.alpha,
        };
        let srgb = wgpu::TextureFormat::Rgba8UnormSrgb;
        let pipelines = Pipelines {
            prep: pipeline("fs_prep", HDR, None),
            down: pipeline("fs_down", HDR, None),
            blur: pipeline("fs_blur", HDR, None),
            bloom_screen: pipeline("fs_bloom", srgb, Some(screen)),
            bloom_over: pipeline("fs_bloom", srgb, Some(over)),
            glass: pipeline("fs_glass", srgb, Some(over)),
            over: pipeline("fs_over", wgpu::TextureFormat::Rgba8Unorm, Some(over)),
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mite-ui effects"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mite-ui effect params"),
            size: PARAM_STRIDE * PARAM_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipelines, layout, sampler, params, slot: 0, targets: None, physical: Vec::new() }
    }

    fn group(&self, device: &wgpu::Device, a: &wgpu::TextureView, b: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mite-ui effects"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params,
                        offset: 0,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(a) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        let quarter = (half.0.div_ceil(2), half.1.div_ceil(2));
        let vello = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let pass = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let work_size = (size.0, size.1 + GLOW_GAP + half.1);
        let work = Tex::new(device, "mite-ui layer", work_size, rgba, vello);
        let half = [Tex::new(device, "mite-ui half", half, HDR, pass), Tex::new(device, "mite-ui half", half, HDR, pass)];
        let quarter = [Tex::new(device, "mite-ui quarter", quarter, HDR, pass), Tex::new(device, "mite-ui quarter", quarter, HDR, pass)];
        let groups = Groups {
            work: self.group(device, &work.view, &work.view),
            half: [self.group(device, &half[0].view, &half[0].view), self.group(device, &half[1].view, &half[1].view)],
            quarter: [
                self.group(device, &quarter[0].view, &quarter[0].view),
                self.group(device, &quarter[1].view, &quarter[1].view),
            ],
            bloom: self.group(device, &half[0].view, &quarter[0].view),
        };
        self.targets = Some(Targets { size, work, work_size, half, quarter, groups });
    }

    #[allow(clippy::too_many_arguments)]
    fn pass(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
        dst: &wgpu::TextureView,
        params: Params,
        clear: bool,
        scissor: Option<[u32; 4]>,
    ) {
        let offset = self.slot * PARAM_STRIDE;
        self.slot = (self.slot + 1) % PARAM_SLOTS;
        queue.write_buffer(&self.params, offset, &params.bytes());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mite-ui effect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[offset as u32]);
        if let Some([x, y, w, h]) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        pass.draw(0..3, 0..1);
    }

    /// Renders `top` (what the view drew last) and the layers `fx` recorded under it into
    /// `target`. `transform` maps logical to physical pixels; `overlay` draws into the top layer
    /// in physical pixels (window chrome).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut Renderer,
        fx: &mut Effects,
        top: &mut Scene,
        transform: Affine,
        theme: &Theme,
        target: &Offscreen,
        overlay: &dyn Fn(&mut Scene),
    ) -> Result<(), String> {
        fx.split(top, None);
        let n = fx.layers.len();
        while self.physical.len() < n {
            self.physical.push(Scene::new());
        }
        let size = (target.width, target.height);
        let plain = n == 1 && fx.layers[0].glow.encoding().is_empty();
        let glow_transform = Affine::translate((0.0, f64::from(size.1 + GLOW_GAP))) * Affine::scale(0.5) * transform;
        for (i, (layer, scene)) in fx.layers.iter().zip(&mut self.physical).enumerate() {
            scene.reset();
            if i == 0 && !plain {
                // The window colour, which Vello's base colour would otherwise paint over the
                // glow strip too.
                let frame = vello::kurbo::Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1));
                scene.fill(vello::peniko::Fill::NonZero, Affine::IDENTITY, theme.window, None, &frame);
            }
            scene.append(&layer.scene, Some(transform));
            if !layer.glow.encoding().is_empty() {
                scene.append(&layer.glow, Some(glow_transform));
            }
        }
        overlay(&mut self.physical[n - 1]);

        let params = |base_color: Color, (width, height): (u32, u32)| RenderParams {
            base_color,
            width,
            height,
            antialiasing_method: AaConfig::Area,
        };
        let result = (|| {
            if plain {
                return renderer
                    .render_to_texture(device, queue, &self.physical[0], &target.view, &params(theme.window, size))
                    .map_err(|e| e.to_string());
            }
            self.ensure_targets(device, size);
            // Physical px per logical px, for the radii.
            let scale = transform.as_coeffs()[0];
            let screen = theme.kind != ThemeKind::Light;
            for i in 0..n {
                if let Some(glass) = fx.layers[i].glass.filter(|_| i > 0) {
                    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mite-ui frost") });
                    self.frost(device, queue, &mut encoder, target, glass, transform, scale);
                    queue.submit([encoder.finish()]);
                }
                let t = self.targets.as_ref().expect("targets");
                renderer
                    .render_to_texture(device, queue, &self.physical[i], &t.work.view, &params(Color::TRANSPARENT, t.work_size))
                    .map_err(|e| e.to_string())?;
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mite-ui effects") });
                let (pipeline, group) = (self.pipelines.over.clone(), t.groups.work.clone());
                self.pass(queue, &mut encoder, &pipeline, &group, &target.view, Params::map(1.0), false, Some([0, 0, size.0, size.1]));
                if !fx.layers[i].glow.encoding().is_empty() {
                    self.bloom(queue, &mut encoder, target, scale, screen);
                }
                queue.submit([encoder.finish()]);
            }
            Ok(())
        })();
        fx.recycle();
        result
    }

    /// Blurs the layer's glow (tight at half size, wide at quarter size) and composites it.
    fn bloom(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &Offscreen, scale: f64, screen: bool) {
        let t = self.targets.as_ref().expect("targets");
        let p = &self.pipelines;
        let (prep, down, blur) = (p.prep.clone(), p.down.clone(), p.blur.clone());
        let bloom = if screen { p.bloom_screen.clone() } else { p.bloom_over.clone() };
        let g = &t.groups;
        let (glow, h0, h1, q0, q1, both) = (g.work.clone(), g.half[0].clone(), g.half[1].clone(), g.quarter[0].clone(), g.quarter[1].clone(), g.bloom.clone());
        let (hv0, hv1, qv0, qv1) = (t.half[0].view.clone(), t.half[1].view.clone(), t.quarter[0].view.clone(), t.quarter[1].view.clone());
        let tight = BLOOM_TIGHT * scale / 2.0;
        let wide = BLOOM_WIDE * scale / 4.0;
        let strip = Params { dir: [0.0, (target.height + GLOW_GAP) as f32], ..Params::map(1.0) };
        self.pass(queue, encoder, &prep, &glow, &hv0, strip, true, None);
        self.pass(queue, encoder, &blur, &h0, &hv1, Params::blur(tight, [1.0, 0.0]), true, None);
        self.pass(queue, encoder, &blur, &h1, &hv0, Params::blur(tight, [0.0, 1.0]), true, None);
        self.pass(queue, encoder, &down, &h0, &qv0, Params::map(2.0), true, None);
        self.pass(queue, encoder, &blur, &q0, &qv1, Params::blur(wide, [1.0, 0.0]), true, None);
        self.pass(queue, encoder, &blur, &q1, &qv0, Params::blur(wide, [0.0, 1.0]), true, None);
        let k = Params { k: BLOOM_WEIGHTS, ..Params::default() };
        self.pass(queue, encoder, &bloom, &both, &target.srgb_view, k, false, None);
    }

    /// Blurs the frame so far and lays it, saturated, into the glass panel's rounded rect.
    #[allow(clippy::too_many_arguments)]
    fn frost(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &Offscreen,
        glass: Glass,
        transform: Affine,
        scale: f64,
    ) {
        let frame = self.group(device, &target.srgb_view, &target.srgb_view);
        let t = self.targets.as_ref().expect("targets");
        let p = &self.pipelines;
        let (down, blur, composite) = (p.down.clone(), p.blur.clone(), p.glass.clone());
        let g = &t.groups;
        let (h0, q0, q1) = (g.half[0].clone(), g.quarter[0].clone(), g.quarter[1].clone());
        let (hv0, qv0, qv1) = (t.half[0].view.clone(), t.quarter[0].view.clone(), t.quarter[1].view.clone());
        let sigma = glass.blur * scale / 4.0;
        self.pass(queue, encoder, &down, &frame, &hv0, Params::map(2.0), true, None);
        self.pass(queue, encoder, &down, &h0, &qv0, Params::map(2.0), true, None);
        self.pass(queue, encoder, &blur, &q0, &qv1, Params::blur(sigma, [1.0, 0.0]), true, None);
        self.pass(queue, encoder, &blur, &q1, &qv0, Params::blur(sigma, [0.0, 1.0]), true, None);

        let r = transform.transform_rect_bbox(glass.shape.rect());
        let (w, h) = (f64::from(target.width), f64::from(target.height));
        let (x0, y0) = (r.x0.floor().clamp(0.0, w), r.y0.floor().clamp(0.0, h));
        let (x1, y1) = (r.x1.ceil().clamp(0.0, w), r.y1.ceil().clamp(0.0, h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let params = Params {
            map: 0.25,
            rect: [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32],
            corner: (glass.shape.radii().top_left * scale) as f32,
            saturate: GLASS_SATURATE,
            ..Params::default()
        };
        let scissor = [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32];
        self.pass(queue, encoder, &composite, &q0, &target.srgb_view, params, false, Some(scissor));
    }
}
